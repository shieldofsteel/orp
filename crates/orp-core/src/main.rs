mod cli;
mod server;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let args = cli::args::Cli::parse();

    match args.command {
        cli::args::Commands::Start {
            config,
            template,
            port,
        } => {
            cli::commands::run_start(config, template, port).await?;
        }
        cli::args::Commands::Query { query } => {
            cli::commands::run_query(&query).await?;
        }
        cli::args::Commands::Status => {
            cli::commands::run_status().await?;
        }
        cli::args::Commands::Connectors { action } => match action {
            cli::args::ConnectorAction::List => {
                cli::commands::run_connectors_list().await?;
            }
        },
    }

    Ok(())
}
