//! Configuration schema (BUILD_CORE_ENGINE.md §6).
//!
//! Supports environment variable substitution using `${env.VAR}` syntax in
//! string values. Call [`Config::resolve_env_vars`] before validating.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub use crate::error::ConfigError;

/// Top-level ORP configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub retention: RetentionPolicy,
    pub security: SecurityConfig,
    #[serde(default)]
    pub connectors: Vec<ConnectorDef>,
    pub entity_resolution: EntityResolutionConfig,
    #[serde(default)]
    pub monitors: Vec<MonitorRule>,
    pub api: ApiConfig,
    pub frontend: FrontendConfig,
    pub logging: LoggingConfig,
    #[serde(default)]
    pub templates: Vec<TemplateConfig>,
}

// ── Server ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub workers: u32,
    pub log_level: String,
    pub telemetry_enabled: bool,
    pub telemetry_endpoint: Option<String>,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 9090,
            workers: 4,
            log_level: "info".to_string(),
            telemetry_enabled: false,
            telemetry_endpoint: None,
        }
    }
}

// ── Storage ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StorageConfig {
    pub duckdb: DuckDbConfig,
    pub kuzu: KuzuConfig,
    pub rocksdb: RocksDbConfig,
    pub sqlite: SqliteConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuckDbConfig {
    pub path: String,
    pub memory_limit_gb: u32,
    pub max_connections: u32,
}

impl Default for DuckDbConfig {
    fn default() -> Self {
        Self {
            path: "./data.duckdb".to_string(),
            memory_limit_gb: 4,
            max_connections: 10,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KuzuConfig {
    pub path: String,
    pub memory_limit_gb: u32,
    pub sync_interval_seconds: u64,
}

impl Default for KuzuConfig {
    fn default() -> Self {
        Self {
            path: "./data.kuzu".to_string(),
            memory_limit_gb: 2,
            sync_interval_seconds: 30,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RocksDbConfig {
    pub path: String,
    pub cache_size_mb: u32,
}

impl Default for RocksDbConfig {
    fn default() -> Self {
        Self {
            path: "./state.db".to_string(),
            cache_size_mb: 512,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SqliteConfig {
    pub path: String,
}

impl Default for SqliteConfig {
    fn default() -> Self {
        Self {
            path: "./config.sqlite".to_string(),
        }
    }
}

// ── Retention ─────────────────────────────────────────────────────────────────

/// Data retention policy (BUILD_CORE_ENGINE.md §6.1 `retention` block).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetentionPolicy {
    /// How long to keep raw events (days).
    pub events_ttl_days: u32,
    /// How long to keep periodic snapshots (days).
    pub snapshots_ttl_days: u32,
    /// How long to keep audit log entries (days). Set to 0 to disable deletion.
    pub audit_log_ttl_days: u32,
    /// Number of rows to delete per sweep (controls I/O pressure).
    pub delete_batch_size: u32,
}

impl Default for RetentionPolicy {
    fn default() -> Self {
        Self {
            events_ttl_days: 90,
            snapshots_ttl_days: 30,
            audit_log_ttl_days: 365,
            delete_batch_size: 10_000,
        }
    }
}

// ── Security ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub oidc: OidcConfig,
    pub abac: AbacConfig,
    pub signing: SigningConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcConfig {
    pub enabled: bool,
    pub provider_url: Option<String>,
    pub client_id: Option<String>,
    /// Use `${env.ORP_OIDC_CLIENT_SECRET}` to reference an env var.
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    pub redirect_uri: Option<String>,
}

impl Default for OidcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider_url: None,
            client_id: None,
            client_secret: None,
            scopes: vec!["openid".to_string(), "profile".to_string()],
            redirect_uri: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AbacConfig {
    pub enabled: bool,
    pub policy_file: Option<String>,
}

impl Default for AbacConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            policy_file: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SigningConfig {
    pub algorithm: String,
    pub private_key_path: Option<String>,
}

impl Default for SigningConfig {
    fn default() -> Self {
        Self {
            algorithm: "Ed25519".to_string(),
            private_key_path: None,
        }
    }
}

// ── Connectors ────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorDef {
    pub name: String,
    #[serde(rename = "type")]
    pub connector_type: String,
    pub enabled: bool,
    pub url: Option<String>,
    pub entity_type: String,
    pub trust_score: f64,
    pub schedule: Option<String>,
    #[serde(default)]
    pub headers: HashMap<String, String>,
    pub retry_policy: Option<RetryPolicyConfig>,
    #[serde(default)]
    pub mapping: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetryPolicyConfig {
    pub max_retries: u32,
    pub backoff_ms: u64,
}

// ── Entity resolution ─────────────────────────────────────────────────────────

/// Entity resolution configuration (BUILD_CORE_ENGINE.md §6.1 `entity_resolution` block).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityResolutionConfig {
    pub enabled: bool,
    /// `"structural"` (Phase 1) or `"probabilistic"` (Phase 2).
    pub phase: String,
    pub structural: StructuralResolutionConfig,
    pub probabilistic: ProbabilisticResolutionConfig,
}

impl Default for EntityResolutionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            phase: "structural".to_string(),
            structural: StructuralResolutionConfig::default(),
            probabilistic: ProbabilisticResolutionConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuralResolutionConfig {
    /// Field names that uniquely identify an entity (e.g., `["mmsi", "icao_hex"]`).
    pub fields: Vec<String>,
}

impl Default for StructuralResolutionConfig {
    fn default() -> Self {
        Self {
            fields: vec!["mmsi".to_string(), "icao_hex".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbabilisticResolutionConfig {
    pub enabled: bool,
    pub model_path: Option<String>,
    pub confidence_threshold: f64,
}

impl Default for ProbabilisticResolutionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            model_path: None,
            confidence_threshold: 0.85,
        }
    }
}

// ── Monitors ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorRule {
    pub rule_id: String,
    pub name: String,
    pub entity_type: String,
    pub condition: String,
    pub action: String,
    pub action_target: Option<String>,
    pub enabled: bool,
}

// ── API ───────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub rate_limit_per_minute: u32,
    pub cors_enabled: bool,
    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,
    pub api_key_header: String,
    /// Use `${env.JWT_SECRET}` to reference an env var.
    pub jwt_secret: Option<String>,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            rate_limit_per_minute: 1000,
            cors_enabled: true,
            cors_allowed_origins: vec!["*".to_string()],
            api_key_header: "X-API-Key".to_string(),
            jwt_secret: None,
        }
    }
}

// ── Frontend ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontendConfig {
    pub enabled: bool,
    pub port: u16,
    pub assets_path: String,
    /// `[lat, lon]` for the default map centre.
    pub default_map_center: [f64; 2],
    pub default_zoom: u8,
}

impl Default for FrontendConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 9090,
            assets_path: "./frontend/dist".to_string(),
            default_map_center: [51.92, 4.27], // Rotterdam
            default_zoom: 8,
        }
    }
}

// ── Logging ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
    pub format: String,
    pub output: String,
    pub audit_log_path: String,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: "json".to_string(),
            output: "stdout".to_string(),
            audit_log_path: "./audit.log".to_string(),
        }
    }
}

// ── Templates ─────────────────────────────────────────────────────────────────

/// Pre-configured setup templates (BUILD_CORE_ENGINE.md §6.1 `templates` block).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemplateConfig {
    pub name: String,
    pub description: String,
    pub connectors: Vec<String>,
    pub sample_data_ttl_hours: u32,
}

// ── Config impl ───────────────────────────────────────────────────────────────



impl Config {
    /// Load from a YAML file, apply env var substitution, then validate.
    pub fn load_from_file(path: &str) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let content = Self::substitute_env_vars(&content);
        let config: Config = serde_yaml::from_str(&content)
            .map_err(|e| ConfigError::ParseError(e.to_string()))?;
        config.validate().map_err(ConfigError::ValidationError)?;
        Ok(config)
    }

    /// Load from a YAML string directly (useful for testing).
    pub fn from_yaml(yaml: &str) -> Result<Self, ConfigError> {
        let yaml = Self::substitute_env_vars(yaml);
        let config: Config = serde_yaml::from_str(&yaml)
            .map_err(|e| ConfigError::ParseError(e.to_string()))?;
        Ok(config)
    }

    /// Load from disk or fall back to defaults.
    pub fn load_or_default() -> Self {
        Self::load_from_file("config.yaml").unwrap_or_default()
    }

    /// Validate the configuration, returning a list of all errors found.
    ///
    /// Delegates the heavy lifting to [`crate::validation`].
    pub fn validate(&self) -> Result<(), Vec<String>> {
        crate::validation::validate_config(self)
    }

    /// Replace `${env.VAR_NAME}` placeholders with actual environment variable values.
    ///
    /// If the variable is not set, the placeholder is replaced with an empty string
    /// and a warning is logged.
    pub fn substitute_env_vars(input: &str) -> String {
        // Pattern: ${env.SOME_VAR}
        let mut result = input.to_string();
        let search_start = 0;

        while let Some(start) = result[search_start..].find("${env.") {
            let abs_start = search_start + start;
            if let Some(end_offset) = result[abs_start..].find('}') {
                let abs_end = abs_start + end_offset;
                let placeholder = &result[abs_start..=abs_end]; // "${env.FOO}"
                let var_name = &result[abs_start + 6..abs_end]; // "FOO"

                let value = std::env::var(var_name).unwrap_or_else(|_| {
                    tracing::warn!(
                        var = var_name,
                        "Environment variable referenced in config not found; substituting empty string"
                    );
                    String::new()
                });

                result = result.replacen(placeholder, &value, 1);
                // Don't advance past the substituted text — a value could itself
                // contain `${env.` (unlikely but defensive).
            } else {
                break;
            }
        }
        result
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_validates() {
        let config = Config::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_env_var_substitution() {
        std::env::set_var("ORP_TEST_SECRET", "supersecret");
        let input = r#"jwt_secret: "${env.ORP_TEST_SECRET}""#;
        let result = Config::substitute_env_vars(input);
        assert!(result.contains("supersecret"));
        assert!(!result.contains("${env."));
        std::env::remove_var("ORP_TEST_SECRET");
    }

    #[test]
    fn test_env_var_missing_becomes_empty() {
        std::env::remove_var("ORP_DOES_NOT_EXIST_12345");
        let input = "secret: ${env.ORP_DOES_NOT_EXIST_12345}";
        let result = Config::substitute_env_vars(input);
        assert!(result.contains("secret: "));
        assert!(!result.contains("${env."));
    }

    #[test]
    fn test_retention_defaults() {
        let r = RetentionPolicy::default();
        assert_eq!(r.events_ttl_days, 90);
        assert_eq!(r.audit_log_ttl_days, 365);
    }

    #[test]
    fn test_from_yaml_minimal() {
        let yaml = r#"
server:
  host: "127.0.0.1"
  port: 8080
  workers: 2
  log_level: "debug"
  telemetry_enabled: false
"#;
        let config = Config::from_yaml(yaml).expect("parse failed");
        assert_eq!(config.server.port, 8080);
        assert_eq!(config.server.workers, 2);
    }

    #[test]
    fn test_from_yaml_with_connectors() {
        let yaml = r#"
server:
  host: "0.0.0.0"
  port: 9090
  workers: 4
  log_level: "info"
  telemetry_enabled: false
connectors:
  - name: "ais"
    type: "ais"
    enabled: true
    entity_type: "ship"
    trust_score: 0.9
"#;
        let config = Config::from_yaml(yaml).expect("parse failed");
        assert_eq!(config.connectors.len(), 1);
        assert_eq!(config.connectors[0].name, "ais");
    }

    #[test]
    fn test_load_or_default_returns_defaults() {
        // When config.yaml doesn't exist, should use defaults
        let config = Config::load_or_default();
        assert_eq!(config.server.port, 9090);
    }

    #[test]
    fn test_default_storage_config() {
        let config = StorageConfig::default();
        assert_eq!(config.duckdb.memory_limit_gb, 4);
        assert_eq!(config.kuzu.sync_interval_seconds, 30);
    }

    #[test]
    fn test_default_security_config() {
        let config = SecurityConfig::default();
        assert!(!config.oidc.enabled);
        assert!(config.abac.enabled);
        assert_eq!(config.signing.algorithm, "Ed25519");
    }

    #[test]
    fn test_default_frontend_config() {
        let config = FrontendConfig::default();
        assert!(config.enabled);
        assert_eq!(config.port, 9090);
    }

    #[test]
    fn test_from_yaml_invalid_syntax() {
        let yaml = "{{{{invalid yaml}}}";
        assert!(Config::from_yaml(yaml).is_err());
    }

    #[test]
    fn test_env_var_with_default_syntax() {
        // ${env.NONEXISTENT} should be replaced with empty string
        let input = "key: ${env.THIS_VAR_DOES_NOT_EXIST_98765}";
        let result = Config::substitute_env_vars(input);
        assert!(!result.contains("${env."));
    }
}
