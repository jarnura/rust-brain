//! rustbrain-benchmarker — eval suite runner for rust-brain agent accuracy.
//!
//! Usage: `benchmarker bench run [--suite default] [--release v1.2]`

pub mod ci;
pub mod registry;
pub mod reporter;
pub mod run_manager;

use clap::{Parser, Subcommand};
use sqlx::postgres::PgPoolOptions;
use tracing::info;

const DEFAULT_RUNS_PER_CASE: u32 = 2;
const DEFAULT_MAX_CONCURRENT: usize = 3;

#[derive(Parser)]
#[command(name = "benchmarker", about = "Eval suite runner for rust-brain")]
struct Cli {
    /// Postgres connection URL (overrides DATABASE_URL env var).
    #[arg(long, env = "DATABASE_URL")]
    db_url: Option<String>,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run a full eval suite and emit a CI advisory report.
    Bench {
        /// Suite name (must be synced first with `sync`).
        #[arg(long, default_value = "default")]
        suite: String,
        /// Optional release tag stored on the bench_run row.
        #[arg(long)]
        release: Option<String>,
        /// Independent runs executed per eval case.
        #[arg(long, default_value_t = DEFAULT_RUNS_PER_CASE)]
        runs: u32,
        /// Maximum number of cases to run concurrently.
        #[arg(long, default_value_t = DEFAULT_MAX_CONCURRENT)]
        concurrency: usize,
        /// UUID of a previous bench_run to use as a regression baseline.
        #[arg(long)]
        baseline: Option<uuid::Uuid>,
        /// Emit JSON advisory only (suppress human-readable output).
        #[arg(long)]
        json: bool,
    },
    /// Sync a YAML suite file into the database.
    Sync {
        /// Path to the YAML suite file.
        path: String,
        /// Suite name to register cases under.
        #[arg(long, default_value = "default")]
        suite: String,
    },
    /// List cases registered for a suite.
    List {
        /// Suite name.
        #[arg(long, default_value = "default")]
        suite: String,
    },
    /// Print an advisory report for a completed bench_run.
    Report {
        /// UUID of the bench_run to report on.
        run_id: uuid::Uuid,
        /// UUID of a baseline bench_run for regression comparison.
        #[arg(long)]
        baseline: Option<uuid::Uuid>,
        /// Emit JSON only (suppress human-readable output).
        #[arg(long)]
        json: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("rustbrain_benchmarker=info".parse()?),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Bench {
            suite,
            release,
            runs,
            concurrency,
            baseline,
            json,
        } => {
            let pool = connect_db(cli.db_url.as_deref()).await?;

            info!(suite = %suite, ?release, runs, concurrency, "Starting bench run");

            let result =
                run_manager::run_suite(&pool, &suite, runs, concurrency, release.as_deref())
                    .await?;

            // Optional regression comparison against a baseline
            let comparison = if let Some(baseline_id) = baseline {
                let baseline_report = reporter::generate_report(&pool, baseline_id).await?;
                let current_report = reporter::generate_report(&pool, result.bench_run_id).await?;
                Some(reporter::compare_runs(&baseline_report, &current_report))
            } else {
                None
            };

            if json {
                let advisory = ci::build_advisory(&result, release.as_deref(), comparison.as_ref());
                println!("{}", ci::format_json(&advisory)?);
            } else {
                ci::print_advisory(&result, release.as_deref(), comparison.as_ref())?;
            }
        }

        Commands::Sync { path, suite } => {
            let cases = registry::load_suite(std::path::Path::new(&path))?;
            info!(count = cases.len(), suite = %suite, "Loaded eval cases from YAML");

            if let Some(db_url) = &cli.db_url {
                let pool = PgPoolOptions::new()
                    .max_connections(2)
                    .connect(db_url)
                    .await?;
                let affected = registry::sync_to_db(&pool, &suite, &cases).await?;
                println!("Synced {affected} eval cases into suite '{suite}'.");
            } else {
                println!(
                    "Loaded {} cases from {path}; pass --db-url to sync to Postgres.",
                    cases.len()
                );
            }
        }

        Commands::List { suite } => {
            let pool = connect_db(cli.db_url.as_deref()).await?;
            let cases = registry::list_suite(&pool, &suite).await?;
            if cases.is_empty() {
                println!("No cases registered for suite '{suite}'. Run `benchmarker sync` first.");
            } else {
                println!("Suite '{suite}' — {} cases:", cases.len());
                for c in &cases {
                    println!("  [{:>6}]  {}  {}", c.pr, c.repo, c.id);
                }
            }
        }

        Commands::Report {
            run_id,
            baseline,
            json,
        } => {
            let pool = connect_db(cli.db_url.as_deref()).await?;
            let report = reporter::generate_report(&pool, run_id).await?;

            let comparison = if let Some(baseline_id) = baseline {
                let baseline_report = reporter::generate_report(&pool, baseline_id).await?;
                Some(reporter::compare_runs(&baseline_report, &report))
            } else {
                None
            };

            let advisory = ci::build_advisory_from_report(&report, comparison.as_ref());

            if json {
                println!("{}", ci::format_json(&advisory)?);
            } else {
                println!("{}", ci::format_human(&advisory));
            }
        }
    }

    Ok(())
}

async fn connect_db(db_url: Option<&str>) -> anyhow::Result<sqlx::PgPool> {
    let url = db_url
        .map(str::to_string)
        .or_else(|| std::env::var("DATABASE_URL").ok())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "No database URL provided. Pass --db-url or set DATABASE_URL environment variable."
            )
        })?;

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&url)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to connect to Postgres: {e}"))?;

    Ok(pool)
}
