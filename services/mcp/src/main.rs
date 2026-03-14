//! rust-brain MCP Server
//!
//! Model Context Protocol server for rust-brain code intelligence tools.
//! Provides a thin HTTP proxy over the rust-brain REST API.

mod client;
mod config;
mod error;
mod server;
mod tools;

use config::Config;
use server::McpServer;
use tracing::{info, Level};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive(Level::INFO.into())
                .add_directive("rustbrain_mcp=debug".parse()?),
        )
        .with_writer(std::io::stderr) // Log to stderr, keep stdout for MCP
        .init();

    // Parse configuration
    let config = Config::parse_args();
    info!("Starting rust-brain MCP server v{}", env!("CARGO_PKG_VERSION"));
    info!("Transport: {:?}", config.transport);
    info!("API URL: {}", config.api_base_url);

    // Create and run the server
    let mut server = McpServer::new(config)?;
    server.run().await?;

    info!("MCP server stopped");
    Ok(())
}
