//! Structural Comparator — parses unified git diffs and computes
//! file-level precision/recall and line-level similarity.
//!
//! ## Metrics
//!
//! - **File precision** `|expected ∩ actual| / |actual|` — are we touching the
//!   right files without excess noise?
//! - **File recall** `|expected ∩ actual| / |expected|` — are we covering all
//!   required files?
//! - **Line similarity** — Jaro-Winkler similarity averaged over added lines in
//!   files that appear in both diffs.
//!
//! Non-Rust files are reported but not included in the line-similarity score.

use std::collections::{HashMap, HashSet};

use strsim::jaro_winkler;

use crate::models::{ComparisonResult, FilePatch, Hunk};

// =============================================================================
// Diff parser
// =============================================================================

/// Parse a unified diff string into a list of [`FilePatch`] objects.
///
/// Handles multi-file diffs. File paths are extracted from `--- a/...` /
/// `+++ b/...` header lines. The `/dev/null` sentinel used for new or deleted
/// files is preserved as-is.
pub fn parse_diff(diff: &str) -> Vec<FilePatch> {
    let mut patches: Vec<FilePatch> = Vec::new();
    let mut current_patch: Option<FilePatch> = None;
    let mut current_hunk: Option<Hunk> = None;

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            // Flush the previous hunk and patch
            flush_hunk(&mut current_patch, &mut current_hunk);
            if let Some(patch) = current_patch.take() {
                patches.push(patch);
            }
            // Parse the path from "diff --git a/<path> b/<path>"
            // We take the b/ path as the canonical file path.
            let path = extract_b_path(rest);
            current_patch = Some(FilePatch {
                path,
                hunks: Vec::new(),
                lines_added: 0,
                lines_removed: 0,
            });
            continue;
        }

        if line.starts_with("--- ") || line.starts_with("+++ ") {
            // These header lines refine the path; skip them — we already have
            // the path from the `diff --git` line.
            continue;
        }

        if let Some(rest) = line.strip_prefix("@@ ") {
            flush_hunk(&mut current_patch, &mut current_hunk);
            current_hunk = Some(parse_hunk_header(rest));
            continue;
        }

        if let Some(hunk) = current_hunk.as_mut() {
            hunk.lines.push(line.to_string());
            if line.starts_with('+') {
                if let Some(patch) = current_patch.as_mut() {
                    patch.lines_added += 1;
                }
            } else if line.starts_with('-') {
                if let Some(patch) = current_patch.as_mut() {
                    patch.lines_removed += 1;
                }
            }
        }
    }

    flush_hunk(&mut current_patch, &mut current_hunk);
    if let Some(patch) = current_patch.take() {
        patches.push(patch);
    }

    patches
}

/// Extract the `b/` path from a `diff --git a/<x> b/<y>` header.
fn extract_b_path(rest: &str) -> String {
    // rest = "a/src/main.rs b/src/main.rs"
    if let Some(b_start) = rest.rfind(" b/") {
        rest[b_start + 3..].to_string()
    } else if let Some(b_start) = rest.rfind(' ') {
        // Fallback: take whatever is after the last space
        rest[b_start + 1..].to_string()
    } else {
        rest.to_string()
    }
}

/// Parse a `@@ -old_start,old_count +new_start,new_count @@` hunk header.
///
/// Returns a zero-initialised [`Hunk`] with the header values filled in.
fn parse_hunk_header(header: &str) -> Hunk {
    // header = "-1,7 +1,7 @@ fn foo() {"  (everything after the initial "@@ ")
    let mut old_start = 0u32;
    let mut old_count = 0u32;
    let mut new_start = 0u32;
    let mut new_count = 0u32;

    if let Some(at_end) = header.find(" @@") {
        let range_str = &header[..at_end]; // "-1,7 +1,7"
        let parts: Vec<&str> = range_str.splitn(2, ' ').collect();
        if parts.len() == 2 {
            parse_range(
                parts[0].trim_start_matches('-'),
                &mut old_start,
                &mut old_count,
            );
            parse_range(
                parts[1].trim_start_matches('+'),
                &mut new_start,
                &mut new_count,
            );
        }
    }

    Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        lines: Vec::new(),
    }
}

/// Parse `"start,count"` or just `"start"` into the provided mutable references.
fn parse_range(s: &str, start: &mut u32, count: &mut u32) {
    let mut parts = s.splitn(2, ',');
    *start = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    *count = parts.next().and_then(|v| v.parse().ok()).unwrap_or(1);
}

/// Flush `current_hunk` into `current_patch`.
fn flush_hunk(patch: &mut Option<FilePatch>, hunk: &mut Option<Hunk>) {
    if let (Some(p), Some(h)) = (patch.as_mut(), hunk.take()) {
        p.hunks.push(h);
    }
}

// =============================================================================
// Comparison
// =============================================================================

/// Compare two sets of parsed file patches and return precision/recall/similarity.
///
/// # Arguments
///
/// - `expected` — patches from the merged PR (ground truth).
/// - `actual` — patches produced by the rust-brain agent.
pub fn compare(expected: &[FilePatch], actual: &[FilePatch]) -> ComparisonResult {
    let expected_paths: HashSet<&str> = expected.iter().map(|p| p.path.as_str()).collect();
    let actual_paths: HashSet<&str> = actual.iter().map(|p| p.path.as_str()).collect();

    let intersection: HashSet<&str> = expected_paths
        .intersection(&actual_paths)
        .copied()
        .collect();

    let file_precision = if actual_paths.is_empty() {
        0.0
    } else {
        intersection.len() as f64 / actual_paths.len() as f64
    };

    let file_recall = if expected_paths.is_empty() {
        0.0
    } else {
        intersection.len() as f64 / expected_paths.len() as f64
    };

    // Build lookup maps for added lines per path
    let expected_added = added_lines_by_path(expected);
    let actual_added = added_lines_by_path(actual);

    let line_similarity = compute_line_similarity(&expected_added, &actual_added, &intersection);

    // Collect non-Rust files from both diffs
    let all_paths: HashSet<&str> = expected_paths.union(&actual_paths).copied().collect();
    let mut non_rust_files: Vec<String> = all_paths
        .into_iter()
        .filter(|p| !is_rust_file(p))
        .map(|p| p.to_string())
        .collect();
    non_rust_files.sort();

    ComparisonResult {
        file_precision,
        file_recall,
        line_similarity,
        non_rust_files,
    }
}

/// Extract added lines (lines starting with `+`) grouped by file path.
fn added_lines_by_path(patches: &[FilePatch]) -> HashMap<&str, Vec<&str>> {
    let mut map: HashMap<&str, Vec<&str>> = HashMap::new();
    for patch in patches {
        let lines: Vec<&str> = patch
            .hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .filter(|l| l.starts_with('+'))
            .map(|l| l.trim_start_matches('+'))
            .collect();
        map.entry(patch.path.as_str()).or_default().extend(lines);
    }
    map
}

/// Compute the mean Jaro-Winkler similarity of added lines across shared files.
///
/// For each file in `intersection`, pairs added lines from expected and actual
/// by position (zip). Files with no added lines contribute 1.0 (identical).
fn compute_line_similarity(
    expected: &HashMap<&str, Vec<&str>>,
    actual: &HashMap<&str, Vec<&str>>,
    intersection: &HashSet<&str>,
) -> f64 {
    if intersection.is_empty() {
        return 0.0;
    }

    let mut total_similarity = 0.0f64;
    let mut count = 0usize;

    for path in intersection {
        if !is_rust_file(path) {
            continue; // Non-Rust files excluded from line scoring
        }

        let exp_lines = expected.get(path).map(|v| v.as_slice()).unwrap_or(&[]);
        let act_lines = actual.get(path).map(|v| v.as_slice()).unwrap_or(&[]);

        if exp_lines.is_empty() && act_lines.is_empty() {
            // Both have no added lines — perfect match
            total_similarity += 1.0;
            count += 1;
            continue;
        }

        // Pair lines by position; shorter list limits the pairing
        let mut pair_sim = 0.0f64;
        for (e, a) in exp_lines.iter().zip(act_lines.iter()) {
            pair_sim += jaro_winkler(e, a);
        }

        // Account for length mismatch: unpaired lines contribute 0 similarity
        let max_len = exp_lines.len().max(act_lines.len());
        let file_sim = if max_len == 0 {
            1.0
        } else {
            pair_sim / max_len as f64
        };

        total_similarity += file_sim;
        count += 1;
    }

    if count == 0 {
        0.0
    } else {
        total_similarity / count as f64
    }
}

/// Return true if the path ends with `.rs`.
fn is_rust_file(path: &str) -> bool {
    path.ends_with(".rs")
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // -------------------------------------------------------------------------
    // parse_diff tests
    // -------------------------------------------------------------------------

    const SIMPLE_DIFF: &str = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
-    println!("hello");
+    println!("hello, world");
+    println!("done");
 }
"#;

    #[test]
    fn parse_simple_diff() {
        let patches = parse_diff(SIMPLE_DIFF);
        assert_eq!(patches.len(), 1);
        let p = &patches[0];
        assert_eq!(p.path, "src/main.rs");
        assert_eq!(p.lines_added, 2);
        assert_eq!(p.lines_removed, 1);
        assert_eq!(p.hunks.len(), 1);
        let h = &p.hunks[0];
        assert_eq!(h.old_start, 1);
        assert_eq!(h.old_count, 3);
        assert_eq!(h.new_start, 1);
        assert_eq!(h.new_count, 4);
    }

    #[test]
    fn parse_empty_diff() {
        let patches = parse_diff("");
        assert!(patches.is_empty());
    }

    #[test]
    fn parse_multi_file_diff() {
        let diff = r#"diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,2 +1,3 @@
 pub mod foo;
+pub mod bar;

diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,1 +1,1 @@
-fn old() {}
+fn new() {}
"#;
        let patches = parse_diff(diff);
        assert_eq!(patches.len(), 2);
        assert_eq!(patches[0].path, "src/lib.rs");
        assert_eq!(patches[0].lines_added, 1);
        assert_eq!(patches[1].path, "src/main.rs");
        assert_eq!(patches[1].lines_added, 1);
        assert_eq!(patches[1].lines_removed, 1);
    }

    #[test]
    fn parse_new_file_diff() {
        let diff = r#"diff --git a/src/new.rs b/src/new.rs
new file mode 100644
--- /dev/null
+++ b/src/new.rs
@@ -0,0 +1,3 @@
+pub fn hello() {
+    println!("hi");
+}
"#;
        let patches = parse_diff(diff);
        assert_eq!(patches.len(), 1);
        assert_eq!(patches[0].path, "src/new.rs");
        assert_eq!(patches[0].lines_added, 3);
        assert_eq!(patches[0].lines_removed, 0);
    }

    #[test]
    fn parse_hunk_header_parses_correctly() {
        let hunk = parse_hunk_header("-10,5 +10,7 @@ impl Foo {");
        assert_eq!(hunk.old_start, 10);
        assert_eq!(hunk.old_count, 5);
        assert_eq!(hunk.new_start, 10);
        assert_eq!(hunk.new_count, 7);
    }

    #[test]
    fn parse_hunk_header_single_line() {
        let hunk = parse_hunk_header("-1 +1 @@");
        assert_eq!(hunk.old_start, 1);
        assert_eq!(hunk.old_count, 1); // default when count omitted
    }

    #[test]
    fn extract_b_path_standard() {
        assert_eq!(extract_b_path("a/src/main.rs b/src/main.rs"), "src/main.rs");
    }

    #[test]
    fn extract_b_path_nested() {
        assert_eq!(
            extract_b_path("a/crates/foo/src/lib.rs b/crates/foo/src/lib.rs"),
            "crates/foo/src/lib.rs"
        );
    }

    // -------------------------------------------------------------------------
    // compare tests
    // -------------------------------------------------------------------------

    fn make_patch(path: &str, added: &[&str], removed: u32) -> FilePatch {
        let hunk_lines: Vec<String> = added
            .iter()
            .map(|l| format!("+{l}"))
            .chain((0..removed).map(|_| "- old line".to_string()))
            .collect();
        FilePatch {
            path: path.to_string(),
            hunks: vec![Hunk {
                old_start: 1,
                old_count: removed,
                new_start: 1,
                new_count: added.len() as u32,
                lines: hunk_lines,
            }],
            lines_added: added.len() as u32,
            lines_removed: removed,
        }
    }

    #[test]
    fn compare_perfect_match() {
        let expected = vec![make_patch("src/lib.rs", &["fn foo() {}", "}"], 0)];
        let actual = expected.clone();
        let result = compare(&expected, &actual);
        assert!((result.file_precision - 1.0).abs() < 1e-9);
        assert!((result.file_recall - 1.0).abs() < 1e-9);
        assert!(result.line_similarity > 0.99);
        assert!(result.non_rust_files.is_empty());
    }

    #[test]
    fn compare_empty_both() {
        let result = compare(&[], &[]);
        assert_eq!(result.file_precision, 0.0);
        assert_eq!(result.file_recall, 0.0);
        assert_eq!(result.line_similarity, 0.0);
    }

    #[test]
    fn compare_actual_empty_precision_zero() {
        let expected = vec![make_patch("src/lib.rs", &["fn foo() {}"], 0)];
        let result = compare(&expected, &[]);
        assert_eq!(result.file_precision, 0.0);
        assert_eq!(result.file_recall, 0.0);
    }

    #[test]
    fn compare_expected_empty_recall_zero() {
        let actual = vec![make_patch("src/lib.rs", &["fn foo() {}"], 0)];
        let result = compare(&[], &actual);
        assert_eq!(result.file_recall, 0.0);
        // precision = 0/1 = 0 (intersection empty)
        assert_eq!(result.file_precision, 0.0);
    }

    #[test]
    fn compare_partial_overlap() {
        let expected = vec![
            make_patch("src/a.rs", &["fn a() {}"], 0),
            make_patch("src/b.rs", &["fn b() {}"], 0),
        ];
        let actual = vec![
            make_patch("src/a.rs", &["fn a() {}"], 0),
            make_patch("src/c.rs", &["fn c() {}"], 0),
        ];
        let result = compare(&expected, &actual);
        // intersection = {src/a.rs}, actual = {src/a.rs, src/c.rs}
        assert!((result.file_precision - 0.5).abs() < 1e-9);
        // intersection = {src/a.rs}, expected = {src/a.rs, src/b.rs}
        assert!((result.file_recall - 0.5).abs() < 1e-9);
    }

    #[test]
    fn compare_flags_non_rust_files() {
        let expected = vec![
            make_patch("Cargo.toml", &["version = \"0.2.0\""], 0),
            make_patch("src/lib.rs", &["pub fn f() {}"], 0),
        ];
        let actual = vec![
            make_patch("Cargo.toml", &["version = \"0.2.0\""], 0),
            make_patch("src/lib.rs", &["pub fn f() {}"], 0),
        ];
        let result = compare(&expected, &actual);
        assert!(
            result.non_rust_files.contains(&"Cargo.toml".to_string()),
            "Expected Cargo.toml in non_rust_files, got {:?}",
            result.non_rust_files
        );
    }

    #[test]
    fn compare_similar_lines_high_similarity() {
        let expected = vec![make_patch(
            "src/lib.rs",
            &["fn process(x: u32) -> u32 { x + 1 }"],
            0,
        )];
        let actual = vec![make_patch(
            "src/lib.rs",
            &["fn process(x: u32) -> u32 { x + 2 }"],
            0,
        )];
        let result = compare(&expected, &actual);
        // Should be high similarity (one character diff)
        assert!(
            result.line_similarity > 0.8,
            "Expected high similarity, got {}",
            result.line_similarity
        );
    }

    #[test]
    fn compare_completely_different_lines_low_similarity() {
        let expected = vec![make_patch(
            "src/lib.rs",
            &["fn alpha() -> bool { true }"],
            0,
        )];
        let actual = vec![make_patch(
            "src/lib.rs",
            &["impl Display for MyType { fn fmt(&self, f: &mut Formatter) {} }"],
            0,
        )];
        let result = compare(&expected, &actual);
        // These strings share some characters but are semantically unrelated
        assert!(
            result.line_similarity < 0.9,
            "Expected lower similarity for unrelated lines, got {}",
            result.line_similarity
        );
    }

    #[test]
    fn compare_monorepo_multiple_crates() {
        let expected = vec![
            make_patch("crates/foo/src/lib.rs", &["pub fn foo() {}"], 0),
            make_patch("crates/bar/src/lib.rs", &["pub fn bar() {}"], 0),
        ];
        let actual = vec![
            make_patch("crates/foo/src/lib.rs", &["pub fn foo() {}"], 0),
            make_patch("crates/bar/src/lib.rs", &["pub fn bar() {}"], 0),
        ];
        let result = compare(&expected, &actual);
        assert!((result.file_precision - 1.0).abs() < 1e-9);
        assert!((result.file_recall - 1.0).abs() < 1e-9);
    }

    #[test]
    fn is_rust_file_detects_correctly() {
        assert!(is_rust_file("src/main.rs"));
        assert!(is_rust_file("crates/foo/src/lib.rs"));
        assert!(!is_rust_file("Cargo.toml"));
        assert!(!is_rust_file("README.md"));
        assert!(!is_rust_file("src/main.rs.bak"));
    }

    #[test]
    fn non_rust_files_sorted() {
        let expected = vec![
            make_patch("z.toml", &["a = 1"], 0),
            make_patch("a.yaml", &["key: val"], 0),
        ];
        let result = compare(&expected, &[]);
        // Should be sorted alphabetically
        assert_eq!(result.non_rust_files, vec!["a.yaml", "z.toml"]);
    }
}
