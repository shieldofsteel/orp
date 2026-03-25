use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "orp")]
#[command(about = "ORP — Open Reality Protocol: Palantir-grade data fusion in a single binary")]
#[command(version)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the ORP server with all services
    Start {
        /// Path to config file (default: config.yaml)
        #[arg(short, long)]
        config: Option<String>,

        /// Use a pre-configured template (e.g., "maritime")
        #[arg(short, long)]
        template: Option<String>,

        /// Override the server port
        #[arg(short, long)]
        port: Option<u16>,
    },

    /// Execute an ORP-QL query
    Query {
        /// The ORP-QL query string
        #[arg(short, long)]
        query: String,
    },

    /// Show system status
    Status,
}
