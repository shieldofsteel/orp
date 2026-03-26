mod cli;
pub mod error;
pub mod retry;
mod server;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing (suppressed for piped/quiet output via RUST_LOG)
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .init();

    let args = cli::args::Cli::parse();
    let host = &args.host;

    match args.command {
        cli::args::Commands::Start {
            config,
            template,
            port,
            dev,
            headless,
            no_auth,
        } => {
            cli::commands::run_start(config, template, port, dev, headless, no_auth).await?;
        }

        cli::args::Commands::Query {
            query,
            file,
            output,
        } => {
            // Allow reading query from stdin via `--file -`
            let query_str = if let Some(ref path) = file {
                if path == "-" {
                    use std::io::Read;
                    let mut buf = String::new();
                    std::io::stdin()
                        .read_to_string(&mut buf)
                        .map_err(|e| anyhow::anyhow!("Failed to read stdin: {}", e))?;
                    buf
                } else {
                    std::fs::read_to_string(path)
                        .map_err(|e| anyhow::anyhow!("Failed to read query file '{}': {}", path, e))?
                }
            } else if let Some(q) = query {
                q
            } else {
                // Last resort: try reading stdin if not a TTY
                use std::io::{IsTerminal, Read};
                if !std::io::stdin().is_terminal() {
                    let mut buf = String::new();
                    std::io::stdin()
                        .read_to_string(&mut buf)
                        .map_err(|e| anyhow::anyhow!("Failed to read stdin: {}", e))?;
                    buf.trim().to_string()
                } else {
                    anyhow::bail!(
                        "No query provided.\n  \
                         Usage: orp query \"SELECT * FROM entities\"\n  \
                         Or:    orp query --file query.orpql\n  \
                         Or:    echo \"SELECT * FROM entities\" | orp query"
                    );
                }
            };

            let fmt = args.output.unwrap_or(output);
            cli::commands::run_query(host, &query_str, fmt).await?;
        }

        cli::args::Commands::Status { output } => {
            let fmt = args.output.unwrap_or(output);
            cli::commands::run_status(host, fmt).await?;
        }

        cli::args::Commands::Connectors { action } => match action {
            cli::args::ConnectorAction::List { output } => {
                let fmt = args.output.unwrap_or(output);
                cli::commands::run_connectors_list(host, fmt).await?;
            }
            cli::args::ConnectorAction::Add {
                name,
                connector_type,
                entity_type,
                trust_score,
            } => {
                cli::commands::run_connectors_add(host, &name, connector_type, &entity_type, trust_score)
                    .await?;
            }
            cli::args::ConnectorAction::Remove { id, yes } => {
                cli::commands::run_connectors_remove(host, &id, yes).await?;
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
                let fmt = args.output.unwrap_or(output);
                cli::commands::run_entities_search(
                    host,
                    near.as_deref(),
                    radius,
                    entity_type.as_deref(),
                    limit,
                    fmt,
                )
                .await?;
            }
            cli::args::EntityAction::Get { id, output } => {
                let fmt = args.output.unwrap_or(output);
                cli::commands::run_entities_get(host, &id, fmt).await?;
            }
        },

        cli::args::Commands::Events {
            entity,
            since,
            limit,
            output,
        } => {
            let fmt = args.output.unwrap_or(output);
            cli::commands::run_events(host, entity.as_deref(), since.as_deref(), limit, fmt)
                .await?;
        }

        cli::args::Commands::Monitors { action } => match action {
            cli::args::MonitorAction::List { output } => {
                let fmt = args.output.unwrap_or(output);
                cli::commands::run_monitors_list(host, fmt).await?;
            }
            cli::args::MonitorAction::Add {
                name,
                entity_type,
                condition,
                severity,
            } => {
                cli::commands::run_monitors_add(host, &name, &entity_type, &condition, severity)
                    .await?;
            }
            cli::args::MonitorAction::Remove { id, yes } => {
                cli::commands::run_monitors_remove(host, &id, yes).await?;
            }
        },

        cli::args::Commands::Config { action } => match action {
            cli::args::ConfigAction::Validate { file } => {
                cli::commands::run_config_validate(&file)?;
            }
        },

        cli::args::Commands::Connect {
            url,
            name,
            entity_type,
            trust_score,
        } => {
            cli::commands::run_connect(host, &url, name.as_deref(), entity_type.as_deref(), trust_score)
                .await?;
        }

        cli::args::Commands::Ingest {
            file,
            dry_run,
            entity_type,
            trust_score,
        } => {
            cli::commands::run_ingest(host, &file, dry_run, entity_type.as_deref(), trust_score)
                .await?;
        }

        cli::args::Commands::Peer { action } => match action {
            cli::args::PeerAction::Add { address, name, trust_score } => {
                cli::commands::run_peer_add(host, &address, name.as_deref(), trust_score).await?;
            }
            cli::args::PeerAction::List { output } => {
                let fmt = args.output.unwrap_or(output);
                cli::commands::run_peer_list(host, fmt).await?;
            }
            cli::args::PeerAction::Remove { id, yes } => {
                cli::commands::run_peer_remove(host, &id, yes).await?;
            }
        },

        cli::args::Commands::Export {
            format,
            output_file,
            entity_type,
        } => {
            cli::commands::run_export(host, format, output_file.as_deref(), entity_type.as_deref())
                .await?;
        }

        cli::args::Commands::Version => {
            cli::commands::run_version();
        }

        cli::args::Commands::Completions { shell } => {
            cli::commands::run_completions(shell);
        }
    }

    Ok(())
}
