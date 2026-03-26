use clap::{Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

#[derive(Parser)]
#[command(name = "orp")]
#[command(about = "ORP — Open Reality Protocol: Palantir-grade data fusion in a single binary")]
#[command(version, long_about = None)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the ORP server with all services
    Start {
        /// Override the server port
        #[arg(short, long)]
        port: Option<u16>,

        /// Path to config file (default: config.yaml)
        #[arg(short, long)]
        config: Option<String>,

        /// Use a pre-configured template (e.g., "maritime")
        #[arg(short, long)]
        template: Option<String>,

        /// Enable dev mode (permissive auth, verbose logging)
        #[arg(long)]
        dev: bool,
    },

    /// Execute an ORP-QL query against a running instance
    Query {
        /// The ORP-QL query string (inline)
        query: Option<String>,

        /// Read query from file
        #[arg(short, long)]
        file: Option<String>,

        /// Output format
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },

    /// Show system status and health
    Status,

    /// Manage connectors
    Connectors {
        #[command(subcommand)]
        action: ConnectorAction,
    },

    /// Manage entities
    Entities {
        #[command(subcommand)]
        action: EntityAction,
    },

    /// View events
    Events {
        /// Filter events by entity ID
        #[arg(long)]
        entity: Option<String>,

        /// Only show events since (e.g., "1h", "30m", "2024-01-01")
        #[arg(long)]
        since: Option<String>,

        /// Limit number of events returned
        #[arg(short, long, default_value_t = 50)]
        limit: usize,

        /// Output format
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },

    /// Manage monitor rules
    Monitors {
        #[command(subcommand)]
        action: MonitorAction,
    },

    /// Validate a configuration file
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Show version and build info
    Version,

    /// Generate shell completions
    Completions {
        /// Shell to generate completions for
        shell: Shell,
    },
}

#[derive(Subcommand)]
pub enum ConnectorAction {
    /// List all registered connectors
    List,
    /// Register a new connector
    Add {
        /// Connector name
        #[arg(long)]
        name: String,
        /// Connector type (ais, adsb, http, mqtt)
        #[arg(long, rename_all = "kebab-case")]
        connector_type: String,
        /// Entity type this connector produces
        #[arg(long)]
        entity_type: String,
        /// Trust score (0.0–1.0)
        #[arg(long, default_value_t = 0.8)]
        trust_score: f64,
    },
    /// Remove a connector by ID
    Remove {
        /// Connector ID to remove
        id: String,
    },
}

#[derive(Subcommand)]
pub enum EntityAction {
    /// Search entities
    Search {
        /// Search near a location: lat,lon
        #[arg(long)]
        near: Option<String>,

        /// Radius in km (used with --near)
        #[arg(long, default_value_t = 50.0)]
        radius: f64,

        /// Filter by entity type
        #[arg(short = 't', long)]
        entity_type: Option<String>,

        /// Limit results
        #[arg(short, long, default_value_t = 100)]
        limit: usize,

        /// Output format
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },
    /// Get a specific entity by ID
    Get {
        /// Entity ID
        id: String,

        /// Output format
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
}

#[derive(Subcommand)]
pub enum MonitorAction {
    /// List all monitor rules
    List,
    /// Add a new monitor rule
    Add {
        /// Monitor name
        #[arg(long)]
        name: String,
        /// Entity type to monitor
        #[arg(long)]
        entity_type: String,
        /// Condition (e.g., "speed > 25")
        #[arg(long)]
        condition: String,
        /// Severity (info, warning, critical)
        #[arg(long, default_value = "warning")]
        severity: String,
    },
    /// Remove a monitor rule by ID
    Remove {
        /// Monitor rule ID
        id: String,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Validate a configuration file
    Validate {
        /// Path to config file
        file: String,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum OutputFormat {
    /// Pretty table output (default)
    Table,
    /// JSON output
    Json,
    /// CSV output
    Csv,
}
