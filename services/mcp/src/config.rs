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
    #[arg(long, env = "MCP_PORT", default_value = "3001")]
    pub port: u16,

    /// Maximum number of search results
    #[arg(long, env = "MAX_SEARCH_RESULTS", default_value = "50")]
    pub max_search_results: usize,

    /// Default search limit
    #[arg(long, env = "DEFAULT_SEARCH_LIMIT", default_value = "10")]
    pub default_search_limit: usize,

    /// OpenCode host URL
    #[arg(long, env = "OPENCODE_HOST", default_value = "http://opencode:4096")]
    pub opencode_host: String,

    /// OpenCode authentication username
    #[arg(long, env = "OPENCODE_AUTH_USER")]
    pub opencode_auth_user: Option<String>,

    /// OpenCode authentication password
    #[arg(long, env = "OPENCODE_AUTH_PASS")]
    pub opencode_auth_pass: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            transport: Transport::Stdio,
            api_base_url: "http://localhost:8088".to_string(),
            http_timeout: 30,
            #[cfg(feature = "sse")]
            port: 3001,
            max_search_results: 50,
            default_search_limit: 10,
            opencode_host: "http://opencode:4096".to_string(),
            opencode_auth_user: None,
            opencode_auth_pass: None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_transport_default() {
        assert_eq!(Transport::default(), Transport::Stdio);
    }

    #[test]
    fn test_transport_from_str_valid() {
        assert_eq!("stdio".parse::<Transport>().unwrap(), Transport::Stdio);
        assert_eq!("STDIO".parse::<Transport>().unwrap(), Transport::Stdio);
        assert_eq!("StdIO".parse::<Transport>().unwrap(), Transport::Stdio);
        assert_eq!("sse".parse::<Transport>().unwrap(), Transport::Sse);
        assert_eq!("SSE".parse::<Transport>().unwrap(), Transport::Sse);
    }

    #[test]
    fn test_transport_from_str_invalid() {
        let result = "invalid".parse::<Transport>();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("Invalid transport"));
    }

    #[test]
    fn test_config_default() {
        let config = Config::default();
        assert_eq!(config.transport, Transport::Stdio);
        assert_eq!(config.api_base_url, "http://localhost:8088");
        assert_eq!(config.http_timeout, 30);
        assert_eq!(config.max_search_results, 50);
        assert_eq!(config.default_search_limit, 10);
    }

    #[test]
    fn test_config_api_url_basic() {
        let config = Config::default();
        assert_eq!(config.api_url("/health"), "http://localhost:8088/health");
    }

    #[test]
    fn test_config_api_url_with_trailing_slash() {
        let mut config = Config::default();
        config.api_base_url = "http://localhost:8088/".to_string();
        assert_eq!(config.api_url("/health"), "http://localhost:8088/health");
    }

    #[test]
    fn test_config_api_url_empty_path() {
        let config = Config::default();
        assert_eq!(config.api_url(""), "http://localhost:8088");
    }

    #[test]
    fn test_config_api_url_custom_base() {
        let mut config = Config::default();
        config.api_base_url = "https://api.example.com/v1".to_string();
        assert_eq!(config.api_url("/search"), "https://api.example.com/v1/search");
    }

    #[test]
    fn test_transport_debug() {
        assert!(format!("{:?}", Transport::Stdio).contains("Stdio"));
        assert!(format!("{:?}", Transport::Sse).contains("Sse"));
    }

    #[test]
    fn test_transport_eq() {
        assert_eq!(Transport::Stdio, Transport::Stdio);
        assert_ne!(Transport::Stdio, Transport::Sse);
    }
}
