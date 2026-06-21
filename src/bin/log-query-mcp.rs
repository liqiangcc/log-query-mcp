use axum::Router;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use tokio::net::TcpListener;

use log_query_mcp::runtime::{MCP_ENDPOINT, bind_addr_from_env, server_from_env};

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("log-query-mcp: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let bind = bind_addr_from_env()?;
    let server = server_from_env()?;
    let listener = TcpListener::bind(bind).await?;
    let local_addr = listener.local_addr()?;

    let mut http_config = StreamableHttpServerConfig::default()
        .with_stateful_mode(false)
        .with_json_response(true);
    if !local_addr.ip().is_loopback() {
        http_config = http_config.disable_allowed_hosts();
    }

    let service: StreamableHttpService<_, LocalSessionManager> =
        StreamableHttpService::new(move || Ok(server.clone()), Default::default(), http_config);
    let router = Router::new().nest_service(MCP_ENDPOINT, service);

    eprintln!("log-query-mcp: listening on {local_addr} endpoint {MCP_ENDPOINT}");
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
