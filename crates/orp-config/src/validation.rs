//! Configuration validation logic.
//!
//! All validation rules live here. The entry point is [`validate_config`].

use crate::schema::*;

const VALID_LOG_LEVELS: &[&str] = &["trace", "debug", "info", "warn", "error"];
const VALID_LOG_FORMATS: &[&str] = &["json", "text"];
const VALID_SIGNING_ALGOS: &[&str] = &["Ed25519"];
const VALID_RESOLUTION_PHASES: &[&str] = &["structural", "probabilistic"];

/// Run all validation rules against `config`.
///
/// Returns `Ok(())` when valid, or `Err(errors)` with a list of every problem found.
pub fn validate_config(config: &Config) -> Result<(), Vec<String>> {
    let mut errors: Vec<String> = Vec::new();

    validate_server(&config.server, &mut errors);
    validate_storage(&config.storage, &mut errors);
    validate_retention(&config.retention, &mut errors);
    validate_security(&config.security, &mut errors);
    validate_connectors(&config.connectors, &mut errors);
    validate_entity_resolution(&config.entity_resolution, &mut errors);
    validate_monitors(&config.monitors, &mut errors);
    validate_api(&config.api, &mut errors);
    validate_logging(&config.logging, &mut errors);
    validate_templates(&config.templates, &mut errors);

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

// ── Per-section validators ────────────────────────────────────────────────────

fn validate_server(s: &ServerConfig, errors: &mut Vec<String>) {
    if s.port == 0 {
        errors.push("server.port must be > 0".to_string());
    }
    if s.workers == 0 {
        errors.push("server.workers must be >= 1".to_string());
    }
    if !VALID_LOG_LEVELS.contains(&s.log_level.as_str()) {
        errors.push(format!(
            "server.log_level '{}' is invalid; must be one of {:?}",
            s.log_level, VALID_LOG_LEVELS
        ));
    }
    if s.telemetry_enabled && s.telemetry_endpoint.is_none() {
        errors.push(
            "server.telemetry_endpoint must be set when telemetry_enabled = true".to_string(),
        );
    }
}

fn validate_storage(s: &StorageConfig, errors: &mut Vec<String>) {
    if s.duckdb.memory_limit_gb == 0 {
        errors.push("storage.duckdb.memory_limit_gb must be >= 1".to_string());
    }
    if s.duckdb.max_connections == 0 {
        errors.push("storage.duckdb.max_connections must be >= 1".to_string());
    }
    if s.duckdb.path.is_empty() {
        errors.push("storage.duckdb.path must not be empty".to_string());
    }
    if s.kuzu.sync_interval_seconds == 0 {
        errors.push("storage.kuzu.sync_interval_seconds must be >= 1".to_string());
    }
    if s.rocksdb.cache_size_mb == 0 {
        errors.push("storage.rocksdb.cache_size_mb must be >= 1".to_string());
    }
}

fn validate_retention(r: &RetentionPolicy, errors: &mut Vec<String>) {
    if r.events_ttl_days == 0 {
        errors.push("retention.events_ttl_days must be >= 1".to_string());
    }
    if r.delete_batch_size == 0 {
        errors.push("retention.delete_batch_size must be >= 1".to_string());
    }
}

fn validate_security(s: &SecurityConfig, errors: &mut Vec<String>) {
    if !VALID_SIGNING_ALGOS.contains(&s.signing.algorithm.as_str()) {
        errors.push(format!(
            "security.signing.algorithm '{}' is not supported; must be one of {:?}",
            s.signing.algorithm, VALID_SIGNING_ALGOS
        ));
    }
    if s.oidc.enabled {
        if s.oidc.provider_url.is_none() {
            errors.push(
                "security.oidc.provider_url must be set when oidc.enabled = true".to_string(),
            );
        }
        if s.oidc.client_id.is_none() {
            errors.push(
                "security.oidc.client_id must be set when oidc.enabled = true".to_string(),
            );
        }
    }
}

fn validate_connectors(connectors: &[ConnectorDef], errors: &mut Vec<String>) {
    let mut names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (i, c) in connectors.iter().enumerate() {
        if c.name.is_empty() {
            errors.push(format!("connectors[{i}].name must not be empty"));
        } else if !names.insert(c.name.as_str()) {
            errors.push(format!(
                "connectors[{i}].name '{}' is duplicated",
                c.name
            ));
        }
        if c.connector_type.is_empty() {
            errors.push(format!("connectors[{i}] ('{}') type must not be empty", c.name));
        }
        if c.entity_type.is_empty() {
            errors.push(format!(
                "connectors[{i}] ('{}') entity_type must not be empty",
                c.name
            ));
        }
        if !(0.0..=1.0).contains(&c.trust_score) {
            errors.push(format!(
                "connectors[{i}] ('{}') trust_score {} is out of range [0.0, 1.0]",
                c.name, c.trust_score
            ));
        }
    }
}

fn validate_entity_resolution(er: &EntityResolutionConfig, errors: &mut Vec<String>) {
    if !VALID_RESOLUTION_PHASES.contains(&er.phase.as_str()) {
        errors.push(format!(
            "entity_resolution.phase '{}' is invalid; must be one of {:?}",
            er.phase, VALID_RESOLUTION_PHASES
        ));
    }
    if er.probabilistic.enabled && er.probabilistic.model_path.is_none() {
        errors.push(
            "entity_resolution.probabilistic.model_path must be set when probabilistic is enabled"
                .to_string(),
        );
    }
    if er.probabilistic.confidence_threshold < 0.0
        || er.probabilistic.confidence_threshold > 1.0
    {
        errors.push(format!(
            "entity_resolution.probabilistic.confidence_threshold {} is out of range [0.0, 1.0]",
            er.probabilistic.confidence_threshold
        ));
    }
}

fn validate_monitors(monitors: &[MonitorRule], errors: &mut Vec<String>) {
    let mut ids: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (i, m) in monitors.iter().enumerate() {
        if m.rule_id.is_empty() {
            errors.push(format!("monitors[{i}].rule_id must not be empty"));
        } else if !ids.insert(m.rule_id.as_str()) {
            errors.push(format!(
                "monitors[{i}].rule_id '{}' is duplicated",
                m.rule_id
            ));
        }
        if m.name.is_empty() {
            errors.push(format!("monitors[{i}].name must not be empty"));
        }
        if m.condition.is_empty() {
            errors.push(format!(
                "monitors[{i}] ('{}') condition must not be empty",
                m.name
            ));
        }
    }
}

fn validate_api(api: &ApiConfig, errors: &mut Vec<String>) {
    if api.rate_limit_per_minute == 0 {
        errors.push("api.rate_limit_per_minute must be >= 1".to_string());
    }
}

fn validate_logging(l: &LoggingConfig, errors: &mut Vec<String>) {
    if !VALID_LOG_LEVELS.contains(&l.level.as_str()) {
        errors.push(format!(
            "logging.level '{}' is invalid; must be one of {:?}",
            l.level, VALID_LOG_LEVELS
        ));
    }
    if !VALID_LOG_FORMATS.contains(&l.format.as_str()) {
        errors.push(format!(
            "logging.format '{}' is invalid; must be one of {:?}",
            l.format, VALID_LOG_FORMATS
        ));
    }
}

fn validate_templates(templates: &[TemplateConfig], errors: &mut Vec<String>) {
    let mut names: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for (i, t) in templates.iter().enumerate() {
        if t.name.is_empty() {
            errors.push(format!("templates[{i}].name must not be empty"));
        } else if !names.insert(t.name.as_str()) {
            errors.push(format!("templates[{i}].name '{}' is duplicated", t.name));
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Config;

    #[test]
    fn test_valid_default_config() {
        assert!(validate_config(&Config::default()).is_ok());
    }

    #[test]
    fn test_invalid_port_zero() {
        let mut config = Config::default();
        config.server.port = 0;
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("port")));
    }

    #[test]
    fn test_invalid_log_level() {
        let mut config = Config::default();
        config.server.log_level = "verbose".to_string();
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("log_level")));
    }

    #[test]
    fn test_connector_duplicate_name() {
        let config = Config {
            connectors: vec![
                crate::schema::ConnectorDef {
                    name: "ais".to_string(),
                    connector_type: "ais".to_string(),
                    enabled: true,
                    url: None,
                    entity_type: "ship".to_string(),
                    trust_score: 0.9,
                    schedule: None,
                    headers: Default::default(),
                    retry_policy: None,
                    mapping: Default::default(),
                },
                crate::schema::ConnectorDef {
                    name: "ais".to_string(), // duplicate!
                    connector_type: "ais".to_string(),
                    enabled: true,
                    url: None,
                    entity_type: "ship".to_string(),
                    trust_score: 0.8,
                    schedule: None,
                    headers: Default::default(),
                    retry_policy: None,
                    mapping: Default::default(),
                },
            ],
            ..Config::default()
        };
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("duplicated")));
    }

    #[test]
    fn test_connector_invalid_trust_score() {
        let config = Config {
            connectors: vec![crate::schema::ConnectorDef {
                name: "bad".to_string(),
                connector_type: "http".to_string(),
                enabled: true,
                url: None,
                entity_type: "ship".to_string(),
                trust_score: 1.5, // out of range
                schedule: None,
                headers: Default::default(),
                retry_policy: None,
                mapping: Default::default(),
            }],
            ..Config::default()
        };
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("trust_score")));
    }

    #[test]
    fn test_oidc_missing_url_when_enabled() {
        let mut config = Config::default();
        config.security.oidc.enabled = true;
        config.security.oidc.client_id = Some("my-client".to_string());
        // provider_url not set
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("provider_url")));
    }

    #[test]
    fn test_retention_zero_ttl() {
        let mut config = Config::default();
        config.retention.events_ttl_days = 0;
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("events_ttl_days")));
    }

    #[test]
    fn test_telemetry_requires_endpoint() {
        let mut config = Config::default();
        config.server.telemetry_enabled = true;
        config.server.telemetry_endpoint = None;
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("telemetry_endpoint")));
    }

    #[test]
    fn test_invalid_signing_algorithm() {
        let mut config = Config::default();
        config.security.signing.algorithm = "RSA".to_string();
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("signing.algorithm")));
    }

    #[test]
    fn test_duckdb_zero_memory() {
        let mut config = Config::default();
        config.storage.duckdb.memory_limit_gb = 0;
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("memory_limit_gb")));
    }

    #[test]
    fn test_duckdb_zero_connections() {
        let mut config = Config::default();
        config.storage.duckdb.max_connections = 0;
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("max_connections")));
    }

    #[test]
    fn test_empty_duckdb_path() {
        let mut config = Config::default();
        config.storage.duckdb.path = "".to_string();
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("duckdb.path")));
    }

    #[test]
    fn test_kuzu_zero_sync_interval() {
        let mut config = Config::default();
        config.storage.kuzu.sync_interval_seconds = 0;
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("sync_interval_seconds")));
    }

    #[test]
    fn test_rocksdb_zero_cache() {
        let mut config = Config::default();
        config.storage.rocksdb.cache_size_mb = 0;
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("cache_size_mb")));
    }

    #[test]
    fn test_zero_rate_limit() {
        let mut config = Config::default();
        config.api.rate_limit_per_minute = 0;
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("rate_limit_per_minute")));
    }

    #[test]
    fn test_invalid_logging_format() {
        let mut config = Config::default();
        config.logging.format = "xml".to_string();
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("logging.format")));
    }

    #[test]
    fn test_invalid_resolution_phase() {
        let mut config = Config::default();
        config.entity_resolution.phase = "quantum".to_string();
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("entity_resolution.phase")));
    }

    #[test]
    fn test_probabilistic_requires_model_path() {
        let mut config = Config::default();
        config.entity_resolution.probabilistic.enabled = true;
        config.entity_resolution.probabilistic.model_path = None;
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("model_path")));
    }

    #[test]
    fn test_invalid_confidence_threshold() {
        let mut config = Config::default();
        config.entity_resolution.probabilistic.confidence_threshold = 1.5;
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("confidence_threshold")));
    }

    #[test]
    fn test_connector_empty_type() {
        let config = Config {
            connectors: vec![crate::schema::ConnectorDef {
                name: "test".to_string(),
                connector_type: "".to_string(),
                enabled: true,
                url: None,
                entity_type: "ship".to_string(),
                trust_score: 0.9,
                schedule: None,
                headers: Default::default(),
                retry_policy: None,
                mapping: Default::default(),
            }],
            ..Config::default()
        };
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("type must not be empty")));
    }

    #[test]
    fn test_connector_empty_entity_type() {
        let config = Config {
            connectors: vec![crate::schema::ConnectorDef {
                name: "test".to_string(),
                connector_type: "ais".to_string(),
                enabled: true,
                url: None,
                entity_type: "".to_string(),
                trust_score: 0.9,
                schedule: None,
                headers: Default::default(),
                retry_policy: None,
                mapping: Default::default(),
            }],
            ..Config::default()
        };
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("entity_type must not be empty")));
    }

    #[test]
    fn test_zero_workers() {
        let mut config = Config::default();
        config.server.workers = 0;
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("workers")));
    }

    #[test]
    fn test_zero_delete_batch_size() {
        let mut config = Config::default();
        config.retention.delete_batch_size = 0;
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("delete_batch_size")));
    }

    #[test]
    fn test_oidc_missing_client_id() {
        let mut config = Config::default();
        config.security.oidc.enabled = true;
        config.security.oidc.provider_url = Some("https://auth.example.com".to_string());
        config.security.oidc.client_id = None;
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("client_id")));
    }

    #[test]
    fn test_negative_trust_score() {
        let config = Config {
            connectors: vec![crate::schema::ConnectorDef {
                name: "bad".to_string(),
                connector_type: "http".to_string(),
                enabled: true,
                url: None,
                entity_type: "ship".to_string(),
                trust_score: -0.1,
                schedule: None,
                headers: Default::default(),
                retry_policy: None,
                mapping: Default::default(),
            }],
            ..Config::default()
        };
        let errs = validate_config(&config).unwrap_err();
        assert!(errs.iter().any(|e| e.contains("trust_score")));
    }
}
