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
