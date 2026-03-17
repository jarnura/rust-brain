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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
#[derive(Debug, Serialize, Deserialize)]
pub struct ServerCapabilities {
    pub tools: ToolsCapability,
}

#[allow(non_snake_case)]
/// Tools capability
#[derive(Debug, Serialize, Deserialize)]
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
#[derive(Debug, Serialize, Deserialize)]
pub struct InitializeResult {
    pub protocolVersion: String,
    pub capabilities: ServerCapabilities,
    pub serverInfo: Implementation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
}

#[allow(non_snake_case)]
/// Tool definition
#[derive(Debug, Serialize, Deserialize)]
pub struct Tool {
    pub name: String,
    pub description: String,
    pub inputSchema: serde_json::Value,
}

/// Tools list result
#[derive(Debug, Serialize, Deserialize)]
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
                    serde_json::to_string(&self.error_response(None, -32603, "Internal error"))?
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
    pub async fn handle_message(&mut self, line: &str) -> Result<Option<JsonRpcResponse>> {
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
                // Return a client-safe message; internal details are logged above.
                let client_message = match &e {
                    McpError::NotFound(msg) => format!("Not found: {}", msg),
                    McpError::InvalidRequest(msg) => format!("Invalid request: {}", msg),
                    McpError::Api(msg) => format!("API error: {}", msg),
                    // Http errors may contain URLs; IO errors may contain file paths.
                    _ => "Internal error".to_string(),
                };
                vec![Content::Text { text: client_message }]
            }
        };

        Ok(serde_json::to_value(ToolCallResult {
            content,
            isError: Some(is_error),
        })?)
    }

    /// Create an error response
    pub fn error_response(&self, id: Option<Id>, code: i32, message: &str) -> JsonRpcResponse {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            transport: crate::config::Transport::Stdio,
            api_base_url: "http://localhost:8088".to_string(),
            http_timeout: 5,
            #[cfg(feature = "sse")]
            port: 3001,
            max_search_results: 50,
            default_search_limit: 10,
            opencode_host: "http://opencode:4096".to_string(),
            opencode_auth_user: None,
            opencode_auth_pass: None,
        }
    }

    fn create_server() -> McpServer {
        McpServer::new(test_config()).unwrap()
    }

    // === JSON-RPC Request Parsing Tests ===

    #[test]
    fn test_parse_valid_jsonrpc_request() {
        let json = r#"{"jsonrpc": "2.0", "id": 1, "method": "ping", "params": null}"#;
        let request: JsonRpcRequest = serde_json::from_str(json).unwrap();
        
        assert_eq!(request.jsonrpc, "2.0");
        assert_eq!(request.id, Some(Id::Number(1)));
        assert_eq!(request.method, "ping");
    }

    #[test]
    fn test_parse_request_with_string_id() {
        let json = r#"{"jsonrpc": "2.0", "id": "abc-123", "method": "test"}"#;
        let request: JsonRpcRequest = serde_json::from_str(json).unwrap();
        
        assert_eq!(request.id, Some(Id::String("abc-123".to_string())));
    }

    #[test]
    fn test_parse_request_without_params() {
        let json = r#"{"jsonrpc": "2.0", "id": 1, "method": "initialize"}"#;
        let request: JsonRpcRequest = serde_json::from_str(json).unwrap();
        
        assert_eq!(request.params, None);
    }

    #[test]
    fn test_parse_request_with_object_params() {
        let json = r#"{"jsonrpc": "2.0", "id": 1, "method": "test", "params": {"key": "value"}}"#;
        let request: JsonRpcRequest = serde_json::from_str(json).unwrap();
        
        assert!(request.params.is_some());
        let params = request.params.unwrap();
        assert_eq!(params["key"], "value");
    }

    #[test]
    fn test_parse_request_without_id() {
        // Notification - no id
        let json = r#"{"jsonrpc": "2.0", "method": "notify"}"#;
        let request: JsonRpcRequest = serde_json::from_str(json).unwrap();
        
        assert_eq!(request.id, None);
    }

    // === JSON-RPC Response Tests ===

    #[test]
    fn test_serialize_success_response() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(Id::Number(1)),
            result: Some(serde_json::json!({"status": "ok"})),
            error: None,
        };
        
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"jsonrpc\":\"2.0\""));
        assert!(json.contains("\"id\":1"));
        assert!(json.contains("\"result\""));
        assert!(!json.contains("\"error\""));
    }

    #[test]
    fn test_serialize_error_response() {
        let response = JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: Some(Id::Number(1)),
            result: None,
            error: Some(JsonRpcError {
                code: -32600,
                message: "Invalid request".to_string(),
                data: None,
            }),
        };
        
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"error\""));
        assert!(json.contains("\"code\":-32600"));
        assert!(json.contains("\"message\":\"Invalid request\""));
        assert!(!json.contains("\"result\""));
    }

    #[test]
    fn test_error_response_with_data() {
        let error = JsonRpcError {
            code: -32602,
            message: "Invalid params".to_string(),
            data: Some(serde_json::json!({"field": "query"})),
        };
        
        let json = serde_json::to_string(&error).unwrap();
        assert!(json.contains("\"data\""));
        assert!(json.contains("\"field\":\"query\""));
    }

    // === Initialize Tests ===

    #[tokio::test]
    async fn test_handle_initialize() {
        let mut server = create_server();
        
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        });
        
        let result = server.handle_initialize(Some(params)).await.unwrap();
        let init_result: InitializeResult = serde_json::from_value(result).unwrap();
        
        assert_eq!(init_result.protocolVersion, PROTOCOL_VERSION);
        assert_eq!(init_result.serverInfo.name, SERVER_NAME);
        assert!(init_result.instructions.is_some());
    }

    #[tokio::test]
    async fn test_handle_initialize_minimal() {
        let mut server = create_server();
        
        let result = server.handle_initialize(None).await.unwrap();
        let init_result: InitializeResult = serde_json::from_value(result).unwrap();
        
        assert_eq!(init_result.protocolVersion, PROTOCOL_VERSION);
    }

    // === Tools List Tests ===

    #[tokio::test]
    async fn test_handle_tools_list() {
        let server = create_server();
        
        let result = server.handle_tools_list().await.unwrap();
        let tools_result: ToolsListResult = serde_json::from_value(result).unwrap();
        
        // Should have all 7 tools
        assert_eq!(tools_result.tools.len(), 7);
        
        let tool_names: Vec<&str> = tools_result.tools.iter()
            .map(|t| t.name.as_str())
            .collect();
        
        assert!(tool_names.contains(&"search_code"));
        assert!(tool_names.contains(&"get_function"));
        assert!(tool_names.contains(&"get_callers"));
        assert!(tool_names.contains(&"get_trait_impls"));
        assert!(tool_names.contains(&"find_type_usages"));
        assert!(tool_names.contains(&"get_module_tree"));
        assert!(tool_names.contains(&"query_graph"));
    }

    #[tokio::test]
    async fn test_tool_definitions_have_required_fields() {
        let server = create_server();
        
        let result = server.handle_tools_list().await.unwrap();
        let tools_result: ToolsListResult = serde_json::from_value(result).unwrap();
        
        for tool in &tools_result.tools {
            assert!(!tool.name.is_empty());
            assert!(!tool.description.is_empty());
            assert!(tool.inputSchema.is_object());
        }
    }

    // === Error Response Tests ===

    #[test]
    fn test_error_response_creation() {
        let server = create_server();
        
        let response = server.error_response(Some(Id::Number(1)), -32601, "Method not found");
        
        assert_eq!(response.jsonrpc, "2.0");
        assert_eq!(response.id, Some(Id::Number(1)));
        assert!(response.error.is_some());
        assert!(response.result.is_none());
        
        let error = response.error.unwrap();
        assert_eq!(error.code, -32601);
        assert_eq!(error.message, "Method not found");
    }

    // === Message Handling Tests ===

    #[tokio::test]
    async fn test_handle_invalid_json() {
        let mut server = create_server();
        
        let result = server.handle_message("not valid json").await.unwrap();
        assert!(result.is_some());
        
        let response = result.unwrap();
        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert_eq!(error.code, -32700); // Parse error
    }

    #[tokio::test]
    async fn test_handle_invalid_jsonrpc_version() {
        let mut server = create_server();
        
        let json = r#"{"jsonrpc": "1.0", "id": 1, "method": "test"}"#;
        let result = server.handle_message(json).await.unwrap();
        
        let response = result.unwrap();
        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert_eq!(error.code, -32600); // Invalid request
    }

    #[tokio::test]
    async fn test_handle_unknown_method() {
        let mut server = create_server();
        server.initialized = true;
        
        let json = r#"{"jsonrpc": "2.0", "id": 1, "method": "unknown_method"}"#;
        let result = server.handle_message(json).await.unwrap();
        
        let response = result.unwrap();
        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert_eq!(error.code, -32601); // Method not found
    }

    #[tokio::test]
    async fn test_handle_ping() {
        let mut server = create_server();
        server.initialized = true;
        
        let json = r#"{"jsonrpc": "2.0", "id": 1, "method": "ping"}"#;
        let result = server.handle_message(json).await.unwrap();
        
        let response = result.unwrap();
        assert!(response.result.is_some());
        assert!(response.error.is_none());
    }

    #[tokio::test]
    async fn test_tools_list_before_initialized() {
        let mut server = create_server();
        // Don't set initialized to true
        
        let json = r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/list"}"#;
        let result = server.handle_message(json).await.unwrap();
        
        let response = result.unwrap();
        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert_eq!(error.code, -32002); // Server not initialized
    }

    #[tokio::test]
    async fn test_tools_call_before_initialized() {
        let mut server = create_server();
        // Don't set initialized to true
        
        let json = r#"{"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {"name": "search_code", "arguments": {}}}"#;
        let result = server.handle_message(json).await.unwrap();
        
        let response = result.unwrap();
        assert!(response.error.is_some());
        let error = response.error.unwrap();
        assert_eq!(error.code, -32002); // Server not initialized
    }

    #[tokio::test]
    async fn test_initialized_notification() {
        let mut server = create_server();
        server.initialized = false;
        
        let json = r#"{"jsonrpc": "2.0", "method": "notifications/initialized"}"#;
        let result = server.handle_message(json).await.unwrap();
        
        // Notifications return None (no response)
        assert!(result.is_none());
        assert!(server.initialized);
    }

    // === Tool Call Request Tests ===

    #[test]
    fn test_parse_tool_call_request() {
        let json = r#"{"name": "search_code", "arguments": {"query": "test"}}"#;
        let request: ToolCallRequest = serde_json::from_str(json).unwrap();
        
        assert_eq!(request.name, "search_code");
        assert_eq!(request.arguments["query"], "test");
    }

    #[test]
    fn test_parse_tool_call_request_no_args() {
        let json = r#"{"name": "get_function"}"#;
        let request: ToolCallRequest = serde_json::from_str(json).unwrap();
        
        assert_eq!(request.name, "get_function");
        assert!(request.arguments.is_null());
    }

    // === Content Type Tests ===

    #[test]
    fn test_serialize_text_content() {
        let content = Content::Text {
            text: "Hello, world!".to_string(),
        };
        
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("\"type\":\"text\""));
        assert!(json.contains("\"text\":\"Hello, world!\""));
    }

    #[test]
    fn test_serialize_image_content() {
        let content = Content::Image {
            data: "base64data".to_string(),
            mimeType: "image/png".to_string(),
        };
        
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("\"type\":\"image\""));
        assert!(json.contains("\"data\":\"base64data\""));
        assert!(json.contains("\"mimeType\":\"image/png\""));
    }

    #[test]
    fn test_serialize_resource_content() {
        let content = Content::Resource {
            resource: Resource {
                uri: "file:///test.rs".to_string(),
                mimeType: Some("text/x-rust".to_string()),
                text: Some("fn main() {}".to_string()),
            },
        };
        
        let json = serde_json::to_string(&content).unwrap();
        assert!(json.contains("\"type\":\"resource\""));
        assert!(json.contains("\"uri\":\"file:///test.rs\""));
    }

    // === Tool Call Result Tests ===

    #[test]
    fn test_serialize_tool_call_result() {
        let result = ToolCallResult {
            content: vec![Content::Text {
                text: "Success".to_string(),
            }],
            isError: Some(false),
        };
        
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"content\""));
        assert!(json.contains("\"isError\":false"));
    }

    #[test]
    fn test_serialize_tool_call_error_result() {
        let result = ToolCallResult {
            content: vec![Content::Text {
                text: "Error: Something went wrong".to_string(),
            }],
            isError: Some(true),
        };
        
        let json = serde_json::to_string(&result).unwrap();
        assert!(json.contains("\"isError\":true"));
    }

    // === ID Type Tests ===

    #[test]
    fn test_id_number_serialization() {
        let id = Id::Number(42);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "42");
    }

    #[test]
    fn test_id_string_serialization() {
        let id = Id::String("abc".to_string());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"abc\"");
    }

    #[test]
    fn test_id_deserialization_number() {
        let id: Id = serde_json::from_str("42").unwrap();
        assert_eq!(id, Id::Number(42));
    }

    #[test]
    fn test_id_deserialization_string() {
        let id: Id = serde_json::from_str("\"abc\"").unwrap();
        assert_eq!(id, Id::String("abc".to_string()));
    }
}
