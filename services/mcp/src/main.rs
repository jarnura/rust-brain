//! rust-brain MCP Server
//!
//! Model Context Protocol server for rust-brain code intelligence tools.
//! Provides a thin HTTP proxy over the rust-brain REST API.

mod client;
mod config;
mod error;
mod server;
#[cfg(feature = "sse")]
mod sse_transport;
mod tools;

use client::OpenCodeClient;
use config::{Config, Transport};
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
    info!(
        "Starting rust-brain MCP server v{}",
        env!("CARGO_PKG_VERSION")
    );
    info!("Transport: {:?}", config.transport);
    info!("API URL: {}", config.api_base_url);
    info!("OpenCode URL: {}", config.opencode_host);

    // Initialize OpenCodeClient
    let _opencode_client = OpenCodeClient::new(&config)?;

    match config.transport {
        Transport::Stdio => {
            let mut server = McpServer::new(config)?;
            server.run().await?;
        }
        Transport::Sse => {
            #[cfg(feature = "sse")]
            {
                sse_transport::run_sse_server(config).await?;
            }
            #[cfg(not(feature = "sse"))]
            {
                anyhow::bail!(
                    "SSE transport requires the 'sse' feature. \
                     Rebuild with: cargo build --features sse"
                );
            }
        }
    }

    info!("MCP server stopped");
    Ok(())
}
