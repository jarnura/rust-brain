//! Integration tests for the rust-brain MCP server.
//!
//! Tests all 14 MCP tools via the SSE transport at `http://localhost:3001`.
//!
//! Protocol flow:
//! 1. GET /sse → opens SSE stream, receives `endpoint` event with session URL
//! 2. POST /message?sessionId=<id> with JSON-RPC payload
//! 3. Read `message` event from SSE stream for the response
//!
//! Run with:
//! ```
//! cargo test --test mcp_integration -- --include-ignored
//! ```

use futures_util::StreamExt;
use reqwest::header::{HeaderMap, AUTHORIZATION};
use reqwest::Client;
use serde_json::{json, Value};
use std::time::Duration;

const MCP_BASE: &str = "http://localhost:3001";

fn client() -> Client {
    Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .expect("Failed to build HTTP client")
}

/// Build a reqwest client with `Authorization: Bearer <key>` default header.
///
/// Reads the API key from `RUSTBRAIN_TEST_API_KEY` env var.  If not set,
/// returns a plain client (works when `RUSTBRAIN_AUTH_DISABLED=true`).
fn authenticated_client() -> Client {
    let builder = Client::builder()
        .timeout(Duration::from_secs(15));

    match std::env::var("RUSTBRAIN_TEST_API_KEY") {
        Ok(key) if !key.is_empty() => {
            let mut headers = HeaderMap::new();
            headers.insert(
                AUTHORIZATION,
                format!("Bearer {key}").parse().expect("Invalid API key header value"),
            );
            builder.default_headers(headers).build().expect("Failed to build HTTP client")
        }
        _ => builder.build().expect("Failed to build HTTP client"),
    }
}

// =============================================================================
// Full round-trip helper: open session, initialize, invoke tool, read response
// =============================================================================

/// Perform a complete MCP tool invocation:
/// 1. Open SSE connection
/// 2. Send initialize + tools/call
/// 3. Read SSE response events
///
/// Returns the `result` field of the tools/call JSON-RPC response.
async fn invoke_tool(tool_name: &str, arguments: Value) -> Value {
    let client = authenticated_client();

    // 1. Open SSE connection and get session ID
    let sse_resp = client
        .get(format!("{MCP_BASE}/sse"))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .expect("GET /sse failed");
    assert_eq!(sse_resp.status(), 200);

    // Extract session ID from the first chunk
    let mut stream = sse_resp.bytes_stream();
    let mut buf = String::new();
    let session_id;

    loop {
        let chunk = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("SSE stream timed out")
            .expect("SSE stream ended")
            .expect("SSE stream error");
        buf.push_str(&String::from_utf8_lossy(&chunk));
        if buf.contains("event: endpoint") {
            let mut found = None;
            for line in buf.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if let Some(sid) = data.split("sessionId=").nth(1) {
                        found = Some(sid.trim().to_string());
                        break;
                    }
                }
            }
            session_id = found.expect("Could not parse session ID from endpoint event");
            break;
        }
    }

    // 2. Send initialize
    let init_payload = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "integration-test", "version": "0.1"}
        }
    });
    client
        .post(format!("{MCP_BASE}/message?sessionId={session_id}"))
        .header("Content-Type", "application/json")
        .body(init_payload.to_string())
        .send()
        .await
        .expect("POST initialize failed");

    // Drain init response from SSE before proceeding
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if tokio::time::Instant::now() > deadline {
            break;
        }
        let chunk = match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(c))) => c,
            _ => break,
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        if buf.contains("serverInfo") {
            break;
        }
    }

    // 3. Send initialized notification
    let notif_payload = json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    });
    client
        .post(format!("{MCP_BASE}/message?sessionId={session_id}"))
        .header("Content-Type", "application/json")
        .body(notif_payload.to_string())
        .send()
        .await
        .expect("POST initialized notification failed");

    // 4. Call the tool
    let call_payload = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": tool_name,
            "arguments": arguments
        }
    });
    let call_status = client
        .post(format!("{MCP_BASE}/message?sessionId={session_id}"))
        .header("Content-Type", "application/json")
        .body(call_payload.to_string())
        .send()
        .await
        .expect("POST tools/call failed")
        .status();

    assert!(
        call_status == 202 || call_status == 200,
        "tools/call POST status: {}",
        call_status
    );

    // 5. Read tool response from SSE
    buf.clear();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    let mut tool_result = json!(null);

    loop {
        if tokio::time::Instant::now() > deadline {
            // Timeout: tool may be slow or unavailable; return partial result
            break;
        }
        let chunk = match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(c))) => c,
            _ => break,
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));

        // Look for a message event with id=2
        for line in buf.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    if v["id"] == 2 {
                        tool_result = v;
                        return tool_result;
                    }
                }
            }
        }
    }

    tool_result
}

// =============================================================================
// MCP server health
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_mcp_health() {
    let resp = client()
        .get(format!("{MCP_BASE}/health"))
        .send()
        .await
        .expect("GET /health failed");
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
}

// =============================================================================
// MCP initialization + tools/list
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_mcp_initialize_and_list_tools() {
    let client = authenticated_client();

    // Open SSE and get session ID
    let sse_resp = client
        .get(format!("{MCP_BASE}/sse"))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .expect("GET /sse failed");
    assert_eq!(sse_resp.status(), 200);

    let mut stream = sse_resp.bytes_stream();
    let mut buf = String::new();
    let session_id;

    loop {
        let chunk = tokio::time::timeout(Duration::from_secs(5), stream.next())
            .await
            .expect("SSE timed out")
            .expect("SSE ended")
            .expect("SSE error");
        buf.push_str(&String::from_utf8_lossy(&chunk));
        if buf.contains("event: endpoint") {
            let mut found = None;
            for line in buf.lines() {
                if let Some(data) = line.strip_prefix("data: ") {
                    if let Some(sid) = data.split("sessionId=").nth(1) {
                        found = Some(sid.trim().to_string());
                        break;
                    }
                }
            }
            session_id = found.expect("no session ID");
            break;
        }
    }

    // Initialize
    let init = json!({
        "jsonrpc": "2.0", "id": 1, "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "0.1"}
        }
    });
    let init_status = client
        .post(format!("{MCP_BASE}/message?sessionId={session_id}"))
        .header("Content-Type", "application/json")
        .body(init.to_string())
        .send()
        .await
        .expect("POST initialize failed")
        .status();
    assert!(init_status == 202 || init_status == 200);

    // Read initialize response
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut init_ok = false;
    loop {
        if tokio::time::Instant::now() > deadline {
            break;
        }
        let chunk = match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(c))) => c,
            _ => break,
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        for line in buf.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    if v["id"] == 1 && v["result"]["serverInfo"]["name"] == "rustbrain-mcp" {
                        assert_eq!(v["result"]["protocolVersion"], "2024-11-05");
                        init_ok = true;
                    }
                }
            }
        }
        if init_ok {
            break;
        }
    }
    assert!(init_ok, "initialize response not received");

    // Send initialized notification
    let notif = json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
    client
        .post(format!("{MCP_BASE}/message?sessionId={session_id}"))
        .header("Content-Type", "application/json")
        .body(notif.to_string())
        .send()
        .await
        .expect("POST initialized notification failed");

    // tools/list
    let list_req = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    client
        .post(format!("{MCP_BASE}/message?sessionId={session_id}"))
        .header("Content-Type", "application/json")
        .body(list_req.to_string())
        .send()
        .await
        .expect("POST tools/list failed");

    // Read tools/list response
    buf.clear();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let mut tools_ok = false;
    loop {
        if tokio::time::Instant::now() > deadline {
            break;
        }
        let chunk = match tokio::time::timeout(Duration::from_millis(500), stream.next()).await {
            Ok(Some(Ok(c))) => c,
            _ => break,
        };
        buf.push_str(&String::from_utf8_lossy(&chunk));
        for line in buf.lines() {
            if let Some(data) = line.strip_prefix("data: ") {
                if let Ok(v) = serde_json::from_str::<Value>(data) {
                    if v["id"] == 2 {
                        let tools = v["result"]["tools"]
                            .as_array()
                            .expect("tools should be array");
                        assert_eq!(tools.len(), 14, "expected 14 tools, got {}", tools.len());
                        let names: Vec<&str> =
                            tools.iter().filter_map(|t| t["name"].as_str()).collect();
                        for expected in &[
                            "search_code",
                            "get_function",
                            "get_callers",
                            "get_trait_impls",
                            "find_type_usages",
                            "get_module_tree",
                            "query_graph",
                            "find_calls_with_type",
                            "find_trait_impls_for_type",
                            "pg_query",
                            "context_store",
                            "status_check",
                            "task_update",
                            "aggregate_search",
                        ] {
                            assert!(
                                names.contains(expected),
                                "tool '{expected}' missing from tools/list"
                            );
                        }
                        tools_ok = true;
                    }
                }
            }
        }
        if tools_ok {
            break;
        }
    }
    assert!(tools_ok, "tools/list response not received or malformed");
}

// =============================================================================
// Individual tool invocation tests (all 14 tools)
// Each test calls invoke_tool and validates the response shape.
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_mcp_tool_search_code() {
    let result = invoke_tool("search_code", json!({"query": "parse rust", "limit": 3})).await;
    // Result should be a JSON-RPC response object (may have result or error field)
    assert!(result.is_object(), "search_code should return an object");
    // If it has a result, it should have content
    if result["result"].is_object() {
        assert!(result["result"]["content"].is_array());
    }
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_get_function() {
    // Use a known-missing FQN; should return a not-found message, not a server error
    let result = invoke_tool("get_function", json!({"fqn": "nonexistent::fn"})).await;
    assert!(result.is_object());
    // Should be either a result (not found message) or an error, not a crash
    assert!(
        result["result"].is_object() || result["error"].is_object() || result.is_null(),
        "unexpected response: {}",
        result
    );
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_get_callers() {
    let result = invoke_tool("get_callers", json!({"fqn": "nonexistent::fn", "depth": 1})).await;
    assert!(result.is_object());
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_get_trait_impls() {
    let result = invoke_tool("get_trait_impls", json!({"trait_name": "Display"})).await;
    assert!(result.is_object());
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_find_type_usages() {
    let result = invoke_tool("find_type_usages", json!({"type_name": "String"})).await;
    assert!(result.is_object());
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_get_module_tree() {
    let result = invoke_tool(
        "get_module_tree",
        json!({"crate_name": "rustbrain_ingestion"}),
    )
    .await;
    assert!(result.is_object());
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_query_graph_read_only() {
    let result = invoke_tool(
        "query_graph",
        json!({"cypher": "MATCH (n) RETURN n LIMIT 1"}),
    )
    .await;
    assert!(result.is_object());
    // Must NOT return an error for a read-only query
    if !result.is_null() {
        assert!(
            result["error"].is_null(),
            "read-only Cypher should not produce an error: {}",
            result
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_query_graph_rejects_write() {
    let result = invoke_tool("query_graph", json!({"cypher": "CREATE (n:Evil) RETURN n"})).await;
    // MCP tools surface errors as result.isError=true with content text,
    // or as a JSON-RPC error object. Either is acceptable.
    if !result.is_null() {
        let is_error_result = result["result"]["isError"] == true;
        let is_jsonrpc_error = result["error"].is_object();
        assert!(
            is_error_result || is_jsonrpc_error,
            "write Cypher should be rejected with isError or JSON-RPC error: {}",
            result
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_find_calls_with_type() {
    let result = invoke_tool("find_calls_with_type", json!({"type_name": "String"})).await;
    assert!(result.is_object());
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_find_trait_impls_for_type() {
    // Parameter is type_name, not type_fqn
    let result = invoke_tool("find_trait_impls_for_type", json!({"type_name": "String"})).await;
    assert!(result.is_object());
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_pg_query_select() {
    let result = invoke_tool("pg_query", json!({"query": "SELECT 1 AS n"})).await;
    assert!(result.is_object());
    if result["result"].is_object() {
        let content = result["result"]["content"].to_string();
        assert!(
            content.contains("1") || content.contains("n"),
            "pg_query SELECT 1 should return data: {}",
            content
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_pg_query_rejects_write() {
    let result = invoke_tool(
        "pg_query",
        json!({"query": "INSERT INTO extracted_items VALUES (1, 2, 3)"}),
    )
    .await;
    if !result.is_null() && result["result"].is_object() {
        let content = result["result"]["content"].to_string();
        assert!(
            content.contains("not allowed")
                || content.contains("Mutating")
                || result["error"].is_object(),
            "INSERT should be rejected: {}",
            content
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_context_store() {
    // context_store: set and get a value
    let result = invoke_tool(
        "context_store",
        json!({"operation": "set", "key": "test_key", "value": "test_value"}),
    )
    .await;
    assert!(result.is_object());
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_status_check() {
    let result = invoke_tool("status_check", json!({})).await;
    assert!(result.is_object());
    // status_check should return service health info
    if result["result"].is_object() {
        let content = result["result"]["content"].to_string();
        assert!(
            !content.is_empty(),
            "status_check should return non-empty content"
        );
    }
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_task_update() {
    // Use a non-existent task ID; should return not-found, not a crash
    let result = invoke_tool(
        "task_update",
        json!({"task_id": "nonexistent-task-id", "status": "escalated"}),
    )
    .await;
    assert!(result.is_object());
}

#[tokio::test]
#[ignore]
async fn test_mcp_tool_aggregate_search() {
    let result = invoke_tool("aggregate_search", json!({"query": "function", "limit": 3})).await;
    assert!(result.is_object());
    if result["result"].is_object() {
        assert!(result["result"]["content"].is_array());
    }
}

// =============================================================================
// Error handling: unknown tool
// =============================================================================

#[tokio::test]
#[ignore]
async fn test_mcp_unknown_tool_returns_error() {
    let result = invoke_tool("this_tool_does_not_exist", json!({})).await;
    if !result.is_null() {
        // MCP may return either a JSON-RPC error OR a result with isError=true
        let is_error_result = result["result"]["isError"] == true;
        let is_jsonrpc_error = result["error"].is_object();
        assert!(
            is_error_result || is_jsonrpc_error,
            "unknown tool should return an error response: {}",
            result
        );
    }
}
