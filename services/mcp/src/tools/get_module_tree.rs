//! MCP tool: get_module_tree
//!
//! Get the module structure of a crate

use crate::client::ApiClient;
use crate::error::Result;
use serde::Deserialize;
use tracing::instrument;

/// Request for module tree
#[derive(Debug, Deserialize)]
pub struct GetModuleTreeRequest {
    /// Name of the crate
    pub crate_name: String,
}

/// An item within a module
#[derive(Debug, serde::Serialize, Deserialize, Clone)]
pub struct ModuleItem {
    /// Name of the item
    pub name: String,
    /// Type of item (function, struct, enum, etc.)
    pub kind: String,
    /// Visibility (public, private, etc.)
    pub visibility: String,
}

/// A node in the module tree
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct ModuleNode {
    /// Name of the module
    pub name: String,
    /// Full path of the module
    pub path: String,
    /// Child modules
    pub children: Vec<ModuleNode>,
    /// Items defined in this module
    pub items: Vec<ModuleItem>,
}

/// Response with module tree
#[derive(Debug, serde::Serialize, Deserialize)]
pub struct ModuleTreeResponse {
    /// The crate that was queried
    pub crate_name: String,
    /// Root module node
    pub root: ModuleNode,
}

/// Execute the get_module_tree tool
#[instrument(skip(client))]
pub async fn execute(client: &ApiClient, request: GetModuleTreeRequest) -> Result<String> {
    let encoded_crate = url::form_urlencoded::byte_serialize(request.crate_name.as_bytes()).collect::<String>();
    let response: ModuleTreeResponse = client
        .get(&format!(
            "/tools/get_module_tree?crate_name={}",
            encoded_crate
        ))
        .await?;

    let mut output = format!("# Module Tree: `{}`\n\n", response.crate_name);

    fn render_module(node: &ModuleNode, output: &mut String, depth: usize) {
        let indent = "  ".repeat(depth);
        
        // Module header
        output.push_str(&format!(
            "{}## `{}`\n{}Path: `{}`\n\n",
            indent, node.name, indent, node.path
        ));

        // Items in this module
        if !node.items.is_empty() {
            output.push_str(&format!("{}Items:\n", indent));
            
            // Group items by kind
            let mut by_kind: std::collections::BTreeMap<String, Vec<&ModuleItem>> =
                std::collections::BTreeMap::new();
            for item in &node.items {
                by_kind.entry(item.kind.clone()).or_default().push(item);
            }

            for (kind, items) in by_kind {
                output.push_str(&format!("{}  **{}** ({}):\n", indent, kind, items.len()));
                for item in items {
                    let vis_marker = match item.visibility.as_str() {
                        "public" | "pub" => "+",
                        _ => "-",
                    };
                    output.push_str(&format!(
                        "{}    {} `{}`\n",
                        indent, vis_marker, item.name
                    ));
                }
            }
            output.push('\n');
        }

        // Child modules
        if !node.children.is_empty() {
            output.push_str(&format!("{}Modules:\n", indent));
            for child in &node.children {
                render_module(child, output, depth + 1);
            }
        }
    }

    render_module(&response.root, &mut output, 0);

    // Summary
    fn count_items(node: &ModuleNode) -> (usize, usize) {
        let items = node.items.len();
        let modules = node.children.len();
        let (child_items, child_modules) = node
            .children
            .iter()
            .map(count_items)
            .fold((0, 0), |(a, b), (c, d)| (a + c, b + d));
        (items + child_items, modules + child_modules)
    }

    let (total_items, total_modules) = count_items(&response.root);
    output.push_str(&format!(
        "\n**Summary:** {} modules, {} items\n",
        total_modules, total_items
    ));

    Ok(output)
}

/// Get the MCP tool definition
pub fn definition() -> serde_json::Value {
    serde_json::json!({
        "name": "get_module_tree",
        "description": "Get the hierarchical module structure of a crate. Shows all modules and their contents (functions, structs, enums, etc.) organized in a tree. Useful for understanding the organization of a crate.",
        "inputSchema": {
            "type": "object",
            "properties": {
                "crate_name": {
                    "type": "string",
                    "description": "Name of the crate to analyze"
                }
            },
            "required": ["crate_name"]
        }
    })
}
