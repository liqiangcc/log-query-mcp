use std::{env, sync::Arc};

use anyhow::{Context, Result};
use log_query_mcp::{AppConfig, LogQueryRuntime, LogQueryServer};
use rmcp::{ServiceExt, transport::stdio};

const DEFAULT_CONFIG_PATH: &str = "log-query-mcp.json";

#[tokio::main]
async fn main() -> Result<()> {
    let config_path =
        env::var("LOG_QUERY_MCP_CONFIG").unwrap_or_else(|_| DEFAULT_CONFIG_PATH.to_owned());
    let config = AppConfig::load(&config_path).context("failed to load service configuration")?;
    let runtime = Arc::new(
        LogQueryRuntime::from_config(config).context("failed to initialize log query runtime")?,
    );
    let service = LogQueryServer::new(runtime)
        .serve(stdio())
        .await
        .context("failed to start stdio MCP service")?;
    service
        .waiting()
        .await
        .context("stdio MCP service stopped unexpectedly")?;
    Ok(())
}
