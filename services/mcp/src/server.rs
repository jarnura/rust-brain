//! MCP Server implementation
//!
//! Implements the Model Context Protocol for stdio transport

use crate::client::ApiClient;
use crate::config::Config;
use crate::error::{McpError, Result};
use crate::tools;
use serde::{Deserialize, Serialize};
use std::io::{self, BufRead, Write};
use tracing::{debug, error, info, instrument};

/// MCP Protocol version
pub const PROTOCOL_VERSION: &str = "2024-11-05";

/// MCP Server name
pub const SERVER_NAME: &str = "rustbrain-mcp";

/// MCP Server version
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

// =============================================================================
// MCP Protocol Types
// Note: Field names use camelCase to match the MCP JSON-RPC specification
// =============================================================================

/// JSON-RPC request
#[derive(Debug, Deserialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Option<Id>,
    pub method: String,
    #[serde(default)]
    pub params: Option<serde_json::Value>,
}

/// JSON-RPC response
#[derive(Debug, Serialize)]
pub struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Id>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

/// JSON-RPC error
#[derive(Debug, Serialize)]
pub struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

/// Request/Response ID
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    String(String),
}

// =============================================================================
// MCP Method Types
// =============================================================================

#[allow(non_snake_case)]
/// Initialize request
#[derive(Debug, Deserialize)]
pub struct InitializeRequest {
    pub protocolVersion: String,
    pub capabilities: ClientCapabilities,
    #[serde(default)]
    pub clientInfo: Option<Implementation>,
}

/// Client capabilities
#[derive(Debug, Deserialize, Default)]
pub struct ClientCapabilities {
    #[serde(default)]
    pub roots: Option<RootsCapability>,
    #[serde(default)]
    pub sampling: Option<()>,
}

#[allow(non_snake_case)]
/// Roots capability
#[derive(Debug, Deserialize, Default)]
pub struct RootsCapability {
    #[serde(default)]
    pub listChanged: Option<bool>,
}

/// Server capabilities
#[derive(Debug, Serialize)]
pub struct ServerCapabilities {
    pub tools: ToolsCapability,
}

#[allow(non_snake_case)]
/// Tools capability
#[derive(Debug, Serialize)]
pub struct ToolsCapability {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listChanged: Option<bool>,
}

/// Implementation info
#[derive(Debug, Serialize, Deserialize)]
pub struct Implementation {
    pub name: String,
    pub version: String,
}

#[allow(non_snake_case)]
/// Initialize result
#[derive(Debug, Serialize)]
pub struct InitializeResult {
    pub protocolVersion: String,
    pub capabilities: ServerCapabilities,
    pub serverInfo: Implementation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[allow(non_snake_case)]
/// Tool definition
#[derive(Debug, Serialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub inputSchema: serde_json::Value,
}

/// Tools list result
#[derive(Debug, Serialize)]
pub struct ToolsListResult {
    pub tools: Vec<Tool>,
}

/// Tool call request
#[derive(Debug, Deserialize)]
pub struct ToolCallRequest {
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

#[allow(non_snake_case)]
/// Tool call result
#[derive(Debug, Serialize)]
pub struct ToolCallResult {
    pub content: Vec<Content>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isError: Option<bool>,
}

#[allow(non_snake_case)]
/// Content types
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
pub enum Content {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "image")]
    Image { data: String, mimeType: String },
    #[serde(rename = "resource")]
    Resource { resource: Resource },
}

#[allow(non_snake_case)]
/// Resource reference
#[derive(Debug, Serialize)]
pub struct Resource {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mimeType: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

// =============================================================================
// Server Implementation
// =============================================================================

/// MCP Server
pub struct McpServer {
    config: Config,
    client: ApiClient,
    initialized: bool,
}

impl McpServer {
    /// Create a new MCP server
    pub fn new(config: Config) -> Result<Self> {
        let client = ApiClient::new(&config)?;
        Ok(Self {
            config,
            client,
            initialized: false,
        })
    }

    /// Run the MCP server (stdio transport)
    pub async fn run(&mut self) -> Result<()> {
        info!("Starting MCP server (stdio transport)");
        info!("API base URL: {}", self.config.api_base_url);

        let stdin = io::stdin();
        let mut stdout = io::stdout();

        for line in stdin.lock().lines() {
            let line = match line {
                Ok(l) => l,
                Err(e) => {
                    error!("Failed to read from stdin: {}", e);
                    continue;
                }
            };

            if line.trim().is_empty() {
                continue;
            }

            debug!("Received: {}", line);

            let response = self.handle_message(&line).await;

            let response_json = match response {
                Ok(Some(r)) => serde_json::to_string(&r)?,
                Ok(None) => continue, // Notification, no response
                Err(e) => {
                    error!("Error handling message: {}", e);
                    serde_json::to_string(&self.error_response(None, -32603, &e.to_string()))?
                }
            };

            debug!("Sending: {}", response_json);
            writeln!(stdout, "{}", response_json)?;
            stdout.flush()?;
        }

        Ok(())
    }

    /// Handle an incoming message
    #[instrument(skip(self))]
    async fn handle_message(&mut self, line: &str) -> Result<Option<JsonRpcResponse>> {
        let request: JsonRpcRequest = match serde_json::from_str(line) {
            Ok(r) => r,
            Err(e) => {
                return Ok(Some(self.error_response(
                    None,
                    -32700,
                    &format!("Parse error: {}", e),
                )));
            }
        };

        // Validate JSON-RPC version
        if request.jsonrpc != "2.0" {
            return Ok(Some(self.error_response(
                request.id,
                -32600,
                "Invalid JSON-RPC version",
            )));
        }

        // Handle the method
        let result = match request.method.as_str() {
            "initialize" => self.handle_initialize(request.params).await?,
            "notifications/initialized" => {
                self.initialized = true;
                info!("Client initialized");
                return Ok(None); // No response for notifications
            }
            "tools/list" => {
                if !self.initialized {
                    return Ok(Some(self.error_response(
                        request.id,
                        -32002,
                        "Server not initialized",
                    )));
                }
                self.handle_tools_list().await?
            }
            "tools/call" => {
                if !self.initialized {
                    return Ok(Some(self.error_response(
                        request.id,
                        -32002,
                        "Server not initialized",
                    )));
                }
                self.handle_tools_call(request.params).await?
            }
            "ping" => serde_json::json!({}),
            _ => {
                return Ok(Some(self.error_response(
                    request.id,
                    -32601,
                    &format!("Method not found: {}", request.method),
                )));
            }
        };

        Ok(Some(JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: request.id,
            result: Some(result),
            error: None,
        }))
    }

    /// Handle initialize request
    async fn handle_initialize(
        &mut self,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let _request: Option<InitializeRequest> = params
            .map(|p| serde_json::from_value(p))
            .transpose()?;

        let result = InitializeResult {
            protocolVersion: PROTOCOL_VERSION.to_string(),
            capabilities: ServerCapabilities {
                tools: ToolsCapability {
                    listChanged: None,
                },
            },
            serverInfo: Implementation {
                name: SERVER_NAME.to_string(),
                version: SERVER_VERSION.to_string(),
            },
            instructions: Some(
                "Rust-brain MCP server provides code intelligence tools for Rust codebases. \
                 Use search_code to find functions by semantic similarity, \
                 get_function to get detailed information, \
                 get_callers to trace call graphs, \
                 get_trait_impls to find trait implementations, \
                 find_type_usages to find type references, \
                 get_module_tree to explore module structure, \
                 and query_graph for custom graph queries."
                    .to_string(),
            ),
        };

        Ok(serde_json::to_value(result)?)
    }

    /// Handle tools/list request
    async fn handle_tools_list(&self) -> Result<serde_json::Value> {
        let definitions = tools::all_definitions();
        let tools_list: Vec<Tool> = definitions
            .into_iter()
            .filter_map(|d| {
                let name = d.get("name")?.as_str()?.to_string();
                let description = d.get("description")?.as_str()?.to_string();
                let input_schema = d.get("inputSchema")?.clone();
                Some(Tool {
                    name,
                    description,
                    inputSchema: input_schema,
                })
            })
            .collect();

        Ok(serde_json::to_value(ToolsListResult { tools: tools_list })?)
    }

    /// Handle tools/call request
    async fn handle_tools_call(
        &self,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value> {
        let request: ToolCallRequest = serde_json::from_value(
            params.ok_or_else(|| McpError::InvalidRequest("Missing params".to_string()))?
        )?;

        let arguments = if request.arguments.is_null() {
            serde_json::json!({})
        } else {
            request.arguments
        };

        debug!("Tool call: {} with args {:?}", request.name, arguments);

        let result = tools::execute_tool(&self.client, &request.name, arguments).await;

        let is_error = result.is_err();

        let content = match result {
            Ok(text) => vec![Content::Text { text }],
            Err(e) => {
                error!("Tool error: {}", e);
                vec![
                    Content::Text {
                        text: format!("Error: {}", e),
                    },
                ]
            }
        };

        Ok(serde_json::to_value(ToolCallResult {
            content,
            isError: Some(is_error),
        })?)
    }

    /// Create an error response
    fn error_response(&self, id: Option<Id>, code: i32, message: &str) -> JsonRpcResponse {
        JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id,
            result: None,
            error: Some(JsonRpcError {
                code,
                message: message.to_string(),
                data: None,
            }),
        }
    }
}
