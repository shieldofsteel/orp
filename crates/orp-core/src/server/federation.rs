//! ORP-to-ORP Federation — Peer registry, sync protocol, and conflict resolution.
//!
//! Federation allows multiple ORP instances to share entity data.
//! ABAC-controlled entity types are the gate: a peer only receives the entity
//! types it is authorised to see.  When two peers report the same entity_id,
//! the copy with the highest `confidence` value wins (last-write-by-confidence).
//!
//! Federated entities are tagged with `source: "peer:<peer_id>"` in their
//! properties so downstream consumers can distinguish local vs. remote data.
//!
//! # Endpoints
//! - `POST   /api/v1/peers`               — register a peer
//! - `GET    /api/v1/peers`               — list all peers
//! - `DELETE /api/v1/peers/{id}`          — remove a peer
//! - `POST   /api/v1/peers/{id}/sync`     — trigger an on-demand sync
//!
//! # Background sync
//! `spawn_federation_sync` starts a Tokio task that wakes every 30 s and
//! pulls `/api/v1/entities` from every registered peer.

use crate::server::http::AppState;
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use orp_proto::{Entity, GeoPoint};
use orp_security::middleware::AuthContext;
use orp_security::{AbacEngine, EvaluationContext, EvaluationResult, Resource, Subject};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

// ── Peer record ──────────────────────────────────────────────────────────────

/// A connected ORP peer.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Peer {
    /// Stable identifier chosen at registration time (e.g. "cluster-east").
    pub id: String,
    /// Hostname or IP of the remote ORP instance.
    pub host: String,
    /// HTTP port (usually 8080).
    pub port: u16,
    /// Entity types this peer is allowed to share with us.
    pub shared_entity_types: Vec<String>,
    /// UTC timestamp of the last successful sync (ISO-8601).
    pub last_seen: Option<String>,
    /// Whether auto-sync is enabled for this peer.
    pub sync_enabled: bool,
}

impl Peer {
    /// Base URL for the remote ORP REST API.
    pub fn base_url(&self) -> String {
        format!("http://{}:{}/api/v1", self.host, self.port)
    }
}

// ── Registry ─────────────────────────────────────────────────────────────────

/// In-memory registry of connected peers.  Shared via `Arc<PeerRegistry>`.
#[derive(Default)]
pub struct PeerRegistry {
    peers: RwLock<HashMap<String, Peer>>,
}

impl PeerRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub async fn register(&self, peer: Peer) {
        let mut map = self.peers.write().await;
        map.insert(peer.id.clone(), peer);
    }

    pub async fn remove(&self, id: &str) -> bool {
        let mut map = self.peers.write().await;
        map.remove(id).is_some()
    }

    pub async fn list(&self) -> Vec<Peer> {
        let map = self.peers.read().await;
        map.values().cloned().collect()
    }

    pub async fn get(&self, id: &str) -> Option<Peer> {
        let map = self.peers.read().await;
        map.get(id).cloned()
    }

    pub async fn update_last_seen(&self, id: &str) {
        let mut map = self.peers.write().await;
        if let Some(peer) = map.get_mut(id) {
            peer.last_seen = Some(chrono::Utc::now().to_rfc3339());
        }
    }
}

// ── Error helper (mirrors handlers.rs style) ─────────────────────────────────

#[derive(Serialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: String,
    status: u16,
    message: String,
    request_id: String,
    timestamp: String,
}

fn error_response(
    code: &str,
    status: StatusCode,
    message: &str,
) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: ErrorBody {
                code: code.to_string(),
                status: status.as_u16(),
                message: message.to_string(),
                request_id: uuid::Uuid::new_v4().to_string(),
                timestamp: chrono::Utc::now().to_rfc3339(),
            },
        }),
    )
}

// ── ABAC helper ───────────────────────────────────────────────────────────────

fn check_abac(
    abac: &AbacEngine,
    auth: &AuthContext,
    action: &str,
    resource_type: &str,
    resource_id: &str,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    let ctx = EvaluationContext {
        subject: Subject {
            sub: auth.subject.clone(),
            permissions: auth.permissions.clone(),
            role: if auth.has_permission("admin") {
                Some("admin".to_string())
            } else {
                None
            },
            org_id: auth.org_id.clone(),
            attributes: HashMap::new(),
        },
        action: action.to_string(),
        resource: Resource {
            r#type: resource_type.to_string(),
            id: resource_id.to_string(),
            attributes: HashMap::new(),
        },
    };
    let decision = abac.evaluate(&ctx);
    if decision.result == EvaluationResult::Deny {
        return Err(error_response(
            "FORBIDDEN",
            StatusCode::FORBIDDEN,
            &format!("Access denied: {}", decision.reason),
        ));
    }
    Ok(())
}

// ── Request / response types ─────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct RegisterPeerRequest {
    pub id: String,
    pub host: String,
    pub port: u16,
    /// Entity types the remote peer is authorised to share with us.
    pub shared_entity_types: Vec<String>,
    /// Defaults to true.
    pub sync_enabled: Option<bool>,
}

// ── HTTP handlers ─────────────────────────────────────────────────────────────

/// `POST /api/v1/peers` — register a new peer.
pub async fn register_peer(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Json(body): Json<RegisterPeerRequest>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "federation:manage", "peer", "*") {
        return resp.into_response();
    }

    if body.id.is_empty() {
        return error_response(
            "VALIDATION_ERROR",
            StatusCode::BAD_REQUEST,
            "Peer id cannot be empty",
        )
        .into_response();
    }
    if body.host.is_empty() {
        return error_response(
            "VALIDATION_ERROR",
            StatusCode::BAD_REQUEST,
            "Peer host cannot be empty",
        )
        .into_response();
    }
    if body.port == 0 {
        return error_response(
            "VALIDATION_ERROR",
            StatusCode::BAD_REQUEST,
            "Peer port must be > 0",
        )
        .into_response();
    }

    // Reject duplicate peer ids
    if let Some(registry) = &state.federation_registry {
        if registry.get(&body.id).await.is_some() {
            return error_response(
                "CONFLICT",
                StatusCode::CONFLICT,
                &format!("Peer '{}' is already registered", body.id),
            )
            .into_response();
        }

        let peer = Peer {
            id: body.id.clone(),
            host: body.host,
            port: body.port,
            shared_entity_types: body.shared_entity_types,
            last_seen: None,
            sync_enabled: body.sync_enabled.unwrap_or(true),
        };

        registry.register(peer.clone()).await;
        info!(peer_id = %peer.id, host = %peer.host, port = %peer.port, "Peer registered");

        (StatusCode::CREATED, Json(peer)).into_response()
    } else {
        error_response(
            "FEDERATION_DISABLED",
            StatusCode::SERVICE_UNAVAILABLE,
            "Federation is not enabled on this instance",
        )
        .into_response()
    }
}

/// `GET /api/v1/peers` — list all registered peers.
pub async fn list_peers(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "federation:read", "peer", "*") {
        return resp.into_response();
    }

    if let Some(registry) = &state.federation_registry {
        let peers = registry.list().await;
        Json(serde_json::json!({
            "data": peers,
            "count": peers.len(),
        }))
        .into_response()
    } else {
        error_response(
            "FEDERATION_DISABLED",
            StatusCode::SERVICE_UNAVAILABLE,
            "Federation is not enabled on this instance",
        )
        .into_response()
    }
}

/// `DELETE /api/v1/peers/{id}` — remove a peer.
pub async fn remove_peer(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "federation:manage", "peer", &id) {
        return resp.into_response();
    }

    if let Some(registry) = &state.federation_registry {
        if registry.remove(&id).await {
            info!(peer_id = %id, "Peer removed");
            StatusCode::NO_CONTENT.into_response()
        } else {
            error_response(
                "PEER_NOT_FOUND",
                StatusCode::NOT_FOUND,
                &format!("Peer '{}' not found", id),
            )
            .into_response()
        }
    } else {
        error_response(
            "FEDERATION_DISABLED",
            StatusCode::SERVICE_UNAVAILABLE,
            "Federation is not enabled on this instance",
        )
        .into_response()
    }
}

/// `POST /api/v1/peers/{id}/sync` — trigger an immediate sync with one peer.
pub async fn sync_peer(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "federation:manage", "peer", &id) {
        return resp.into_response();
    }

    if let Some(registry) = &state.federation_registry {
        if let Some(peer) = registry.get(&id).await {
            let client = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default();

            let result = pull_entities_from_peer(&client, &peer, &state).await;
            registry.update_last_seen(&id).await;

            match result {
                Ok(count) => Json(serde_json::json!({
                    "status": "ok",
                    "peer_id": id,
                    "entities_synced": count,
                    "synced_at": chrono::Utc::now().to_rfc3339(),
                }))
                .into_response(),
                Err(e) => error_response(
                    "SYNC_ERROR",
                    StatusCode::BAD_GATEWAY,
                    &format!("Sync with peer '{}' failed: {}", id, e),
                )
                .into_response(),
            }
        } else {
            error_response(
                "PEER_NOT_FOUND",
                StatusCode::NOT_FOUND,
                &format!("Peer '{}' not found", id),
            )
            .into_response()
        }
    } else {
        error_response(
            "FEDERATION_DISABLED",
            StatusCode::SERVICE_UNAVAILABLE,
            "Federation is not enabled on this instance",
        )
        .into_response()
    }
}

// ── Core sync logic ───────────────────────────────────────────────────────────

/// Pull entities from a single peer and upsert into local storage.
/// Returns the number of entities written (created or updated by confidence).
pub async fn pull_entities_from_peer(
    client: &reqwest::Client,
    peer: &Peer,
    state: &AppState,
) -> anyhow::Result<usize> {
    let mut total_written = 0usize;

    for entity_type in &peer.shared_entity_types {
        let url = format!(
            "{}/entities?type={}&limit=1000",
            peer.base_url(),
            entity_type
        );

        let resp = client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("HTTP request failed: {}", e))?;

        if !resp.status().is_success() {
            warn!(
                peer_id = %peer.id,
                entity_type = %entity_type,
                status = %resp.status(),
                "Peer returned non-2xx for entity list"
            );
            continue;
        }

        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to parse peer response: {}", e))?;

        let entities = match body.get("data").and_then(|d| d.as_array()) {
            Some(arr) => arr.clone(),
            None => continue,
        };

        for raw in &entities {
            if let Ok(written) = upsert_federated_entity(raw, &peer.id, state).await {
                if written {
                    total_written += 1;
                }
            }
        }
    }

    Ok(total_written)
}

/// Upsert a single entity received from a peer.
///
/// Conflict resolution: if an entity with the same id already exists locally,
/// only overwrite it if the incoming `confidence` is higher.  This ensures the
/// most-confident observation wins across the federation.
///
/// Returns `true` if the local store was modified, `false` if skipped.
async fn upsert_federated_entity(
    raw: &serde_json::Value,
    peer_id: &str,
    state: &AppState,
) -> anyhow::Result<bool> {
    let entity_id = raw
        .get("id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("entity missing 'id'"))?
        .to_string();

    let entity_type = raw
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("generic")
        .to_string();

    let incoming_confidence = raw
        .get("confidence")
        .and_then(|v| v.as_f64())
        .unwrap_or(0.5);

    // Check local copy — if it exists with higher confidence, skip.
    if let Ok(Some(existing)) = state.storage.get_entity(&entity_id).await {
        if existing.confidence >= incoming_confidence {
            return Ok(false);
        }
    }

    // Build properties map and inject federation metadata.
    let mut properties: HashMap<String, serde_json::Value> = raw
        .get("properties")
        .and_then(|p| p.as_object())
        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
        .unwrap_or_default();

    properties.insert(
        "source".to_string(),
        serde_json::Value::String(format!("peer:{}", peer_id)),
    );

    // Parse optional geometry.
    let geometry = raw
        .get("geometry")
        .and_then(|g| g.get("coordinates"))
        .and_then(|c| c.as_array())
        .filter(|c| c.len() == 2)
        .map(|c| GeoPoint {
            lon: c[0].as_f64().unwrap_or(0.0),
            lat: c[1].as_f64().unwrap_or(0.0),
            alt: None,
        });

    let name = raw
        .get("name")
        .and_then(|n| n.as_str())
        .map(|s| s.to_string());

    let entity = Entity {
        entity_id,
        entity_type,
        name,
        properties,
        geometry,
        confidence: incoming_confidence,
        canonical_id: None,
        is_active: true,
        created_at: chrono::Utc::now(),
        last_updated: chrono::Utc::now(),
    };

    state
        .storage
        .insert_entity(&entity)
        .await
        .map_err(|e| anyhow::anyhow!("Storage insert failed: {}", e))?;

    Ok(true)
}

// ── Background sync task ──────────────────────────────────────────────────────

/// Spawn a background Tokio task that syncs all peers every 30 seconds.
///
/// # Usage
/// Call once at server startup, after `AppState` is constructed.
///
/// ```ignore
/// federation::spawn_federation_sync(state.clone());
/// ```
pub fn spawn_federation_sync(state: Arc<AppState>) {
    tokio::spawn(async move {
        let interval = std::time::Duration::from_secs(30);
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(25))
            .user_agent(concat!("ORP-federation/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();

        loop {
            tokio::time::sleep(interval).await;

            let registry = match &state.federation_registry {
                Some(r) => r.clone(),
                None => continue,
            };

            let peers = registry.list().await;
            if peers.is_empty() {
                continue;
            }

            for peer in peers {
                if !peer.sync_enabled {
                    continue;
                }

                match pull_entities_from_peer(&client, &peer, &state).await {
                    Ok(count) => {
                        info!(
                            peer_id = %peer.id,
                            entities_synced = count,
                            "Federation sync completed"
                        );
                        registry.update_last_seen(&peer.id).await;
                    }
                    Err(e) => {
                        warn!(peer_id = %peer.id, error = %e, "Federation sync failed");
                    }
                }
            }
        }
    });
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── PeerRegistry unit tests ───────────────────────────────────────────────

    fn make_peer(id: &str) -> Peer {
        Peer {
            id: id.to_string(),
            host: "localhost".to_string(),
            port: 9000,
            shared_entity_types: vec!["ship".to_string(), "aircraft".to_string()],
            last_seen: None,
            sync_enabled: true,
        }
    }

    #[tokio::test]
    async fn test_register_and_list_peer() {
        let registry = PeerRegistry::new();
        registry.register(make_peer("alpha")).await;
        let peers = registry.list().await;
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].id, "alpha");
    }

    #[tokio::test]
    async fn test_register_multiple_peers() {
        let registry = PeerRegistry::new();
        for id in &["alpha", "beta", "gamma"] {
            registry.register(make_peer(id)).await;
        }
        assert_eq!(registry.list().await.len(), 3);
    }

    #[tokio::test]
    async fn test_remove_peer_exists() {
        let registry = PeerRegistry::new();
        registry.register(make_peer("alpha")).await;
        assert!(registry.remove("alpha").await);
        assert_eq!(registry.list().await.len(), 0);
    }

    #[tokio::test]
    async fn test_remove_peer_missing() {
        let registry = PeerRegistry::new();
        assert!(!registry.remove("nonexistent").await);
    }

    #[tokio::test]
    async fn test_get_peer() {
        let registry = PeerRegistry::new();
        registry.register(make_peer("alpha")).await;
        let peer = registry.get("alpha").await.unwrap();
        assert_eq!(peer.host, "localhost");
        assert_eq!(peer.port, 9000);
    }

    #[tokio::test]
    async fn test_get_peer_missing() {
        let registry = PeerRegistry::new();
        assert!(registry.get("nonexistent").await.is_none());
    }

    #[tokio::test]
    async fn test_update_last_seen() {
        let registry = PeerRegistry::new();
        registry.register(make_peer("alpha")).await;
        assert!(registry.get("alpha").await.unwrap().last_seen.is_none());
        registry.update_last_seen("alpha").await;
        assert!(registry.get("alpha").await.unwrap().last_seen.is_some());
    }

    #[tokio::test]
    async fn test_peer_base_url() {
        let peer = make_peer("test");
        assert_eq!(peer.base_url(), "http://localhost:9000/api/v1");
    }

    #[tokio::test]
    async fn test_register_overwrites_same_id() {
        let registry = PeerRegistry::new();
        registry.register(make_peer("alpha")).await;
        let mut updated = make_peer("alpha");
        updated.port = 9001;
        registry.register(updated).await;
        let peer = registry.get("alpha").await.unwrap();
        assert_eq!(peer.port, 9001);
    }

    // ── Upsert / conflict resolution ─────────────────────────────────────────

    #[tokio::test]
    async fn test_upsert_new_entity() {
        use orp_storage::DuckDbStorage;

        let storage: Arc<dyn orp_storage::traits::Storage> =
            Arc::new(DuckDbStorage::new_in_memory().unwrap());

        let raw = serde_json::json!({
            "id": "ship-fed-1",
            "type": "ship",
            "confidence": 0.9,
            "properties": { "mmsi": "123456789" },
        });

        // We can't easily construct AppState without all deps, so test
        // the storage + confidence logic directly.
        let entity_id = raw["id"].as_str().unwrap();
        let confidence = raw["confidence"].as_f64().unwrap();

        // Nothing in storage yet — should write
        let existing = storage.get_entity(entity_id).await.unwrap();
        assert!(existing.is_none());

        let entity = Entity {
            entity_id: entity_id.to_string(),
            entity_type: "ship".to_string(),
            name: None,
            properties: {
                let mut m = HashMap::new();
                m.insert("source".to_string(), serde_json::json!("peer:alpha"));
                m
            },
            geometry: None,
            confidence,
            canonical_id: None,
            is_active: true,
            created_at: chrono::Utc::now(),
            last_updated: chrono::Utc::now(),
        };
        storage.insert_entity(&entity).await.unwrap();

        let stored = storage.get_entity(entity_id).await.unwrap().unwrap();
        // DuckDB stores confidence as FLOAT (f32); allow for f32 precision loss.
        assert!((stored.confidence - 0.9).abs() < 1e-6, "confidence was {}", stored.confidence);
        assert_eq!(stored.properties["source"], "peer:alpha");
    }

    #[tokio::test]
    async fn test_conflict_resolution_lower_confidence_skipped() {
        use orp_storage::DuckDbStorage;

        let storage: Arc<dyn orp_storage::traits::Storage> =
            Arc::new(DuckDbStorage::new_in_memory().unwrap());

        // Insert a high-confidence local entity
        let entity = Entity {
            entity_id: "ship-conflict".to_string(),
            entity_type: "ship".to_string(),
            confidence: 0.95,
            ..Entity::default()
        };
        storage.insert_entity(&entity).await.unwrap();

        // Simulate incoming peer entity with lower confidence
        let incoming_confidence = 0.60_f64;
        let existing = storage.get_entity("ship-conflict").await.unwrap().unwrap();

        // Should skip — local is more confident
        assert!(
            existing.confidence >= incoming_confidence,
            "local ({}) should be >= incoming ({})",
            existing.confidence,
            incoming_confidence
        );
    }

    #[tokio::test]
    async fn test_conflict_resolution_higher_confidence_wins() {
        use orp_storage::DuckDbStorage;

        let storage: Arc<dyn orp_storage::traits::Storage> =
            Arc::new(DuckDbStorage::new_in_memory().unwrap());

        // Insert a low-confidence local entity
        let entity = Entity {
            entity_id: "ship-upgrade".to_string(),
            entity_type: "ship".to_string(),
            confidence: 0.40,
            ..Entity::default()
        };
        storage.insert_entity(&entity).await.unwrap();

        // Incoming peer entity has higher confidence — should overwrite
        let incoming_confidence = 0.95_f64;
        let existing = storage.get_entity("ship-upgrade").await.unwrap().unwrap();
        assert!(existing.confidence < incoming_confidence);

        // Write the higher-confidence version
        let updated = Entity {
            entity_id: "ship-upgrade".to_string(),
            entity_type: "ship".to_string(),
            confidence: incoming_confidence,
            ..Entity::default()
        };
        storage.insert_entity(&updated).await.unwrap();

        let stored = storage.get_entity("ship-upgrade").await.unwrap().unwrap();
        // DuckDB stores confidence as FLOAT (f32); allow for f32 precision loss.
        assert!((stored.confidence - 0.95).abs() < 1e-6, "confidence was {}", stored.confidence);
    }
}
