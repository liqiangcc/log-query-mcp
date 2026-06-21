use std::{env, net::SocketAddr, sync::Arc};

use anyhow::{Context, Result};
use log_query_mcp::{AppConfig, LogQueryRuntime, LogQueryServer};
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
                .unwrap_or_else(|_| "log_query_mcp=info".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let bind_address = env::var("LOG_QUERY_MCP_BIND")
        .unwrap_or_else(|_| DEFAULT_BIND_ADDRESS.to_owned())
        .parse::<SocketAddr>()
        .context("LOG_QUERY_MCP_BIND must be a valid socket address")?;
    let config_path =
        env::var("LOG_QUERY_MCP_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_owned());
    let config = AppConfig::load(&config_path).context("failed to load service configuration")?;
    let runtime = Arc::new(
        LogQueryRuntime::from_config(config).context("failed to initialize log query runtime")?,
    );
    let source_count = runtime.registry().len();
    let limits = runtime.registry().limits().clone();

    let cancellation = CancellationToken::new();
    let server_runtime = Arc::clone(&runtime);
    let service = StreamableHttpService::new(
        move || Ok(LogQueryServer::new(Arc::clone(&server_runtime))),
        LocalSessionManager::default().into(),
        StreamableHttpServerConfig::default().with_cancellation_token(cancellation.child_token()),
    );
    let router = axum::Router::new().nest_service("/mcp", service);
    let listener = tokio::net::TcpListener::bind(bind_address)
        .await
        .with_context(|| format!("failed to bind {bind_address}"))?;

    tracing::info!(
        %bind_address,
        source_count,
        max_scan_files_per_query = limits.max_scan_files_per_query,
        max_scan_bytes_per_page = limits.max_scan_bytes_per_page,
        max_response_bytes = limits.max_response_bytes,
        query_timeout_millis = limits.query_timeout_millis,
        max_concurrent_scans = limits.max_concurrent_scans,
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
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};

        match signal(SignalKind::terminate()) {
            Ok(mut terminate) => {
                tokio::select! {
                    result = tokio::signal::ctrl_c() => {
                        if let Err(error) = result {
                            tracing::error!(%error, "failed to listen for Ctrl+C");
                        }
                    }
                    _ = terminate.recv() => {
                        tracing::info!("received SIGTERM");
                    }
                }
            }
            Err(error) => {
                tracing::error!(%error, "failed to install SIGTERM handler");
                if let Err(error) = tokio::signal::ctrl_c().await {
                    tracing::error!(%error, "failed to listen for Ctrl+C");
                }
            }
        }
    }

    #[cfg(not(unix))]
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to listen for Ctrl+C");
    }

    cancellation.cancel();
}
