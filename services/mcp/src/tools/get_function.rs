//! MCP tool: get_function
//!
//! Get detailed information about a function by its fully qualified name

use crate::client::ApiClient;
use crate::error::Result;
use serde::Deserialize;
use tracing::instrument;

/// Request for function details
#[derive(Debug, Deserialize)]
pub struct GetFunctionRequest {
    /// Fully qualified name of the function (e.g., "crate::module::function_name")
    pub fqn: String,
}

/// Information about a caller
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct CallerInfo {
    /// Fully qualified name of the caller
    pub fqn: String,
    /// Short name
    pub name: String,
    /// File path
    pub file_path: String,
    /// Line number
    pub line: u32,
}

/// Information about a callee
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct CalleeInfo {
    /// Fully qualified name of the callee
    pub fqn: String,
    /// Short name
    pub name: String,
}

/// Detailed function information
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct FunctionDetail {
    /// Fully qualified name
    pub fqn: String,
    /// Short name
    pub name: String,
    /// Type of item (function, method, etc.)
    pub kind: String,
    /// Visibility (public, private, etc.)
    pub visibility: Option<String>,
    /// Function signature
    pub signature: Option<String>,
    /// Documentation comment
    pub docstring: Option<String>,
    /// File path
    pub file_path: String,
    /// Start line
    pub start_line: u32,
    /// End line
    pub end_line: u32,
    /// Module path
    pub module_path: Option<String>,
    /// Crate name
    pub crate_name: Option<String>,
    /// Functions that call this function
    pub callers: Vec<CallerInfo>,
    /// Functions that this function calls
    pub callees: Vec<CalleeInfo>,
}

/// Execute the get_function tool
#[instrument(skip(client))]
pub async fn execute(client: &ApiClient, request: GetFunctionRequest) -> Result<String> {
    let encoded_fqn = url::form_urlencoded::byte_serialize(request.fqn.as_bytes()).collect::<String>();
    let response: FunctionDetail = client
        .get(&format!("/tools/get_function?fqn={}", encoded_fqn))
        .await?;

    let mut output = format!(
        "# {} `{}`\n\n",
        response.kind.to_uppercase(),
        response.fqn
    );

    // Basic info
    output.push_str(&format!(
        "**Location:** `{}:{}-{}`\n",
        response.file_path, response.start_line, response.end_line
    ));

    if let Some(ref visibility) = response.visibility {
        output.push_str(&format!("**Visibility:** {}\n", visibility));
    }

    if let Some(ref signature) = response.signature {
        output.push_str(&format!("**Signature:**\n```rust\n{}\n```\n", signature));
    }

    if let Some(ref module_path) = response.module_path {
        output.push_str(&format!("**Module:** {}\n", module_path));
    }

    if let Some(ref crate_name) = response.crate_name {
        output.push_str(&format!("**Crate:** {}\n", crate_name));
    }

    // Documentation
    if let Some(ref docstring) = response.docstring {
        if !docstring.is_empty() {
            output.push_str(&format!(
                "\n## Documentation\n\n{}\n",
                docstring
            ));
        }
    }

    // Callers
    if !response.callers.is_empty() {
        output.push_str(&format!(
            "\n## Callers ({})\n\n",
            response.callers.len()
        ));
        for caller in &response.callers {
            output.push_str(&format!(
                "- `{}` at `{}:{}`\n",
                caller.fqn, caller.file_path, caller.line
            ));
        }
    }

    // Callees
    if !response.callees.is_empty() {
        output.push_str(&format!(
            "\n## Callees ({})\n\n",
            response.callees.len()
        ));
        for callee in &response.callees {
            output.push_str(&format!("- `{}`\n", callee.fqn));
        }
    }

    Ok(output)
}

/// Get the MCP tool definition
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "get_function",
        "description": "Get detailed information about a function, method, or other code item by its fully qualified name. Returns signature, documentation, callers, and callees.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "fqn": {
                    "type": "string",
                    "description": "Fully qualified name of the item (e.g., 'crate::module::function_name'). Use search_code to find the FQN if you don't know it."
                }
            },
            "required": ["fqn"]
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_definition_has_required_fields() {
        let def = definition();
        
        assert_eq!(def["name"], "get_function");
        assert!(!def["description"].as_str().unwrap().is_empty());
        assert!(def["inputSchema"].is_object());
    }

    #[test]
    fn test_definition_schema_properties() {
        let schema = &definition()["inputSchema"];
        
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["fqn"].is_object());
        
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("fqn")));
    }

    #[test]
    fn test_get_function_request_deserialization() {
        let json = r#"{"fqn": "crate::module::function"}"#;
        let request: GetFunctionRequest = serde_json::from_str(json).unwrap();
        
        assert_eq!(request.fqn, "crate::module::function");
    }

    #[test]
    fn test_caller_info_deserialization() {
        let json = r#"{
            "fqn": "crate::caller::func",
            "name": "func",
            "file_path": "src/caller.rs",
            "line": 42
        }"#;
        
        let caller: CallerInfo = serde_json::from_str(json).unwrap();
        
        assert_eq!(caller.fqn, "crate::caller::func");
        assert_eq!(caller.name, "func");
        assert_eq!(caller.file_path, "src/caller.rs");
        assert_eq!(caller.line, 42);
    }

    #[test]
    fn test_callee_info_deserialization() {
        let json = r#"{
            "fqn": "crate::callee::func",
            "name": "func"
        }"#;
        
        let callee: CalleeInfo = serde_json::from_str(json).unwrap();
        
        assert_eq!(callee.fqn, "crate::callee::func");
        assert_eq!(callee.name, "func");
    }

    #[test]
    fn test_function_detail_deserialization() {
        let json = r#"{
            "fqn": "crate::module::function",
            "name": "function",
            "kind": "function",
            "visibility": "pub",
            "signature": "pub fn function(x: i32) -> String",
            "docstring": "A function",
            "file_path": "src/module.rs",
            "start_line": 10,
            "end_line": 20,
            "module_path": "crate::module",
            "crate_name": "my_crate",
            "callers": [],
            "callees": []
        }"#;
        
        let detail: FunctionDetail = serde_json::from_str(json).unwrap();
        
        assert_eq!(detail.fqn, "crate::module::function");
        assert_eq!(detail.name, "function");
        assert_eq!(detail.kind, "function");
        assert_eq!(detail.visibility, Some("pub".to_string()));
        assert_eq!(detail.signature, Some("pub fn function(x: i32) -> String".to_string()));
        assert_eq!(detail.docstring, Some("A function".to_string()));
        assert_eq!(detail.file_path, "src/module.rs");
        assert_eq!(detail.start_line, 10);
        assert_eq!(detail.end_line, 20);
        assert_eq!(detail.module_path, Some("crate::module".to_string()));
        assert_eq!(detail.crate_name, Some("my_crate".to_string()));
        assert!(detail.callers.is_empty());
        assert!(detail.callees.is_empty());
    }

    #[test]
    fn test_function_detail_with_callers_and_callees() {
        let json = r#"{
            "fqn": "crate::module::function",
            "name": "function",
            "kind": "function",
            "file_path": "src/module.rs",
            "start_line": 10,
            "end_line": 20,
            "callers": [
                {
                    "fqn": "crate::other::caller",
                    "name": "caller",
                    "file_path": "src/other.rs",
                    "line": 5
                }
            ],
            "callees": [
                {
                    "fqn": "crate::util::helper",
                    "name": "helper"
                }
            ]
        }"#;
        
        let detail: FunctionDetail = serde_json::from_str(json).unwrap();
        
        assert_eq!(detail.callers.len(), 1);
        assert_eq!(detail.callers[0].fqn, "crate::other::caller");
        
        assert_eq!(detail.callees.len(), 1);
        assert_eq!(detail.callees[0].fqn, "crate::util::helper");
    }

    #[test]
    fn test_function_detail_minimal() {
        let json = r#"{
            "fqn": "crate::func",
            "name": "func",
            "kind": "function",
            "file_path": "src/lib.rs",
            "start_line": 1,
            "end_line": 5,
            "callers": [],
            "callees": []
        }"#;
        
        let detail: FunctionDetail = serde_json::from_str(json).unwrap();
        
        assert_eq!(detail.visibility, None);
        assert_eq!(detail.signature, None);
        assert_eq!(detail.docstring, None);
        assert_eq!(detail.module_path, None);
        assert_eq!(detail.crate_name, None);
    }

    #[test]
    fn test_function_detail_serialization() {
        let detail = FunctionDetail {
            fqn: "crate::func".to_string(),
            name: "func".to_string(),
            kind: "function".to_string(),
            visibility: Some("pub".to_string()),
            signature: None,
            docstring: None,
            file_path: "src/lib.rs".to_string(),
            start_line: 1,
            end_line: 5,
            module_path: None,
            crate_name: None,
            callers: vec![],
            callees: vec![],
        };
        
        let json = serde_json::to_string(&detail).unwrap();
        assert!(json.contains("\"fqn\":\"crate::func\""));
        assert!(json.contains("\"kind\":\"function\""));
    }

    #[test]
    fn test_caller_info_serialization() {
        let caller = CallerInfo {
            fqn: "crate::caller".to_string(),
            name: "caller".to_string(),
            file_path: "src/lib.rs".to_string(),
            line: 10,
        };
        
        let json = serde_json::to_string(&caller).unwrap();
        assert!(json.contains("\"fqn\":\"crate::caller\""));
        assert!(json.contains("\"line\":10"));
    }

    #[test]
    fn test_callee_info_serialization() {
        let callee = CalleeInfo {
            fqn: "crate::callee".to_string(),
            name: "callee".to_string(),
        };
        
        let json = serde_json::to_string(&callee).unwrap();
        assert!(json.contains("\"fqn\":\"crate::callee\""));
        assert!(json.contains("\"name\":\"callee\""));
    }
}
