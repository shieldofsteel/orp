use crate::schema::*;
use std::env;

/// Returns a maritime-focused configuration template
pub fn get_maritime_template() -> Config {
    Config {
        server: ServerConfig {
            host: "0.0.0.0".to_string(),
            port: 9090,
            workers: 4,
            log_level: "info".to_string(),
            telemetry_enabled: false,
            telemetry_endpoint: None,
        },
        storage: StorageConfig::default(),
        retention: RetentionPolicy::default(),
        security: SecurityConfig::default(),
        connectors: vec![
            ConnectorDef {
                name: "ais_demo".to_string(),
                connector_type: "ais".to_string(),
                enabled: true,
                url: Some("tcp://ais.example.com:5631".to_string()),
                entity_type: "ship".to_string(),
                trust_score: 0.95,
                schedule: None,
                headers: Default::default(),
                retry_policy: None,
                mapping: Default::default(),
            },
            ConnectorDef {
                name: "adsb_demo".to_string(),
                connector_type: "adsb".to_string(),
                enabled: true,
                url: None,
                entity_type: "aircraft".to_string(),
                trust_score: 0.90,
                schedule: None,
                headers: Default::default(),
                retry_policy: None,
                mapping: Default::default(),
            },
        ],
        entity_resolution: EntityResolutionConfig::default(),
        monitors: vec![MonitorRule {
            rule_id: "speed_alert".to_string(),
            name: "High speed alert".to_string(),
            entity_type: "ship".to_string(),
            condition: "speed > 25".to_string(),
            action: "alert".to_string(),
            action_target: None,
            enabled: true,
        }],
        api: ApiConfig::default(),
        frontend: FrontendConfig {
            enabled: true,
            port: 9090,
            assets_path: "./frontend/dist".to_string(),
            default_map_center: [51.92, 4.27], // Rotterdam
            default_zoom: 8,
        },
        logging: LoggingConfig::default(),
        templates: vec![],
    }
}

/// Substitute `${env.VAR_NAME}` patterns in a string with environment variable values.
/// Supports default values: `${env.VAR_NAME:-default_value}`
pub fn substitute_env_vars(input: &str) -> String {
    let mut result = input.to_string();
    let re_pattern = "${env.";

    while let Some(start) = result.find(re_pattern) {
        let after_prefix = start + re_pattern.len();
        if let Some(end) = result[after_prefix..].find('}') {
            let var_expr = &result[after_prefix..after_prefix + end];
            let (var_name, default_value) = if let Some(sep) = var_expr.find(":-") {
                (&var_expr[..sep], Some(&var_expr[sep + 2..]))
            } else {
                (var_expr, None)
            };

            let replacement = env::var(var_name)
                .ok()
                .or_else(|| default_value.map(String::from))
                .unwrap_or_default();

            result = format!(
                "{}{}{}",
                &result[..start],
                replacement,
                &result[after_prefix + end + 1..]
            );
        } else {
            break;
        }
    }

    result
}

/// Substitute environment variables in a YAML config string
pub fn substitute_config_env_vars(yaml_content: &str) -> String {
    yaml_content
        .lines()
        .map(substitute_env_vars)
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_maritime_template() {
        let config = get_maritime_template();
        assert_eq!(config.server.port, 9090);
        assert_eq!(config.connectors.len(), 2);
        assert!(config.connectors.iter().any(|c| c.connector_type == "ais"));
        assert!(config.connectors.iter().any(|c| c.connector_type == "adsb"));
    }

    #[test]
    fn test_env_var_substitution() {
        env::set_var("ORP_TEST_VAR", "hello");
        let result = substitute_env_vars("prefix_${env.ORP_TEST_VAR}_suffix");
        assert_eq!(result, "prefix_hello_suffix");
        env::remove_var("ORP_TEST_VAR");
    }

    #[test]
    fn test_env_var_default() {
        env::remove_var("ORP_NONEXISTENT");
        let result = substitute_env_vars("${env.ORP_NONEXISTENT:-default_val}");
        assert_eq!(result, "default_val");
    }

    #[test]
    fn test_env_var_missing_no_default() {
        env::remove_var("ORP_MISSING");
        let result = substitute_env_vars("${env.ORP_MISSING}");
        assert_eq!(result, "");
    }

    #[test]
    fn test_no_substitution() {
        let result = substitute_env_vars("no variables here");
        assert_eq!(result, "no variables here");
    }
}

