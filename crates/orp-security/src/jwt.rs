//! JWT token creation and validation.
//!
//! Supports HS256 (HMAC-SHA256) and RS256 (RSA-SHA256) signing algorithms.
//! Token format matches the ORP spec (Section 1.2 & 6).

use chrono::Utc;
use jsonwebtoken::{
    decode, encode, Algorithm, DecodingKey, EncodingKey, Header, TokenData, Validation,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// JWT claim errors.
#[derive(Debug, Error)]
pub enum JwtError {
    #[error("Token expired")]
    TokenExpired,
    #[error("Token invalid: {0}")]
    Invalid(String),
    #[error("Token encoding failed: {0}")]
    EncodingFailed(String),
    #[error("Configuration error: {0}")]
    Config(String),
}

impl From<jsonwebtoken::errors::Error> for JwtError {
    fn from(e: jsonwebtoken::errors::Error) -> Self {
        use jsonwebtoken::errors::ErrorKind;
        match e.kind() {
            ErrorKind::ExpiredSignature => JwtError::TokenExpired,
            _ => JwtError::Invalid(e.to_string()),
        }
    }
}

/// The full JWT claims payload matching the ORP spec.
///
/// ```json
/// {
///   "sub": "user-id-12345",
///   "email": "alice@company.com",
///   "name": "Alice Chen",
///   "iat": 1711411200,
///   "exp": 1711414800,
///   "iss": "http://localhost:9090/auth",
///   "aud": "orp-client",
///   "scope": "api:read api:write entities:read",
///   "org_id": "org-456",
///   "permissions": ["entities:read", "entities:write"]
/// }
/// ```
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Claims {
    /// Subject — user ID
    pub sub: String,
    /// User email address
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Display name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Issued-at (Unix timestamp)
    pub iat: i64,
    /// Expiration (Unix timestamp)
    pub exp: i64,
    /// Issuer
    pub iss: String,
    /// Audience
    pub aud: String,
    /// Space-separated scope string
    pub scope: String,
    /// Organization ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub org_id: Option<String>,
    /// Fine-grained permission list
    pub permissions: Vec<String>,
}

impl Claims {
    /// Returns true if the claims are not yet expired.
    pub fn is_valid(&self) -> bool {
        self.exp > Utc::now().timestamp()
    }

    /// Returns true if the claims contain the given permission string.
    pub fn has_permission(&self, perm: &str) -> bool {
        self.permissions.iter().any(|p| p == perm)
            || self.permissions.iter().any(|p| p == "admin")
    }

    /// Returns the list of scopes parsed from the scope string.
    pub fn scopes(&self) -> Vec<&str> {
        self.scope.split_whitespace().collect()
    }
}

/// Signing algorithm selector.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum JwtAlgorithm {
    HS256,
    RS256,
}

/// Configuration for JWT signing / verification.
#[derive(Clone, Debug)]
pub struct JwtConfig {
    pub algorithm: JwtAlgorithm,
    /// HS256: the HMAC secret bytes
    pub hs256_secret: Option<Vec<u8>>,
    /// RS256: PEM-encoded RSA private key (for signing)
    pub rs256_private_key_pem: Option<String>,
    /// RS256: PEM-encoded RSA public key (for verification)
    pub rs256_public_key_pem: Option<String>,
    /// Token issuer claim
    pub issuer: String,
    /// Token audience claim
    pub audience: String,
    /// Access token lifetime in seconds
    pub expiration_seconds: i64,
    /// Refresh token lifetime in seconds
    pub refresh_expiration_seconds: i64,
}

impl Default for JwtConfig {
    fn default() -> Self {
        let hs256_secret = std::env::var("JWT_SECRET").ok().map(|s| s.into_bytes());
        Self {
            algorithm: JwtAlgorithm::HS256,
            hs256_secret,
            rs256_private_key_pem: None,
            rs256_public_key_pem: None,
            issuer: "http://localhost:9090/auth".to_string(),
            audience: "orp-client".to_string(),
            expiration_seconds: 3600,
            refresh_expiration_seconds: 86400,
        }
    }
}

/// Stateless JWT service — creates and validates tokens.
#[derive(Clone, Debug)]
pub struct JwtService {
    config: JwtConfig,
}

impl JwtService {
    pub fn new(config: JwtConfig) -> Result<Self, JwtError> {
        // Validate config
        match config.algorithm {
            JwtAlgorithm::HS256 => {
                if config.hs256_secret.is_none() {
                    return Err(JwtError::Config("HS256 secret is required".into()));
                }
            }
            JwtAlgorithm::RS256 => {
                if config.rs256_private_key_pem.is_none()
                    && config.rs256_public_key_pem.is_none()
                {
                    return Err(JwtError::Config(
                        "RS256 requires at least one of private or public key".into(),
                    ));
                }
            }
        }
        Ok(Self { config })
    }

    /// Create a new JwtService with HS256 config from JWT_SECRET env var.
    ///
    /// Panics if JWT_SECRET is not set. For tests, use `JwtService::new()` with
    /// an explicit config.
    pub fn from_env() -> Result<Self, JwtError> {
        let config = JwtConfig::default();
        Self::new(config)
    }

    /// Issue an access token for the given subject + attributes.
    pub fn issue_token(
        &self,
        sub: &str,
        email: Option<&str>,
        name: Option<&str>,
        org_id: Option<&str>,
        permissions: Vec<String>,
    ) -> Result<String, JwtError> {
        let now = Utc::now().timestamp();
        let scope = permissions.join(" ");

        let claims = Claims {
            sub: sub.to_string(),
            email: email.map(|s| s.to_string()),
            name: name.map(|s| s.to_string()),
            iat: now,
            exp: now + self.config.expiration_seconds,
            iss: self.config.issuer.clone(),
            aud: self.config.audience.clone(),
            scope,
            org_id: org_id.map(|s| s.to_string()),
            permissions,
        };

        self.encode_claims(&claims)
    }

    /// Issue a refresh token (longer-lived, minimal claims).
    pub fn issue_refresh_token(&self, sub: &str) -> Result<String, JwtError> {
        let now = Utc::now().timestamp();
        let claims = Claims {
            sub: sub.to_string(),
            email: None,
            name: None,
            iat: now,
            exp: now + self.config.refresh_expiration_seconds,
            iss: self.config.issuer.clone(),
            aud: format!("{}-refresh", self.config.audience),
            scope: "refresh".to_string(),
            org_id: None,
            permissions: vec!["refresh".to_string()],
        };
        self.encode_claims(&claims)
    }

    /// Encode claims to a JWT string.
    fn encode_claims(&self, claims: &Claims) -> Result<String, JwtError> {
        let header = Header::new(self.algorithm());
        let key = self.encoding_key()?;
        encode(&header, claims, &key).map_err(|e| JwtError::EncodingFailed(e.to_string()))
    }

    /// Validate a token string and return the decoded claims.
    pub fn validate_token(&self, token: &str) -> Result<Claims, JwtError> {
        let key = self.decoding_key()?;
        let mut validation = Validation::new(self.algorithm());
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&[&self.config.audience]);

        let data: TokenData<Claims> = decode(token, &key, &validation)?;
        Ok(data.claims)
    }

    /// Validate a refresh token — checks signature and issuer, uses refresh audience.
    pub fn validate_refresh_token(&self, token: &str) -> Result<Claims, JwtError> {
        let key = self.decoding_key()?;
        let mut validation = Validation::new(self.algorithm());
        validation.set_issuer(&[&self.config.issuer]);
        validation.set_audience(&[format!("{}-refresh", self.config.audience)]);
        let data: TokenData<Claims> = decode(token, &key, &validation)?;
        Ok(data.claims)
    }

    fn algorithm(&self) -> Algorithm {
        match self.config.algorithm {
            JwtAlgorithm::HS256 => Algorithm::HS256,
            JwtAlgorithm::RS256 => Algorithm::RS256,
        }
    }

    fn encoding_key(&self) -> Result<EncodingKey, JwtError> {
        match self.config.algorithm {
            JwtAlgorithm::HS256 => {
                let secret = self
                    .config
                    .hs256_secret
                    .as_ref()
                    .ok_or_else(|| JwtError::Config("HS256 secret missing".into()))?;
                Ok(EncodingKey::from_secret(secret))
            }
            JwtAlgorithm::RS256 => {
                let pem = self
                    .config
                    .rs256_private_key_pem
                    .as_deref()
                    .ok_or_else(|| JwtError::Config("RS256 private key missing".into()))?;
                EncodingKey::from_rsa_pem(pem.as_bytes())
                    .map_err(|e| JwtError::Config(format!("Invalid RSA private key: {e}")))
            }
        }
    }

    fn decoding_key(&self) -> Result<DecodingKey, JwtError> {
        match self.config.algorithm {
            JwtAlgorithm::HS256 => {
                let secret = self
                    .config
                    .hs256_secret
                    .as_ref()
                    .ok_or_else(|| JwtError::Config("HS256 secret missing".into()))?;
                Ok(DecodingKey::from_secret(secret))
            }
            JwtAlgorithm::RS256 => {
                // Prefer public key for verification; fall back to private key
                if let Some(pub_pem) = &self.config.rs256_public_key_pem {
                    DecodingKey::from_rsa_pem(pub_pem.as_bytes())
                        .map_err(|e| JwtError::Config(format!("Invalid RSA public key: {e}")))
                } else if let Some(priv_pem) = &self.config.rs256_private_key_pem {
                    DecodingKey::from_rsa_pem(priv_pem.as_bytes())
                        .map_err(|e| JwtError::Config(format!("Invalid RSA key: {e}")))
                } else {
                    Err(JwtError::Config("No RS256 key available for decoding".into()))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn default_service() -> JwtService {
        JwtService::new(JwtConfig {
            hs256_secret: Some(b"test-secret-for-unit-tests-32bytes!".to_vec()),
            ..JwtConfig::default()
        })
        .unwrap()
    }

    #[test]
    fn test_issue_and_validate_token() {
        let svc = default_service();
        let token = svc
            .issue_token(
                "user-123",
                Some("alice@example.com"),
                Some("Alice"),
                Some("org-1"),
                vec!["entities:read".into(), "graph:read".into()],
            )
            .unwrap();

        assert!(!token.is_empty());
        let claims = svc.validate_token(&token).unwrap();
        assert_eq!(claims.sub, "user-123");
        assert_eq!(claims.email.as_deref(), Some("alice@example.com"));
        assert_eq!(claims.name.as_deref(), Some("Alice"));
        assert_eq!(claims.org_id.as_deref(), Some("org-1"));
        assert!(claims.permissions.contains(&"entities:read".to_string()));
        assert!(claims.permissions.contains(&"graph:read".to_string()));
        assert!(claims.is_valid());
    }

    #[test]
    fn test_has_permission() {
        let svc = default_service();
        let token = svc
            .issue_token(
                "user-123",
                None,
                None,
                None,
                vec!["entities:read".into()],
            )
            .unwrap();
        let claims = svc.validate_token(&token).unwrap();
        assert!(claims.has_permission("entities:read"));
        assert!(!claims.has_permission("entities:write"));
    }

    #[test]
    fn test_admin_has_all_permissions() {
        let svc = default_service();
        let token = svc
            .issue_token("admin-1", None, None, None, vec!["admin".into()])
            .unwrap();
        let claims = svc.validate_token(&token).unwrap();
        // admin is treated as wildcard
        assert!(claims.has_permission("entities:read"));
        assert!(claims.has_permission("entities:write"));
        assert!(claims.has_permission("graph:delete"));
    }

    #[test]
    fn test_invalid_token_rejected() {
        let svc = default_service();
        let result = svc.validate_token("not.a.jwt");
        assert!(result.is_err());
    }

    #[test]
    fn test_wrong_secret_rejected() {
        let svc1 = default_service();
        let token = svc1
            .issue_token("user-1", None, None, None, vec![])
            .unwrap();

        let cfg2 = JwtConfig {
            hs256_secret: Some(b"completely-different-secret-xyz!".to_vec()),
            ..JwtConfig::default()
        };
        let svc2 = JwtService::new(cfg2).unwrap();
        let result = svc2.validate_token(&token);
        assert!(result.is_err());
    }

    #[test]
    fn test_refresh_token_issued() {
        let svc = default_service();
        let token = svc.issue_refresh_token("user-1").unwrap();
        assert!(!token.is_empty());
    }

    #[test]
    fn test_scopes_parsed() {
        let svc = default_service();
        let token = svc
            .issue_token(
                "u1",
                None,
                None,
                None,
                vec!["entities:read".into(), "graph:read".into()],
            )
            .unwrap();
        let claims = svc.validate_token(&token).unwrap();
        let scopes = claims.scopes();
        assert!(scopes.contains(&"entities:read"));
        assert!(scopes.contains(&"graph:read"));
    }

    #[test]
    fn test_missing_secret_returns_error() {
        let cfg = JwtConfig {
            hs256_secret: None,
            ..JwtConfig::default()
        };
        let result = JwtService::new(cfg);
        assert!(result.is_err());
    }
}
