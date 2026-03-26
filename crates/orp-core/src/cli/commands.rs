use anyhow::Result;
use colored::Colorize;
use orp_config::{get_maritime_template, Config};
use orp_connector::adapters::ais::AisConnector;
use orp_connector::traits::ConnectorConfig;
use orp_connector::Connector;
use orp_proto::{EventPayload, OrpEvent};
use orp_query::QueryExecutor;
use orp_security::api_keys::ApiKeyService;
use orp_security::{AbacEngine, AuthState, JwtService};
use orp_storage::traits::Storage;
use orp_storage::DuckDbStorage;
use orp_stream::{
    DefaultStreamProcessor, MonitorCondition, MonitorEngine, MonitorRule, StreamProcessor,
    ThresholdOp,
};
use std::collections::HashMap;
use std::io::IsTerminal;
use std::sync::Arc;
use tabled::builder::Builder;
use tabled::settings::{object::Rows, Color, Style};
use tokio::sync::mpsc;

use super::args::{ConnectorType, OutputFormat, Severity};
use crate::server;
use orp_stream::RocksDbDedupWindow;

// ── Terminal detection ────────────────────────────────────────────────────────

/// Returns true if we should emit ANSI colors.
///
/// Priority:
///   1. `NO_COLOR` env var set → no color
///   2. `CLICOLOR_FORCE=1`    → force color
///   3. stdout is a TTY       → color; otherwise no color (piped)
pub fn colors_enabled() -> bool {
    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }
    if std::env::var("CLICOLOR_FORCE")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        return true;
    }
    std::io::stdout().is_terminal()
}

// ── Print helpers ─────────────────────────────────────────────────────────────

pub fn print_header(msg: &str) {
    if colors_enabled() {
        println!("{}", msg.bold().cyan());
    } else {
        println!("{}", msg);
    }
}

pub fn print_success(msg: &str) {
    if colors_enabled() {
        println!("{} {}", "✓".green().bold(), msg.green());
    } else {
        println!("OK: {}", msg);
    }
}

pub fn print_error(msg: &str) {
    if colors_enabled() {
        eprintln!("{} {}", "✗".red().bold(), msg.red());
    } else {
        eprintln!("ERROR: {}", msg);
    }
}

#[allow(dead_code)]
pub fn print_warning(msg: &str) {
    if colors_enabled() {
        eprintln!("{} {}", "⚠".yellow().bold(), msg.yellow());
    } else {
        eprintln!("WARN: {}", msg);
    }
}

/// Print an error and exit with code 1.
#[allow(dead_code)]
pub fn fatal(msg: &str) -> ! {
    print_error(msg);
    std::process::exit(1);
}

// ── Confirmation prompt ───────────────────────────────────────────────────────

/// Ask for y/N confirmation unless `skip` is true or we're non-interactive.
/// Returns `false` (abort) if stdin is not a TTY and skip=false.
pub fn confirm(prompt: &str, skip: bool) -> bool {
    if skip {
        return true;
    }
    if !std::io::stdin().is_terminal() {
        print_error(&format!(
            "{} — pass --yes to confirm non-interactively",
            prompt
        ));
        return false;
    }
    eprint!("{} [y/N] ", prompt);
    let mut input = String::new();
    let _ = std::io::stdin().read_line(&mut input);
    matches!(input.trim().to_lowercase().as_str(), "y" | "yes")
}

// ── Table rendering (using `tabled`) ─────────────────────────────────────────

/// Build a pretty table from a slice of row maps + explicit column order.
pub fn render_table(rows: &[HashMap<String, serde_json::Value>], columns: &[&str]) -> String {
    if rows.is_empty() {
        return "  (no results)".to_string();
    }

    let mut builder = Builder::default();
    builder.push_record(columns.iter().map(|s| s.to_string()));

    for row in rows {
        let record: Vec<String> = columns
            .iter()
            .map(|col| {
                row.get(*col)
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Null => String::new(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default()
            })
            .collect();
        builder.push_record(record);
    }

    let mut table = builder.build();
    table.with(Style::rounded());

    if colors_enabled() {
        table.modify(Rows::first(), Color::BOLD);
    }

    table.to_string()
}

/// Build a table from key/value pairs (for single-entity display).
pub fn render_kv_table(pairs: &[(&str, String)]) -> String {
    let mut builder = Builder::default();
    builder.push_record(["FIELD".to_string(), "VALUE".to_string()]);
    for (k, v) in pairs {
        builder.push_record([k, v.as_str()]);
    }
    let mut table = builder.build();
    table.with(Style::rounded());
    if colors_enabled() {
        table.modify(Rows::first(), Color::BOLD);
    }
    table.to_string()
}

// ── CSV helper ────────────────────────────────────────────────────────────────

pub fn format_csv(rows: &[HashMap<String, serde_json::Value>], columns: &[&str]) -> String {
    let mut wtr = csv::Writer::from_writer(vec![]);
    let _ = wtr.write_record(columns);
    for row in rows {
        let record: Vec<String> = columns
            .iter()
            .map(|col| {
                row.get(*col)
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Null => String::new(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default()
            })
            .collect();
        let _ = wtr.write_record(&record);
    }
    let _ = wtr.flush();
    String::from_utf8(wtr.into_inner().unwrap_or_default()).unwrap_or_default()
}

// ── Parse simple condition ────────────────────────────────────────────────────

/// Parse simple condition strings like "speed > 25" into MonitorCondition.
fn parse_simple_condition(condition: &str) -> Option<MonitorCondition> {
    let parts: Vec<&str> = condition.split_whitespace().collect();
    if parts.len() != 3 {
        return None;
    }

    let property = parts[0].to_string();
    let operator = match parts[1] {
        ">" => ThresholdOp::GreaterThan,
        "<" => ThresholdOp::LessThan,
        ">=" => ThresholdOp::GreaterThanOrEqual,
        "<=" => ThresholdOp::LessThanOrEqual,
        "=" | "==" => ThresholdOp::Equal,
        "!=" => ThresholdOp::NotEqual,
        _ => return None,
    };

    let value: f64 = parts[2].parse().ok()?;

    Some(MonitorCondition::PropertyThreshold {
        property,
        operator,
        value,
    })
}

// ── Parse relative time ───────────────────────────────────────────────────────

fn parse_relative_time(s: &str) -> Option<String> {
    let s = s.trim();
    let (num_str, unit) = if let Some(stripped) = s.strip_suffix('h') {
        (stripped, "hours")
    } else if let Some(stripped) = s.strip_suffix('m') {
        (stripped, "minutes")
    } else if let Some(stripped) = s.strip_suffix('d') {
        (stripped, "days")
    } else {
        return None;
    };

    let num: i64 = num_str.parse().ok()?;
    let duration = match unit {
        "hours" => chrono::Duration::hours(num),
        "minutes" => chrono::Duration::minutes(num),
        "days" => chrono::Duration::days(num),
        _ => return None,
    };

    Some((chrono::Utc::now() - duration).to_rfc3339())
}

// ── HTTP client helper ────────────────────────────────────────────────────────

fn base_url(host: &str) -> String {
    let h = host.trim_end_matches('/');
    if h.starts_with("http://") || h.starts_with("https://") {
        h.to_string()
    } else {
        format!("http://{}", h)
    }
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// Start the ORP server
pub async fn run_start(
    config_path: Option<String>,
    template: Option<String>,
    port_override: Option<u16>,
    dev: bool,
) -> Result<()> {
    if dev {
        std::env::set_var("ORP_DEV_MODE", "true");
    }

    let config = if let Some(ref path) = config_path {
        Config::load_from_file(path).map_err(|e| anyhow::anyhow!("{}", e))?
    } else if template.as_deref() == Some("maritime") {
        get_maritime_template()
    } else {
        Config::load_or_default()
    };

    let port = port_override.unwrap_or(config.server.port);

    if colors_enabled() {
        println!(
            r#"
  ╔═══════════════════════════════════════════════════════════╗
  ║                                                           ║
  ║   ██████╗ ██████╗ ██████╗                                 ║
  ║  ██╔═══██╗██╔══██╗██╔══██╗                                ║
  ║  ██║   ██║██████╔╝██████╔╝                                ║
  ║  ██║   ██║██╔══██╗██╔═══╝                                 ║
  ║  ╚██████╔╝██║  ██║██║                                     ║
  ║   ╚═════╝ ╚═╝  ╚═╝╚═╝                                    ║
  ║                                                           ║
  ║  Open Reality Protocol v{}                             ║
  ║  Palantir-grade data fusion in a single binary            ║
  ║                                                           ║
  ╚═══════════════════════════════════════════════════════════╝
"#,
            env!("CARGO_PKG_VERSION")
        );
    }

    tracing::info!("Initializing ORP...");

    // Initialize storage
    tracing::info!("Initializing DuckDB storage...");
    let storage: Arc<dyn Storage> = Arc::new(
        DuckDbStorage::new_in_memory()
            .map_err(|e| anyhow::anyhow!("Storage init failed: {}", e))?,
    );

    // Load demo data (ports)
    tracing::info!("Loading demo port data...");
    let ports = orp_testbed::generate_synthetic_ports(10);
    for port_entity in &ports {
        storage
            .insert_entity(port_entity)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to insert port: {}", e))?;
    }
    tracing::info!("Loaded {} ports", ports.len());

    // Initialize stream processor — use a unique dedup path per run to avoid
    // stale duplicate hashes from previous sessions blocking new events.
    let dedup_path = std::env::temp_dir().join(format!("orp-dedup-{}", std::process::id()));
    std::fs::create_dir_all(&dedup_path).ok();
    let dedup = Arc::new(
        RocksDbDedupWindow::open(&dedup_path, 3600)
            .map_err(|e| anyhow::anyhow!("Dedup init failed: {}", e))?,
    );
    let processor: Arc<dyn StreamProcessor> =
        Arc::new(DefaultStreamProcessor::new(storage.clone(), dedup, None, 50));

    // Initialize query executor
    let query_executor = Arc::new(QueryExecutor::new(storage.clone()));

    // Initialize monitor engine with default rules
    let monitor_engine = Arc::new(MonitorEngine::new());
    for monitor_def in &config.monitors {
        if monitor_def.enabled {
            let condition =
                parse_simple_condition(&monitor_def.condition).unwrap_or_else(|| {
                    MonitorCondition::PropertyThreshold {
                        property: "speed".to_string(),
                        operator: ThresholdOp::GreaterThan,
                        value: 25.0,
                    }
                });

            monitor_engine
                .add_rule(MonitorRule {
                    rule_id: monitor_def.rule_id.clone(),
                    name: monitor_def.name.clone(),
                    description: format!("Condition: {}", monitor_def.condition),
                    entity_type: monitor_def.entity_type.clone(),
                    condition,
                    action: orp_stream::MonitorAction::Alert,
                    enabled: true,
                    cooldown_seconds: 300,
                    severity: orp_stream::AlertSeverity::Warning,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                })
                .await;
        }
    }
    tracing::info!(
        "Monitor engine initialized with {} rules",
        monitor_engine.get_rules().await.len()
    );

    // Start AIS connector (demo data)
    tracing::info!("Starting AIS connector (demo mode)...");
    let (tx, mut rx) = mpsc::channel(10000);

    let ais_config = ConnectorConfig {
        connector_id: "ais-demo".to_string(),
        connector_type: "ais".to_string(),
        url: None,
        entity_type: "ship".to_string(),
        enabled: true,
        trust_score: 0.95,
        properties: HashMap::new(),
    };

    let ais_connector = AisConnector::new(ais_config);
    ais_connector
        .start(tx)
        .await
        .map_err(|e| anyhow::anyhow!("AIS connector failed: {}", e))?;

    let _ = storage
        .register_data_source(&orp_proto::DataSource {
            source_id: "ais-demo".to_string(),
            source_name: "AIS Demo Feed".to_string(),
            source_type: "ais".to_string(),
            trust_score: 0.95,
            events_ingested: 0,
            enabled: true,
        })
        .await;

    // Background task: process events from connector
    let processor_bg = processor.clone();
    let monitor_bg = monitor_engine.clone();
    let storage_bg = storage.clone();
    tokio::spawn(async move {
        while let Some(source_event) = rx.recv().await {
            let event = OrpEvent::new(
                source_event.entity_id.clone(),
                source_event.entity_type.clone(),
                EventPayload::PositionUpdate {
                    latitude: source_event.latitude.unwrap_or(0.0),
                    longitude: source_event.longitude.unwrap_or(0.0),
                    altitude: None,
                    accuracy_meters: None,
                    speed_knots: source_event
                        .properties
                        .get("speed")
                        .and_then(|v| v.as_f64()),
                    heading_degrees: source_event
                        .properties
                        .get("heading")
                        .and_then(|v| v.as_f64()),
                    course_degrees: source_event
                        .properties
                        .get("course")
                        .and_then(|v| v.as_f64()),
                },
                source_event.connector_id.clone(),
                0.95,
            );

            let ctx = orp_stream::StreamContext {
                event,
                dedup_window_seconds: 3600,
                batch_size: 50,
            };
            if let Err(e) = processor_bg.process_event(ctx).await {
                tracing::warn!("Failed to process event: {}", e);
            }

            // Send property change events for name and other metadata so entity.name is set
            for prop_key in &["name", "ship_type", "mmsi"] {
                if let Some(val) = source_event.properties.get(*prop_key) {
                    let prop_event = OrpEvent::new(
                        source_event.entity_id.clone(),
                        source_event.entity_type.clone(),
                        EventPayload::PropertyChange {
                            key: prop_key.to_string(),
                            old_value: None,
                            new_value: val.clone(),
                            is_derived: false,
                        },
                        source_event.connector_id.clone(),
                        0.95,
                    );
                    let prop_ctx = orp_stream::StreamContext {
                        event: prop_event,
                        dedup_window_seconds: 3600,
                        batch_size: 50,
                    };
                    if let Err(e) = processor_bg.process_event(prop_ctx).await {
                        tracing::warn!("Failed to process property event: {}", e);
                    }
                }
            }

            if let Ok(Some(entity)) = storage_bg.get_entity(&source_event.entity_id).await {
                let alerts = monitor_bg.evaluate(&entity).await;
                for alert in &alerts {
                    tracing::warn!("🚨 Alert: {} — {}", alert.rule_name, alert.message);
                }
            }

            // Flush immediately for the first batch to ensure entities appear quickly
            let _ = processor_bg.flush().await;
        }
    });

    // Periodic flush
    let processor_flush = processor.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(5));
        loop {
            interval.tick().await;
            if let Err(e) = processor_flush.flush().await {
                tracing::warn!("Flush error: {}", e);
            }
        }
    });

    let is_dev_mode = dev
        || std::env::var("ORP_DEV_MODE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);

    let auth_state: Arc<AuthState> = if is_dev_mode {
        tracing::warn!("⚠️  ORP_DEV_MODE is enabled — authentication is permissive");
        Arc::new(AuthState::dev())
    } else {
        match JwtService::from_env() {
            Ok(jwt_svc) => {
                let api_key_svc = Arc::new(ApiKeyService::new());
                Arc::new(AuthState::production(Arc::new(jwt_svc), api_key_svc))
            }
            Err(_) => {
                tracing::warn!(
                    "JWT_SECRET not set and ORP_DEV_MODE not enabled — auth will reject all \
                     requests. Set JWT_SECRET or pass --dev"
                );
                Arc::new(AuthState::default())
            }
        }
    };

    let abac_engine: Arc<AbacEngine> = if is_dev_mode {
        Arc::new(AbacEngine::default_permissive())
    } else {
        Arc::new(AbacEngine::default_production())
    };

    tracing::info!("Starting HTTP server on {}:{}", config.server.host, port);
    tracing::info!("Dashboard: http://localhost:{}/", port);
    tracing::info!("API:       http://localhost:{}/api/v1/", port);
    tracing::info!("Health:    http://localhost:{}/api/v1/health", port);

    let api_key_service = Arc::new(ApiKeyService::new());

    server::http::start_server(server::http::ServerConfig {
        storage,
        query_executor,
        processor,
        monitor_engine,
        auth_state,
        abac_engine,
        api_key_service,
        audit_signer: None,
        port,
    })
    .await?;

    Ok(())
}

/// Execute an ORP-QL query
pub async fn run_query(host: &str, query: &str, format: OutputFormat) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/query", base_url(host));
    let response = client
        .post(&url)
        .json(&serde_json::json!({ "query": query }))
        .send()
        .await;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await?;
            let parsed: serde_json::Value =
                serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!(body));

            if !status.is_success() {
                let msg = parsed
                    .get("error")
                    .and_then(|e| e.as_str())
                    .unwrap_or(&body);
                print_error(&format!("Query failed (HTTP {}): {}", status, msg));
                std::process::exit(1);
            }

            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&parsed)?);
                }
                OutputFormat::Table => {
                    if let Some(results) = parsed.get("results").and_then(|r| r.as_array()) {
                        let columns: Vec<String> = parsed
                            .get("columns")
                            .and_then(|c| c.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_default();

                        let rows: Vec<HashMap<String, serde_json::Value>> = results
                            .iter()
                            .filter_map(|r| serde_json::from_value(r.clone()).ok())
                            .collect();

                        let cols: Vec<String> = if columns.is_empty() && !rows.is_empty() {
                            rows[0].keys().cloned().collect()
                        } else {
                            columns
                        };
                        let col_refs: Vec<&str> = cols.iter().map(String::as_str).collect();
                        println!("{}", render_table(&rows, &col_refs));

                        if let Some(meta) = parsed.get("metadata") {
                            let time = meta
                                .get("execution_time_ms")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0);
                            let count = meta
                                .get("rows_returned")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(rows.len() as u64);
                            eprintln!("\n{} rows in {:.1}ms", count, time);
                        }
                    } else {
                        println!("{}", serde_json::to_string_pretty(&parsed)?);
                    }
                }
                OutputFormat::Csv => {
                    if let Some(results) = parsed.get("results").and_then(|r| r.as_array()) {
                        let rows: Vec<HashMap<String, serde_json::Value>> = results
                            .iter()
                            .filter_map(|r| serde_json::from_value(r.clone()).ok())
                            .collect();
                        let columns: Vec<String> = parsed
                            .get("columns")
                            .and_then(|c| c.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str().map(String::from))
                                    .collect()
                            })
                            .unwrap_or_else(|| {
                                rows.first()
                                    .map(|r| r.keys().cloned().collect())
                                    .unwrap_or_default()
                            });
                        let col_refs: Vec<&str> = columns.iter().map(String::as_str).collect();
                        print!("{}", format_csv(&rows, &col_refs));
                    }
                }
            }
        }
        Err(e) => {
            print_error(&format!(
                "Cannot reach ORP server at {}: {}",
                host, e
            ));
            eprintln!("  → Start the server with: orp start --template maritime");
            eprintln!("  → Or point to a different host: orp --host <host:port> …");
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Show system status
pub async fn run_status(host: &str, format: OutputFormat) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/health", base_url(host));
    let response = client.get(&url).send().await;

    match response {
        Ok(resp) => {
            let body = resp.text().await?;
            let parsed: serde_json::Value =
                serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!(body));

            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&parsed)?);
                    return Ok(());
                }
                OutputFormat::Csv => {
                    // status as simple csv
                    let status = parsed.get("status").and_then(|s| s.as_str()).unwrap_or("?");
                    let version = parsed.get("version").and_then(|v| v.as_str()).unwrap_or("?");
                    println!("status,version");
                    println!("{},{}", status, version);
                    return Ok(());
                }
                OutputFormat::Table => {}
            }

            print_header("ORP Status");
            println!();

            if let Some(status) = parsed.get("status").and_then(|s| s.as_str()) {
                let status_display = if status == "healthy" {
                    if colors_enabled() {
                        format!("{}", "● healthy".green())
                    } else {
                        "healthy".to_string()
                    }
                } else if colors_enabled() {
                    format!("{}", format!("● {}", status).red())
                } else {
                    status.to_string()
                };
                println!("  Status:  {}", status_display);
            }
            if let Some(version) = parsed.get("version").and_then(|v| v.as_str()) {
                println!("  Version: {}", version);
            }
            if let Some(uptime) = parsed.get("uptime_seconds").and_then(|u| u.as_u64()) {
                let hours = uptime / 3600;
                let mins = (uptime % 3600) / 60;
                let secs = uptime % 60;
                println!("  Uptime:  {}h {}m {}s", hours, mins, secs);
            }
            println!("  Host:    {}", host);

            if let Some(components) = parsed.get("components").and_then(|c| c.as_object()) {
                println!();
                print_header("Components");
                for (name, info) in components {
                    let status = info
                        .get("status")
                        .and_then(|s| s.as_str())
                        .unwrap_or("unknown");
                    let latency = info
                        .get("latency_ms")
                        .and_then(|l| l.as_f64())
                        .map(|l| format!(" ({:.1}ms)", l))
                        .unwrap_or_default();
                    let indicator = if status == "healthy" {
                        if colors_enabled() {
                            format!("{}", "●".green())
                        } else {
                            "OK ".to_string()
                        }
                    } else {
                        if colors_enabled() {
                            format!("{}", "●".red())
                        } else {
                            "ERR".to_string()
                        }
                    };
                    println!("  {} {}{}", indicator, name, latency);
                }
            }
        }
        Err(e) => {
            print_error(&format!("Cannot reach ORP server at {}: {}", host, e));
            eprintln!("  → Start the server with: orp start --template maritime");
            eprintln!("  → Or set ORP_HOST env var to point to the correct server");
            std::process::exit(1);
        }
    }

    Ok(())
}

/// List connectors
pub async fn run_connectors_list(host: &str, format: OutputFormat) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/connectors", base_url(host));
    let response = client.get(&url).send().await;

    match response {
        Ok(resp) => {
            let body = resp.text().await?;
            let parsed: serde_json::Value =
                serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!(body));

            let data = parsed
                .get("data")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();

            match format {
                OutputFormat::Json => {
                    println!("{}", serde_json::to_string_pretty(&parsed)?);
                }
                OutputFormat::Csv => {
                    let rows: Vec<HashMap<String, serde_json::Value>> = data
                        .iter()
                        .filter_map(|r| serde_json::from_value(r.clone()).ok())
                        .collect();
                    print!("{}", format_csv(&rows, &["source_id", "source_name", "source_type", "trust_score", "enabled"]));
                }
                OutputFormat::Table => {
                    if data.is_empty() {
                        println!("  No connectors registered.");
                        println!("  → Add one with: orp connectors add --name <name> --connector-type ais --entity-type ship");
                    } else {
                        let rows: Vec<HashMap<String, serde_json::Value>> = data
                            .iter()
                            .filter_map(|r| serde_json::from_value(r.clone()).ok())
                            .collect();
                        println!("{}", render_table(&rows, &["source_id", "source_name", "source_type", "trust_score", "enabled"]));
                    }
                }
            }
        }
        Err(e) => {
            print_error(&format!("Cannot reach ORP server at {}: {}", host, e));
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Add a connector
pub async fn run_connectors_add(
    host: &str,
    name: &str,
    connector_type: ConnectorType,
    entity_type: &str,
    trust_score: f64,
) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/connectors", base_url(host));
    let response = client
        .post(&url)
        .json(&serde_json::json!({
            "name": name,
            "connector_type": connector_type.as_str(),
            "entity_type": entity_type,
            "trust_score": trust_score,
        }))
        .send()
        .await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            print_success(&format!("Connector '{}' registered.", name));
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await?;
            print_error(&format!("Failed to add connector (HTTP {}): {}", status, body));
            std::process::exit(1);
        }
        Err(e) => {
            print_error(&format!("Cannot reach ORP server at {}: {}", host, e));
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Remove a connector
pub async fn run_connectors_remove(host: &str, id: &str, yes: bool) -> Result<()> {
    if !confirm(&format!("Remove connector '{}'?", id), yes) {
        println!("Aborted.");
        return Ok(());
    }

    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/connectors/{}", base_url(host), id);
    let response = client.delete(&url).send().await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            print_success(&format!("Connector '{}' removed.", id));
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await?;
            print_error(&format!("Failed to remove connector (HTTP {}): {}", status, body));
            std::process::exit(1);
        }
        Err(e) => {
            print_error(&format!("Cannot reach ORP server at {}: {}", host, e));
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Search entities
pub async fn run_entities_search(
    host: &str,
    near: Option<&str>,
    radius: f64,
    entity_type: Option<&str>,
    limit: usize,
    format: OutputFormat,
) -> Result<()> {
    let client = reqwest::Client::new();
    let mut params = vec![format!("limit={}", limit)];

    if let Some(near_str) = near {
        params.push(format!("near={},{}", near_str, radius));
    }
    if let Some(etype) = entity_type {
        params.push(format!("type={}", etype));
    }
    let url = format!(
        "{}/api/v1/entities/search?{}",
        base_url(host),
        params.join("&")
    );

    let response = client.get(&url).send().await;

    match response {
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await?;
            let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|_| {
                serde_json::json!({ "error": body })
            });

            if !status.is_success() {
                let msg = parsed.get("error").and_then(|e| e.as_str()).unwrap_or(&body);
                print_error(&format!("Search failed (HTTP {}): {}", status, msg));
                std::process::exit(1);
            }

            let data = parsed
                .get("data")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();
            let count = parsed.get("count").and_then(|c| c.as_u64()).unwrap_or(data.len() as u64);

            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&parsed)?),
                OutputFormat::Table => {
                    let rows: Vec<HashMap<String, serde_json::Value>> = data
                        .iter()
                        .filter_map(|r| serde_json::from_value(r.clone()).ok())
                        .collect();
                    println!("{}", render_table(&rows, &["id", "type", "name", "confidence"]));
                    eprintln!("\n{} entities found", count);
                }
                OutputFormat::Csv => {
                    let rows: Vec<HashMap<String, serde_json::Value>> = data
                        .iter()
                        .filter_map(|r| serde_json::from_value(r.clone()).ok())
                        .collect();
                    print!("{}", format_csv(&rows, &["id", "type", "name", "confidence"]));
                }
            }
        }
        Err(e) => {
            print_error(&format!("Cannot reach ORP server at {}: {}", host, e));
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Get entity by ID
pub async fn run_entities_get(host: &str, id: &str, format: OutputFormat) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/entities/{}", base_url(host), id);
    let response = client.get(&url).send().await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            let body = resp.text().await?;
            let parsed: serde_json::Value = serde_json::from_str(&body)?;

            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&parsed)?),
                OutputFormat::Table => {
                    // Render the entity as a key-value table
                    if let Some(obj) = parsed.as_object() {
                        let pairs: Vec<(&str, String)> = obj
                            .iter()
                            .map(|(k, v)| {
                                let val = match v {
                                    serde_json::Value::String(s) => s.clone(),
                                    serde_json::Value::Null => String::new(),
                                    other => other.to_string(),
                                };
                                (k.as_str(), val)
                            })
                            .collect();
                        println!("{}", render_kv_table(&pairs));
                    } else {
                        println!("{}", serde_json::to_string_pretty(&parsed)?);
                    }
                }
                OutputFormat::Csv => {
                    // Single entity as one CSV row
                    if let Some(obj) = parsed.as_object() {
                        let keys: Vec<&str> = obj.keys().map(String::as_str).collect();
                        let mut wtr = csv::Writer::from_writer(vec![]);
                        let _ = wtr.write_record(&keys);
                        let vals: Vec<String> = obj
                            .values()
                            .map(|v| match v {
                                serde_json::Value::String(s) => s.clone(),
                                serde_json::Value::Null => String::new(),
                                other => other.to_string(),
                            })
                            .collect();
                        let _ = wtr.write_record(&vals);
                        let _ = wtr.flush();
                        if let Ok(inner) = wtr.into_inner() {
                            print!("{}", String::from_utf8_lossy(&inner));
                        }
                    }
                }
            }
        }
        Ok(resp) => {
            let status = resp.status();
            if status.as_u16() == 404 {
                print_error(&format!("Entity '{}' not found.", id));
                eprintln!("  → Use `orp entities search` to find valid entity IDs");
            } else {
                let body = resp.text().await?;
                print_error(&format!("Failed to get entity (HTTP {}): {}", status, body));
            }
            std::process::exit(1);
        }
        Err(e) => {
            print_error(&format!("Cannot reach ORP server at {}: {}", host, e));
            std::process::exit(1);
        }
    }

    Ok(())
}

/// View events
pub async fn run_events(
    host: &str,
    entity: Option<&str>,
    since: Option<&str>,
    limit: usize,
    format: OutputFormat,
) -> Result<()> {
    let client = reqwest::Client::new();
    let mut params = vec![format!("limit={}", limit)];
    if let Some(eid) = entity {
        params.push(format!("entity_id={}", eid));
    }
    if let Some(s) = since {
        let since_val = parse_relative_time(s).unwrap_or_else(|| s.to_string());
        params.push(format!("since={}", since_val));
    }
    let url = format!(
        "{}/api/v1/events?{}",
        base_url(host),
        params.join("&")
    );

    let response = client.get(&url).send().await;

    match response {
        Ok(resp) => {
            let body = resp.text().await?;
            let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|_| {
                serde_json::json!({ "error": body })
            });

            let data = parsed
                .get("data")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();

            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&parsed)?),
                OutputFormat::Table => {
                    let rows: Vec<HashMap<String, serde_json::Value>> = data
                        .iter()
                        .filter_map(|r| serde_json::from_value(r.clone()).ok())
                        .collect();
                    if rows.is_empty() {
                        println!("  No events found.");
                    } else {
                        println!("{}", render_table(&rows, &["id", "entity_id", "event_type", "timestamp"]));
                    }
                }
                OutputFormat::Csv => {
                    let rows: Vec<HashMap<String, serde_json::Value>> = data
                        .iter()
                        .filter_map(|r| serde_json::from_value(r.clone()).ok())
                        .collect();
                    print!("{}", format_csv(&rows, &["id", "entity_id", "event_type", "timestamp"]));
                }
            }
        }
        Err(e) => {
            print_error(&format!("Cannot reach ORP server at {}: {}", host, e));
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Monitors list
pub async fn run_monitors_list(host: &str, format: OutputFormat) -> Result<()> {
    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/monitors", base_url(host));
    let response = client.get(&url).send().await;

    match response {
        Ok(resp) => {
            let body = resp.text().await?;
            let parsed: serde_json::Value = serde_json::from_str(&body).unwrap_or_else(|_| {
                serde_json::json!({ "error": body })
            });

            let data = parsed
                .get("data")
                .and_then(|d| d.as_array())
                .cloned()
                .unwrap_or_default();

            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&parsed)?),
                OutputFormat::Csv => {
                    let rows: Vec<HashMap<String, serde_json::Value>> = data
                        .iter()
                        .filter_map(|r| serde_json::from_value(r.clone()).ok())
                        .collect();
                    print!("{}", format_csv(&rows, &["rule_id", "name", "entity_type", "severity", "enabled"]));
                }
                OutputFormat::Table => {
                    if data.is_empty() {
                        println!("  No monitor rules defined.");
                        println!("  → Add one with: orp monitors add --name \"Fast Ship\" --entity-type ship --condition \"speed > 25\"");
                    } else {
                        let rows: Vec<HashMap<String, serde_json::Value>> = data
                            .iter()
                            .filter_map(|r| serde_json::from_value(r.clone()).ok())
                            .collect();
                        println!("{}", render_table(&rows, &["rule_id", "name", "entity_type", "severity", "enabled"]));
                    }
                }
            }
        }
        Err(e) => {
            print_error(&format!("Cannot reach ORP server at {}: {}", host, e));
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Monitors add
pub async fn run_monitors_add(
    host: &str,
    name: &str,
    entity_type: &str,
    condition: &str,
    severity: Severity,
) -> Result<()> {
    // Validate condition before sending
    if parse_simple_condition(condition).is_none() {
        print_error(&format!(
            "Invalid condition '{}'. Expected format: \"<property> <op> <value>\" (e.g., \"speed > 25\")",
            condition
        ));
        eprintln!("  Supported operators: >, <, >=, <=, =, !=");
        std::process::exit(1);
    }

    let parts: Vec<&str> = condition.split_whitespace().collect();
    let (property, operator, value) = (
        parts[0].to_string(),
        parts[1].to_string(),
        parts[2].parse::<f64>().unwrap_or(0.0),
    );

    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/monitors", base_url(host));
    let response = client
        .post(&url)
        .json(&serde_json::json!({
            "name": name,
            "entity_type": entity_type,
            "condition": {
                "type": "property_threshold",
                "property": property,
                "operator": operator,
                "value": value,
            },
            "severity": severity.as_str(),
        }))
        .send()
        .await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            print_success(&format!("Monitor rule '{}' created.", name));
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await?;
            print_error(&format!("Failed to create monitor (HTTP {}): {}", status, body));
            std::process::exit(1);
        }
        Err(e) => {
            print_error(&format!("Cannot reach ORP server at {}: {}", host, e));
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Monitors remove
pub async fn run_monitors_remove(host: &str, id: &str, yes: bool) -> Result<()> {
    if !confirm(&format!("Remove monitor rule '{}'?", id), yes) {
        println!("Aborted.");
        return Ok(());
    }

    let client = reqwest::Client::new();
    let url = format!("{}/api/v1/monitors/{}", base_url(host), id);
    let response = client.delete(&url).send().await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            print_success(&format!("Monitor rule '{}' removed.", id));
        }
        Ok(resp) => {
            let status = resp.status();
            let body = resp.text().await?;
            print_error(&format!("Failed to remove monitor (HTTP {}): {}", status, body));
            std::process::exit(1);
        }
        Err(e) => {
            print_error(&format!("Cannot reach ORP server at {}: {}", host, e));
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Validate config
pub fn run_config_validate(file: &str) -> Result<()> {
    print_header(&format!("Validating: {}", file));
    println!();

    match Config::load_from_file(file) {
        Ok(_config) => {
            print_success("Configuration is valid.");
        }
        Err(e) => {
            print_error(&format!("Configuration invalid: {}", e));
            std::process::exit(1);
        }
    }

    Ok(())
}

/// Show version + build info
pub fn run_version() {
    print_header("ORP — Open Reality Protocol");
    println!();
    println!("  Version:  {}", env!("CARGO_PKG_VERSION"));
    println!("  Edition:  Rust 2021");
    println!("  Target:   {}", std::env::consts::ARCH);
    println!("  OS:       {}", std::env::consts::OS);
}

/// Generate shell completions
pub fn run_completions(shell: clap_complete::Shell) {
    use clap::CommandFactory;
    clap_complete::generate(
        shell,
        &mut super::args::Cli::command(),
        "orp",
        &mut std::io::stdout(),
    );
}
