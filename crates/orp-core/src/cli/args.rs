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
    #[arg(
        long,
        global = true,
        env = "ORP_HOST",
        default_value = "http://localhost:9090"
    )]
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
    ///   orp start --headless              # API-only mode for servers/Pi/embedded
    ///   orp start --no-auth               # Dev shortcut (permissive auth + dev mode)
    Start {
        /// Override the server port. Defaults to the value in config.yaml
        /// (typically 9090 for HTTP). When TLS flags are passed and `--port`
        /// is not, the default becomes 9443.
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

        /// Headless mode: serve API + WebSocket only, no web frontend.
        /// Ideal for Raspberry Pi, servers, embedded, and CI environments.
        #[arg(long)]
        headless: bool,

        /// Dev shortcut: enables --dev AND sets ORP_DEV_MODE=true.
        /// Equivalent to --dev with ORP_DEV_MODE=true in the environment.
        #[arg(long)]
        no_auth: bool,

        /// Use an in-memory DuckDB instance instead of persisting to disk.
        /// Default is persistent (the configured `storage.duckdb.path`); pass
        /// this for tests, demos, or short-lived CI runs where state should
        /// vanish on shutdown.
        #[arg(long)]
        in_memory: bool,

        /// Seed the API-key store with this admin key on first startup
        /// (only when the `api_keys` table is empty). Pass the literal raw
        /// key value (e.g. `orpk_prod_<id>_<plaintext>`); ORP hashes it
        /// with Argon2id before persisting. If the table is already
        /// non-empty this flag is ignored. If the table is empty AND
        /// this flag is unset, ORP generates a random admin key and
        /// prints it to stderr exactly once — capture it from the
        /// startup logs and rotate before going to production.
        #[arg(long, value_name = "RAW_KEY")]
        bootstrap_admin_key: Option<String>,

        /// Enable the dedicated federation mTLS listener. When set, ORP
        /// also requires `--federation-cert`, `--federation-key`, and
        /// `--federation-ca`.
        #[arg(long, env = "ORP_FED_TLS")]
        federation_tls: bool,

        /// Server certificate (PEM) presented to peers connecting in over
        /// the federation mTLS port. Required when `--federation-tls` is set.
        #[arg(long, env = "ORP_FED_CERT", value_name = "PATH")]
        federation_cert: Option<String>,

        /// Server private key (PEM) for `--federation-cert`.
        #[arg(long, env = "ORP_FED_KEY", value_name = "PATH")]
        federation_key: Option<String>,

        /// CA certificate (PEM) used to verify connecting peers' client
        /// certs. Required when `--federation-tls` is set.
        #[arg(long, env = "ORP_FED_CA", value_name = "PATH")]
        federation_ca: Option<String>,

        /// Bind address for the federation mTLS listener.
        /// Defaults to `0.0.0.0:9443`.
        #[arg(long, env = "ORP_FED_TLS_LISTEN", value_name = "ADDR")]
        federation_tls_listen: Option<String>,

        /// Path to the local Ed25519 signing key (32 raw bytes or 64-char
        /// hex). Without this, ORP generates an ephemeral key at startup
        /// and peers must be re-keyed on every restart.
        #[arg(long, env = "ORP_FED_SIGNING_KEY", value_name = "PATH")]
        federation_signing_key: Option<String>,

        /// Stable identifier for this node when pushing federated entities
        /// to peers. Defaults to a per-process UUID.
        #[arg(long, env = "ORP_NODE_ID", value_name = "ID")]
        node_id: Option<String>,

        /// Path to PEM-encoded TLS server certificate (chain). Pair with
        /// `--tls-key` to enable HTTPS termination via rustls.
        #[arg(long, value_name = "PATH", requires = "tls_key")]
        tls_cert: Option<String>,

        /// Path to PEM-encoded TLS server private key. Required with
        /// `--tls-cert`.
        #[arg(long, value_name = "PATH", requires = "tls_cert")]
        tls_key: Option<String>,

        /// Optional path to a PEM bundle of trusted client CAs. When set,
        /// the server requires every client to present a certificate signed
        /// by one of these CAs (mTLS). Requires `--tls-cert`/`--tls-key`.
        #[arg(long, value_name = "PATH", requires_all = ["tls_cert", "tls_key"])]
        tls_client_ca: Option<String>,

        /// When TLS is active, also bind a plain-HTTP listener on this port
        /// that 301-redirects every request to the HTTPS origin. Common
        /// values: 80 (when ORP is fronted directly).
        #[arg(long, value_name = "PORT", requires_all = ["tls_cert", "tls_key"])]
        redirect_http: Option<u16>,
    },

    /// Connect a data source in one command
    ///
    /// Protocols: ais://, adsb://, mqtt://, http://, ws://, syslog://
    ///
    /// Examples:
    /// ```text
    ///   orp connect ais://0.0.0.0:10110
    ///   orp connect adsb://192.168.1.100:30005
    ///   orp connect mqtt://broker.local:1883/sensors/+
    ///   orp connect http://api.example.com/feed
    ///   orp connect ws://stream.example.com/updates
    ///   orp connect syslog://0.0.0.0:514
    /// ```
    Connect {
        /// Connection URL in the form `<protocol>://<host:port>[/path]`
        url: String,

        /// Human-readable name for this connector (defaults to the URL)
        #[arg(long)]
        name: Option<String>,

        /// Entity type this feed produces (auto-detected from protocol if omitted)
        #[arg(long)]
        entity_type: Option<String>,

        /// Trust score for data from this connector (0.0–1.0)
        #[arg(long, default_value_t = 0.8, value_parser = parse_trust_score)]
        trust_score: f64,
    },

    /// Bulk ingest entities from a JSON or CSV file
    ///
    /// Examples:
    ///   orp ingest vessels.json
    ///   orp ingest aircraft.csv
    ///   orp ingest --dry-run contacts.json
    Ingest {
        /// Path to the JSON (array of objects) or CSV file to ingest
        file: String,

        /// Preview what would be ingested without writing to the database
        #[arg(long)]
        dry_run: bool,

        /// Override entity type for all records (auto-detected if omitted)
        #[arg(long)]
        entity_type: Option<String>,

        /// Trust score for ingested records (0.0–1.0)
        #[arg(long, default_value_t = 0.9, value_parser = parse_trust_score)]
        trust_score: f64,
    },

    /// Manage federated ORP peers
    ///
    /// Examples:
    ///   orp peer add 192.168.1.50:9090
    ///   orp peer list
    ///   orp peer remove peer-abc123
    Peer {
        #[command(subcommand)]
        action: PeerAction,
    },

    /// Export all entities to a file or stdout
    ///
    /// Examples:
    ///   orp export --format geojson > entities.geojson
    ///   orp export --format csv --output-file dump.csv
    ///   orp export --format json --entity-type ship
    Export {
        /// Export format
        #[arg(long, value_enum, default_value_t = ExportFormat::Json)]
        format: ExportFormat,

        /// Write output to a file instead of stdout
        #[arg(long, value_name = "PATH")]
        output_file: Option<String>,

        /// Filter by entity type (ship, aircraft, vehicle, …)
        #[arg(long)]
        entity_type: Option<String>,
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

    /// Run preflight diagnostics — `green/yellow/red` per check.
    ///
    /// Verifies that ORP can start cleanly on this host: `protoc` available,
    /// DuckDB and RocksDB paths writable, the configured port free, the
    /// config file (if present) parseable, and — when `--https-url` is
    /// supplied — that the cert chain validates.
    ///
    /// Examples:
    ///   orp doctor
    ///   orp doctor --config config.yaml
    ///   orp doctor --https-url https://orp.example.com/api/v1/health
    ///
    /// Exit codes:
    ///   0   everything green or yellow (warnings only)
    ///   1   at least one red check
    Doctor {
        /// Path to a config.yaml to validate alongside the host checks.
        /// Defaults to `config.yaml` in the working directory if it exists.
        #[arg(short, long, value_name = "PATH")]
        config: Option<String>,

        /// HTTPS URL to probe for cert chain validity (e.g.
        /// `https://orp.example.com/api/v1/health`). When omitted the cert
        /// check is skipped.
        #[arg(long, value_name = "URL")]
        https_url: Option<String>,
    },

    /// Inspect, verify, and export the persistent audit log.
    ///
    /// Reads directly from the DuckDB audit_log table — does not require a
    /// running ORP server. Useful for compliance audits where an external
    /// reviewer needs cryptographic proof the log was not tampered with.
    ///
    /// Examples:
    ///   orp audit verify --db ~/.local/share/orp/orp.duckdb --public-key <hex>
    ///   orp audit export --db ~/.local/share/orp/orp.duckdb --out audit.jsonl
    Audit {
        #[command(subcommand)]
        action: AuditAction,
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

    /// Generate a self-signed TLS certificate for development and testing
    ///
    /// Writes `cert.pem` and `key.pem` (or the paths given) suitable for
    /// `orp start --tls-cert cert.pem --tls-key key.pem`. NOT for production —
    /// browsers and clients will reject the cert by default. For production,
    /// see docs/TLS.md (Let's Encrypt + corporate PKI).
    ///
    /// Examples:
    ///   orp gen-cert
    ///   orp gen-cert --cn orp.example.test --san orp.example.test --san 127.0.0.1
    ///   orp gen-cert --out-dir /etc/orp/tls --days 90
    GenCert {
        /// Output path for the certificate PEM
        #[arg(long, value_name = "PATH", default_value = "cert.pem")]
        cert_out: String,

        /// Output path for the private key PEM
        #[arg(long, value_name = "PATH", default_value = "key.pem")]
        key_out: String,

        /// Optional output directory. When set, both files are written here
        /// using their default names (cert.pem, key.pem) unless `--cert-out`
        /// / `--key-out` are absolute paths.
        #[arg(long, value_name = "DIR")]
        out_dir: Option<String>,

        /// Common Name (CN) for the certificate subject
        #[arg(long, default_value = "localhost")]
        cn: String,

        /// Subject Alternative Names. Repeat the flag for multiple values.
        /// Accepts DNS names and IP literals. Defaults: localhost, 127.0.0.1, ::1.
        #[arg(long = "san", value_name = "DNS_OR_IP")]
        sans: Vec<String>,

        /// Validity period in days
        #[arg(long, default_value_t = 365)]
        days: u32,

        /// Overwrite existing files instead of refusing
        #[arg(long)]
        force: bool,
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
pub enum AuditAction {
    /// Re-derive every audit row's hash, walk the prev-hash chain, and verify
    /// each Ed25519 signature with the supplied public key. Exits non-zero
    /// (and prints the offending sequence number) on any mismatch.
    Verify {
        /// Path to the DuckDB file containing `audit_log`.
        #[arg(long, value_name = "PATH")]
        db: String,

        /// Hex-encoded Ed25519 public key (32 bytes / 64 hex chars). Falls
        /// back to `ORP_AUDIT_PUBKEY` when omitted.
        #[arg(long = "public-key", env = "ORP_AUDIT_PUBKEY")]
        public_key: String,
    },

    /// Stream every audit row to a JSONL file. Each line carries `prev_hash`,
    /// `hash`, `signature`, and a per-row `verified` boolean (true iff both
    /// the chain hash and the signature check pass). External auditors can
    /// then re-verify lines independently with `--public-key`.
    Export {
        /// Path to the DuckDB file containing `audit_log`.
        #[arg(long, value_name = "PATH")]
        db: String,

        /// Output JSONL path. Use `-` for stdout. (`--out` rather than
        /// `--output` to avoid conflicting with the global `-o/--output`
        /// format flag.)
        #[arg(long = "out", short = 'O', value_name = "PATH", default_value = "-")]
        out: String,

        /// Hex-encoded Ed25519 public key. When omitted (and `ORP_AUDIT_PUBKEY`
        /// is unset), `verified` is reported as `false` for every row — the
        /// chain hash is still recomputed but signatures cannot be checked.
        #[arg(long = "public-key", env = "ORP_AUDIT_PUBKEY")]
        public_key: Option<String>,
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

#[derive(Subcommand)]
pub enum PeerAction {
    /// Register a peer ORP instance for federation
    ///
    /// Example: orp peer add 192.168.1.50:9090
    Add {
        /// Peer host and port in the form host:port
        #[arg(value_name = "HOST:PORT")]
        address: String,

        /// Human-readable name for this peer (defaults to address)
        #[arg(long)]
        name: Option<String>,

        /// Trust score for data received from this peer (0.0–1.0)
        #[arg(long, default_value_t = 0.7, value_parser = parse_trust_score)]
        trust_score: f64,
    },

    /// List all registered peers and their connection status
    ///
    /// Example: orp peer list --output json
    List {
        /// Output format [default: table]
        #[arg(short, long, value_enum, default_value_t = OutputFormat::Table)]
        output: OutputFormat,
    },

    /// Disconnect and remove a peer by ID
    ///
    /// Example: orp peer remove peer-abc123
    Remove {
        /// Peer ID to remove
        id: String,

        /// Skip confirmation prompt
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ExportFormat {
    /// GeoJSON FeatureCollection — for maps and GIS tools
    Geojson,
    /// JSON array of entity objects
    Json,
    /// CSV — spreadsheet-compatible
    Csv,
}

impl ExportFormat {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Geojson => "geojson",
            Self::Json => "json",
            Self::Csv => "csv",
        }
    }
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
    /// WebSocket stream
    Ws,
    /// Syslog UDP/TCP receiver
    Syslog,
}

impl ConnectorType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ais => "ais",
            Self::Adsb => "adsb",
            Self::Http => "http",
            Self::Mqtt => "mqtt",
            Self::Ws => "ws",
            Self::Syslog => "syslog",
        }
    }

    /// Infer the connector type from a URL scheme.
    pub fn from_scheme(scheme: &str) -> Option<Self> {
        match scheme {
            "ais" => Some(Self::Ais),
            "adsb" => Some(Self::Adsb),
            "mqtt" => Some(Self::Mqtt),
            "http" | "https" => Some(Self::Http),
            "ws" | "wss" => Some(Self::Ws),
            "syslog" => Some(Self::Syslog),
            _ => None,
        }
    }

    /// Guess the entity type produced by this connector when not explicitly set.
    pub fn default_entity_type(&self) -> &'static str {
        match self {
            Self::Ais => "ship",
            Self::Adsb => "aircraft",
            Self::Mqtt | Self::Http | Self::Ws | Self::Syslog => "generic",
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
    let v: f64 = s
        .parse()
        .map_err(|_| format!("'{}' is not a valid number", s))?;
    if !(0.0..=1.0).contains(&v) {
        return Err(format!(
            "trust score must be between 0.0 and 1.0 (got {})",
            v
        ));
    }
    Ok(v)
}
