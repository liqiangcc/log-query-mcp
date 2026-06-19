use std::{env, net::SocketAddr, sync::Arc};

use anyhow::{Context, Result};
use log_query_mcp::{LogQueryServer, QueryService, QueryServiceLimits, SourceRegistry};
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio_util::sync::CancellationToken;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:8000";
const DEFAULT_CONFIG_PATH: &str = "log-query-mcp.json";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "log_query_mcp=info,tower_http=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let bind_address = env::var("LOG_QUERY_MCP_BIND")
        .unwrap_or_else(|_| DEFAULT_BIND_ADDRESS.to_owned())
        .parse::<SocketAddr>()
        .context("LOG_QUERY_MCP_BIND must be a valid socket address")?;
    let config_path =
        env::var("LOG_QUERY_MCP_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_owned());
    let registry = Arc::new(
        SourceRegistry::from_config_path(&config_path)
            .with_context(|| format!("failed to load configuration from {config_path}"))?,
    );
    let query_service = Arc::new(
        QueryService::new(registry, QueryServiceLimits::default())
            .context("failed to initialize log query service")?,
    );
    let source_count = query_service.list_sources().sources.len();

    let cancellation = CancellationToken::new();
    let server_query_service = Arc::clone(&query_service);
    let service = StreamableHttpService::new(
        move || Ok(LogQueryServer::new(Arc::clone(&server_query_service))),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default().with_cancellation_token(cancellation.child_token()),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(bind_address)
        .await
        .with_context(|| format!("failed to bind {bind_address}"))?;

    tracing::info!(
        %bind_address,
        %config_path,
        source_count,
        endpoint = "/mcp",
        "starting log-query-mcp"
    );

    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal(cancellation))
        .await
        .context("HTTP server failed")?;

    Ok(())
}

async fn shutdown_signal(cancellation: CancellationToken) {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to listen for Ctrl+C");
    }
    cancellation.cancel();
}
