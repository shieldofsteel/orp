//! API key generation, validation, storage, and rate limiting.
//!
//! Keys follow the format: `orpk_prod_<random-hex>` as specified in the ORP API spec.

use chrono::{DateTime, Duration, Utc};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};
use thiserror::Error;

/// Errors from the API key service.
#[derive(Debug, Error)]
pub enum ApiKeyError {
    #[error("Key not found")]
    NotFound,
    #[error("Key revoked")]
    Revoked,
    #[error("Key expired")]
    Expired,
    #[error("Rate limit exceeded — retry after {retry_after_seconds}s")]
    RateLimitExceeded { retry_after_seconds: u64 },
    #[error("Permission denied: required scope '{0}' not present")]
    MissingScope(String),
    #[error("Storage error: {0}")]
    Storage(String),
    #[error("Invalid key format")]
    InvalidFormat,
}

/// An API key record stored in the backend.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApiKeyRecord {
    /// Full key with prefix (e.g. `orpk_prod_abc123...`). Only stored as hash in prod.
    pub id: String,
    /// SHA-256 hex hash of the raw key
    pub key_hash: String,
    /// Human-readable name
    pub name: String,
    /// Granted scopes/permissions
    pub scopes: Vec<String>,
    /// Max requests per second (0 = unlimited)
    pub rate_limit_per_second: u64,
    /// Expiry timestamp
    pub expires_at: Option<DateTime<Utc>>,
    /// Whether the key has been revoked
    pub is_revoked: bool,
    /// Organization this key belongs to
    pub org_id: Option<String>,
    /// When the key was created
    pub created_at: DateTime<Utc>,
    /// Last time this key was used
    pub last_used_at: Option<DateTime<Utc>>,
}

impl ApiKeyRecord {
    pub fn is_expired(&self) -> bool {
        self.expires_at
            .map(|exp| exp < Utc::now())
            .unwrap_or(false)
    }
}

/// Request to create a new API key.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateApiKeyRequest {
    /// Human-readable label
    pub name: String,
    /// Allowed scopes (e.g. `["entities:read", "monitors:read"]`)
    pub scopes: Vec<String>,
    /// Max requests per second (default: 1000)
    pub rate_limit: Option<u64>,
    /// Lifetime in seconds from now (default: 1 year)
    pub expires_in: Option<i64>,
    /// Organization ID to associate with
    pub org_id: Option<String>,
}

/// Response after creating a new API key — includes the raw key (shown once only).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CreateApiKeyResponse {
    /// The raw key — shown exactly once; not stored in plaintext
    pub api_key: String,
    /// Stable key ID (UUID)
    pub id: String,
    pub name: String,
    pub scopes: Vec<String>,
    pub rate_limit: u64,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// Result from a successful key validation.
#[derive(Clone, Debug)]
pub struct ApiKeyValidationResult {
    pub key_id: String,
    pub name: Option<String>,
    pub scopes: Vec<String>,
    pub org_id: Option<String>,
    pub is_expired: bool,
    pub is_revoked: bool,
}

/// Per-key rate limit tracking slot.
#[derive(Clone, Debug)]
struct RateLimitSlot {
    /// Current request count in the current window
    count: u64,
    /// When the current window started
    window_start: DateTime<Utc>,
}

impl Default for RateLimitSlot {
    fn default() -> Self {
        Self {
            count: 0,
            window_start: Utc::now(),
        }
    }
}

/// In-memory + optional persistent API key service.
///
/// For production use, swap the `store` map with DuckDB queries via `duckdb` crate.
/// The current implementation stores keys in memory with hash-based lookup.
#[derive(Clone, Debug)]
pub struct ApiKeyService {
    /// key_hash → record
    store: Arc<RwLock<HashMap<String, ApiKeyRecord>>>,
    /// key_id → rate-limit window
    rate_limits: Arc<RwLock<HashMap<String, RateLimitSlot>>>,
}

impl ApiKeyService {
    /// Create a new in-memory API key service.
    pub fn new() -> Self {
        Self {
            store: Arc::new(RwLock::new(HashMap::new())),
            rate_limits: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Generate a new API key from the given request.
    ///
    /// Returns both the record and the raw key (plaintext) — the raw key is shown once only.
    pub fn create_key(&self, req: CreateApiKeyRequest) -> Result<CreateApiKeyResponse, ApiKeyError> {
        let raw_key = generate_api_key();
        let key_hash = hash_key(&raw_key);
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        let rate_limit = req.rate_limit.unwrap_or(1000);
        let expires_at = req
            .expires_in
            .map(|secs| now + Duration::seconds(secs));

        let record = ApiKeyRecord {
            id: id.clone(),
            key_hash: key_hash.clone(),
            name: req.name.clone(),
            scopes: req.scopes.clone(),
            rate_limit_per_second: rate_limit,
            expires_at,
            is_revoked: false,
            org_id: req.org_id,
            created_at: now,
            last_used_at: None,
        };

        self.store
            .write()
            .map_err(|e| ApiKeyError::Storage(e.to_string()))?
            .insert(key_hash, record);

        Ok(CreateApiKeyResponse {
            api_key: raw_key,
            id,
            name: req.name,
            scopes: req.scopes,
            rate_limit,
            created_at: now,
            expires_at,
        })
    }

    /// Validate a raw API key and check rate limits.
    pub async fn validate_key(&self, raw_key: &str) -> Result<ApiKeyValidationResult, ApiKeyError> {
        if !raw_key.starts_with("orpk_prod_") && !raw_key.starts_with("orpk_dev_") {
            return Err(ApiKeyError::InvalidFormat);
        }

        let key_hash = hash_key(raw_key);

        let record = {
            let store = self
                .store
                .read()
                .map_err(|e| ApiKeyError::Storage(e.to_string()))?;
            store.get(&key_hash).cloned().ok_or(ApiKeyError::NotFound)?
        };

        if record.is_revoked {
            return Err(ApiKeyError::Revoked);
        }

        if record.is_expired() {
            return Err(ApiKeyError::Expired);
        }

        // Rate limit check — sliding 1-second window
        if record.rate_limit_per_second > 0 {
            self.check_rate_limit(&record.id, record.rate_limit_per_second)?;
        }

        // Update last_used_at
        {
            let mut store = self
                .store
                .write()
                .map_err(|e| ApiKeyError::Storage(e.to_string()))?;
            if let Some(rec) = store.get_mut(&key_hash) {
                rec.last_used_at = Some(Utc::now());
            }
        }

        Ok(ApiKeyValidationResult {
            key_id: record.id,
            name: Some(record.name),
            scopes: record.scopes,
            org_id: record.org_id,
            is_expired: false,
            is_revoked: false,
        })
    }

    /// Revoke a key by its ID.
    pub fn revoke_key(&self, key_id: &str) -> Result<(), ApiKeyError> {
        let mut store = self
            .store
            .write()
            .map_err(|e| ApiKeyError::Storage(e.to_string()))?;
        for record in store.values_mut() {
            if record.id == key_id {
                record.is_revoked = true;
                return Ok(());
            }
        }
        Err(ApiKeyError::NotFound)
    }

    /// List all keys (without exposing raw key material).
    pub fn list_keys(&self) -> Result<Vec<ApiKeyRecord>, ApiKeyError> {
        let store = self
            .store
            .read()
            .map_err(|e| ApiKeyError::Storage(e.to_string()))?;
        Ok(store.values().cloned().collect())
    }

    fn check_rate_limit(&self, key_id: &str, limit: u64) -> Result<(), ApiKeyError> {
        let mut rl = self
            .rate_limits
            .write()
            .map_err(|e| ApiKeyError::Storage(e.to_string()))?;
        let now = Utc::now();
        let slot = rl.entry(key_id.to_string()).or_default();

        let elapsed = (now - slot.window_start).num_seconds();
        if elapsed >= 1 {
            // Reset window
            slot.count = 1;
            slot.window_start = now;
        } else {
            slot.count += 1;
            if slot.count > limit {
                let retry_after = (1 - elapsed).max(1) as u64;
                return Err(ApiKeyError::RateLimitExceeded {
                    retry_after_seconds: retry_after,
                });
            }
        }

        Ok(())
    }
}

impl Default for ApiKeyService {
    fn default() -> Self {
        Self::new()
    }
}

/// Generate a new API key with the `orpk_prod_` prefix followed by 32 random hex bytes.
fn generate_api_key() -> String {
    let mut rng = rand::thread_rng();
    let bytes: Vec<u8> = (0..24).map(|_| rng.gen::<u8>()).collect();
    format!("orpk_prod_{}", hex::encode(bytes))
}

/// SHA-256 hash of the raw key for storage.
fn hash_key(raw_key: &str) -> String {
    use sha2::{Digest, Sha256};
    let hash = Sha256::digest(raw_key.as_bytes());
    hex::encode(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_service() -> ApiKeyService {
        ApiKeyService::new()
    }

    fn make_request() -> CreateApiKeyRequest {
        CreateApiKeyRequest {
            name: "test-key".to_string(),
            scopes: vec!["entities:read".to_string(), "monitors:read".to_string()],
            rate_limit: Some(100),
            expires_in: Some(3600),
            org_id: Some("org-1".to_string()),
        }
    }

    #[test]
    fn test_key_format() {
        let key = generate_api_key();
        assert!(key.starts_with("orpk_prod_"), "key={key}");
        // prefix(10) + 48 hex chars
        assert_eq!(key.len(), 10 + 48, "key={key}");
    }

    #[test]
    fn test_create_key_returns_valid_format() {
        let svc = test_service();
        let resp = svc.create_key(make_request()).unwrap();
        assert!(resp.api_key.starts_with("orpk_prod_"));
        assert_eq!(resp.name, "test-key");
        assert!(resp.scopes.contains(&"entities:read".to_string()));
    }

    #[tokio::test]
    async fn test_validate_key_success() {
        let svc = test_service();
        let resp = svc.create_key(make_request()).unwrap();
        let result = svc.validate_key(&resp.api_key).await.unwrap();
        assert_eq!(result.scopes, vec!["entities:read", "monitors:read"]);
        assert_eq!(result.org_id.as_deref(), Some("org-1"));
        assert!(!result.is_revoked);
        assert!(!result.is_expired);
    }

    #[tokio::test]
    async fn test_invalid_format_rejected() {
        let svc = test_service();
        let result = svc.validate_key("not-a-valid-key").await;
        assert!(matches!(result, Err(ApiKeyError::InvalidFormat)));
    }

    #[tokio::test]
    async fn test_unknown_key_rejected() {
        let svc = test_service();
        let result = svc.validate_key("orpk_prod_aaabbbccc000111").await;
        assert!(matches!(result, Err(ApiKeyError::NotFound)));
    }

    #[tokio::test]
    async fn test_revoked_key_rejected() {
        let svc = test_service();
        let resp = svc.create_key(make_request()).unwrap();
        svc.revoke_key(&resp.id).unwrap();
        let result = svc.validate_key(&resp.api_key).await;
        assert!(matches!(result, Err(ApiKeyError::Revoked)));
    }

    #[tokio::test]
    async fn test_expired_key_rejected() {
        let svc = test_service();
        let req = CreateApiKeyRequest {
            expires_in: Some(-1), // already expired
            ..make_request()
        };
        let resp = svc.create_key(req).unwrap();
        let result = svc.validate_key(&resp.api_key).await;
        assert!(matches!(result, Err(ApiKeyError::Expired)));
    }

    #[test]
    fn test_list_keys() {
        let svc = test_service();
        svc.create_key(make_request()).unwrap();
        svc.create_key(make_request()).unwrap();
        let keys = svc.list_keys().unwrap();
        assert_eq!(keys.len(), 2);
    }

    #[tokio::test]
    async fn test_rate_limit_exceeded() {
        let svc = test_service();
        let req = CreateApiKeyRequest {
            rate_limit: Some(2), // only 2 req/s
            ..make_request()
        };
        let resp = svc.create_key(req).unwrap();

        // First two should succeed
        svc.validate_key(&resp.api_key).await.unwrap();
        svc.validate_key(&resp.api_key).await.unwrap();
        // Third should be rate limited
        let result = svc.validate_key(&resp.api_key).await;
        assert!(matches!(
            result,
            Err(ApiKeyError::RateLimitExceeded { .. })
        ));
    }
}
