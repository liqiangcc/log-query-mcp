use std::{env, sync::Arc};

use anyhow::{Context, Result};
use log_query_mcp::{
    LogQueryServer, QueryService, QueryServiceLimits, SourceRegistry,
};
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

const DEFAULT_CONFIG_PATH: &str = "log-query-mcp.json";

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

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

    tracing::info!(%config_path, source_count, "starting log-query-mcp stdio");
    let service = LogQueryServer::new(query_service).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
