//! Configuration for the rust-brain API server.
//!
//! All settings are loaded from environment variables via [`Config::from_env`].
//! See that method's documentation for the full variable table and defaults.

/// Replaces the password portion of a connection URL with `***` for safe logging.
///
/// Looks for the last `@` in the URL, then the last `:` before it, and masks
/// everything between the colon and the `@`. URLs without an `@` (no
/// credentials) are returned unchanged.
///
/// # Examples
///
/// ```
/// use rustbrain_api::config::redact_url;
///
/// assert_eq!(
///     redact_url("postgresql://user:secret@host:5432/db"),
///     "postgresql://user:***@host:5432/db"
/// );
/// assert_eq!(redact_url("http://localhost:8080"), "http://localhost:8080");
/// ```
pub fn redact_url(url: &str) -> String {
    if let Some(at_pos) = url.rfind('@') {
        if let Some(colon_pos) = url[..at_pos].rfind(':') {
            let scheme_and_user = &url[..colon_pos + 1];
            let rest = &url[at_pos..];
            format!("{}***{}", scheme_and_user, rest)
        } else {
            url.to_string()
        }
    } else {
        url.to_string()
    }
}

/// API server configuration loaded from environment variables.
///
/// Covers database connections, embedding settings, the chat model, the
/// listening port, and OpenCode integration credentials.
#[derive(Debug, Clone)]
pub struct Config {
    /// PostgreSQL connection URL
    pub database_url: String,
    /// Neo4j Bolt URI
    pub neo4j_uri: String,
    /// Neo4j username
    pub neo4j_user: String,
    /// Neo4j password
    pub neo4j_password: String,
    /// Qdrant REST API base URL
    pub qdrant_host: String,
    /// Ollama API base URL
    pub ollama_host: String,
    /// Ollama embedding model name
    pub embedding_model: String,
    /// Expected vector dimensions for the embedding model
    pub embedding_dimensions: usize,
    /// Qdrant collection name for code embeddings
    pub collection_name: String,
    /// Ollama chat model name (used by playground chat)
    pub chat_model: String,
    /// TCP port the API server listens on
    pub port: u16,
    /// OpenCode server base URL
    pub opencode_host: String,
    /// Optional HTTP Basic Auth username for OpenCode
    pub opencode_auth_user: Option<String>,
    /// Optional HTTP Basic Auth password for OpenCode
    pub opencode_auth_pass: Option<String>,
    /// Docker network used for per-execution OpenCode containers
    pub docker_network: String,
    /// OpenCode Docker image used for per-execution containers
    pub opencode_image: String,
    /// Host-side root directory for workspace clones.
    ///
    /// Must be accessible by the host Docker daemon so it can be bind-mounted
    /// into the ingestion container. Defaults to `/tmp/rustbrain-clones`.
    /// Override via `WORKSPACE_HOST_CLONE_ROOT` env var.
    pub workspace_host_clone_root: String,
    /// Docker image used to run the ingestion pipeline for workspaces.
    ///
    /// Defaults to `rustbrain-ingestion:latest`.
    /// Override via `INGESTION_IMAGE` env var.
    pub ingestion_image: String,
    /// Default execution timeout in seconds.
    ///
    /// Applied when no per-execution timeout is specified.
    /// Defaults to 7200 (2 hours).
    /// Override via `EXECUTION_TIMEOUT_SECS` env var.
    pub execution_timeout_secs: u32,
    /// Public hostname or IP for execution containers.
    ///
    /// When set, execution containers publish their port to a random host port
    /// and the public endpoint is constructed as `http://{public_host}:{mapped_port}`.
    /// This makes containers reachable from Tailscale-connected devices.
    /// Override via `RUSTBRAIN_PUBLIC_HOST` env var.
    pub public_host: Option<String>,
    /// How long (in seconds) to keep execution containers alive after successful
    /// completion, allowing users to debug via the mapped host port.
    ///
    /// When `0` (the default), containers are removed immediately after the
    /// execution finishes. Set via `RUSTBRAIN_CONTAINER_KEEP_ALIVE_SECS`.
    pub container_keep_alive_secs: u64,
    /// Container readiness timeout in seconds.
    ///
    /// How long to wait for an OpenCode container to become ready before
    /// considering the execution failed. Defaults to 60 seconds.
    /// Override via `OPENCODE_READY_TIMEOUT_SECS`.
    pub opencode_ready_timeout_secs: u32,
    /// Host-side path to the OpenCode config directory.
    ///
    /// When set, execution containers bind-mount `opencode.json` (LLM provider
    /// config) and `.opencode/` (agent definitions) from this directory. Without
    /// this, spawned containers have no LLM provider and no agent prompts.
    /// Override via `OPENCODE_CONFIG_HOST_PATH`.
    pub opencode_config_host_path: Option<String>,
    /// MCP-SSE URL passed to execution containers as an environment variable.
    ///
    /// Allows OpenCode agents to discover rust-brain code intelligence tools.
    /// Defaults to `http://mcp-sse:3001/sse` (Docker-network reachable).
    /// Override via `MCP_SSE_URL`.
    pub mcp_sse_url: String,
}

impl Config {
    /// Loads configuration from environment variables.
    ///
    /// | Variable | Required | Default |
    /// |---|---|---|
    /// | `DATABASE_URL` | **yes** | — |
    /// | `NEO4J_URI` | no | `bolt://neo4j:7687` |
    /// | `NEO4J_USER` | no | `neo4j` |
    /// | `NEO4J_PASSWORD` | **yes** | — |
    /// | `QDRANT_HOST` | no | `http://qdrant:6333` |
    /// | `OLLAMA_HOST` | no | `http://ollama:11434` |
    /// | `EMBEDDING_MODEL` | no | `nomic-embed-text` |
    /// | `EMBEDDING_DIMENSIONS` | no | `768` |
    /// | `QDRANT_COLLECTION` | no | `code_embeddings` |
    /// | `CHAT_MODEL` | no | `codellama:7b` |
    /// | `API_PORT` | no | `8080` |
    /// | `OPENCODE_HOST` | no | `http://opencode:4096` |
    /// | `OPENCODE_AUTH_USER` | no | _(none)_ |
    /// | `OPENCODE_AUTH_PASS` | no | _(none)_ |
    /// | `DOCKER_NETWORK` | no | `rustbrain` |
    /// | `OPENCODE_IMAGE` | no | `opencode:latest` |
    /// | `EXECUTION_TIMEOUT_SECS` | no | `7200` |
    /// | `RUSTBRAIN_PUBLIC_HOST` | no | _(none)_ |
    /// | `RUSTBRAIN_CONTAINER_KEEP_ALIVE_SECS` | no | `0` |
    /// | `OPENCODE_READY_TIMEOUT_SECS` | no | `60` |
    /// | `OPENCODE_CONFIG_HOST_PATH` | no | _(none)_ |
    /// | `MCP_SSE_URL` | no | `http://mcp-sse:3001/sse` |
    ///
    /// # Panics
    ///
    /// Panics if `DATABASE_URL` or `NEO4J_PASSWORD` is not set.
    pub fn from_env() -> Self {
        Self {
            database_url: std::env::var("DATABASE_URL")
                .expect("DATABASE_URL environment variable must be set"),
            neo4j_uri: std::env::var("NEO4J_URI")
                .unwrap_or_else(|_| "bolt://neo4j:7687".to_string()),
            neo4j_user: std::env::var("NEO4J_USER").unwrap_or_else(|_| "neo4j".to_string()),
            neo4j_password: std::env::var("NEO4J_PASSWORD")
                .expect("NEO4J_PASSWORD environment variable must be set"),
            qdrant_host: std::env::var("QDRANT_HOST")
                .unwrap_or_else(|_| "http://qdrant:6333".to_string()),
            ollama_host: std::env::var("OLLAMA_HOST")
                .unwrap_or_else(|_| "http://ollama:11434".to_string()),
            embedding_model: std::env::var("EMBEDDING_MODEL")
                .unwrap_or_else(|_| "nomic-embed-text".to_string()),
            embedding_dimensions: std::env::var("EMBEDDING_DIMENSIONS")
                .map(|s| s.parse().unwrap_or(768))
                .unwrap_or(768),
            collection_name: std::env::var("QDRANT_COLLECTION")
                .unwrap_or_else(|_| "code_embeddings".to_string()),
            chat_model: std::env::var("CHAT_MODEL").unwrap_or_else(|_| "codellama:7b".to_string()),
            port: std::env::var("API_PORT")
                .map(|s| s.parse().unwrap_or(8080))
                .unwrap_or(8080),
            opencode_host: std::env::var("OPENCODE_HOST")
                .unwrap_or_else(|_| "http://opencode:4096".to_string()),
            opencode_auth_user: std::env::var("OPENCODE_AUTH_USER").ok(),
            opencode_auth_pass: std::env::var("OPENCODE_AUTH_PASS").ok(),
            docker_network: std::env::var("DOCKER_NETWORK")
                .unwrap_or_else(|_| "rustbrain".to_string()),
            opencode_image: std::env::var("OPENCODE_IMAGE")
                .unwrap_or_else(|_| "opencode:latest".to_string()),
            workspace_host_clone_root: std::env::var("WORKSPACE_HOST_CLONE_ROOT")
                .unwrap_or_else(|_| "/tmp/rustbrain-clones".to_string()),
            ingestion_image: std::env::var("INGESTION_IMAGE")
                .unwrap_or_else(|_| "rustbrain-ingestion:latest".to_string()),
            execution_timeout_secs: std::env::var("EXECUTION_TIMEOUT_SECS")
                .map(|s| s.parse().unwrap_or(7200))
                .unwrap_or(7200),
            public_host: std::env::var("RUSTBRAIN_PUBLIC_HOST")
                .ok()
                .filter(|s| !s.is_empty()),
            container_keep_alive_secs: std::env::var("RUSTBRAIN_CONTAINER_KEEP_ALIVE_SECS")
                .map(|s| s.parse().unwrap_or(0))
                .unwrap_or(0),
            opencode_ready_timeout_secs: std::env::var("OPENCODE_READY_TIMEOUT_SECS")
                .map(|s| s.parse().unwrap_or(60))
                .unwrap_or(60),
            opencode_config_host_path: std::env::var("OPENCODE_CONFIG_HOST_PATH")
                .ok()
                .filter(|s| !s.is_empty()),
            mcp_sse_url: std::env::var("MCP_SSE_URL")
                .unwrap_or_else(|_| "http://mcp-sse:3001/sse".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_config_fields() {
        let config = Config {
            database_url: "postgresql://user:pass@localhost:5432/db".to_string(),
            neo4j_uri: "bolt://neo4j:7687".to_string(),
            neo4j_user: "neo4j".to_string(),
            neo4j_password: "secret".to_string(),
            qdrant_host: "http://qdrant:6333".to_string(),
            ollama_host: "http://ollama:11434".to_string(),
            embedding_model: "nomic-embed-text".to_string(),
            embedding_dimensions: 768,
            collection_name: "code_embeddings".to_string(),
            chat_model: "codellama:7b".to_string(),
            port: 8080,
            opencode_host: "http://opencode:4096".to_string(),
            opencode_auth_user: None,
            opencode_auth_pass: None,
            docker_network: "rustbrain".to_string(),
            opencode_image: "opencode:latest".to_string(),
            workspace_host_clone_root: "/tmp/rustbrain-clones".to_string(),
            ingestion_image: "rustbrain-ingestion:latest".to_string(),
            execution_timeout_secs: 7200,
            public_host: None,
            container_keep_alive_secs: 0,
            opencode_ready_timeout_secs: 60,
            opencode_config_host_path: Some("/opt/rustbrain/configs/opencode".to_string()),
            mcp_sse_url: "http://mcp-sse:3001/sse".to_string(),
        };

        assert_eq!(config.embedding_dimensions, 768);
        assert_eq!(config.port, 8080);
        assert_eq!(config.embedding_model, "nomic-embed-text");
        assert_eq!(
            config.opencode_config_host_path.as_deref(),
            Some("/opt/rustbrain/configs/opencode")
        );
        assert_eq!(config.mcp_sse_url, "http://mcp-sse:3001/sse");
    }
}
