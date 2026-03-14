//! Configuration for the MCP server

use clap::Parser;

/// Transport mode for the MCP server
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Transport {
    /// Standard input/output (default)
    #[default]
    Stdio,
    /// Server-Sent Events over HTTP
    Sse,
}

impl std::str::FromStr for Transport {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "stdio" => Ok(Transport::Stdio),
            "sse" => Ok(Transport::Sse),
            _ => Err(format!("Invalid transport: {}", s)),
        }
    }
}

/// MCP Server configuration
#[derive(Debug, Clone, Parser)]
#[command(name = "rustbrain-mcp", about = "MCP server for rust-brain code intelligence")]
pub struct Config {
    /// Transport mode (stdio or sse)
    #[arg(long, env = "MCP_TRANSPORT", default_value = "stdio")]
    pub transport: Transport,

    /// API base URL
    #[arg(long, env = "API_BASE_URL", default_value = "http://localhost:8088")]
    pub api_base_url: String,

    /// HTTP timeout in seconds
    #[arg(long, env = "HTTP_TIMEOUT", default_value = "30")]
    pub http_timeout: u64,

    /// Port for SSE mode
    #[cfg(feature = "sse")]
    #[arg(long, env = "MCP_PORT", default_value = "3000")]
    pub port: u16,

    /// Maximum number of search results
    #[arg(long, env = "MAX_SEARCH_RESULTS", default_value = "50")]
    pub max_search_results: usize,

    /// Default search limit
    #[arg(long, env = "DEFAULT_SEARCH_LIMIT", default_value = "10")]
    pub default_search_limit: usize,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            transport: Transport::Stdio,
            api_base_url: "http://localhost:8088".to_string(),
            http_timeout: 30,
            #[cfg(feature = "sse")]
            port: 3000,
            max_search_results: 50,
            default_search_limit: 10,
        }
    }
}

impl Config {
    /// Create configuration from environment and CLI args
    pub fn parse_args() -> Self {
        Parser::parse()
    }

    /// Get the full URL for an API endpoint
    pub fn api_url(&self, path: &str) -> String {
        format!("{}{}", self.api_base_url.trim_end_matches('/'), path)
    }
}
