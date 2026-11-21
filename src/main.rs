mod catalog;
mod probe;
mod scenario;
mod web;

use clap::{Parser, Subcommand};
use rmcp::ServiceExt;
use std::path::PathBuf;

#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Subcommand)]
enum Mode {
    /// Start the MCP probe on stdin and stdout.
    Stdio {
        #[arg(long)]
        db: PathBuf,
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u64).range(0..=60_000))]
        response_delay_ms: u64,
        #[arg(long)]
        scenario_tools: bool,
    },
    /// Register a scenario definition from a trusted local caller.
    RegisterScenario {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        file: PathBuf,
    },
    /// Serve the loopback-only browser acceptance environment.
    Web {
        #[arg(long)]
        db: PathBuf,
        #[arg(long, default_value_t = 4173)]
        port: u16,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().mode {
        Mode::Stdio {
            db,
            response_delay_ms,
            scenario_tools,
        } => {
            probe::Probe::new(db, response_delay_ms, scenario_tools)?
                .serve(rmcp::transport::stdio())
                .await?
                .waiting()
                .await?;
        }
        Mode::RegisterScenario { db, file } => scenario::Scenarios::new(db)?.register(&file)?,
        Mode::Web { db, port } => web::serve(db, port).await?,
    }
    Ok(())
}
