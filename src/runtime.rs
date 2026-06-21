use std::{env, net::SocketAddr, path::PathBuf, sync::Arc};

use thiserror::Error;

use crate::{
    AppConfig, ConfigLoadError, LogQueryMcpServer, SourceRegistry, SourceRegistryError,
    StatefulQueryError, StatefulQueryService, ToolError,
};

pub const CONFIG_ENV: &str = "LOG_QUERY_MCP_CONFIG";
pub const BIND_ENV: &str = "LOG_QUERY_MCP_BIND";
pub const DEFAULT_BIND: &str = "127.0.0.1:8000";
pub const MCP_ENDPOINT: &str = "/mcp";

pub fn server_from_env() -> Result<LogQueryMcpServer, RuntimeError> {
    server_from_config_path(config_path_from_env()?)
}

pub fn server_from_config_path(path: PathBuf) -> Result<LogQueryMcpServer, RuntimeError> {
    let config = AppConfig::load(path)?;
    let registry = SourceRegistry::from_config(config)?;
    let query_service = StatefulQueryService::new(Arc::new(registry))?;
    LogQueryMcpServer::new(query_service).map_err(RuntimeError::McpServer)
}

pub fn config_path_from_env() -> Result<PathBuf, RuntimeError> {
    env::var_os(CONFIG_ENV)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .ok_or(RuntimeError::MissingConfigEnv)
}

pub fn bind_addr_from_env() -> Result<SocketAddr, RuntimeError> {
    let configured = env::var(BIND_ENV).ok();
    let value = configured.as_deref().unwrap_or(DEFAULT_BIND);
    let bind = value
        .parse::<SocketAddr>()
        .map_err(|_| RuntimeError::InvalidBind(value.to_owned()))?;
    if configured.is_none() && !bind.ip().is_loopback() {
        return Err(RuntimeError::DefaultBindNotLoopback(bind));
    }
    Ok(bind)
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("LOG_QUERY_MCP_CONFIG is required")]
    MissingConfigEnv,

    #[error("failed to load configuration")]
    ConfigLoad(#[from] ConfigLoadError),

    #[error("failed to build source registry")]
    SourceRegistry(#[from] SourceRegistryError),

    #[error("failed to build query service")]
    QueryService(#[from] StatefulQueryError),

    #[error("failed to build MCP server: {0:?}")]
    McpServer(ToolError),

    #[error("LOG_QUERY_MCP_BIND must be a socket address, got {0:?}")]
    InvalidBind(String),

    #[error("default bind address must remain loopback, got {0}")]
    DefaultBindNotLoopback(SocketAddr),
}
