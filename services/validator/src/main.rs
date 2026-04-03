//! rustbrain-validator — evaluate rust-brain agent code generation accuracy.
//!
//! Usage:
//! ```text
//! validator validate <repo> <pr_number> [--runs 2] [--inverted] [--timeout 7200]
//!                    [--opencode-url <url>] [--ingestion-bin <path>]
//! ```

pub mod comparator;
pub mod executor;
pub mod extractor;
pub mod github;
pub mod judge;
pub mod models;
pub mod opencode;
pub mod preparator;
pub mod scorer;
pub mod storage;

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use tracing::info;

use crate::comparator::{compare, parse_diff};
use crate::executor::ExecutorParams;
use crate::github::GithubClient;
use crate::opencode::OpenCodeClient;

// =============================================================================
// CLI
// =============================================================================

#[derive(Parser)]
#[command(
    name = "validator",
    about = "Evaluate rust-brain agent code generation accuracy against real PRs",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Validate rust-brain's output for a specific PR.
    Validate(ValidateArgs),
}

#[derive(Parser, Debug)]
struct ValidateArgs {
    /// GitHub repository in `owner/repo` format.
    repo: String,

    /// Pull request number to validate.
    pr_number: u32,

    /// Local clone of the repository (must already exist on disk).
    #[arg(long)]
    repo_path: PathBuf,

    /// Number of independent runs per PR (default: 2).
    #[arg(long, default_value = "2")]
    runs: u8,

    /// Treat this PR as expected-to-be-rejected (inverted rubric).
    #[arg(long)]
    inverted: bool,

    /// Executor timeout in seconds (default: 7200).
    #[arg(long, default_value_t = executor::DEFAULT_TIMEOUT_SECS)]
    timeout: u64,

    /// OpenCode base URL (e.g. `http://localhost:4096`).
    #[arg(long, env = "OPENCODE_URL", default_value = "http://opencode:4096")]
    opencode_url: String,

    /// Optional Basic Auth username for OpenCode.
    #[arg(long, env = "OPENCODE_USER")]
    opencode_user: Option<String>,

    /// Optional Basic Auth password for OpenCode.
    #[arg(long, env = "OPENCODE_PASS")]
    opencode_pass: Option<String>,

    /// Path to the ingestion binary (omit to skip ingestion).
    #[arg(long)]
    ingestion_bin: Option<String>,

    /// Emit results as JSON to stdout (default: human-readable).
    #[arg(long)]
    json: bool,
}

// =============================================================================
// Entry point
// =============================================================================

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("rustbrain_validator=info".parse().unwrap()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Validate(args) => run_validate(args).await,
    }
}

async fn run_validate(args: ValidateArgs) -> Result<()> {
    let github = GithubClient::from_env();
    let opencode = OpenCodeClient::new(
        args.opencode_url.clone(),
        args.opencode_user.clone(),
        args.opencode_pass.clone(),
    );

    info!(repo = %args.repo, pr_number = args.pr_number, runs = args.runs, "Starting validation");

    // Step 1: Extract PR metadata and requirements text
    let (pr_context, requirements) =
        extractor::extract_pr(&github, &args.repo, args.pr_number).await?;

    info!(
        used_fallback = requirements.used_fallback,
        requirements_len = requirements.text.len(),
        "PR requirements extracted"
    );

    // Step 2: Prepare environment — checkout parent commit + ingest
    preparator::prepare_env(
        &github,
        &args.repo_path,
        &pr_context,
        args.ingestion_bin.as_deref(),
    )
    .await
    .context("Environment preparation failed")?;

    // Step 3: Capture the expected diff (from the PR itself)
    let expected_diff_str = capture_pr_diff(&github, &args.repo_path, &pr_context).await?;
    let expected_patches = parse_diff(&expected_diff_str);

    info!(
        expected_files = expected_patches.len(),
        "Expected diff parsed"
    );

    // Step 4: Run the agent N times and compare each run
    let mut run_results = Vec::new();
    for run_idx in 0..args.runs {
        info!(run = run_idx, total = args.runs, "Starting executor run");

        let params = ExecutorParams::new(&requirements.text, &args.repo_path)
            .with_timeout(args.timeout)
            .with_title(format!(
                "validator-{}-pr{}-run{}",
                args.repo.replace('/', "-"),
                args.pr_number,
                run_idx
            ));

        let actual_diff_str = executor::execute(&opencode, &params).await?;
        let actual_patches = parse_diff(&actual_diff_str);
        let comparison = compare(&expected_patches, &actual_patches);

        info!(
            run = run_idx,
            file_precision = comparison.file_precision,
            file_recall = comparison.file_recall,
            line_similarity = comparison.line_similarity,
            non_rust_files = comparison.non_rust_files.len(),
            "Run comparison complete"
        );

        if args.json {
            println!("{}", serde_json::to_string_pretty(&comparison)?);
        } else {
            print_comparison_human(run_idx, &comparison);
        }

        run_results.push(comparison);
    }

    info!("Validation complete");
    Ok(())
}

/// Capture the diff introduced by the PR: `git diff <parent>..HEAD`.
async fn capture_pr_diff(
    client: &GithubClient,
    repo_path: &std::path::Path,
    pr_context: &crate::github::PrContext,
) -> Result<String> {
    use tokio::process::Command;

    // Get the first commit of the PR to compare from its parent
    let first_oid = pr_context
        .commits
        .first()
        .map(|c| c.oid.as_str())
        .unwrap_or("HEAD");

    let path_str = repo_path.to_string_lossy().to_string();
    let range = format!("{first_oid}^..HEAD");

    let out = Command::new("git")
        .args(["-C", &path_str, "diff", &range])
        .output()
        .await
        .context("Failed to run git diff for expected PR diff")?;

    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        anyhow::bail!("git diff {range} failed: {stderr}");
    }

    // Suppress unused variable warning — client is used for its type
    let _ = client;

    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn print_comparison_human(run_idx: u8, c: &crate::models::ComparisonResult) {
    println!("--- Run {run_idx} ---");
    println!("  File precision : {:.2}", c.file_precision);
    println!("  File recall    : {:.2}", c.file_recall);
    println!("  Line similarity: {:.4}", c.line_similarity);
    if !c.non_rust_files.is_empty() {
        println!("  Non-Rust files : {}", c.non_rust_files.join(", "));
    }
}
