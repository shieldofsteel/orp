use clap::{Parser, Subcommand, ValueEnum};
use clap_complete::Shell;

/// ORP — Open Reality Protocol: Palantir-grade data fusion in a single binary.
///
/// Examples:
///   orp start --template maritime
///   orp query "SELECT * FROM entities WHERE type = 'ship'"
///   orp entities search --near 1.3521,103.8198 --radius 25
///   orp events --since 1h --output json
///   orp completions zsh > ~/.zfunc/_orp
#[derive(Parser)]
#[command(name = "orp")]
#[command(about = "ORP — Open Reality Protocol: Palantir-grade data fusion in a single binary")]
#[command(version, long_about = None)]
#[command(propagate_version = true)]
pub struct Cli {
    /// ORP server host (default: localhost:9090). Can also be set via ORP_HOST env var.
    #[arg(long, global = true, env = "ORP_HOST", default_value = "http://localhost:9090")]
    pub host: String,

    /// Suppress non-essential output (errors still shown on stderr)
    #[arg(short, long, global = true)]
    pub quiet: bool,

    /// Output format override (overrides subcommand default)
    #[arg(short = 'o', long = "output", global = true, value_enum)]
    pub output: Option<OutputFormat>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Start the ORP server with all services
    ///
    /// Examples:
    ///   orp start
    ///   orp start --template maritime
    ///   orp start --port 8080 --dev
    Start {
        /// Override the server port
        #[arg(short, long)]
        port: Option<u16>,

        /// Path to config file (default: config.yaml)
        #[arg(short, long, value_name = "PATH")]
        config: Option<String>,

        /// Use a pre-configured template (e.g., "maritime", "air-traffic")
        #[arg(short, long, value_name = "NAME")]
        template: Option<String>,

        /// Enable dev mode (permissive auth, verbose logging, auto-reloads)
        #[arg(long)]
        dev: bool,
    },

    /// Execute an ORP-QL query against a running instance
    ///
    /// Examples:
    ///   orp query "SELECT * FROM entities WHERE type = 'ship'"
    ///   orp query --file ./my_query.orpql --output json
    ///   orp query "SELECT id, name FROM entities" | jq '.results'
    Query {
        /// The ORP-QL query string (inline). Omit to read from --file or stdin.
        query: Option<String>,

        /// Read query from file (use '-' for stdin)
        #[arg(short, long, value_name = "PATH")]
        file: Option<String>,

        /// Output format [default: table]
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },

    /// Show system status and health of a running ORP instance
    ///
    /// Examples:
    ///   orp status
    ///   orp status --output json | jq '.status'
    Status {
        /// Output format [default: table]
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },

    /// Manage data connectors (AIS, ADS-B, HTTP, MQTT, …)
    Connectors {
        #[command(subcommand)]
        action: ConnectorAction,
    },

    /// Manage tracked entities
    Entities {
        #[command(subcommand)]
        action: EntityAction,
    },

    /// View the event stream
    ///
    /// Examples:
    ///   orp events --since 1h
    ///   orp events --entity SHIP-001 --output json
    ///   orp events --since 30m --output csv > events.csv
    Events {
        /// Filter events by entity ID
        #[arg(long, value_name = "ENTITY_ID")]
        entity: Option<String>,

        /// Only show events since (e.g., "1h", "30m", "2d", RFC-3339)
        #[arg(long, value_name = "DURATION")]
        since: Option<String>,

        /// Maximum number of events to return
        #[arg(short, long, default_value_t = 50)]
        limit: usize,

        /// Output format [default: table]
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },

    /// Manage monitor rules (alert when conditions are met)
    Monitors {
        #[command(subcommand)]
        action: MonitorAction,
    },

    /// Manage and validate ORP configuration files
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Show version and build information
    Version,

    /// Generate shell completion scripts
    ///
    /// Examples:
    ///   orp completions bash > /etc/bash_completion.d/orp
    ///   orp completions zsh > ~/.zfunc/_orp
    ///   orp completions fish > ~/.config/fish/completions/orp.fish
    ///   orp completions powershell | Out-String | Invoke-Expression
    Completions {
        /// Target shell
        shell: Shell,
    },
}

#[derive(Subcommand)]
pub enum ConnectorAction {
    /// List all registered connectors
    ///
    /// Example: orp connectors list --output json
    List {
        /// Output format [default: table]
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },

    /// Register a new data connector
    ///
    /// Example: orp connectors add --name ais-feed --connector-type ais --entity-type ship
    Add {
        /// Human-readable connector name
        #[arg(long)]
        name: String,

        /// Connector protocol type
        #[arg(long, value_enum)]
        connector_type: ConnectorType,

        /// Entity type this connector produces (e.g., ship, aircraft, vehicle)
        #[arg(long)]
        entity_type: String,

        /// Trust score for data from this connector (0.0–1.0)
        #[arg(long, default_value_t = 0.8, value_parser = parse_trust_score)]
        trust_score: f64,
    },

    /// Remove a connector by ID (cannot be undone)
    ///
    /// Example: orp connectors remove ais-demo
    Remove {
        /// Connector ID to remove
        id: String,

        /// Skip confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum EntityAction {
    /// Search tracked entities with optional geo/type filters
    ///
    /// Examples:
    ///   orp entities search --near 1.3521,103.8198 --radius 50
    ///   orp entities search --entity-type ship --output json
    Search {
        /// Search near a location: lat,lon (e.g., "51.5,-0.127")
        #[arg(long, value_name = "LAT,LON")]
        near: Option<String>,

        /// Search radius in kilometres (used with --near)
        #[arg(long, default_value_t = 50.0)]
        radius: f64,

        /// Filter by entity type (ship, aircraft, vehicle, …)
        #[arg(short = 't', long, value_name = "TYPE")]
        entity_type: Option<String>,

        /// Maximum number of results
        #[arg(short, long, default_value_t = 100)]
        limit: usize,

        /// Output format [default: table]
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },

    /// Get a specific entity by ID
    ///
    /// Example: orp entities get SHIP-001 --output json
    Get {
        /// Entity ID
        id: String,

        /// Output format [default: json]
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Json)]
        output: OutputFormat,
    },
}

#[derive(Subcommand)]
pub enum MonitorAction {
    /// List all active monitor rules
    ///
    /// Example: orp monitors list --output json
    List {
        /// Output format [default: table]
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },

    /// Create a new monitor rule
    ///
    /// Examples:
    ///   orp monitors add --name "Fast Ship" --entity-type ship --condition "speed > 25" --severity warning
    ///   orp monitors add --name "High Alt" --entity-type aircraft --condition "altitude > 40000" --severity critical
    Add {
        /// Monitor rule name
        #[arg(long)]
        name: String,

        /// Entity type to watch (ship, aircraft, vehicle, …)
        #[arg(long)]
        entity_type: String,

        /// Alert condition expression (e.g., "speed > 25", "altitude <= 100")
        #[arg(long)]
        condition: String,

        /// Alert severity level
        #[arg(long, value_enum, default_value_t = Severity::Warning)]
        severity: Severity,
    },

    /// Remove a monitor rule by ID (cannot be undone)
    ///
    /// Example: orp monitors remove rule-001
    Remove {
        /// Monitor rule ID
        id: String,

        /// Skip confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
pub enum ConfigAction {
    /// Validate a configuration file and report any errors
    ///
    /// Example: orp config validate config.yaml
    Validate {
        /// Path to config file
        #[arg(value_name = "FILE")]
        file: String,
    },
}

// ── Value Enums ──────────────────────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum OutputFormat {
    /// Pretty table (default for interactive terminals)
    Table,
    /// JSON — structured, machine-readable
    Json,
    /// CSV — spreadsheet-compatible
    Csv,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ConnectorType {
    /// Automatic Identification System (maritime vessels)
    Ais,
    /// Automatic Dependent Surveillance–Broadcast (aircraft)
    Adsb,
    /// Generic HTTP/REST data source
    Http,
    /// MQTT message broker
    Mqtt,
}

impl ConnectorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ais => "ais",
            Self::Adsb => "adsb",
            Self::Http => "http",
            Self::Mqtt => "mqtt",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Severity {
    /// Informational — no action required
    Info,
    /// Warning — review recommended
    Warning,
    /// Critical — immediate action required
    Critical,
}

impl Severity {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

// ── Validators ───────────────────────────────────────────────────────────────

fn parse_trust_score(s: &str) -> Result<f64, String> {
    let v: f64 = s.parse().map_err(|_| format!("'{}' is not a valid number", s))?;
    if !(0.0..=1.0).contains(&v) {
        return Err(format!("trust score must be between 0.0 and 1.0 (got {})", v));
    }
    Ok(v)
}
