use crate::schema::*;

/// Returns a maritime-focused configuration template
pub fn get_maritime_template() -> Config {
    Config {
        server: ServerConfig {
            host: "0.0.0.0".to_string(),
            port: 9090,
            workers: 4,
            log_level: "info".to_string(),
        },
        storage: StorageConfig::default(),
        security: SecurityConfig::default(),
        connectors: vec![ConnectorDef {
            name: "ais_demo".to_string(),
            connector_type: "ais".to_string(),
            enabled: true,
            url: Some("tcp://ais.example.com:5631".to_string()),
            entity_type: "ship".to_string(),
            trust_score: 0.95,
            schedule: None,
        }],
        monitors: vec![MonitorRule {
            rule_id: "speed_alert".to_string(),
            name: "High speed alert".to_string(),
            entity_type: "ship".to_string(),
            condition: "speed > 25".to_string(),
            action: "alert".to_string(),
            enabled: true,
        }],
        api: ApiConfig::default(),
        frontend: FrontendConfig {
            enabled: true,
            port: 9090,
            default_map_center: [51.92, 4.27], // Rotterdam
        },
        logging: LoggingConfig::default(),
    }
}
