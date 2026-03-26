mod cli;
pub mod error;
pub mod retry;
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
            dev,
        } => {
            cli::commands::run_start(config, template, port, dev).await?;
        }
        cli::args::Commands::Query {
            query,
            file,
            output,
        } => {
            let query_str = if let Some(path) = file {
                std::fs::read_to_string(&path)
                    .map_err(|e| anyhow::anyhow!("Failed to read query file '{}': {}", path, e))?
            } else if let Some(q) = query {
                q
            } else {
                anyhow::bail!("Provide a query string or use --file <path>");
            };
            cli::commands::run_query(&query_str, output).await?;
        }
        cli::args::Commands::Status => {
            cli::commands::run_status().await?;
        }
        cli::args::Commands::Connectors { action } => match action {
            cli::args::ConnectorAction::List => {
                cli::commands::run_connectors_list().await?;
            }
            cli::args::ConnectorAction::Add {
                name,
                connector_type,
                entity_type,
                trust_score,
            } => {
                cli::commands::run_connectors_add(&name, &connector_type, &entity_type, trust_score)
                    .await?;
            }
            cli::args::ConnectorAction::Remove { id } => {
                cli::commands::run_connectors_remove(&id).await?;
            }
        },
        cli::args::Commands::Entities { action } => match action {
            cli::args::EntityAction::Search {
                near,
                radius,
                entity_type,
                limit,
                output,
            } => {
                cli::commands::run_entities_search(
                    near.as_deref(),
                    radius,
                    entity_type.as_deref(),
                    limit,
                    output,
                )
                .await?;
            }
            cli::args::EntityAction::Get { id, output } => {
                cli::commands::run_entities_get(&id, output).await?;
            }
        },
        cli::args::Commands::Events {
            entity,
            since,
            limit,
            output,
        } => {
            cli::commands::run_events(entity.as_deref(), since.as_deref(), limit, output).await?;
        }
        cli::args::Commands::Monitors { action } => match action {
            cli::args::MonitorAction::List => {
                cli::commands::run_monitors_list().await?;
            }
            cli::args::MonitorAction::Add {
                name,
                entity_type,
                condition,
                severity,
            } => {
                cli::commands::run_monitors_add(&name, &entity_type, &condition, &severity).await?;
            }
            cli::args::MonitorAction::Remove { id } => {
                cli::commands::run_monitors_remove(&id).await?;
            }
        },
        cli::args::Commands::Config { action } => match action {
            cli::args::ConfigAction::Validate { file } => {
                cli::commands::run_config_validate(&file)?;
            }
        },
        cli::args::Commands::Version => {
            cli::commands::run_version();
        }
        cli::args::Commands::Completions { shell } => {
            cli::commands::run_completions(shell);
        }
    }

    Ok(())
}
