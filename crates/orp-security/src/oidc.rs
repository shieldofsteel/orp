use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    #[error("Token expired")]
    TokenExpired,
    #[error("Token invalid: {0}")]
    TokenInvalid(String),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Forbidden: {0}")]
    Forbidden(String),
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[derive(Default)]
pub struct OidcConfig {
    pub enabled: bool,
    pub provider_url: String,
    pub client_id: String,
}


#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Claims {
    pub sub: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub permissions: Vec<String>,
    pub exp: i64,
}

pub struct OidcClient {
    config: OidcConfig,
}

impl OidcClient {
    pub fn new(config: OidcConfig) -> Self {
        Self { config }
    }

    /// Validate a bearer token. Stub implementation:
    /// - If OIDC is disabled, returns default claims (permissive mode)
    /// - If OIDC is enabled, returns an error (no real provider configured)
    pub fn validate_token(&self, _token: &str) -> Result<Claims, SecurityError> {
        if !self.config.enabled {
            // Permissive mode - return default admin claims
            Ok(Claims {
                sub: "anonymous".to_string(),
                email: None,
                name: Some("Anonymous User".to_string()),
                permissions: vec![
                    "entities:read".to_string(),
                    "entities:write".to_string(),
                    "graph:read".to_string(),
                    "monitors:read".to_string(),
                    "monitors:write".to_string(),
                    "query:execute".to_string(),
                    "admin".to_string(),
                ],
                exp: chrono::Utc::now().timestamp() + 3600,
            })
        } else {
            Err(SecurityError::TokenInvalid(
                "OIDC provider not configured".to_string(),
            ))
        }
    }
}
