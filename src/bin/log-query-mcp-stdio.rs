use anyhow::Result;
use log_query_mcp::LogQueryServer;
use rmcp::{ServiceExt, transport::stdio};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .init();

    tracing::info!("starting log-query-mcp stdio POC");
    let service = LogQueryServer::new().serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
