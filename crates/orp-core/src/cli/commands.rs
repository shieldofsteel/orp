use anyhow::Result;
use orp_config::{get_maritime_template, Config};
use orp_connector::adapters::ais::AisConnector;
use orp_connector::traits::ConnectorConfig;
use orp_connector::Connector;
use orp_proto::{EventPayload, OrpEvent};
use orp_query::QueryExecutor;
use orp_storage::DuckDbStorage;
use orp_storage::traits::Storage;
use orp_stream::StreamProcessor;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::server;

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
        DuckDbStorage::new_in_memory().map_err(|e| anyhow::anyhow!("Storage init failed: {}", e))?,
    );

    // Load demo data (ports)
    tracing::info!("Loading demo port data...");
    let ports = orp_testbed::generate_synthetic_ports(10);
    for port in &ports {
        storage
            .insert_entity(port)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to insert port: {}", e))?;
    }
    tracing::info!("Loaded {} ports", ports.len());

    // Initialize stream processor
    let processor = Arc::new(StreamProcessor::new(storage.clone(), 50, 5));

    // Initialize query executor
    let query_executor = Arc::new(QueryExecutor::new(storage.clone()));

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

    // Background task: process events from connector
    let processor_bg = processor.clone();
    tokio::spawn(async move {
        while let Some(source_event) = rx.recv().await {
            let event = OrpEvent::new(
                source_event.entity_id.clone(),
                source_event.entity_type.clone(),
                "position_update".to_string(),
                EventPayload::PositionUpdate {
                    latitude: source_event.latitude.unwrap_or(0.0),
                    longitude: source_event.longitude.unwrap_or(0.0),
                    altitude: None,
                    speed_knots: source_event
                        .properties
                        .get("speed")
                        .and_then(|v| v.as_f64())
                        .map(|v| v as f32),
                    heading_degrees: source_event
                        .properties
                        .get("heading")
                        .and_then(|v| v.as_f64())
                        .map(|v| v as f32),
                    course_degrees: source_event
                        .properties
                        .get("course")
                        .and_then(|v| v.as_f64())
                        .map(|v| v as f32),
                },
                source_event.connector_id.clone(),
                0.95,
            );

            // Also set name and ship_type on the event entity
            if let Err(e) = processor_bg.process_event(event).await {
                tracing::warn!("Failed to process event: {}", e);
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

    tracing::info!("Starting HTTP server on {}:{}", config.server.host, port);
    tracing::info!(
        "Dashboard: http://localhost:{}/",
        port
    );
    tracing::info!(
        "API:       http://localhost:{}/api/v1/",
        port
    );
    tracing::info!(
        "Health:    http://localhost:{}/api/v1/health",
        port
    );

    // Start HTTP server (blocks until shutdown)
    server::http::start_server(storage, query_executor, processor, port).await?;

    Ok(())
}

/// Execute an ORP-QL query
pub async fn run_query(query: &str) -> Result<()> {
    // Connect to running instance via HTTP
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
            println!("{}", body);
        }
        Err(_) => {
            println!("ORP server is not running.");
            println!("Start with: orp start --template maritime");
        }
    }

    Ok(())
}
