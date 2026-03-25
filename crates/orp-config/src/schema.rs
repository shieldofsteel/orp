use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Validation errors: {0:?}")]
    ValidationError(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub storage: StorageConfig,
    pub security: SecurityConfig,
    pub connectors: Vec<ConnectorDef>,
    pub monitors: Vec<MonitorRule>,
    pub api: ApiConfig,
    pub frontend: FrontendConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub workers: u32,
    pub log_level: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub duckdb: DuckDbConfig,
    pub rocksdb: RocksDbConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuckDbConfig {
    pub path: String,
    pub memory_limit_gb: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RocksDbConfig {
    pub path: String,
    pub cache_size_mb: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub signing_algorithm: String,
    pub oidc_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorDef {
    pub name: String,
    pub connector_type: String,
    pub enabled: bool,
    pub url: Option<String>,
    pub entity_type: String,
    pub trust_score: f32,
    pub schedule: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorRule {
    pub rule_id: String,
    pub name: String,
    pub entity_type: String,
    pub condition: String,
    pub action: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    pub rate_limit_per_minute: u32,
    pub cors_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrontendConfig {
    pub enabled: bool,
    pub port: u16,
    pub default_map_center: [f64; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub level: String,
    pub format: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig::default(),
            storage: StorageConfig::default(),
            security: SecurityConfig::default(),
            connectors: Vec::new(),
            monitors: Vec::new(),
            api: ApiConfig::default(),
            frontend: FrontendConfig::default(),
            logging: LoggingConfig::default(),
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 9090,
            workers: 4,
            log_level: "info".to_string(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            duckdb: DuckDbConfig {
                path: "./data/orp.duckdb".to_string(),
                memory_limit_gb: 4,
            },
            rocksdb: RocksDbConfig {
                path: "./data/state.db".to_string(),
                cache_size_mb: 512,
            },
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            signing_algorithm: "Ed25519".to_string(),
            oidc_enabled: false,
        }
    }
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            rate_limit_per_minute: 1000,
            cors_enabled: true,
        }
    }
}

impl Default for FrontendConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            port: 9090,
            default_map_center: [51.92, 4.27],
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            format: "json".to_string(),
        }
    }
}

impl Config {
    pub fn validate(&self) -> Result<(), Vec<String>> {
        let mut errors = Vec::new();

        if self.server.port == 0 {
            errors.push("Server port must be > 0".to_string());
        }

        if self.storage.duckdb.memory_limit_gb == 0 {
            errors.push("DuckDB memory limit must be >= 1GB".to_string());
        }

        for conn in &self.connectors {
            if conn.name.is_empty() {
                errors.push("Connector name cannot be empty".to_string());
            }
            if conn.trust_score < 0.0 || conn.trust_score > 1.0 {
                errors.push(format!(
                    "Connector '{}' trust_score must be 0.0-1.0",
                    conn.name
                ));
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    pub fn load_from_file(path: &str) -> Result<Self, ConfigError> {
        let content = std::fs::read_to_string(path)?;
        let config: Config =
            serde_yaml::from_str(&content).map_err(|e| ConfigError::ParseError(e.to_string()))?;
        config
            .validate()
            .map_err(ConfigError::ValidationError)?;
        Ok(config)
    }

    pub fn load_or_default() -> Self {
        match Self::load_from_file("config.yaml") {
            Ok(config) => config,
            Err(_) => {
                tracing::info!("No config.yaml found, using default configuration");
                Self::default()
            }
        }
    }
}
