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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_definition_has_required_fields() {
        let def = definition();
        
        assert_eq!(def["name"], "get_module_tree");
        assert!(!def["description"].as_str().unwrap().is_empty());
        assert!(def["inputSchema"].is_object());
    }

    #[test]
    fn test_definition_schema_properties() {
        let schema = &definition()["inputSchema"];
        
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["crate_name"].is_object());
        
        let required = schema["required"].as_array().unwrap();
        assert!(required.contains(&serde_json::json!("crate_name")));
    }

    #[test]
    fn test_get_module_tree_request_deserialization() {
        let json = r#"{"crate_name": "my_crate"}"#;
        let request: GetModuleTreeRequest = serde_json::from_str(json).unwrap();
        
        assert_eq!(request.crate_name, "my_crate");
    }

    #[test]
    fn test_module_item_deserialization() {
        let json = r#"{
            "name": "my_function",
            "kind": "function",
            "visibility": "pub"
        }"#;
        
        let item: ModuleItem = serde_json::from_str(json).unwrap();
        
        assert_eq!(item.name, "my_function");
        assert_eq!(item.kind, "function");
        assert_eq!(item.visibility, "pub");
    }

    #[test]
    fn test_module_item_serialization() {
        let item = ModuleItem {
            name: "func".to_string(),
            kind: "function".to_string(),
            visibility: "pub".to_string(),
        };
        
        let json = serde_json::to_string(&item).unwrap();
        assert!(json.contains("\"name\":\"func\""));
        assert!(json.contains("\"kind\":\"function\""));
        assert!(json.contains("\"visibility\":\"pub\""));
    }

    #[test]
    fn test_module_node_deserialization() {
        let json = r#"{
            "name": "models",
            "path": "crate::models",
            "children": [],
            "items": [
                {
                    "name": "User",
                    "kind": "struct",
                    "visibility": "pub"
                }
            ]
        }"#;
        
        let node: ModuleNode = serde_json::from_str(json).unwrap();
        
        assert_eq!(node.name, "models");
        assert_eq!(node.path, "crate::models");
        assert!(node.children.is_empty());
        assert_eq!(node.items.len(), 1);
        assert_eq!(node.items[0].name, "User");
    }

    #[test]
    fn test_module_node_with_children() {
        let json = r#"{
            "name": "crate",
            "path": "crate",
            "children": [
                {
                    "name": "models",
                    "path": "crate::models",
                    "children": [],
                    "items": []
                }
            ],
            "items": []
        }"#;
        
        let node: ModuleNode = serde_json::from_str(json).unwrap();
        
        assert_eq!(node.children.len(), 1);
        assert_eq!(node.children[0].name, "models");
    }

    #[test]
    fn test_module_tree_response_deserialization() {
        let json = r#"{
            "crate_name": "my_crate",
            "root": {
                "name": "crate",
                "path": "crate",
                "children": [],
                "items": []
            }
        }"#;
        
        let response: ModuleTreeResponse = serde_json::from_str(json).unwrap();
        
        assert_eq!(response.crate_name, "my_crate");
        assert_eq!(response.root.name, "crate");
    }

    #[test]
    fn test_module_node_serialization() {
        let node = ModuleNode {
            name: "models".to_string(),
            path: "crate::models".to_string(),
            children: vec![],
            items: vec![ModuleItem {
                name: "User".to_string(),
                kind: "struct".to_string(),
                visibility: "pub".to_string(),
            }],
        };
        
        let json = serde_json::to_string(&node).unwrap();
        assert!(json.contains("\"name\":\"models\""));
        assert!(json.contains("\"path\":\"crate::models\""));
        assert!(json.contains("\"items\""));
    }

    #[test]
    fn test_module_tree_response_serialization() {
        let response = ModuleTreeResponse {
            crate_name: "my_crate".to_string(),
            root: ModuleNode {
                name: "crate".to_string(),
                path: "crate".to_string(),
                children: vec![],
                items: vec![],
            },
        };
        
        let json = serde_json::to_string(&response).unwrap();
        assert!(json.contains("\"crate_name\":\"my_crate\""));
        assert!(json.contains("\"root\""));
    }

    #[test]
    fn test_module_item_clone() {
        let item = ModuleItem {
            name: "func".to_string(),
            kind: "function".to_string(),
            visibility: "pub".to_string(),
        };
        
        let cloned = item.clone();
        assert_eq!(item.name, cloned.name);
        assert_eq!(item.kind, cloned.kind);
        assert_eq!(item.visibility, cloned.visibility);
    }
}
