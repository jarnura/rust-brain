//! rustbrain-benchmarker — eval suite runner for rust-brain agent accuracy.
//!
//! Usage: `benchmarker bench run [--suite default] [--release v1.2]`

pub mod registry;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "benchmarker", about = "Eval suite runner for rust-brain")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run an eval suite
    Bench {
        #[arg(long, default_value = "default")]
        suite: String,
        #[arg(long)]
        release: Option<String>,
    },
    /// Sync a YAML suite file into the database
    Sync {
        /// Path to the YAML suite file
        path: String,
        #[arg(long, default_value = "default")]
        suite: String,
    },
    /// List cases registered for a suite
    List {
        #[arg(long, default_value = "default")]
        suite: String,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("benchmarker=info".parse()?),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Bench { suite, release } => {
            tracing::info!(?suite, ?release, "bench run not yet implemented");
            println!("bench run: suite={suite} release={release:?}");
        }
        Commands::Sync { path, suite } => {
            let cases = registry::load_suite(std::path::Path::new(&path))?;
            println!(
                "Loaded {} cases from {path}; use --db-url to sync to Postgres.",
                cases.len()
            );
            tracing::info!(?suite, count = cases.len(), "sync dry-run");
        }
        Commands::List { suite } => {
            println!("list: suite={suite} (connect to DB to list persisted cases)");
        }
    }

    Ok(())
}
