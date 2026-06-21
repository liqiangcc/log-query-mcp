use rmcp::{ServiceExt, transport::stdio};

use log_query_mcp::runtime::server_from_env;

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("log-query-mcp-stdio: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let server = server_from_env()?;
    eprintln!("log-query-mcp-stdio: serving MCP over stdio");
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
