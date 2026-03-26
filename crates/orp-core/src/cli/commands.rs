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
use std::sync::Arc;
use tokio::sync::mpsc;

use super::args::OutputFormat;
use crate::server;
use orp_stream::RocksDbDedupWindow;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Check NO_COLOR env var to respect the convention.
fn colors_enabled() -> bool {
    std::env::var("NO_COLOR").is_err()
}

fn print_header(msg: &str) {
    if colors_enabled() {
        println!("{}", msg.bold().cyan());
    } else {
        println!("{}", msg);
    }
}

fn print_success(msg: &str) {
    if colors_enabled() {
        println!("{} {}", "✓".green().bold(), msg.green());
    } else {
        println!("OK: {}", msg);
    }
}

fn print_error(msg: &str) {
    if colors_enabled() {
        eprintln!("{} {}", "✗".red().bold(), msg.red());
    } else {
        eprintln!("ERROR: {}", msg);
    }
}

fn format_csv(rows: &[HashMap<String, serde_json::Value>], columns: &[String]) -> String {
    let mut wtr = csv::Writer::from_writer(vec![]);
    // Header
    let _ = wtr.write_record(columns);
    // Rows
    for row in rows {
        let record: Vec<String> = columns
            .iter()
            .map(|col| {
                row.get(col)
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

fn format_query_table(rows: &[HashMap<String, serde_json::Value>], columns: &[String]) -> String {
    if rows.is_empty() {
        return "No results.".to_string();
    }

    // Build a simple table with columns as headers
    let mut lines = Vec::new();

    // Determine column widths
    let col_widths: Vec<usize> = columns
        .iter()
        .map(|col| {
            let header_w = col.len();
            let max_val = rows
                .iter()
                .map(|row| {
                    row.get(col)
                        .map(|v| match v {
                            serde_json::Value::String(s) => s.len(),
                            serde_json::Value::Null => 4,
                            other => other.to_string().len(),
                        })
                        .unwrap_or(0)
                })
                .max()
                .unwrap_or(0);
            header_w.max(max_val).min(40)
        })
        .collect();

    // Header
    let header: String = columns
        .iter()
        .zip(&col_widths)
        .map(|(col, w)| format!("{:<width$}", col, width = w))
        .collect::<Vec<_>>()
        .join(" │ ");
    lines.push(header);

    // Separator
    let sep: String = col_widths
        .iter()
        .map(|w| "─".repeat(*w))
        .collect::<Vec<_>>()
        .join("─┼─");
    lines.push(sep);

    // Rows
    for row in rows {
        let row_str: String = columns
            .iter()
            .zip(&col_widths)
            .map(|(col, w)| {
                let val = row
                    .get(col)
                    .map(|v| match v {
                        serde_json::Value::String(s) => s.clone(),
                        serde_json::Value::Null => "null".to_string(),
                        other => other.to_string(),
                    })
                    .unwrap_or_default();
                let truncated = if val.len() > *w {
                    format!("{}…", &val[..*w - 1])
                } else {
                    val
                };
                format!("{:<width$}", truncated, width = w)
            })
            .collect::<Vec<_>>()
            .join(" │ ");
        lines.push(row_str);
    }

    lines.join("\n")
}

// ── Parse simple condition ──────────────────────────────────────────────────

/// Parse simple condition strings like "speed > 25" into MonitorCondition
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

// ── Commands ────────────────────────────────────────────────────────────────

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
  ║  Open Reality Protocol v0.1.0                             ║
  ║  Palantir-grade data fusion in a single binary            ║
  ║                                                           ║
  ╚═══════════════════════════════════════════════════════════╝
"#
    );

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

    // Initialize stream processor
    let dedup_path = std::env::temp_dir().join("orp-dedup");
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

    // Register connector data source
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

            // Run monitor evaluation on updated entity
            if let Ok(Some(entity)) = storage_bg.get_entity(&source_event.entity_id).await {
                let alerts = monitor_bg.evaluate(&entity).await;
                for alert in &alerts {
                    tracing::warn!("🚨 Alert: {} — {}", alert.rule_name, alert.message);
                }
            }

            // Update name/ship_type properties directly
            if source_event.properties.contains_key("name") {
                let _ = processor_bg.flush().await;
            }
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

    // ── Initialize auth + ABAC ──────────────────────────────────────────────
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
                    "JWT_SECRET not set and ORP_DEV_MODE not enabled — auth will reject all requests. \
                     Set JWT_SECRET or ORP_DEV_MODE=true"
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
pub async fn run_query(query: &str, format: OutputFormat) -> Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .post("http://localhost:9090/api/v1/query")
        .json(&serde_json::json!({ "query": query }))
        .send()
        .await;

    match response {
        Ok(resp) => {
            let body = resp.text().await?;
            let parsed: serde_json::Value =
                serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!(body));

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
                            .filter_map(|r| {
                                serde_json::from_value(r.clone()).ok()
                            })
                            .collect();

                        let cols = if columns.is_empty() && !rows.is_empty() {
                            rows[0].keys().cloned().collect()
                        } else {
                            columns
                        };

                        println!("{}", format_query_table(&rows, &cols));

                        if let Some(meta) = parsed.get("metadata") {
                            let time = meta
                                .get("execution_time_ms")
                                .and_then(|v| v.as_f64())
                                .unwrap_or(0.0);
                            let count = meta
                                .get("rows_returned")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            println!(
                                "\n{} rows in {:.1}ms",
                                count, time
                            );
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
                        print!("{}", format_csv(&rows, &columns));
                    }
                }
            }
        }
        Err(_) => {
            print_error("ORP server is not running. Start it with `orp start`");
        }
    }

    Ok(())
}

/// Show system status
pub async fn run_status() -> Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .get("http://localhost:9090/api/v1/health")
        .send()
        .await;

    match response {
        Ok(resp) => {
            let body = resp.text().await?;
            let parsed: serde_json::Value =
                serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!(body));

            print_header("ORP Status");
            println!();

            if let Some(status) = parsed.get("status").and_then(|s| s.as_str()) {
                let status_display = if status == "healthy" {
                    if colors_enabled() {
                        format!("{}", "● healthy".green())
                    } else {
                        "healthy".to_string()
                    }
                } else {
                    if colors_enabled() {
                        format!("{}", format!("● {}", status).red())
                    } else {
                        status.to_string()
                    }
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
                            "OK".to_string()
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
        Err(_) => {
            print_error("ORP server is not running.");
            println!("Start with: orp start --template maritime");
        }
    }

    Ok(())
}

/// List connectors
pub async fn run_connectors_list() -> Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .get("http://localhost:9090/api/v1/connectors")
        .send()
        .await;

    match response {
        Ok(resp) => {
            let body = resp.text().await?;
            let parsed: serde_json::Value =
                serde_json::from_str(&body).unwrap_or_else(|_| serde_json::json!(body));

            if let Some(data) = parsed.get("data").and_then(|d| d.as_array()) {
                print_header("Connectors");
                println!();
                if data.is_empty() {
                    println!("  No connectors registered.");
                } else {
                    for item in data {
                        let id = item.get("source_id").and_then(|v| v.as_str()).unwrap_or("?");
                        let name = item.get("source_name").and_then(|v| v.as_str()).unwrap_or("?");
                        let ctype = item.get("source_type").and_then(|v| v.as_str()).unwrap_or("?");
                        let enabled = item.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false);
                        let indicator = if enabled {
                            if colors_enabled() { format!("{}", "●".green()) } else { "ON".to_string() }
                        } else {
                            if colors_enabled() { format!("{}", "●".red()) } else { "OFF".to_string() }
                        };
                        println!("  {} {} [{}] — {}", indicator, name, ctype, id);
                    }
                }
            } else {
                println!("{}", serde_json::to_string_pretty(&parsed)?);
            }
        }
        Err(_) => {
            print_error("ORP server is not running.");
        }
    }

    Ok(())
}

/// Add a connector
pub async fn run_connectors_add(
    name: &str,
    connector_type: &str,
    entity_type: &str,
    trust_score: f64,
) -> Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .post("http://localhost:9090/api/v1/connectors")
        .json(&serde_json::json!({
            "name": name,
            "connector_type": connector_type,
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
            let body = resp.text().await?;
            print_error(&format!("Failed to add connector: {}", body));
        }
        Err(_) => {
            print_error("ORP server is not running.");
        }
    }

    Ok(())
}

/// Remove a connector
pub async fn run_connectors_remove(id: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .delete(format!("http://localhost:9090/api/v1/connectors/{}", id))
        .send()
        .await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            print_success(&format!("Connector '{}' removed.", id));
        }
        Ok(resp) => {
            let body = resp.text().await?;
            print_error(&format!("Failed to remove connector: {}", body));
        }
        Err(_) => {
            print_error("ORP server is not running.");
        }
    }

    Ok(())
}

/// Search entities
pub async fn run_entities_search(
    near: Option<&str>,
    radius: f64,
    entity_type: Option<&str>,
    limit: usize,
    format: OutputFormat,
) -> Result<()> {
    let client = reqwest::Client::new();
    let mut url = "http://localhost:9090/api/v1/entities/search".to_string();
    let mut params = vec![format!("limit={}", limit)];

    if let Some(near_str) = near {
        params.push(format!("near={},{}", near_str, radius));
    }
    if let Some(etype) = entity_type {
        params.push(format!("type={}", etype));
    }
    if !params.is_empty() {
        url = format!("{}?{}", url, params.join("&"));
    }

    let response = client.get(&url).send().await;

    match response {
        Ok(resp) => {
            let body = resp.text().await?;
            let parsed: serde_json::Value = serde_json::from_str(&body)?;

            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&parsed)?),
                OutputFormat::Table => {
                    if let Some(data) = parsed.get("data").and_then(|d| d.as_array()) {
                        let rows: Vec<HashMap<String, serde_json::Value>> = data
                            .iter()
                            .filter_map(|r| serde_json::from_value(r.clone()).ok())
                            .collect();
                        let cols = vec![
                            "id".to_string(),
                            "type".to_string(),
                            "name".to_string(),
                            "confidence".to_string(),
                        ];
                        println!("{}", format_query_table(&rows, &cols));
                        let count = parsed.get("count").and_then(|c| c.as_u64()).unwrap_or(0);
                        println!("\n{} entities found", count);
                    }
                }
                OutputFormat::Csv => {
                    if let Some(data) = parsed.get("data").and_then(|d| d.as_array()) {
                        let rows: Vec<HashMap<String, serde_json::Value>> = data
                            .iter()
                            .filter_map(|r| serde_json::from_value(r.clone()).ok())
                            .collect();
                        let cols = vec![
                            "id".to_string(),
                            "type".to_string(),
                            "name".to_string(),
                        ];
                        print!("{}", format_csv(&rows, &cols));
                    }
                }
            }
        }
        Err(_) => {
            print_error("ORP server is not running.");
        }
    }

    Ok(())
}

/// Get entity by ID
pub async fn run_entities_get(id: &str, format: OutputFormat) -> Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .get(format!("http://localhost:9090/api/v1/entities/{}", id))
        .send()
        .await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            let body = resp.text().await?;
            let parsed: serde_json::Value = serde_json::from_str(&body)?;

            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&parsed)?),
                OutputFormat::Table | OutputFormat::Csv => {
                    println!("{}", serde_json::to_string_pretty(&parsed)?);
                }
            }
        }
        Ok(resp) => {
            let body = resp.text().await?;
            print_error(&format!("Entity not found: {}", body));
        }
        Err(_) => {
            print_error("ORP server is not running.");
        }
    }

    Ok(())
}

/// View events
pub async fn run_events(
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
        // Support relative time like "1h", "30m"
        let since_val = parse_relative_time(s).unwrap_or_else(|| s.to_string());
        params.push(format!("since={}", since_val));
    }
    let url = format!(
        "http://localhost:9090/api/v1/events?{}",
        params.join("&")
    );

    let response = client.get(&url).send().await;

    match response {
        Ok(resp) => {
            let body = resp.text().await?;
            let parsed: serde_json::Value = serde_json::from_str(&body)?;

            match format {
                OutputFormat::Json => println!("{}", serde_json::to_string_pretty(&parsed)?),
                OutputFormat::Table => {
                    if let Some(data) = parsed.get("data").and_then(|d| d.as_array()) {
                        let rows: Vec<HashMap<String, serde_json::Value>> = data
                            .iter()
                            .filter_map(|r| serde_json::from_value(r.clone()).ok())
                            .collect();
                        let cols = vec![
                            "id".to_string(),
                            "entity_id".to_string(),
                            "event_type".to_string(),
                            "timestamp".to_string(),
                        ];
                        println!("{}", format_query_table(&rows, &cols));
                    }
                }
                OutputFormat::Csv => {
                    if let Some(data) = parsed.get("data").and_then(|d| d.as_array()) {
                        let rows: Vec<HashMap<String, serde_json::Value>> = data
                            .iter()
                            .filter_map(|r| serde_json::from_value(r.clone()).ok())
                            .collect();
                        let cols = vec![
                            "id".to_string(),
                            "entity_id".to_string(),
                            "event_type".to_string(),
                            "timestamp".to_string(),
                        ];
                        print!("{}", format_csv(&rows, &cols));
                    }
                }
            }
        }
        Err(_) => {
            print_error("ORP server is not running.");
        }
    }

    Ok(())
}

/// Monitors list
pub async fn run_monitors_list() -> Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .get("http://localhost:9090/api/v1/monitors")
        .send()
        .await;

    match response {
        Ok(resp) => {
            let body = resp.text().await?;
            let parsed: serde_json::Value = serde_json::from_str(&body)?;
            println!("{}", serde_json::to_string_pretty(&parsed)?);
        }
        Err(_) => {
            print_error("ORP server is not running.");
        }
    }

    Ok(())
}

/// Monitors add
pub async fn run_monitors_add(
    name: &str,
    entity_type: &str,
    condition: &str,
    severity: &str,
) -> Result<()> {
    let client = reqwest::Client::new();

    // Parse condition into parts
    let parts: Vec<&str> = condition.split_whitespace().collect();
    let (property, operator, value) = if parts.len() == 3 {
        (
            parts[0].to_string(),
            parts[1].to_string(),
            parts[2].parse::<f64>().unwrap_or(0.0),
        )
    } else {
        ("speed".to_string(), ">".to_string(), 25.0)
    };

    let response = client
        .post("http://localhost:9090/api/v1/monitors")
        .json(&serde_json::json!({
            "name": name,
            "entity_type": entity_type,
            "condition": {
                "type": "property_threshold",
                "property": property,
                "operator": operator,
                "value": value,
            },
            "severity": severity,
        }))
        .send()
        .await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            print_success(&format!("Monitor '{}' created.", name));
        }
        Ok(resp) => {
            let body = resp.text().await?;
            print_error(&format!("Failed to create monitor: {}", body));
        }
        Err(_) => {
            print_error("ORP server is not running.");
        }
    }

    Ok(())
}

/// Monitors remove
pub async fn run_monitors_remove(id: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .delete(format!("http://localhost:9090/api/v1/monitors/{}", id))
        .send()
        .await;

    match response {
        Ok(resp) if resp.status().is_success() => {
            print_success(&format!("Monitor '{}' removed.", id));
        }
        Ok(resp) => {
            let body = resp.text().await?;
            print_error(&format!("Failed to remove monitor: {}", body));
        }
        Err(_) => {
            print_error("ORP server is not running.");
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

// ── Parse relative time ─────────────────────────────────────────────────────

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

    let since = chrono::Utc::now() - duration;
    Some(since.to_rfc3339())
}
