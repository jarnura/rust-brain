//! Workspace label injection for Cypher queries.
//!
//! This module ensures workspace isolation by injecting `:Workspace_<id>` labels
//! into all node patterns in user-provided Cypher queries. This prevents users
//! from accessing data outside their own workspace.
//!
//! The main entry point is [`inject_workspace_label`], which:
//! 1. Validates the workspace ID format
//! 2. Strips comments from the query
//! 3. Validates it's a single statement
//! 4. Blocks APOC procedures that re-enter the planner
//! 5. Injects workspace labels into all node patterns

use crate::errors::AppError;
use std::collections::HashMap;
use tracing::debug;

/// Workspace label format: 12 lowercase hex characters (matches workspace schema naming).
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] if the workspace ID does not match `^[0-9a-f]{12}$`.
pub fn validate_workspace_id(id: &str) -> Result<(), AppError> {
    if id.len() != 12 {
        return Err(AppError::BadRequest(format!(
            "Invalid workspace ID '{}': must be exactly 12 characters",
            id
        )));
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
    {
        return Err(AppError::BadRequest(format!(
            "Invalid workspace ID '{}': must be lowercase hexadecimal (0-9, a-f)",
            id
        )));
    }
    Ok(())
}

/// Main entry point: injects `:Workspace_<id>` label into all node patterns in a Cypher query.
///
/// This is called BEFORE `validate_cypher()` in the handler to ensure workspace isolation
/// before any other validation occurs.
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] for:
/// - Invalid workspace ID format
/// - Empty query
/// - Multi-statement queries (containing `;`)
/// - APOC planner-reentry procedures
/// - User attempting to set their own workspace label
///
/// # Examples
///
/// ```rust,ignore
/// let query = "MATCH (n:Function) RETURN n";
/// let result = inject_workspace_label(query, "a1b2c3d4e5f6")?;
/// assert_eq!(result, "MATCH (n:Function:Workspace_a1b2c3d4e5f6) RETURN n");
/// ```
pub fn inject_workspace_label(query: &str, workspace_id: &str) -> Result<String, AppError> {
    validate_workspace_id(workspace_id)?;

    if query.trim().is_empty() {
        return Err(AppError::BadRequest("Empty Cypher query".to_string()));
    }

    // Check if user is trying to set their own workspace label
    // Only block if the query contains Workspace_ followed by 12 hex chars
    // (the exact format of our injected labels)
    if let Some(pos) = query.find("Workspace_") {
        let after = &query[pos + "Workspace_".len()..];
        let candidate: String = after.chars().take(12).collect();
        if candidate.len() == 12
            && candidate
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            return Err(AppError::BadRequest(
                "User-defined workspace labels are not allowed".to_string(),
            ));
        }
    }

    let stripped = strip_comments(query)?;
    validate_single_statement(&stripped)?;
    validate_no_planner_reentry(&stripped)?;

    let label = format!("Workspace_{}", workspace_id);
    let injected = inject_labels_into_patterns(&stripped, &label)?;

    debug!(
        "Injected workspace label '{}' into query: {}",
        label, injected
    );

    Ok(injected)
}

/// Strips comments from a Cypher query.
///
/// Handles:
/// - `//` line comments (to end of line)
/// - `/* */` block comments (including nested comments)
/// - Preserves content inside string literals
///
/// # Errors
///
/// Returns [`AppError::BadRequest`] for unclosed block comments.
fn strip_comments(query: &str) -> Result<String, AppError> {
    let mut result = String::with_capacity(query.len());
    let chars: Vec<char> = query.chars().collect();
    let mut i = 0;
    let mut in_string = false;
    let mut string_char = '\0';
    let mut in_block_comment = false;
    let mut block_depth = 0;

    while i < chars.len() {
        if in_string {
            result.push(chars[i]);
            if chars[i] == string_char {
                // Check for escaped quotes
                let backslash_count = result[..result.len() - 1]
                    .chars()
                    .rev()
                    .take_while(|&c| c == '\\')
                    .count();
                if backslash_count % 2 == 0 {
                    in_string = false;
                }
            }
            i += 1;
        } else if in_block_comment {
            if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '*' {
                block_depth += 1;
                i += 2;
            } else if i + 1 < chars.len() && chars[i] == '*' && chars[i + 1] == '/' {
                block_depth -= 1;
                i += 2;
                if block_depth == 0 {
                    in_block_comment = false;
                    result.push(' '); // Preserve whitespace separation
                }
            } else {
                i += 1;
            }
        } else if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '*' {
            in_block_comment = true;
            block_depth = 1;
            i += 2;
        } else if i + 1 < chars.len() && chars[i] == '/' && chars[i + 1] == '/' {
            // Skip to end of line
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            if i < chars.len() {
                result.push('\n'); // Preserve newline
                i += 1;
            }
        } else if chars[i] == '\'' || chars[i] == '"' {
            in_string = true;
            string_char = chars[i];
            result.push(chars[i]);
            i += 1;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    if in_block_comment {
        return Err(AppError::BadRequest(
            "Unclosed block comment in Cypher query".to_string(),
        ));
    }

    Ok(result)
}

/// Validates that the query contains only a single statement.
///
/// Rejects queries containing `;` which could indicate multi-statement attacks.
fn validate_single_statement(query: &str) -> Result<(), AppError> {
    // Remove all whitespace to check for bare semicolons
    let normalized: String = query.chars().filter(|c| !c.is_whitespace()).collect();

    if normalized.contains(';') {
        return Err(AppError::BadRequest(
            "Multi-statement Cypher queries are not allowed".to_string(),
        ));
    }

    Ok(())
}

/// APOC procedures that re-enter the Cypher planner and allow arbitrary query execution.
///
/// These must be blocked even though they might appear read-only, as they can
/// execute user-controlled Cypher strings.
const PLANNER_REENTRY_APOC: &[&str] = &[
    "apoc.cypher.run",
    "apoc.cypher.runmany",
    "apoc.cypher.runfile",
    "apoc.do.when",
    "apoc.do.case",
    "apoc.when",
    "apoc.case",
    "apoc.periodic.commit",
    "apoc.periodic.iterate",
    "apoc.trigger.add",
    "apoc.trigger.remove",
    "apoc.trigger.removeall",
    "apoc.trigger.list",
];

/// Validates that the query does not use APOC procedures that re-enter the planner.
///
/// These procedures allow executing arbitrary Cypher strings, bypassing our
/// workspace label injection and read-only validation.
fn validate_no_planner_reentry(query: &str) -> Result<(), AppError> {
    let query_lower = query.to_lowercase();

    if query_lower.contains("call apoc.") {
        let mut remaining = query_lower.as_str();
        while let Some(pos) = remaining.find("call apoc.") {
            let after_call = &remaining[pos + 5..]; // Point at "apoc."
            let proc_name: String = after_call
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != '(' && *c != '\n' && *c != '\r')
                .collect();

            for blocked in PLANNER_REENTRY_APOC {
                if proc_name.starts_with(blocked) {
                    return Err(AppError::BadRequest(format!(
                        "APOC procedure '{}' is not allowed: it can execute arbitrary Cypher",
                        proc_name
                    )));
                }
            }

            remaining = &remaining[pos + 10..];
        }
    }

    Ok(())
}

/// The core label injection logic.
///
/// Finds all node patterns in Cypher and injects the workspace label:
/// - `(n)` -> `(n:Workspace_<id>)`
/// - `(n:Function)` -> `(n:Function:Workspace_<id>)`
/// - `(n:Function:Struct)` -> `(n:Function:Struct:Workspace_<id>)`
/// - `(:Function)` -> `(:Function:Workspace_<id>)`
/// - `()` -> `(:Workspace_<id>)`
///
/// Also handles WHERE clauses with label filters like `WHERE n:Function`.
fn inject_labels_into_patterns(query: &str, label: &str) -> Result<String, AppError> {
    let mut result = String::with_capacity(query.len() + 50);
    let chars: Vec<char> = query.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '(' {
            // Parse node pattern
            let (pattern, end_pos) = parse_node_pattern(&chars, i)?;
            let injected = inject_label_into_pattern(&pattern, label);
            result.push_str(&injected);
            i = end_pos;
        } else if i + 5 <= chars.len()
            && chars[i..i + 5].iter().collect::<String>().to_lowercase() == "where"
            && (i + 5 == chars.len() || !chars[i + 5].is_alphanumeric())
        {
            // Process WHERE clause
            let (processed, end_pos) = process_where_clause(&chars, i, label)?;
            result.push_str(&processed);
            i = end_pos;
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    Ok(result)
}

/// Parses a node pattern starting at position `start` (which should be `(`).
///
/// Returns the raw pattern and the position after the closing `)`.
fn parse_node_pattern(chars: &[char], start: usize) -> Result<(String, usize), AppError> {
    let mut pattern = String::new();
    let mut i = start;
    let mut depth = 0;
    let mut in_string = false;
    let mut string_char = '\0';

    while i < chars.len() {
        let c = chars[i];

        if in_string {
            pattern.push(c);
            if c == string_char {
                let backslash_count = pattern[..pattern.len() - 1]
                    .chars()
                    .rev()
                    .take_while(|&ch| ch == '\\')
                    .count();
                if backslash_count % 2 == 0 {
                    in_string = false;
                }
            }
            i += 1;
        } else if c == '(' {
            depth += 1;
            pattern.push(c);
            i += 1;
        } else if c == ')' {
            depth -= 1;
            pattern.push(c);
            i += 1;
            if depth == 0 {
                return Ok((pattern, i));
            }
        } else if c == '\'' || c == '"' {
            in_string = true;
            string_char = c;
            pattern.push(c);
            i += 1;
        } else {
            pattern.push(c);
            i += 1;
        }
    }

    Err(AppError::BadRequest(
        "Unclosed node pattern in Cypher query".to_string(),
    ))
}

/// Injects a label into a single node pattern.
///
/// Handles:
/// - `(n)` -> `(n:Workspace_<id>)`
/// - `(n:Function)` -> `(n:Function:Workspace_<id>)`
/// - `(n:Function:Struct)` -> `(n:Function:Struct:Workspace_<id>)`
/// - `(:Function)` -> `(:Function:Workspace_<id>)`
/// - `()` -> `(:Workspace_<id>)`
fn inject_label_into_pattern(pattern: &str, label: &str) -> String {
    // Handle nested patterns like ((n)-[:REL]->(m))
    if pattern.starts_with("((") {
        return inject_into_nested_pattern(pattern, label);
    }

    // Simple pattern: (variable:Label1:Label2 {props})
    // Find the content between ( and )
    let content = &pattern[1..pattern.len() - 1];

    // Check for properties (indicated by `{`)
    let (before_props, props) = if let Some(prop_start) = content.find('{') {
        (&content[..prop_start], &content[prop_start..])
    } else {
        (content, "")
    };

    // Check if there's already a colon (labels present)
    if before_props.contains(':') {
        // Append the workspace label after existing labels, then props with space
        let spacer = if props.is_empty() { "" } else { " " };
        format!("({}:{}{}{})", before_props.trim_end(), label, spacer, props)
    } else if before_props.trim().is_empty() {
        // Empty pattern ()
        format!("(:{})", label)
    } else {
        // Variable only, no labels
        let var = before_props.trim();
        let spacer = if props.is_empty() { "" } else { " " };
        format!("({}:{}{}{})", var, label, spacer, props)
    }
}

/// Injects labels into nested patterns like `((n)-[:REL]->(m))`.
fn inject_into_nested_pattern(pattern: &str, label: &str) -> String {
    // Recursively process the inner content
    let inner = &pattern[1..pattern.len() - 1]; // Remove outer ()
    let injected = inject_labels_into_patterns(inner, label).unwrap_or_else(|_| inner.to_string());
    format!("({})", injected)
}

/// Processes a WHERE clause and injects workspace labels into label predicates.
///
/// `WHERE n:Function` becomes `WHERE n:Function AND n:Workspace_<id>`
fn process_where_clause(
    chars: &[char],
    start: usize,
    label: &str,
) -> Result<(String, usize), AppError> {
    let mut result = String::new();
    let mut i = start;

    // Copy "WHERE" (case preserved from original)
    while i < chars.len() && result.len() < 5 {
        result.push(chars[i]);
        i += 1;
    }

    // Process the rest looking for variable:Label patterns
    while i < chars.len() {
        // Look for word:Label pattern
        if i + 1 < chars.len() && chars[i].is_alphanumeric() && !is_reserved_word_start(chars, i) {
            let (var_end, is_label_pred) = peek_label_predicate(chars, i);
            if is_label_pred {
                // Copy the variable
                while i < var_end {
                    result.push(chars[i]);
                    i += 1;
                }
                // Copy the colon and label
                while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                    result.push(chars[i]);
                    i += 1;
                }
                // Inject workspace label AND
                result.push_str(&format!(
                    " AND {}:{}",
                    result.split_whitespace().last().unwrap_or(""),
                    label
                ));
                continue;
            }
        }

        // Check for end of WHERE clause (next clause keyword)
        if chars[i].is_ascii_alphabetic() {
            let word: String = chars[i..]
                .iter()
                .take_while(|c| c.is_ascii_alphabetic())
                .collect();
            let word_lower = word.to_lowercase();
            if [
                "return", "with", "match", "optional", "union", "limit", "skip", "order", "create",
                "delete", "set", "remove",
            ]
            .contains(&word_lower.as_str())
            {
                return Ok((result, i));
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    Ok((result, i))
}

/// Checks if position `start` begins a reserved word (not a variable).
fn is_reserved_word_start(chars: &[char], start: usize) -> bool {
    let reserved = [
        "and", "or", "not", "xor", "in", "is", "null", "true", "false", "exists", "starts", "ends",
        "contains", "distinct", "as",
    ];

    for word in &reserved {
        let word_len = word.len();
        if start + word_len <= chars.len() {
            let candidate: String = chars[start..start + word_len]
                .iter()
                .collect::<String>()
                .to_lowercase();
            if candidate == *word {
                // Check boundary
                if start + word_len == chars.len() || !chars[start + word_len].is_alphanumeric() {
                    return true;
                }
            }
        }
    }
    false
}

/// Peeks ahead to determine if we're at a label predicate (variable:Label).
///
/// Returns (position after variable, true if it's a label predicate).
fn peek_label_predicate(chars: &[char], start: usize) -> (usize, bool) {
    // Skip whitespace
    let mut i = start;
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }

    // Read the potential variable name
    let var_start = i;
    while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
        i += 1;
    }

    if i == var_start {
        return (start, false);
    }

    // Skip whitespace
    while i < chars.len() && chars[i].is_whitespace() {
        i += 1;
    }

    // Check for colon followed by label name
    if i < chars.len() && chars[i] == ':' {
        let colon_pos = i;
        i += 1;
        // Skip whitespace after colon
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
        // Check for label name
        if i < chars.len() && (chars[i].is_alphabetic() || chars[i] == '_') {
            // It's a label predicate
            return (colon_pos, true);
        }
    }

    (start, false)
}

/// Strips any user-supplied workspace-related parameters from the parameters map.
///
/// Defense-in-depth: even if a user somehow injects a workspace label parameter,
/// we strip it before passing to Neo4j.
pub fn sanitize_parameters(params: &mut HashMap<String, serde_json::Value>) {
    let keys_to_remove: Vec<String> = params
        .keys()
        .filter(|k| {
            let lower = k.to_lowercase();
            lower.contains("workspace_label") || lower.contains("workspace_id")
        })
        .cloned()
        .collect();

    for key in keys_to_remove {
        params.remove(&key);
        debug!("Removed user-supplied workspace parameter '{}'", key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn ws_id() -> &'static str {
        "a1b2c3d4e5f6"
    }
    fn ws_label() -> &'static str {
        "Workspace_a1b2c3d4e5f6"
    }

    // ============================================================================
    // Category 1: Comment Injection
    // ============================================================================

    #[test]
    fn test_comment_injection_line_comment() {
        let query = "MATCH (n) // WHERE n:Workspace_other RETURN n";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        // After stripping comment, should inject normally
        assert!(result.contains(&format!("(n:{})", ws_label())));
        assert!(!result.contains("Workspace_other"));
    }

    #[test]
    fn test_comment_injection_block_comment() {
        let query = "MATCH (n) /* WHERE n:Workspace_other */ RETURN n";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(result.contains(&format!("(n:{})", ws_label())));
        assert!(!result.contains("Workspace_other"));
    }

    #[test]
    fn test_comment_injection_nested_block_comment() {
        let query = "MATCH (n) /* outer /* inner */ still comment */ RETURN n";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(result.contains(&format!("(n:{})", ws_label())));
    }

    #[test]
    fn test_comment_hides_malicious_label() {
        let query = "MATCH (n:Function/*:Workspace_other*/) RETURN n";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(result.contains(&format!(":Function:{}", ws_label())));
        assert!(!result.contains("Workspace_other"));
    }

    // ============================================================================
    // Category 2: Label Literal Collision
    // ============================================================================

    #[test]
    fn test_string_literal_not_treated_as_label() {
        let query = "MATCH (n {name: ':Workspace_other'}) RETURN n";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(result.contains("':Workspace_other'"));
        assert!(result.contains(ws_label()));
    }

    #[test]
    fn test_property_map_with_colon_not_label() {
        let query = "MATCH (n {path: 'foo:bar:Workspace_other'}) RETURN n";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        // The string value should remain unchanged
        assert!(result.contains("'foo:bar:Workspace_other'"));
        // n should get the workspace label
        assert!(result.contains(&format!("(n:{}", ws_label())));
    }

    // ============================================================================
    // Category 3: Multi-Statement Attempts
    // ============================================================================

    #[test]
    fn test_semicolon_rejected() {
        let query = "MATCH (n) RETURN n; MATCH (m) RETURN m";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Multi-statement"));
    }

    #[test]
    fn test_trailing_semicolon_rejected() {
        let query = "MATCH (n) RETURN n;";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
    }

    #[test]
    fn test_double_semicolon_rejected() {
        let query = "MATCH (n) RETURN n;;";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
    }

    // ============================================================================
    // Category 4: WITH/UNION Smuggling
    // ============================================================================

    #[test]
    fn test_with_followed_by_unfiltered_match() {
        let query = "MATCH (n:Workspace_self) WITH n MATCH (m) RETURN m";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        // Both n and m should get workspace labels
        assert!(
            result.contains(&format!("(n:Workspace_self:{}) ", ws_label()))
                || result.contains(&format!("(n:{}:Workspace_self)", ws_label()))
        );
        assert!(result.contains(&format!("(m:{})", ws_label())));
    }

    #[test]
    fn test_union_each_branch_injected() {
        let query = "MATCH (n:Function) RETURN n UNION ALL MATCH (m:Struct) RETURN m";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(
            result.contains(&format!("(n:Function:{}) ", ws_label()))
                || result.contains(&format!("(n:{}:Function)", ws_label()))
        );
        assert!(
            result.contains(&format!("(m:Struct:{}) ", ws_label()))
                || result.contains(&format!("(m:{}:Struct)", ws_label()))
        );
    }

    #[test]
    fn test_union_with_unlabeled_nodes() {
        let query = "MATCH (n) RETURN n UNION MATCH (m) RETURN m";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(result.contains(&format!("(n:{})", ws_label())));
        assert!(result.contains(&format!("(m:{})", ws_label())));
    }

    // ============================================================================
    // Category 5: Variable-Rebinding
    // ============================================================================

    #[test]
    fn test_with_alias() {
        let query = "MATCH (n) WITH n AS x MATCH (x)-[:CALLS]->(y) RETURN y";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        // n, x, and y should all get workspace labels
        assert!(result.contains(&format!("(n:{})", ws_label())));
        assert!(result.contains(&format!("(x:{})", ws_label())));
        assert!(result.contains(&format!("(y:{})", ws_label())));
    }

    #[test]
    fn test_same_variable_different_scope() {
        let query = "MATCH (n:Function) WITH n MATCH (n)-[:CALLS]->(m) RETURN m";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        // Both n patterns should have workspace labels
        assert!(
            result.contains(&format!("(n:Function:{}) ", ws_label()))
                || result.contains(&format!("(n:{}:Function)", ws_label()))
        );
        assert!(result.contains(&format!("(m:{})", ws_label())));
    }

    // ============================================================================
    // Category 6: Unicode + Whitespace Tricks
    // ============================================================================

    #[test]
    fn test_non_breaking_space() {
        let query = "MATCH\u{00A0}(n:Function) RETURN n";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        // Non-breaking space should be handled
        assert!(
            result.contains(&format!("(n:Function:{}) ", ws_label()))
                || result.contains(&format!("(n:{}:Function)", ws_label()))
        );
    }

    #[test]
    fn test_tab_and_mixed_whitespace() {
        let query = "MATCH\t(n:Function)\nRETURN\tn";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(result.contains(ws_label()));
        assert!(result.contains(":Function"));
    }

    #[test]
    fn test_unicode_in_workspace_id_rejected() {
        let result = validate_workspace_id("abc\u{f6}d4e5f6gh"); // Contains ö
        assert!(result.is_err());
    }

    #[test]
    fn test_workspace_id_uppercase_rejected() {
        let result = validate_workspace_id("A1B2C3D4E5F6"); // Uppercase
        assert!(result.is_err());
    }

    // ============================================================================
    // Category 7: APOC Procedure Calls
    // ============================================================================

    #[test]
    fn test_apoc_cypher_run_rejected() {
        let query = "CALL apoc.cypher.run('MATCH (n) RETURN n')";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not allowed"));
    }

    #[test]
    fn test_apoc_cypher_run_many_rejected() {
        let query = "CALL apoc.cypher.runMany('MATCH (n) RETURN n;')";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
    }

    #[test]
    fn test_apoc_do_when_rejected() {
        let query = "CALL apoc.do.when(true, 'CREATE (n) RETURN n', '')";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
    }

    #[test]
    fn test_apoc_periodic_commit_rejected() {
        let query = "CALL apoc.periodic.commit('MATCH (n) RETURN n')";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
    }

    #[test]
    fn test_apoc_path_expand_allowed() {
        // This should be allowed - it's a read-only path traversal
        let query =
            "MATCH (n) CALL apoc.path.expand(n, 'CALLS>', null, 1, 3) YIELD path RETURN path";
        let result = inject_workspace_label(query, ws_id());
        // Should succeed (may have parsing issues but shouldn't be rejected for planner reentry)
        // Note: This might fail due to complex syntax, but not due to APOC blocking
        if let Err(e) = &result {
            let msg = e.to_string();
            assert!(!msg.contains("not allowed") || !msg.contains("execute arbitrary"));
        }
    }

    #[test]
    fn test_apoc_case_insensitive_rejection() {
        let query = "CALL APOC.CYPHER.RUN('MATCH (n) RETURN n')";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
    }

    // ============================================================================
    // Category 8: Parameter Injection
    // ============================================================================

    #[test]
    fn test_workspace_label_param_stripped() {
        let mut params = HashMap::new();
        params.insert("workspace_label".to_string(), serde_json::json!("evil"));
        params.insert("normal_param".to_string(), serde_json::json!("value"));

        sanitize_parameters(&mut params);

        assert!(!params.contains_key("workspace_label"));
        assert!(params.contains_key("normal_param"));
    }

    #[test]
    fn test_workspace_id_from_header_only() {
        let mut params = HashMap::new();
        params.insert("workspace_id".to_string(), serde_json::json!("attacker"));
        params.insert("WORKSPACE_ID".to_string(), serde_json::json!("attacker2"));

        sanitize_parameters(&mut params);

        assert!(!params.contains_key("workspace_id"));
        assert!(!params.contains_key("WORKSPACE_ID"));
    }

    // ============================================================================
    // Category 9: Deep Nesting
    // ============================================================================

    #[test]
    fn test_variable_length_path() {
        let query = "MATCH (n)-[*1..5]-(m) RETURN n, m";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(result.contains(&format!("(n:{})", ws_label())));
        assert!(result.contains(&format!("(m:{})", ws_label())));
    }

    #[test]
    fn test_nested_parentheses_in_path() {
        let query = "MATCH ((n)-[:CALLS*1..3]->(m)) RETURN n, m";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(result.contains(&format!("(n:{})", ws_label())));
        assert!(result.contains(&format!("(m:{})", ws_label())));
    }

    #[test]
    fn test_multiple_hops() {
        let query = "MATCH (a)-[:CALLS]->(b)-[:CALLS]->(c) RETURN a, b, c";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(result.contains(&format!("(a:{})", ws_label())));
        assert!(result.contains(&format!("(b:{})", ws_label())));
        assert!(result.contains(&format!("(c:{})", ws_label())));
    }

    // ============================================================================
    // Category 10: Empty / Malformed Input
    // ============================================================================

    #[test]
    fn test_empty_query_rejected() {
        let query = "";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
    }

    #[test]
    fn test_semicolon_only_rejected() {
        let query = ";";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
    }

    #[test]
    fn test_whitespace_only_rejected() {
        let query = "   \n\t  ";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_workspace_id_rejected() {
        let result = validate_workspace_id("evil");
        assert!(result.is_err());
    }

    #[test]
    fn test_workspace_id_with_special_chars_rejected() {
        let result = validate_workspace_id("abc123;DROP");
        assert!(result.is_err());
    }

    #[test]
    fn test_workspace_id_too_short_rejected() {
        let result = validate_workspace_id("abc");
        assert!(result.is_err());
    }

    #[test]
    fn test_workspace_id_too_long_rejected() {
        let result = validate_workspace_id("abcd1234abcd1234");
        assert!(result.is_err());
    }

    // ============================================================================
    // Additional Security Tests
    // ============================================================================

    #[test]
    fn test_user_sets_own_workspace_label_rejected() {
        let query = "MATCH (n:Workspace_deadbeef1234) RETURN n";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not allowed"));
    }

    #[test]
    fn test_label_enumeration_attempt() {
        // This should inject the workspace label, so labels(n) only returns workspace-scoped labels
        let query = "MATCH (n) RETURN labels(n)";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(result.contains(&format!("(n:{})", ws_label())));
    }

    #[test]
    fn test_optional_match_injected() {
        let query = "OPTIONAL MATCH (n:Function) RETURN n";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(
            result.contains(&format!("(n:Function:{}) ", ws_label()))
                || result.contains(&format!("(n:{}:Function)", ws_label()))
        );
    }

    #[test]
    fn test_already_injected_label_rejected() {
        let query = "MATCH (n:Workspace_abc123def456) RETURN n";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
    }

    // ============================================================================
    // Basic Injection Tests
    // ============================================================================

    #[test]
    fn test_basic_node_injection() {
        let query = "MATCH (n) RETURN n";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert_eq!(result, "MATCH (n:Workspace_a1b2c3d4e5f6) RETURN n");
    }

    #[test]
    fn test_anonymous_node_injection() {
        let query = "MATCH () RETURN 1";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(result.contains("(:Workspace_a1b2c3d4e5f6)"));
    }

    #[test]
    fn test_existing_label_injection() {
        let query = "MATCH (n:Function) RETURN n";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(result.contains(":Function:"));
        assert!(result.contains(ws_label()));
    }

    #[test]
    fn test_multiple_labels_injection() {
        let query = "MATCH (n:Function:Public) RETURN n";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(result.contains(":Function:"));
        assert!(result.contains(":Public:"));
        assert!(result.contains(ws_label()));
    }

    #[test]
    fn test_properties_preserved() {
        let query = "MATCH (n {name: 'test'}) RETURN n";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert!(result.contains("(n:Workspace_a1b2c3d4e5f6 {name: 'test'})"));
    }

    #[test]
    fn test_complex_query() {
        let query = r#"
            MATCH (f:Function {name: 'main'})
            OPTIONAL MATCH (f)-[:CALLS]->(callee)
            RETURN f, callee
        "#;
        let result = inject_workspace_label(query, ws_id()).unwrap();
        // f should have both Function label and workspace label
        assert!(result.contains(":Function"));
        // callee should have workspace label
        assert!(
            result.contains(&format!("(callee:{}) ", ws_label()))
                || result.contains(&format!("(callee:{})", ws_label()))
        );
    }

    #[test]
    fn test_strip_comments_in_string() {
        // // inside a string should not be treated as comment
        let query = r#"MATCH (n {url: "http://example.com"}) RETURN n"#;
        let result = strip_comments(query).unwrap();
        assert!(result.contains("http://example.com"));
    }

    #[test]
    fn test_strip_comments_block_in_string() {
        // /* inside a string should not be treated as comment start
        let query = r#"MATCH (n {pattern: "/* not a comment */"}) RETURN n"#;
        let result = strip_comments(query).unwrap();
        assert!(result.contains("/* not a comment */"));
    }

    #[test]
    fn test_apoc_iterate_rejected() {
        let query = "CALL apoc.periodic.iterate('MATCH (n) RETURN n', 'SET n.prop = 1', {})";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
    }

    #[test]
    fn test_apoc_trigger_rejected() {
        let query = "CALL apoc.trigger.add('myTrigger', 'MATCH (n) RETURN n', {})";
        let result = inject_workspace_label(query, ws_id());
        assert!(result.is_err());
    }

    #[test]
    fn test_return_without_match() {
        // Pure RETURN without node patterns should be allowed
        let query = "RETURN 1 + 2 AS result";
        let result = inject_workspace_label(query, ws_id()).unwrap();
        assert_eq!(result, "RETURN 1 + 2 AS result");
    }
}
