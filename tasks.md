## Audit public functions across crates
- workdir: /home/jarnura/projects/rust-brain
- model: haiku
- effort: low

Scan all Rust source files in crates/ and src/ directories. Identify all public functions (`pub fn`, `pub async fn`). Create a summary document listing each public function by crate and module. Output: logging-audit.md with function signatures and file locations.

## Configure logging infrastructure
- workdir: /home/jarnura/projects/rust-brain
- model: sonnet
- effort: medium

Add logging dependency (tracing or log crate) to Cargo.toml. Create/update initialization code in main.rs and lib.rs to set up logging subscriber. Ensure logging is initialized before any public functions are called. Include both stdout and optional file appenders.

## Add logging to main crate public functions
- workdir: /home/jarnura/projects/rust-brain
- model: sonnet
- effort: medium

Add logging statements to all public functions in the primary crate. Log function entry (with input parameters), significant internal operations, and exit (with return values). Use trace! and debug! levels appropriately.

## Add logging to utility/secondary crates
- workdir: /home/jarnura/projects/rust-brain
- model: sonnet
- effort: medium

Add logging statements to all public functions in secondary/library crates. Maintain consistent logging style and levels across all crates. Focus on entry/exit logging and critical decision points.

## Create and run logging tests
- workdir: /home/jarnura/projects/rust-brain
- model: sonnet
- effort: medium

Write integration tests verifying logging output for key public functions. Add tests/logging_integration_test.rs to capture and assert on log messages. Verify 80%+ coverage of logged functions. Run tests to ensure logging doesn't break functionality.
