use anyhow::Result;
use orp_config::{get_maritime_template, Config};
use orp_connector::adapters::ais::AisConnector;
use orp_connector::traits::ConnectorConfig;
use orp_connector::Connector;
use orp_proto::{EventPayload, OrpEvent};
use orp_query::QueryExecutor;
use orp_security::{AbacEngine, AuthState, JwtService};
use orp_security::api_keys::ApiKeyService;
use orp_storage::DuckDbStorage;
use orp_storage::traits::Storage;
use orp_stream::{
    DefaultStreamProcessor, MonitorCondition, MonitorEngine, MonitorRule, StreamProcessor,
    ThresholdOp,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::server;
use orp_stream::RocksDbDedupWindow;

/// Start the ORP server
pub async fn run_start(
    config_path: Option<String>,
    template: Option<String>,
    port_override: Option<u16>,
) -> Result<()> {
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
            // Parse simple condition syntax: "speed > 25"
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
                    tracing::warn!(
                        "🚨 Alert: {} — {}",
                        alert.rule_name,
                        alert.message
                    );
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
    let is_dev_mode = std::env::var("ORP_DEV_MODE")
        .map(|v| v == "true" || v == "1")
        .unwrap_or(false);

    let auth_state: Arc<AuthState> = if is_dev_mode {
        tracing::warn!("⚠️  ORP_DEV_MODE is enabled — authentication is permissive");
        Arc::new(AuthState::dev())
    } else {
        // Production: try to build JWT service from JWT_SECRET
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

    // Build shared API key service
    let api_key_service = Arc::new(ApiKeyService::new());

    // Start HTTP server (blocks until shutdown)
    server::http::start_server(server::http::ServerConfig {
        storage,
        query_executor,
        processor,
        monitor_engine,
        auth_state,
        abac_engine,
        api_key_service,
        audit_signer: None, // freshly generated at startup
        port,
    })
    .await?;

    Ok(())
}

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

/// Execute an ORP-QL query
pub async fn run_query(query: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let response = client
        .post("http://localhost:9090/api/v1/query")
        .json(&serde_json::json!({ "query": query }))
        .send()
        .await;

    match response {
        Ok(resp) => {
            let body = resp.text().await?;
            println!("{}", body);
        }
        Err(_) => {
            println!("Error: ORP server is not running. Start it with `orp start`");
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
            println!("{}", serde_json::to_string_pretty(&parsed).unwrap_or(body));
        }
        Err(_) => {
            println!("ORP server is not running.");
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
            println!("{}", serde_json::to_string_pretty(&parsed).unwrap_or(body));
        }
        Err(_) => {
            println!("ORP server is not running.");
        }
    }

    Ok(())
}
