use crate::server::http::AppState;
use crate::server::media::{
    rewrite_hls_playlist, validate_relay_target, CreateMediaStreamInput, MediaProtocol,
    MediaRegistryError, MediaStream, MediaStreamHandle, HLS_PLAYLIST_MAX_BYTES, RELAY_BYTE_CAP,
    RELAY_IDLE_TIMEOUT,
};
use crate::server::websocket::BroadcastEvent;
use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, StatusCode},
    response::{IntoResponse, Json, Response},
};
use chrono::Utc;
use futures_util::StreamExt;
use orp_proto::{Entity, GeoPoint, Relationship};
use orp_security::middleware::AuthContext;
use orp_security::url_safety::build_safe_client;
use orp_security::{AbacEngine, EvaluationContext, EvaluationResult, Resource, Subject};
use orp_stream::monitor::{
    AlertSeverity, GeofenceTrigger, MonitorAction, MonitorCondition, MonitorRule, ThresholdOp,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::bytes::Bytes;

// ---- Error Response ----

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

// ---- ABAC helper ----

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

// ---- Audit helper ----

/// Append an entry to the immutable, hash-chained audit log.
///
/// Routes through `state.audit_log` (a [`PersistentAuditLog`] in production,
/// [`InMemoryAuditLog`] in tests / `--in-memory`). The trait object computes
/// the content hash and Ed25519 signature over the exact persisted bytes,
/// replacing the v0.2.0 path that signed a pre-image with a `?` placeholder
/// for the sequence number — that signature was never verifiable.
async fn audit_log(
    state: &AppState,
    operation: &str,
    entity_type: Option<&str>,
    entity_id: Option<&str>,
    user_id: &str,
    details: serde_json::Value,
) {
    if let Err(e) = state
        .audit_log
        .record(operation, entity_type, entity_id, Some(user_id), details)
        .await
    {
        tracing::warn!("Audit log write failed: {}", e);
    }
}

// ---- Health ----

#[derive(Serialize)]
pub struct HealthResponse {
    status: String,
    timestamp: String,
    version: String,
    uptime_seconds: u64,
    components: HealthComponents,
}

#[derive(Serialize)]
pub struct HealthComponents {
    database: ComponentHealth,
    graph_engine: ComponentHealth,
    stream_processor: ComponentHealth,
    api_server: ComponentHealth,
    monitor_engine: ComponentHealth,
    /// Per-connector live health, derived from each `Connector::stats()`.
    /// Empty if no connectors are registered with the AppState.
    connectors: Vec<ConnectorHealthEntry>,
    /// Per-peer pending outbox count (federation outbound buffer). Empty when
    /// federation is disabled or the outbox failed to open. Each entry maps
    /// `peer_id` → number of events currently buffered for that peer.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    federation_outbox: Vec<FederationOutboxHealth>,
}

#[derive(Serialize)]
pub struct FederationOutboxHealth {
    pub peer_id: String,
    pub pending_count: u64,
}

#[derive(Serialize)]
pub struct ComponentHealth {
    status: String,
    latency_ms: Option<f64>,
}

#[derive(Serialize)]
pub struct ConnectorHealthEntry {
    pub id: String,
    pub connector_type: String,
    /// "healthy" | "degraded" | "unhealthy". Computed from
    /// `error_rate_window_pct`: rate > 50% → unhealthy, > 10% → degraded,
    /// else healthy.
    pub status: String,
    pub events_processed: u64,
    pub errors: u64,
    /// errors / (events + errors) over the connector's lifetime, expressed
    /// as a percentage. `0.0` when `events + errors == 0` so a freshly-
    /// started connector reads as healthy.
    pub error_rate_window_pct: f64,
}

/// Compute a connector's status string from its (events, errors) counters.
/// Policy: > 50% error rate → unhealthy, > 10% → degraded, else healthy.
/// `events + errors == 0` is healthy by definition.
fn connector_status_from_counts(events: u64, errors: u64) -> (String, f64) {
    let total = events.saturating_add(errors);
    if total == 0 {
        return ("healthy".to_string(), 0.0);
    }
    let rate = errors as f64 / total as f64;
    let status = if rate > 0.50 {
        "unhealthy"
    } else if rate > 0.10 {
        "degraded"
    } else {
        "healthy"
    };
    (status.to_string(), rate * 100.0)
}

pub async fn health_check(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let start = std::time::Instant::now();
    let db_status = match state.storage.health_check().await {
        Ok(()) => "healthy",
        Err(_) => "error",
    };
    let db_latency = start.elapsed().as_secs_f64() * 1000.0;

    let _proc_stats = state.processor.stats();
    let uptime = state.started_at.elapsed().as_secs();

    // Snapshot the live connector registry. We hold the lock just long enough
    // to read each connector's config + stats; both are sync calls so we never
    // suspend with the lock held.
    let connector_entries: Vec<ConnectorHealthEntry> = {
        let guard = state.connectors.lock().await;
        guard
            .iter()
            .map(|c| {
                let cfg = c.config();
                let stats = c.stats();
                let (status, rate_pct) =
                    connector_status_from_counts(stats.events_processed, stats.errors);
                ConnectorHealthEntry {
                    id: cfg.connector_id.clone(),
                    connector_type: cfg.connector_type.clone(),
                    status,
                    events_processed: stats.events_processed,
                    errors: stats.errors,
                    error_rate_window_pct: rate_pct,
                }
            })
            .collect()
    };

    // If any connector is unhealthy, the top-level status degrades but we
    // never fail the /health endpoint outright — dashboards still get JSON.
    let any_unhealthy = connector_entries.iter().any(|c| c.status == "unhealthy");
    let top_status = if any_unhealthy { "degraded" } else { "healthy" };

    // Surface per-peer outbox pending counts when federation is enabled.
    // We touch RocksDB synchronously; the prefix scan is O(pending) per peer
    // and only runs on /health hits, which are not on a hot path.
    let federation_outbox_entries: Vec<FederationOutboxHealth> =
        match (&state.federation_registry, &state.federation_outbox) {
            (Some(registry), Some(outbox)) => {
                let peers = registry.list().await;
                let mut entries = Vec::with_capacity(peers.len());
                for peer in peers {
                    let count = outbox.pending_count(&peer.id).unwrap_or(0);
                    entries.push(FederationOutboxHealth {
                        peer_id: peer.id,
                        pending_count: count,
                    });
                }
                entries
            }
            _ => Vec::new(),
        };

    Json(HealthResponse {
        status: top_status.to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: uptime,
        components: HealthComponents {
            database: ComponentHealth {
                status: db_status.to_string(),
                latency_ms: Some(db_latency),
            },
            graph_engine: ComponentHealth {
                // The graph engine is currently a DuckDB-backed projection
                // (see crates/orp-storage/src/graph_engine.rs). Its health is
                // gated on the underlying database; surface that explicitly so
                // the openapi `graph_engine` component is always populated.
                status: db_status.to_string(),
                latency_ms: None,
            },
            stream_processor: ComponentHealth {
                status: "healthy".to_string(),
                latency_ms: None,
            },
            api_server: ComponentHealth {
                status: "healthy".to_string(),
                latency_ms: None,
            },
            monitor_engine: ComponentHealth {
                status: "healthy".to_string(),
                latency_ms: None,
            },
            connectors: connector_entries,
            federation_outbox: federation_outbox_entries,
        },
    })
}

pub async fn metrics(auth: AuthContext, State(state): State<Arc<AppState>>) -> impl IntoResponse {
    // Require auth — the AuthContext extractor already validated credentials.
    // Additionally check ABAC for metrics access.
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "admin", "system", "metrics") {
        return resp.into_response();
    }

    let stats = state.storage.get_stats().await.unwrap_or_default();
    let proc_stats = state.processor.stats();
    let uptime = state.started_at.elapsed().as_secs();
    let alert_count = state.monitor_engine.get_alerts(10000).await.len();

    format!(
        "# HELP orp_entities_total Total number of entities\n\
         # TYPE orp_entities_total gauge\n\
         orp_entities_total {}\n\
         \n\
         # HELP orp_events_total Total events stored\n\
         # TYPE orp_events_total gauge\n\
         orp_events_total {}\n\
         \n\
         # HELP orp_relationships_total Total relationships\n\
         # TYPE orp_relationships_total gauge\n\
         orp_relationships_total {}\n\
         \n\
         # HELP orp_stream_events_processed Stream events processed\n\
         # TYPE orp_stream_events_processed counter\n\
         orp_stream_events_processed {}\n\
         \n\
         # HELP orp_stream_events_deduplicated Deduplicated events\n\
         # TYPE orp_stream_events_deduplicated counter\n\
         orp_stream_events_deduplicated {}\n\
         \n\
         # HELP orp_alerts_total Total alerts triggered\n\
         # TYPE orp_alerts_total gauge\n\
         orp_alerts_total {}\n\
         \n\
         # HELP orp_uptime_seconds Server uptime\n\
         # TYPE orp_uptime_seconds gauge\n\
         orp_uptime_seconds {}\n",
        stats.total_entities,
        stats.total_events,
        stats.total_relationships,
        proc_stats.events_processed,
        proc_stats.events_deduplicated,
        alert_count,
        uptime,
    )
    .into_response()
}

// ---- Entities ----

#[derive(Deserialize)]
pub struct ListParams {
    page: Option<usize>,
    limit: Option<usize>,
    #[serde(rename = "type")]
    entity_type: Option<String>,
}

#[derive(Serialize)]
struct PaginatedResponse<T: Serialize> {
    data: Vec<T>,
    pagination: Pagination,
}

#[derive(Serialize)]
struct Pagination {
    page: usize,
    limit: usize,
    total_count: u64,
    total_pages: u64,
    has_next: bool,
    has_prev: bool,
}

#[derive(Serialize)]
struct EntityResponse {
    id: String,
    #[serde(rename = "type")]
    entity_type: String,
    name: Option<String>,
    properties: HashMap<String, serde_json::Value>,
    geometry: Option<GeoJsonPoint>,
    confidence: f64,
    is_active: bool,
    created_at: String,
    updated_at: String,
}

#[derive(Serialize)]
struct GeoJsonPoint {
    #[serde(rename = "type")]
    geo_type: String,
    coordinates: [f64; 2],
}

fn entity_to_response(e: &Entity) -> EntityResponse {
    EntityResponse {
        id: e.entity_id.clone(),
        entity_type: e.entity_type.clone(),
        name: e.name.clone(),
        properties: e.properties.clone(),
        geometry: e.geometry.as_ref().map(|g| GeoJsonPoint {
            geo_type: "Point".to_string(),
            coordinates: [g.lon, g.lat],
        }),
        confidence: e.confidence,
        is_active: e.is_active,
        created_at: e.created_at.to_rfc3339(),
        updated_at: e.last_updated.to_rfc3339(),
    }
}

pub async fn list_entities(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:read", "entity", "*") {
        return resp.into_response();
    }

    let page = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = (page - 1) * limit;
    let entity_type = params.entity_type.as_deref().unwrap_or("ship");

    match state
        .storage
        .get_entities_by_type(entity_type, limit, offset)
        .await
    {
        Ok(entities) => {
            let total = state.storage.count_entities().await.unwrap_or(0);
            let total_pages = if limit > 0 {
                (total as f64 / limit as f64).ceil() as u64
            } else {
                0
            };
            let data: Vec<EntityResponse> = entities.iter().map(entity_to_response).collect();

            Json(PaginatedResponse {
                data,
                pagination: Pagination {
                    page,
                    limit,
                    total_count: total,
                    total_pages,
                    has_next: (page as u64) < total_pages,
                    has_prev: page > 1,
                },
            })
            .into_response()
        }
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

#[derive(Deserialize)]
pub struct CreateEntityRequest {
    id: String,
    #[serde(rename = "type")]
    entity_type: String,
    name: Option<String>,
    properties: Option<HashMap<String, serde_json::Value>>,
    geometry: Option<CreateGeoJson>,
    confidence: Option<f64>,
}

#[derive(Deserialize)]
struct CreateGeoJson {
    coordinates: [f64; 2],
}

pub async fn create_entity(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateEntityRequest>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(
        &state.abac_engine,
        &auth,
        "entities:write",
        "entity",
        &body.id,
    ) {
        return resp.into_response();
    }

    if body.id.is_empty() {
        return error_response(
            "VALIDATION_ERROR",
            StatusCode::BAD_REQUEST,
            "Entity id cannot be empty",
        )
        .into_response();
    }

    // Check if entity already exists → 409 CONFLICT
    match state.storage.get_entity(&body.id).await {
        Ok(Some(_)) => {
            return error_response(
                "CONFLICT",
                StatusCode::CONFLICT,
                &format!("Entity with id '{}' already exists", body.id),
            )
            .into_response();
        }
        Err(e) => {
            return error_response(
                "INTERNAL_ERROR",
                StatusCode::INTERNAL_SERVER_ERROR,
                &e.to_string(),
            )
            .into_response();
        }
        Ok(None) => {} // good — entity does not exist
    }

    // Validate lat/lon ranges if geometry is provided
    if let Some(ref geo) = body.geometry {
        let lat = geo.coordinates[1];
        let lon = geo.coordinates[0];
        if !(-90.0..=90.0).contains(&lat) {
            return error_response(
                "VALIDATION_ERROR",
                StatusCode::BAD_REQUEST,
                &format!("Latitude must be between -90 and 90, got {}", lat),
            )
            .into_response();
        }
        if !(-180.0..=180.0).contains(&lon) {
            return error_response(
                "VALIDATION_ERROR",
                StatusCode::BAD_REQUEST,
                &format!("Longitude must be between -180 and 180, got {}", lon),
            )
            .into_response();
        }
    }

    let entity = Entity {
        entity_id: body.id,
        entity_type: body.entity_type,
        name: body.name,
        properties: body.properties.unwrap_or_default(),
        geometry: body.geometry.map(|g| GeoPoint {
            lat: g.coordinates[1],
            lon: g.coordinates[0],
            alt: None,
        }),
        confidence: body.confidence.unwrap_or(1.0),
        ..Entity::default()
    };

    match state.storage.insert_entity(&entity).await {
        Ok(()) => {
            audit_log(
                state.as_ref(),
                "entity_created",
                Some(&entity.entity_type),
                Some(&entity.entity_id),
                &auth.subject,
                serde_json::json!({"entity_id": entity.entity_id}),
            )
            .await;
            // Emit broadcast event for WebSocket clients
            let _ = state.broadcast_tx.send(BroadcastEvent::EntityCreated {
                entity_id: entity.entity_id.clone(),
                entity_type: entity.entity_type.clone(),
                entity_name: entity.name.clone(),
                properties: serde_json::to_value(&entity.properties).unwrap_or_default(),
                geometry: entity
                    .geometry
                    .as_ref()
                    .map(|g| serde_json::json!({"type": "Point", "coordinates": [g.lon, g.lat]})),
                timestamp: chrono::Utc::now().to_rfc3339(),
            });
            (StatusCode::CREATED, Json(entity_to_response(&entity))).into_response()
        }
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

pub async fn get_entity(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:read", "entity", &id) {
        return resp.into_response();
    }

    match state.storage.get_entity(&id).await {
        Ok(Some(entity)) => Json(entity_to_response(&entity)).into_response(),
        Ok(None) => error_response(
            "ENTITY_NOT_FOUND",
            StatusCode::NOT_FOUND,
            &format!("Entity with id '{}' not found", id),
        )
        .into_response(),
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

pub async fn update_entity(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:write", "entity", &id) {
        return resp.into_response();
    }

    match state.storage.get_entity(&id).await {
        Ok(Some(mut entity)) => {
            if let Some(props) = body.get("properties").and_then(|p| p.as_object()) {
                for (k, v) in props {
                    entity.properties.insert(k.clone(), v.clone());
                }
            }
            if let Some(name) = body.get("name").and_then(|n| n.as_str()) {
                entity.name = Some(name.to_string());
            }
            if let Some(geo) = body.get("geometry") {
                if let Some(coords) = geo.get("coordinates").and_then(|c| c.as_array()) {
                    if coords.len() == 2 {
                        let lon = coords[0].as_f64().unwrap_or(0.0);
                        let lat = coords[1].as_f64().unwrap_or(0.0);
                        if !(-90.0..=90.0).contains(&lat) {
                            return error_response(
                                "VALIDATION_ERROR",
                                StatusCode::BAD_REQUEST,
                                &format!("Latitude must be between -90 and 90, got {}", lat),
                            )
                            .into_response();
                        }
                        if !(-180.0..=180.0).contains(&lon) {
                            return error_response(
                                "VALIDATION_ERROR",
                                StatusCode::BAD_REQUEST,
                                &format!("Longitude must be between -180 and 180, got {}", lon),
                            )
                            .into_response();
                        }
                        entity.geometry = Some(GeoPoint {
                            lat,
                            lon,
                            alt: None,
                        });
                    }
                }
            }
            entity.last_updated = chrono::Utc::now();

            match state.storage.insert_entity(&entity).await {
                Ok(()) => {
                    audit_log(
                        state.as_ref(),
                        "entity_updated",
                        Some(&entity.entity_type),
                        Some(&entity.entity_id),
                        &auth.subject,
                        serde_json::json!({"entity_id": entity.entity_id}),
                    )
                    .await;
                    // Emit broadcast event
                    let _ = state.broadcast_tx.send(BroadcastEvent::EntityUpdate {
                        entity_id: entity.entity_id.clone(),
                        entity_type: entity.entity_type.clone(),
                        changes: serde_json::to_value(&entity.properties).unwrap_or_default(),
                        geometry: entity.geometry.as_ref().map(
                            |g| serde_json::json!({"type": "Point", "coordinates": [g.lon, g.lat]}),
                        ),
                        timestamp: chrono::Utc::now().to_rfc3339(),
                    });
                    Json(entity_to_response(&entity)).into_response()
                }
                Err(e) => error_response(
                    "INTERNAL_ERROR",
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &e.to_string(),
                )
                .into_response(),
            }
        }
        Ok(None) => error_response(
            "ENTITY_NOT_FOUND",
            StatusCode::NOT_FOUND,
            &format!("Entity with id '{}' not found", id),
        )
        .into_response(),
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

pub async fn delete_entity(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:delete", "entity", &id) {
        return resp.into_response();
    }

    match state.storage.get_entity(&id).await {
        Ok(Some(deleted_entity)) => match state.storage.delete_entity(&id).await {
            Ok(()) => {
                audit_log(
                    state.as_ref(),
                    "entity_deleted",
                    Some("entity"),
                    Some(&id),
                    &auth.subject,
                    serde_json::json!({"entity_id": id}),
                )
                .await;
                // Emit broadcast event
                let _ = state.broadcast_tx.send(BroadcastEvent::EntityDeleted {
                    entity_id: id.clone(),
                    entity_type: deleted_entity.entity_type.clone(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                });
                StatusCode::NO_CONTENT.into_response()
            }
            Err(e) => error_response(
                "INTERNAL_ERROR",
                StatusCode::INTERNAL_SERVER_ERROR,
                &e.to_string(),
            )
            .into_response(),
        },
        Ok(None) => error_response(
            "ENTITY_NOT_FOUND",
            StatusCode::NOT_FOUND,
            &format!("Entity with id '{}' not found", id),
        )
        .into_response(),
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

#[derive(Deserialize)]
pub struct SearchParams {
    #[serde(rename = "type")]
    entity_type: Option<String>,
    near: Option<String>,
    text_search: Option<String>,
    limit: Option<usize>,
}

pub async fn search_entities(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:read", "entity", "*") {
        return resp.into_response();
    }

    let limit = params.limit.unwrap_or(100).min(1000);
    let start = std::time::Instant::now();

    // Parse "near" parameter: "lat,lon,radius_km"
    if let Some(ref near) = params.near {
        let parts: Vec<&str> = near.split(',').collect();
        if parts.len() != 3 {
            return error_response(
                "VALIDATION_ERROR",
                StatusCode::BAD_REQUEST,
                "Malformed 'near' parameter. Expected format: lat,lon,radius_km",
            )
            .into_response();
        }
        let lat = parts[0].parse::<f64>();
        let lon = parts[1].parse::<f64>();
        let radius = parts[2].parse::<f64>();
        match (lat, lon, radius) {
            (Ok(lat), Ok(lon), Ok(radius)) => {
                if !(-90.0..=90.0).contains(&lat)
                    || !(-180.0..=180.0).contains(&lon)
                    || radius < 0.0
                {
                    return error_response(
                        "VALIDATION_ERROR",
                        StatusCode::BAD_REQUEST,
                        "Invalid 'near' values. lat must be [-90,90], lon [-180,180], radius >= 0",
                    )
                    .into_response();
                }
                match state
                    .storage
                    .get_entities_in_radius(lat, lon, radius, params.entity_type.as_deref())
                    .await
                {
                    Ok(entities) => {
                        let data: Vec<EntityResponse> = entities
                            .iter()
                            .take(limit)
                            .map(entity_to_response)
                            .collect();
                        let search_time = start.elapsed().as_secs_f64() * 1000.0;
                        return Json(serde_json::json!({
                            "data": data,
                            "count": data.len(),
                            "search_time_ms": search_time,
                        }))
                        .into_response();
                    }
                    Err(e) => {
                        return error_response(
                            "INTERNAL_ERROR",
                            StatusCode::INTERNAL_SERVER_ERROR,
                            &e.to_string(),
                        )
                        .into_response();
                    }
                }
            }
            _ => {
                return error_response(
                    "VALIDATION_ERROR",
                    StatusCode::BAD_REQUEST,
                    "Malformed 'near' parameter. lat, lon, and radius_km must be valid numbers.",
                )
                .into_response();
            }
        }
    }

    // Text search
    let search_query = params.text_search.as_deref().unwrap_or("");
    match state
        .storage
        .search_entities(search_query, params.entity_type.as_deref(), limit)
        .await
    {
        Ok(entities) => {
            let data: Vec<EntityResponse> = entities.iter().map(entity_to_response).collect();
            let search_time = start.elapsed().as_secs_f64() * 1000.0;
            Json(serde_json::json!({
                "data": data,
                "count": data.len(),
                "search_time_ms": search_time,
            }))
            .into_response()
        }
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

pub async fn get_entity_relationships(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "graph:read", "relationship", &id) {
        return resp.into_response();
    }

    match state.storage.get_relationships_for_entity(&id).await {
        Ok(rels) => {
            let outgoing: Vec<_> = rels
                .iter()
                .filter(|r| r.source_entity_id == id)
                .map(|r| {
                    serde_json::json!({
                        "id": r.relationship_id,
                        "type": r.relationship_type,
                        "target_id": r.target_entity_id,
                        "properties": r.properties,
                        "confidence": r.confidence,
                    })
                })
                .collect();
            let incoming: Vec<_> = rels
                .iter()
                .filter(|r| r.target_entity_id == id)
                .map(|r| {
                    serde_json::json!({
                        "id": r.relationship_id,
                        "type": r.relationship_type,
                        "source_id": r.source_entity_id,
                        "properties": r.properties,
                        "confidence": r.confidence,
                    })
                })
                .collect();
            Json(serde_json::json!({
                "entity_id": id,
                "outgoing": outgoing,
                "incoming": incoming,
                "total": outgoing.len() + incoming.len(),
            }))
            .into_response()
        }
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

#[derive(Deserialize)]
pub struct EventsParams {
    limit: Option<usize>,
    #[serde(rename = "type")]
    event_type: Option<String>,
}

pub async fn get_entity_events(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<EventsParams>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:read", "event", &id) {
        return resp.into_response();
    }

    let limit = params.limit.unwrap_or(100).min(1000);
    match state.storage.get_events_for_entity(&id, limit).await {
        Ok(events) => {
            let data: Vec<_> = events
                .iter()
                .filter(|e| {
                    params
                        .event_type
                        .as_ref()
                        .is_none_or(|t| &e.event_type == t)
                })
                .map(|e| {
                    serde_json::json!({
                        "id": e.event_id,
                        "entity_id": e.entity_id,
                        "event_type": e.event_type,
                        "timestamp": e.event_timestamp.to_rfc3339(),
                        "source_id": e.source_id,
                        "data": e.data,
                        "confidence": e.confidence,
                    })
                })
                .collect();
            Json(serde_json::json!({
                "data": data,
                "count": data.len(),
            }))
            .into_response()
        }
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

// ---- Events (global) — Missing Endpoint #13 ----

#[derive(Deserialize)]
pub struct GlobalEventsParams {
    entity_id: Option<String>,
    entity_type: Option<String>,
    event_type: Option<String>,
    since: Option<String>,
    until: Option<String>,
    page: Option<usize>,
    limit: Option<usize>,
}

pub async fn list_events_global(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Query(params): Query<GlobalEventsParams>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:read", "event", "*") {
        return resp.into_response();
    }

    let page = params.page.unwrap_or(1).max(1);
    let limit = params.limit.unwrap_or(100).min(1000);
    let offset = (page - 1) * limit;

    let since = params
        .since
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));
    let until = params
        .until
        .as_deref()
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|dt| dt.with_timezone(&chrono::Utc));

    match state
        .storage
        .get_events_global(
            params.entity_id.as_deref(),
            params.entity_type.as_deref(),
            params.event_type.as_deref(),
            since,
            until,
            limit,
            offset,
        )
        .await
    {
        Ok(events) => {
            let total = state
                .storage
                .count_events_global(
                    params.entity_id.as_deref(),
                    params.entity_type.as_deref(),
                    params.event_type.as_deref(),
                    since,
                    until,
                )
                .await
                .unwrap_or(0);
            let total_pages = if limit > 0 {
                (total as f64 / limit as f64).ceil() as u64
            } else {
                0
            };
            let data: Vec<_> = events
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "id": e.event_id,
                        "entity_id": e.entity_id,
                        "event_type": e.event_type,
                        "timestamp": e.event_timestamp.to_rfc3339(),
                        "source_id": e.source_id,
                        "data": e.data,
                        "confidence": e.confidence,
                    })
                })
                .collect();
            Json(serde_json::json!({
                "data": data,
                "pagination": {
                    "page": page,
                    "limit": limit,
                    "total_count": total,
                    "total_pages": total_pages,
                    "has_next": (page as u64) < total_pages,
                    "has_prev": page > 1,
                }
            }))
            .into_response()
        }
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

// ---- Relationships ----

#[derive(Deserialize)]
pub struct CreateRelationshipRequest {
    source_id: String,
    target_id: String,
    #[serde(rename = "type")]
    rel_type: String,
    properties: Option<HashMap<String, serde_json::Value>>,
    confidence: Option<f64>,
}

pub async fn create_relationship(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateRelationshipRequest>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(
        &state.abac_engine,
        &auth,
        "graph:write",
        "relationship",
        "*",
    ) {
        return resp.into_response();
    }

    let rel = Relationship {
        relationship_id: uuid::Uuid::new_v4().to_string(),
        source_entity_id: body.source_id,
        target_entity_id: body.target_id,
        relationship_type: body.rel_type,
        properties: body.properties.unwrap_or_default(),
        confidence: body.confidence.unwrap_or(1.0),
        is_active: true,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    match state.storage.insert_relationship(&rel).await {
        Ok(()) => {
            audit_log(
                state.as_ref(),
                "relationship_created",
                Some("relationship"),
                Some(&rel.relationship_id),
                &auth.subject,
                serde_json::json!({
                    "source": rel.source_entity_id,
                    "target": rel.target_entity_id,
                    "type": rel.relationship_type,
                }),
            )
            .await;
            // Emit broadcast event
            let _ = state
                .broadcast_tx
                .send(BroadcastEvent::RelationshipChanged {
                    relationship_id: rel.relationship_id.clone(),
                    source_id: rel.source_entity_id.clone(),
                    target_id: rel.target_entity_id.clone(),
                    relationship_type: rel.relationship_type.clone(),
                    event: "created".to_string(),
                    timestamp: chrono::Utc::now().to_rfc3339(),
                });
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "id": rel.relationship_id,
                    "source_id": rel.source_entity_id,
                    "target_id": rel.target_entity_id,
                    "type": rel.relationship_type,
                    "properties": rel.properties,
                    "confidence": rel.confidence,
                })),
            )
                .into_response()
        }
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

// ---- Query ----

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct QueryRequest {
    query: String,
    timeout_ms: Option<u64>,
    limit: Option<usize>,
}

pub async fn execute_query(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Json(body): Json<QueryRequest>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "query:execute", "query", "*") {
        return resp.into_response();
    }

    if body.query.trim().is_empty() {
        return error_response(
            "INVALID_QUERY",
            StatusCode::BAD_REQUEST,
            "Query string cannot be empty",
        )
        .into_response();
    }

    audit_log(
        state.as_ref(),
        "query_executed",
        None,
        None,
        &auth.subject,
        serde_json::json!({"query": body.query}),
    )
    .await;

    match state.query_executor.execute(&body.query).await {
        Ok(result) => Json(serde_json::json!({
            "status": "success",
            "results": result.rows,
            "columns": result.columns,
            "metadata": {
                "execution_time_ms": result.execution_time_ms,
                "rows_returned": result.row_count,
                "limit": body.limit,
            }
        }))
        .into_response(),
        Err(e) => {
            error_response("INVALID_QUERY", StatusCode::BAD_REQUEST, &e.to_string()).into_response()
        }
    }
}

pub async fn graph_query(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "graph:read", "graph", "*") {
        return resp.into_response();
    }

    let query = body.get("query").and_then(|q| q.as_str()).unwrap_or("");
    if query.is_empty() {
        return error_response(
            "INVALID_QUERY",
            StatusCode::BAD_REQUEST,
            "Graph query string cannot be empty",
        )
        .into_response();
    }

    let start = std::time::Instant::now();
    match state.storage.graph_query(query).await {
        Ok(results) => {
            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            Json(serde_json::json!({
                "status": "success",
                "results": results,
                "metadata": {
                    "execution_time_ms": elapsed,
                    "rows_returned": results.len(),
                }
            }))
            .into_response()
        }
        // QueryError = unsupported / malformed cypher (caller's fault, 400).
        // Anything else is a backend failure (500).
        Err(e @ orp_storage::StorageError::QueryError(_)) => {
            error_response("INVALID_QUERY", StatusCode::BAD_REQUEST, &e.to_string()).into_response()
        }
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

// ---- Connectors ----

pub async fn list_connectors(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(
        &state.abac_engine,
        &auth,
        "connectors:manage",
        "connector",
        "*",
    ) {
        return resp.into_response();
    }

    match state.storage.get_data_sources().await {
        Ok(sources) => Json(serde_json::json!({
            "data": sources,
            "count": sources.len(),
        }))
        .into_response(),
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

#[derive(Deserialize)]
#[allow(dead_code)]
pub struct CreateConnectorRequest {
    name: String,
    connector_type: String,
    url: Option<String>,
    entity_type: String,
    trust_score: Option<f64>,
}

pub async fn create_connector(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateConnectorRequest>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(
        &state.abac_engine,
        &auth,
        "connectors:manage",
        "connector",
        "*",
    ) {
        return resp.into_response();
    }

    let source = orp_proto::DataSource {
        source_id: format!("{}-{}", body.connector_type, uuid::Uuid::new_v4()),
        source_name: body.name,
        source_type: body.connector_type,
        trust_score: body.trust_score.unwrap_or(0.8),
        events_ingested: 0,
        enabled: true,
    };

    match state.storage.register_data_source(&source).await {
        Ok(()) => {
            audit_log(
                state.as_ref(),
                "connector_created",
                Some("connector"),
                Some(&source.source_id),
                &auth.subject,
                serde_json::json!({"source_id": source.source_id}),
            )
            .await;
            (
                StatusCode::CREATED,
                Json(serde_json::json!({
                    "data": source,
                })),
            )
                .into_response()
        }
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

/// PUT /api/v1/connectors/{id} — Missing Endpoint #13
#[derive(Deserialize)]
pub struct UpdateConnectorRequest {
    name: Option<String>,
    connector_type: Option<String>,
    trust_score: Option<f64>,
    enabled: Option<bool>,
}

pub async fn update_connector(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<UpdateConnectorRequest>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(
        &state.abac_engine,
        &auth,
        "connectors:manage",
        "connector",
        &id,
    ) {
        return resp.into_response();
    }

    match state.storage.get_data_source(&id).await {
        Ok(Some(mut source)) => {
            if let Some(name) = body.name {
                source.source_name = name;
            }
            if let Some(ct) = body.connector_type {
                source.source_type = ct;
            }
            if let Some(ts) = body.trust_score {
                source.trust_score = ts as f32;
            }
            if let Some(en) = body.enabled {
                source.enabled = en;
            }
            match state.storage.update_data_source(&source).await {
                Ok(_) => {
                    audit_log(
                        state.as_ref(),
                        "connector_updated",
                        Some("connector"),
                        Some(&id),
                        &auth.subject,
                        serde_json::json!({"source_id": id}),
                    )
                    .await;
                    Json(serde_json::json!({"data": source})).into_response()
                }
                Err(e) => error_response(
                    "INTERNAL_ERROR",
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &e.to_string(),
                )
                .into_response(),
            }
        }
        Ok(None) => error_response(
            "CONNECTOR_NOT_FOUND",
            StatusCode::NOT_FOUND,
            &format!("Connector '{}' not found", id),
        )
        .into_response(),
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

/// DELETE /api/v1/connectors/{id} — Missing Endpoint #13
pub async fn delete_connector(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(
        &state.abac_engine,
        &auth,
        "connectors:manage",
        "connector",
        &id,
    ) {
        return resp.into_response();
    }

    match state.storage.delete_data_source(&id).await {
        Ok(true) => {
            audit_log(
                state.as_ref(),
                "connector_deleted",
                Some("connector"),
                Some(&id),
                &auth.subject,
                serde_json::json!({"source_id": id}),
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Ok(false) => error_response(
            "CONNECTOR_NOT_FOUND",
            StatusCode::NOT_FOUND,
            &format!("Connector '{}' not found", id),
        )
        .into_response(),
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

// ---- Media streams ----

fn media_registry_error(err: MediaRegistryError) -> (StatusCode, Json<ErrorResponse>) {
    match err {
        MediaRegistryError::Validation(msg) => {
            error_response("VALIDATION_ERROR", StatusCode::BAD_REQUEST, &msg)
        }
        MediaRegistryError::Conflict(msg) => error_response("CONFLICT", StatusCode::CONFLICT, &msg),
        MediaRegistryError::NotFound(msg) => {
            error_response("MEDIA_STREAM_NOT_FOUND", StatusCode::NOT_FOUND, &msg)
        }
    }
}

fn media_stream_entity(stream: &MediaStream) -> Entity {
    Entity {
        entity_id: stream.entity_id(),
        entity_type: "media_stream".to_string(),
        name: Some(stream.name.clone()),
        properties: stream.entity_properties(),
        confidence: 0.95,
        ..Entity::default()
    }
}

pub async fn list_media_streams(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:read", "media", "*") {
        return resp.into_response();
    }

    let streams = state.media_registry.list().await;
    Json(serde_json::json!({
        "data": streams,
        "count": streams.len(),
    }))
    .into_response()
}

pub async fn create_media_stream(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateMediaStreamInput>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:write", "media", "*") {
        return resp.into_response();
    }

    let stream = match state.media_registry.create(body).await {
        Ok(stream) => stream,
        Err(err) => return media_registry_error(err).into_response(),
    };

    let entity = media_stream_entity(&stream);
    if let Err(e) = state.storage.insert_entity(&entity).await {
        let _ = state.media_registry.delete(&stream.id).await;
        return error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response();
    }

    audit_log(
        state.as_ref(),
        "media_stream_created",
        Some("media_stream"),
        Some(&stream.entity_id()),
        &auth.subject,
        serde_json::json!({
            "stream_id": stream.id.clone(),
            "protocol": stream.protocol.as_str(),
            "source_url": stream.source_url_redacted.clone(),
        }),
    )
    .await;

    (
        StatusCode::CREATED,
        Json(serde_json::json!({ "data": stream })),
    )
        .into_response()
}

pub async fn get_media_stream(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = validate_media_id(&id) {
        return resp.into_response();
    }
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:read", "media", &id) {
        return resp.into_response();
    }

    match state.media_registry.get(&id).await {
        Some(stream) => Json(serde_json::json!({ "data": stream })).into_response(),
        None => media_registry_error(MediaRegistryError::NotFound(format!(
            "media stream '{id}' not found"
        )))
        .into_response(),
    }
}

/// Stream-id validator on the read path — catches `..%2f`-decoded segments
/// and any other character set we don't accept on create. Identical to the
/// rule applied in `MediaRegistry::create`, lifted here so the read handlers
/// reject the same shapes (closes audit finding H3).
fn validate_media_id(id: &str) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    if id.is_empty() || id.len() > 96 {
        return Err(error_response(
            "VALIDATION_ERROR",
            StatusCode::BAD_REQUEST,
            "media stream id must be 1-96 characters",
        ));
    }
    if !id
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(error_response(
            "VALIDATION_ERROR",
            StatusCode::BAD_REQUEST,
            "media stream id may contain only ASCII letters, digits, '-' and '_'",
        ));
    }
    Ok(())
}

pub async fn list_media_stats(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:read", "media", "*") {
        return resp.into_response();
    }
    let rows = state.media_registry.list_with_stats().await;
    Json(serde_json::json!({
        "data": rows,
        "count": rows.len(),
    }))
    .into_response()
}

/// Build a 502 with a generic error code — the underlying reqwest error is
/// logged but never surfaced verbatim because reqwest's `Display` includes
/// the full URL with credentials when DNS or connect fails.
fn upstream_error(
    stream_id: &str,
    ctx: &str,
    err: impl std::fmt::Display,
) -> (StatusCode, Json<ErrorResponse>) {
    tracing::warn!(stream_id = %stream_id, error = %err, "media relay {ctx}");
    error_response(
        "MEDIA_RELAY_UPSTREAM",
        StatusCode::BAD_GATEWAY,
        &format!("media upstream {ctx}"),
    )
}

async fn fetch_media_stream_response(
    handle: Arc<MediaStreamHandle>,
    target_url: &str,
    relay_semaphore: tokio::sync::OwnedSemaphorePermit,
) -> Result<Response, (StatusCode, Json<ErrorResponse>)> {
    let stream = &handle.stream;
    validate_relay_target(stream, target_url).map_err(media_relay_error)?;
    let (client, _) = build_safe_client(target_url, stream.allow_private_network)
        .map_err(|e| error_response("MEDIA_RELAY_BLOCKED", StatusCode::BAD_REQUEST, &e))?;
    let upstream = client
        .get(target_url)
        .send()
        .await
        .map_err(|e| upstream_error(&stream.id, "request failed", e))?;
    let status = upstream.status();
    if !status.is_success() {
        handle
            .stats
            .errors
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return Err(error_response(
            "MEDIA_RELAY_UPSTREAM",
            StatusCode::BAD_GATEWAY,
            &format!("media upstream returned HTTP {}", status.as_u16()),
        ));
    }

    let content_type = upstream.headers().get(header::CONTENT_TYPE).cloned();
    let cache_control = upstream.headers().get(header::CACHE_CONTROL).cloned();

    // Channel of size 8: gives the slow-downstream-fast-upstream pair a
    // tiny buffer for prefetch but lets backpressure propagate (when the
    // channel is full the relay task awaits the downstream client before
    // pulling the next chunk from the upstream socket).
    let (tx, rx) = tokio::sync::mpsc::channel::<Result<Bytes, std::io::Error>>(8);
    let cancel = handle.cancel.clone();
    let handle_for_task = Arc::clone(&handle);

    handle
        .stats
        .active_sessions
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    handle
        .stats
        .total_sessions
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

    tokio::spawn(async move {
        // Permit is released when this task completes — i.e. when upstream
        // EOFs, the client disconnects, the cancel token fires (DELETE), or
        // any relay-side cap kicks in.
        let _permit = relay_semaphore;
        let stats = &handle_for_task.stats;
        let mut total_bytes: u64 = 0;
        let mut upstream_stream = upstream.bytes_stream();

        loop {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => {
                    // DELETE on the parent stream — drop everything.
                    break;
                }
                next = tokio::time::timeout(RELAY_IDLE_TIMEOUT, upstream_stream.next()) => {
                    match next {
                        Err(_elapsed) => {
                            stats.errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let _ = tx
                                .send(Err(std::io::Error::new(
                                    std::io::ErrorKind::TimedOut,
                                    "media relay idle timeout",
                                )))
                                .await;
                            break;
                        }
                        Ok(None) => break, // upstream EOF — clean tear-down
                        Ok(Some(Err(_e))) => {
                            stats.errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let _ = tx
                                .send(Err(std::io::Error::other("media upstream stream error")))
                                .await;
                            break;
                        }
                        Ok(Some(Ok(chunk))) => {
                            let n = chunk.len() as u64;
                            total_bytes = total_bytes.saturating_add(n);
                            stats.bytes_relayed.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
                            *stats.last_activity.write().await = Some(Utc::now());
                            if total_bytes > RELAY_BYTE_CAP {
                                stats.errors.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                let _ = tx
                                    .send(Err(std::io::Error::other(
                                        "media relay session byte cap exceeded",
                                    )))
                                    .await;
                                break;
                            }
                            // tx.send fails iff downstream went away;
                            // exit cleanly without bumping `errors`.
                            if tx.send(Ok(chunk)).await.is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        }

        stats
            .active_sessions
            .fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    });

    let body = Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx));
    let mut builder = Response::builder().status(status);
    if let Some(value) = content_type {
        builder = builder.header(header::CONTENT_TYPE, value);
    }
    if let Some(value) = cache_control {
        builder = builder.header(header::CACHE_CONTROL, value);
    }
    builder.body(body).map_err(|e| {
        error_response(
            "MEDIA_RELAY_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("failed to build media relay response: {e}"),
        )
    })
}

/// Fetch a bounded text body (HLS playlist). Caps total bytes at
/// [`HLS_PLAYLIST_MAX_BYTES`] before we stringify — refuses 5GB MPEGURL
/// payloads from a hostile origin.
async fn fetch_media_text(
    handle: &MediaStreamHandle,
    target_url: &str,
) -> Result<String, (StatusCode, Json<ErrorResponse>)> {
    let stream = &handle.stream;
    validate_relay_target(stream, target_url).map_err(media_relay_error)?;
    let (client, _) = build_safe_client(target_url, stream.allow_private_network)
        .map_err(|e| error_response("MEDIA_RELAY_BLOCKED", StatusCode::BAD_REQUEST, &e))?;
    let upstream = client
        .get(target_url)
        .send()
        .await
        .map_err(|e| upstream_error(&stream.id, "request failed", e))?;
    let status = upstream.status();
    if !status.is_success() {
        handle
            .stats
            .errors
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        return Err(error_response(
            "MEDIA_RELAY_UPSTREAM",
            StatusCode::BAD_GATEWAY,
            &format!("media upstream returned HTTP {}", status.as_u16()),
        ));
    }

    // Reject hostile Content-Length up front when present.
    if let Some(len) = upstream.content_length() {
        if len as usize > HLS_PLAYLIST_MAX_BYTES {
            handle
                .stats
                .errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(error_response(
                "MEDIA_RELAY_BLOCKED",
                StatusCode::BAD_REQUEST,
                "HLS playlist exceeds size ceiling",
            ));
        }
    }

    let mut accumulator = Vec::with_capacity(8 * 1024);
    let mut stream_iter = upstream.bytes_stream();
    while let Some(chunk) = stream_iter.next().await {
        let chunk = chunk.map_err(|e| upstream_error(&stream.id, "stream error", e))?;
        if accumulator.len().saturating_add(chunk.len()) > HLS_PLAYLIST_MAX_BYTES {
            handle
                .stats
                .errors
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return Err(error_response(
                "MEDIA_RELAY_BLOCKED",
                StatusCode::BAD_REQUEST,
                "HLS playlist exceeds size ceiling",
            ));
        }
        accumulator.extend_from_slice(&chunk);
    }
    String::from_utf8(accumulator).map_err(|_| {
        error_response(
            "MEDIA_RELAY_UPSTREAM",
            StatusCode::BAD_GATEWAY,
            "HLS playlist body was not valid UTF-8",
        )
    })
}

fn media_relay_error(err: MediaRegistryError) -> (StatusCode, Json<ErrorResponse>) {
    match err {
        MediaRegistryError::Validation(msg) if msg.contains("not implemented yet") => {
            error_response("MEDIA_RELAY_UNSUPPORTED", StatusCode::NOT_IMPLEMENTED, &msg)
        }
        MediaRegistryError::Validation(msg) => {
            error_response("MEDIA_RELAY_BLOCKED", StatusCode::BAD_REQUEST, &msg)
        }
        other => media_registry_error(other),
    }
}

pub async fn relay_media_stream(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = validate_media_id(&id) {
        return resp.into_response();
    }
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:read", "media", &id) {
        return resp.into_response();
    }

    let Some(handle) = state.media_registry.get_handle(&id).await else {
        return media_registry_error(MediaRegistryError::NotFound(format!(
            "media stream '{id}' not found"
        )))
        .into_response();
    };

    if handle.stream.protocol == MediaProtocol::Hls {
        return relay_hls_playlist_for_stream(handle.as_ref())
            .await
            .into_response();
    }

    let Ok(permit) = state.media_registry.relay_semaphore().try_acquire_owned() else {
        return error_response(
            "MEDIA_RELAY_BUSY",
            StatusCode::SERVICE_UNAVAILABLE,
            "media relay capacity exhausted; retry after active sessions release",
        )
        .into_response();
    };

    let target = handle.stream.source_url().to_string();
    match fetch_media_stream_response(handle, &target, permit).await {
        Ok(resp) => resp.into_response(),
        Err(resp) => resp.into_response(),
    }
}

async fn relay_hls_playlist_for_stream(handle: &MediaStreamHandle) -> Response {
    let stream = &handle.stream;
    match fetch_media_text(handle, stream.source_url())
        .await
        .and_then(|text| {
            rewrite_hls_playlist(&stream.id, stream.source_url(), &text).map_err(media_relay_error)
        }) {
        Ok(playlist) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
            playlist,
        )
            .into_response(),
        Err(resp) => resp.into_response(),
    }
}

pub async fn relay_hls_playlist(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = validate_media_id(&id) {
        return resp.into_response();
    }
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:read", "media", &id) {
        return resp.into_response();
    }

    let Some(handle) = state.media_registry.get_handle(&id).await else {
        return media_registry_error(MediaRegistryError::NotFound(format!(
            "media stream '{id}' not found"
        )))
        .into_response();
    };

    if handle.stream.protocol != MediaProtocol::Hls {
        return error_response(
            "MEDIA_RELAY_UNSUPPORTED",
            StatusCode::BAD_REQUEST,
            "playlist relay is only available for HLS media streams",
        )
        .into_response();
    }

    relay_hls_playlist_for_stream(handle.as_ref())
        .await
        .into_response()
}

#[derive(Deserialize)]
pub struct HlsFetchParams {
    url: String,
}

pub async fn relay_hls_asset(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<HlsFetchParams>,
) -> impl IntoResponse {
    if let Err(resp) = validate_media_id(&id) {
        return resp.into_response();
    }
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:read", "media", &id) {
        return resp.into_response();
    }

    // Reject obviously hostile asset URLs before parsing so the parser
    // doesn't waste cycles on a megabyte string.
    if params.url.len() > 2048 {
        return error_response(
            "MEDIA_RELAY_BLOCKED",
            StatusCode::BAD_REQUEST,
            "asset url exceeds 2 KiB",
        )
        .into_response();
    }

    let Some(handle) = state.media_registry.get_handle(&id).await else {
        return media_registry_error(MediaRegistryError::NotFound(format!(
            "media stream '{id}' not found"
        )))
        .into_response();
    };

    let Ok(permit) = state.media_registry.relay_semaphore().try_acquire_owned() else {
        return error_response(
            "MEDIA_RELAY_BUSY",
            StatusCode::SERVICE_UNAVAILABLE,
            "media relay capacity exhausted; retry after active sessions release",
        )
        .into_response();
    };

    match fetch_media_stream_response(handle, &params.url, permit).await {
        Ok(resp) => resp.into_response(),
        Err(resp) => resp.into_response(),
    }
}

pub async fn delete_media_stream(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = validate_media_id(&id) {
        return resp.into_response();
    }
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "entities:write", "media", &id) {
        return resp.into_response();
    }

    match state.media_registry.delete(&id).await {
        Ok(stream) => {
            if let Err(e) = state.storage.delete_entity(&stream.entity_id()).await {
                return error_response(
                    "INTERNAL_ERROR",
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &e.to_string(),
                )
                .into_response();
            }
            audit_log(
                state.as_ref(),
                "media_stream_deleted",
                Some("media_stream"),
                Some(&stream.entity_id()),
                &auth.subject,
                serde_json::json!({"stream_id": stream.id}),
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(err) => media_registry_error(err).into_response(),
    }
}

// ---- Monitors ----

pub async fn list_monitors(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "monitors:read", "monitor", "*") {
        return resp.into_response();
    }

    let rules = state.monitor_engine.get_rules().await;
    Json(serde_json::json!({
        "data": rules,
        "count": rules.len(),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct CreateMonitorRequest {
    name: String,
    description: Option<String>,
    entity_type: String,
    condition: MonitorConditionRequest,
    severity: Option<String>,
    cooldown_seconds: Option<u64>,
    enabled: Option<bool>,
}

#[derive(Deserialize)]
#[serde(tag = "type")]
pub enum MonitorConditionRequest {
    #[serde(rename = "property_threshold")]
    PropertyThreshold {
        property: String,
        operator: String,
        value: f64,
    },
    #[serde(rename = "geofence")]
    Geofence { lat: f64, lon: f64, radius_km: f64 },
    #[serde(rename = "stale")]
    Stale { max_age_seconds: u64 },
    #[serde(rename = "speed_anomaly")]
    SpeedAnomaly { max_change_knots: f64 },
}

fn parse_threshold_op(s: &str) -> ThresholdOp {
    match s {
        ">" | "gt" => ThresholdOp::GreaterThan,
        "<" | "lt" => ThresholdOp::LessThan,
        ">=" | "gte" => ThresholdOp::GreaterThanOrEqual,
        "<=" | "lte" => ThresholdOp::LessThanOrEqual,
        "=" | "eq" => ThresholdOp::Equal,
        "!=" | "neq" => ThresholdOp::NotEqual,
        _ => ThresholdOp::GreaterThan,
    }
}

fn parse_severity(s: &str) -> AlertSeverity {
    match s.to_lowercase().as_str() {
        "critical" => AlertSeverity::Critical,
        "warning" => AlertSeverity::Warning,
        _ => AlertSeverity::Info,
    }
}

fn build_monitor_rule(body: CreateMonitorRequest) -> MonitorRule {
    let condition = match body.condition {
        MonitorConditionRequest::PropertyThreshold {
            property,
            operator,
            value,
        } => MonitorCondition::PropertyThreshold {
            property,
            operator: parse_threshold_op(&operator),
            value,
        },
        MonitorConditionRequest::Geofence {
            lat,
            lon,
            radius_km,
        } => MonitorCondition::Geofence {
            lat,
            lon,
            radius_km,
            trigger_on: GeofenceTrigger::Both,
        },
        MonitorConditionRequest::Stale { max_age_seconds } => {
            MonitorCondition::Stale { max_age_seconds }
        }
        MonitorConditionRequest::SpeedAnomaly { max_change_knots } => {
            MonitorCondition::SpeedAnomaly { max_change_knots }
        }
    };

    MonitorRule {
        rule_id: format!("rule-{}", uuid::Uuid::new_v4()),
        name: body.name,
        description: body.description.unwrap_or_default(),
        entity_type: body.entity_type,
        condition,
        action: MonitorAction::Alert,
        enabled: body.enabled.unwrap_or(true),
        cooldown_seconds: body.cooldown_seconds.unwrap_or(300),
        severity: parse_severity(body.severity.as_deref().unwrap_or("info")),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    }
}

pub async fn create_monitor(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateMonitorRequest>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "monitors:write", "monitor", "*") {
        return resp.into_response();
    }

    let rule = build_monitor_rule(body);
    state.monitor_engine.add_rule(rule.clone()).await;

    audit_log(
        state.as_ref(),
        "monitor_created",
        Some("monitor"),
        Some(&rule.rule_id),
        &auth.subject,
        serde_json::json!({"rule_id": rule.rule_id, "name": rule.name}),
    )
    .await;

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "data": rule,
        })),
    )
        .into_response()
}

pub async fn get_monitor(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "monitors:read", "monitor", &id) {
        return resp.into_response();
    }

    match state.monitor_engine.get_rule(&id).await {
        Some(rule) => Json(serde_json::json!({"data": rule})).into_response(),
        None => error_response(
            "MONITOR_NOT_FOUND",
            StatusCode::NOT_FOUND,
            &format!("Monitor rule '{}' not found", id),
        )
        .into_response(),
    }
}

/// PUT /api/v1/monitors/{id} — Missing Endpoint #13
pub async fn update_monitor(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<CreateMonitorRequest>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "monitors:write", "monitor", &id) {
        return resp.into_response();
    }

    // Remove old, add new with same ID
    let _ = state.monitor_engine.remove_rule(&id).await;

    let mut rule = build_monitor_rule(body);
    rule.rule_id = id.clone();

    state.monitor_engine.add_rule(rule.clone()).await;

    audit_log(
        state.as_ref(),
        "monitor_updated",
        Some("monitor"),
        Some(&id),
        &auth.subject,
        serde_json::json!({"rule_id": id}),
    )
    .await;

    Json(serde_json::json!({"data": rule})).into_response()
}

pub async fn delete_monitor(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "monitors:write", "monitor", &id) {
        return resp.into_response();
    }

    if state.monitor_engine.remove_rule(&id).await {
        audit_log(
            state.as_ref(),
            "monitor_deleted",
            Some("monitor"),
            Some(&id),
            &auth.subject,
            serde_json::json!({"rule_id": id}),
        )
        .await;
        StatusCode::NO_CONTENT.into_response()
    } else {
        error_response(
            "MONITOR_NOT_FOUND",
            StatusCode::NOT_FOUND,
            &format!("Monitor rule '{}' not found", id),
        )
        .into_response()
    }
}

// ---- Alerts ----

#[derive(Deserialize)]
pub struct AlertsParams {
    limit: Option<usize>,
}

pub async fn list_alerts(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Query(params): Query<AlertsParams>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "monitors:read", "alert", "*") {
        return resp.into_response();
    }

    let limit = params.limit.unwrap_or(100).min(1000);
    let alerts = state.monitor_engine.get_alerts(limit).await;
    Json(serde_json::json!({
        "data": alerts,
        "count": alerts.len(),
    }))
    .into_response()
}

pub async fn acknowledge_alert(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "monitors:write", "alert", &id) {
        return resp.into_response();
    }

    if state.monitor_engine.acknowledge_alert(&id).await {
        audit_log(
            state.as_ref(),
            "alert_acknowledged",
            Some("alert"),
            Some(&id),
            &auth.subject,
            serde_json::json!({"alert_id": id}),
        )
        .await;
        Json(serde_json::json!({"status": "acknowledged"})).into_response()
    } else {
        error_response(
            "ALERT_NOT_FOUND",
            StatusCode::NOT_FOUND,
            &format!("Alert '{}' not found", id),
        )
        .into_response()
    }
}

// ---- API Keys ----

#[derive(Deserialize)]
pub struct CreateApiKeyRequestBody {
    name: String,
    scopes: Vec<String>,
    rate_limit: Option<u64>,
    expires_in: Option<i64>,
    org_id: Option<String>,
}

pub async fn create_api_key(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateApiKeyRequestBody>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "api-keys:manage", "api_key", "*") {
        return resp.into_response();
    }

    let req = orp_security::CreateApiKeyRequest {
        name: body.name,
        scopes: body.scopes,
        rate_limit: body.rate_limit,
        expires_in: body.expires_in,
        org_id: body.org_id,
    };

    match state.api_key_service.create_key(req) {
        Ok(resp) => {
            audit_log(
                state.as_ref(),
                "api_key_created",
                Some("api_key"),
                Some(&resp.id),
                &auth.subject,
                serde_json::json!({"key_id": resp.id, "name": resp.name}),
            )
            .await;
            (StatusCode::CREATED, Json(serde_json::json!(resp))).into_response()
        }
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

pub async fn list_api_keys(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "api-keys:manage", "api_key", "*") {
        return resp.into_response();
    }

    match state.api_key_service.list_keys() {
        Ok(keys) => {
            // Strip sensitive fields — don't expose key_hash
            let safe_keys: Vec<serde_json::Value> = keys
                .iter()
                .map(|k| {
                    serde_json::json!({
                        "id": k.id,
                        "name": k.name,
                        "scopes": k.scopes,
                        "rate_limit_per_second": k.rate_limit_per_second,
                        "expires_at": k.expires_at,
                        "is_revoked": k.is_revoked,
                        "org_id": k.org_id,
                        "created_at": k.created_at,
                        "last_used_at": k.last_used_at,
                    })
                })
                .collect();
            Json(serde_json::json!({
                "data": safe_keys,
                "count": safe_keys.len(),
            }))
            .into_response()
        }
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

pub async fn delete_api_key(
    auth: AuthContext,
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if let Err(resp) = check_abac(&state.abac_engine, &auth, "api-keys:manage", "api_key", &id) {
        return resp.into_response();
    }

    match state.api_key_service.revoke_key(&id) {
        Ok(()) => {
            audit_log(
                state.as_ref(),
                "api_key_revoked",
                Some("api_key"),
                Some(&id),
                &auth.subject,
                serde_json::json!({"key_id": id}),
            )
            .await;
            StatusCode::NO_CONTENT.into_response()
        }
        Err(orp_security::ApiKeyError::NotFound) => error_response(
            "API_KEY_NOT_FOUND",
            StatusCode::NOT_FOUND,
            &format!("API key '{}' not found", id),
        )
        .into_response(),
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

// Frontend is now served via ServeDir in http.rs (frontend/dist/)

// Ingest handlers have moved to server::ingest
// Federation handlers have moved to server::federation

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::http::{AppState, RateLimiter};
    use crate::server::websocket::BroadcastEvent;
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::{delete, get, post, put};
    use axum::Router;
    use orp_audit::crypto::EventSigner;
    use orp_query::QueryExecutor;
    use orp_security::{AbacEngine, ApiKeyService, AuthState};
    use orp_storage::DuckDbStorage;
    use orp_stream::{DefaultStreamProcessor, MonitorEngine, RocksDbDedupWindow, StreamProcessor};
    use std::sync::Arc;
    use tokio::sync::broadcast;
    use tower::ServiceExt;

    /// Build test app state with dev mode auth (permissive)
    async fn make_test_state() -> Arc<AppState> {
        let storage: Arc<dyn orp_storage::traits::Storage> =
            Arc::new(DuckDbStorage::new_in_memory().unwrap());

        let dedup_path =
            std::env::temp_dir().join(format!("orp-test-dedup-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dedup_path).ok();
        let dedup = Arc::new(RocksDbDedupWindow::open(&dedup_path, 3600).unwrap());
        let processor: Arc<dyn StreamProcessor> = Arc::new(DefaultStreamProcessor::new(
            storage.clone(),
            dedup,
            None,
            50,
        ));
        let query_executor = Arc::new(QueryExecutor::new(storage.clone()));
        let monitor_engine = Arc::new(MonitorEngine::new());
        let auth_state = Arc::new(AuthState {
            jwt_service: None,
            api_key_service: None,
            oidc_validator: None,
            permissive_mode: true,
        });
        let abac_engine = Arc::new(AbacEngine::default_permissive());
        let api_key_service = Arc::new(ApiKeyService::new());
        let audit_signer = Arc::new(EventSigner::new());
        let audit_log: Arc<dyn orp_audit::AuditLogger> = Arc::new(
            orp_audit::InMemoryAuditLog::with_signer(audit_signer.clone()),
        );
        let (broadcast_tx, _) = broadcast::channel::<BroadcastEvent>(256);

        Arc::new(AppState {
            storage,
            query_executor,
            processor,
            monitor_engine,
            auth_state,
            abac_engine,
            api_key_service,
            audit_signer,
            audit_log,
            broadcast_tx,
            started_at: std::time::Instant::now(),
            layer_registry: None,
            federation_registry: None,
            federation_outbox: None,
            connectors: Arc::new(tokio::sync::Mutex::new(Vec::new())),
            media_registry: Arc::new(crate::server::media::MediaRegistry::new()),
            local_node_id: "test-node".to_string(),
            federation_signer: Arc::new(crate::server::federation_tls::LocalSigner::ephemeral()),
            federation_seq: crate::server::federation_tls::OutboundSeq::new(),
            federation_replay: None,
        })
    }

    /// Build a test router with all routes and dev-mode auth injected
    async fn make_test_app() -> (Router, Arc<AppState>) {
        let state = make_test_state().await;

        // The AuthContext extractor needs AuthState in request extensions.
        // We inject it via a simple middleware layer.
        let state_for_middleware = state.clone();
        let app = Router::new()
            .route("/api/v1/health", get(health_check))
            .route("/api/v1/entities", get(list_entities))
            .route("/api/v1/entities", post(create_entity))
            .route("/api/v1/entities/search", get(search_entities))
            .route("/api/v1/entities/:id", get(get_entity))
            .route("/api/v1/entities/:id", put(update_entity))
            .route("/api/v1/entities/:id", delete(delete_entity))
            .route(
                "/api/v1/entities/:id/relationships",
                get(get_entity_relationships),
            )
            .route("/api/v1/entities/:id/events", get(get_entity_events))
            .route("/api/v1/relationships", post(create_relationship))
            .route("/api/v1/query", post(execute_query))
            .route("/api/v1/graph", post(graph_query))
            .route("/api/v1/connectors", get(list_connectors))
            .route("/api/v1/connectors", post(create_connector))
            .route("/api/v1/connectors/:id", put(update_connector))
            .route("/api/v1/connectors/:id", delete(delete_connector))
            .route("/api/v1/media/streams", get(list_media_streams))
            .route("/api/v1/media/streams", post(create_media_stream))
            .route("/api/v1/media/streams/:id", get(get_media_stream))
            .route("/api/v1/media/streams/:id", delete(delete_media_stream))
            .route("/api/v1/media/streams/:id/relay", get(relay_media_stream))
            .route(
                "/api/v1/media/streams/:id/playlist.m3u8",
                get(relay_hls_playlist),
            )
            .route("/api/v1/media/streams/:id/hls/fetch", get(relay_hls_asset))
            .route("/api/v1/monitors", get(list_monitors))
            .route("/api/v1/monitors", post(create_monitor))
            .route("/api/v1/monitors/:id", get(get_monitor))
            .route("/api/v1/monitors/:id", put(update_monitor))
            .route("/api/v1/monitors/:id", delete(delete_monitor))
            .route("/api/v1/alerts", get(list_alerts))
            .route("/api/v1/alerts/:id/acknowledge", post(acknowledge_alert))
            .route("/api/v1/events", get(list_events_global))
            .route("/api/v1/api-keys", post(create_api_key))
            .route("/api/v1/api-keys", get(list_api_keys))
            .route("/api/v1/api-keys/:id", delete(delete_api_key))
            .route("/api/v1/metrics", get(metrics))
            .layer(axum::middleware::from_fn(
                move |mut req: Request<Body>, next: axum::middleware::Next| {
                    let auth_state = state_for_middleware.auth_state.clone();
                    async move {
                        // Inject an admin AuthContext so all requests are authenticated
                        let admin_ctx = AuthContext {
                            subject: "test-admin".to_string(),
                            permissions: vec!["admin".to_string()],
                            email: Some("admin@test.orp".to_string()),
                            name: Some("Test Admin".to_string()),
                            org_id: None,
                            scopes: vec!["admin".to_string()],
                            auth_method: orp_security::middleware::AuthMethod::DevMode,
                        };
                        req.extensions_mut().insert(admin_ctx);
                        req.extensions_mut().insert(auth_state);
                        next.run(req).await
                    }
                },
            ))
            .with_state(state.clone());

        (app, state)
    }

    fn json_body(body: serde_json::Value) -> Body {
        Body::from(serde_json::to_string(&body).unwrap())
    }

    async fn spawn_media_origin() -> String {
        async fn snapshot() -> impl IntoResponse {
            (
                [(header::CONTENT_TYPE, "image/jpeg")],
                vec![0xff, 0xd8, 0xff, 0xd9],
            )
        }

        async fn playlist() -> impl IntoResponse {
            (
                [(header::CONTENT_TYPE, "application/vnd.apple.mpegurl")],
                "#EXTM3U\n#EXTINF:2.0,\nseg-1.ts\n".to_string(),
            )
        }

        async fn segment() -> impl IntoResponse {
            (
                [(header::CONTENT_TYPE, "video/mp2t")],
                vec![0x47, 0x40, 0x00, 0x10],
            )
        }

        let app = Router::new()
            .route("/snapshot.jpg", get(snapshot))
            .route("/index.m3u8", get(playlist))
            .route("/seg-1.ts", get(segment));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    // ── Health Endpoint ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_health_returns_200() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(Request::get("/api/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_health_response_has_components() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(Request::get("/api/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["status"], "healthy");
        assert!(json["components"].is_object());
        assert!(json["uptime_seconds"].is_number());
    }

    // ── Entity CRUD ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_entity_201() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({
            "id": "ship-test-1",
            "type": "ship",
            "name": "Test Ship",
            "confidence": 0.95,
        });
        let resp = app
            .oneshot(
                Request::post("/api/v1/entities")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_create_entity_empty_id_400() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({
            "id": "",
            "type": "ship",
        });
        let resp = app
            .oneshot(
                Request::post("/api/v1/entities")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_create_entity_duplicate_409() {
        let (app, state) = make_test_app().await;
        // Insert entity directly
        let entity = orp_proto::Entity {
            entity_id: "dup-1".to_string(),
            entity_type: "ship".to_string(),
            ..orp_proto::Entity::default()
        };
        state.storage.insert_entity(&entity).await.unwrap();

        let body = serde_json::json!({ "id": "dup-1", "type": "ship" });
        let resp = app
            .oneshot(
                Request::post("/api/v1/entities")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn test_create_entity_invalid_lat_400() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({
            "id": "ship-bad-geo",
            "type": "ship",
            "geometry": { "coordinates": [0.0, 999.0] },
        });
        let resp = app
            .oneshot(
                Request::post("/api/v1/entities")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_get_entity_200() {
        let (app, state) = make_test_app().await;
        let entity = orp_proto::Entity {
            entity_id: "ship-get-1".to_string(),
            entity_type: "ship".to_string(),
            name: Some("Get Test".to_string()),
            ..orp_proto::Entity::default()
        };
        state.storage.insert_entity(&entity).await.unwrap();

        let resp = app
            .oneshot(
                Request::get("/api/v1/entities/ship-get-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_get_entity_not_found_404() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(
                Request::get("/api/v1/entities/nonexistent-entity")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_update_entity_200() {
        let (app, state) = make_test_app().await;
        let entity = orp_proto::Entity {
            entity_id: "ship-upd-1".to_string(),
            entity_type: "ship".to_string(),
            ..orp_proto::Entity::default()
        };
        state.storage.insert_entity(&entity).await.unwrap();

        let body = serde_json::json!({
            "name": "Updated Ship",
            "properties": { "speed": 15.0 },
        });
        let resp = app
            .oneshot(
                Request::put("/api/v1/entities/ship-upd-1")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_update_entity_not_found_404() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({ "name": "Ghost" });
        let resp = app
            .oneshot(
                Request::put("/api/v1/entities/nonexistent")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_update_entity_invalid_geo_400() {
        let (app, state) = make_test_app().await;
        let entity = orp_proto::Entity {
            entity_id: "ship-bad-upd".to_string(),
            entity_type: "ship".to_string(),
            ..orp_proto::Entity::default()
        };
        state.storage.insert_entity(&entity).await.unwrap();

        let body = serde_json::json!({
            "geometry": { "coordinates": [0.0, -100.0] },
        });
        let resp = app
            .oneshot(
                Request::put("/api/v1/entities/ship-bad-upd")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_delete_entity_204() {
        let (app, state) = make_test_app().await;
        let entity = orp_proto::Entity {
            entity_id: "ship-del-1".to_string(),
            entity_type: "ship".to_string(),
            ..orp_proto::Entity::default()
        };
        state.storage.insert_entity(&entity).await.unwrap();

        let resp = app
            .oneshot(
                Request::delete("/api/v1/entities/ship-del-1")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_delete_entity_not_found_404() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(
                Request::delete("/api/v1/entities/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_list_entities_200() {
        let (app, state) = make_test_app().await;
        for i in 0..5 {
            let entity = orp_proto::Entity {
                entity_id: format!("ship-list-{}", i),
                entity_type: "ship".to_string(),
                ..orp_proto::Entity::default()
            };
            state.storage.insert_entity(&entity).await.unwrap();
        }

        let resp = app
            .oneshot(
                Request::get("/api/v1/entities?type=ship&limit=3")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["pagination"]["total_count"].as_u64().unwrap() >= 5);
    }

    #[tokio::test]
    async fn test_list_entities_pagination() {
        let (app, state) = make_test_app().await;
        for i in 0..10 {
            let entity = orp_proto::Entity {
                entity_id: format!("ship-page-{}", i),
                entity_type: "ship".to_string(),
                ..orp_proto::Entity::default()
            };
            state.storage.insert_entity(&entity).await.unwrap();
        }

        let resp = app
            .oneshot(
                Request::get("/api/v1/entities?type=ship&page=1&limit=5")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["pagination"]["has_next"].as_bool().unwrap_or(false));
    }

    // ── Search ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_search_near_valid() {
        let (app, state) = make_test_app().await;
        let entity = orp_proto::Entity {
            entity_id: "ship-near-1".to_string(),
            entity_type: "ship".to_string(),
            geometry: Some(GeoPoint {
                lat: 51.92,
                lon: 4.47,
                alt: None,
            }),
            ..orp_proto::Entity::default()
        };
        state.storage.insert_entity(&entity).await.unwrap();

        let resp = app
            .oneshot(
                Request::get("/api/v1/entities/search?near=51.92,4.47,50")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_search_near_malformed_400() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(
                Request::get("/api/v1/entities/search?near=bad_value")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_search_near_invalid_lat_400() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(
                Request::get("/api/v1/entities/search?near=999,4.47,50")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── Entity Relationships & Events ────────────────────────────────────────

    #[tokio::test]
    async fn test_get_entity_relationships_200() {
        let (app, state) = make_test_app().await;
        let entity = orp_proto::Entity {
            entity_id: "ship-rel-1".to_string(),
            entity_type: "ship".to_string(),
            ..orp_proto::Entity::default()
        };
        state.storage.insert_entity(&entity).await.unwrap();

        let resp = app
            .oneshot(
                Request::get("/api/v1/entities/ship-rel-1/relationships")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_get_entity_events_200() {
        let (app, state) = make_test_app().await;
        let entity = orp_proto::Entity {
            entity_id: "ship-ev-1".to_string(),
            entity_type: "ship".to_string(),
            ..orp_proto::Entity::default()
        };
        state.storage.insert_entity(&entity).await.unwrap();

        let resp = app
            .oneshot(
                Request::get("/api/v1/entities/ship-ev-1/events")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Relationships ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_relationship_201() {
        let (app, state) = make_test_app().await;
        for id in &["ship-cr-1", "port-cr-1"] {
            let entity = orp_proto::Entity {
                entity_id: id.to_string(),
                entity_type: "ship".to_string(),
                ..orp_proto::Entity::default()
            };
            state.storage.insert_entity(&entity).await.unwrap();
        }

        let body = serde_json::json!({
            "source_id": "ship-cr-1",
            "target_id": "port-cr-1",
            "type": "docked_at",
        });
        let resp = app
            .oneshot(
                Request::post("/api/v1/relationships")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    // ── Query ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_query_empty_400() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({ "query": "" });
        let resp = app
            .oneshot(
                Request::post("/api/v1/query")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_query_valid_200() {
        let (app, state) = make_test_app().await;
        let entity = orp_proto::Entity {
            entity_id: "ship-q-1".to_string(),
            entity_type: "ship".to_string(),
            ..orp_proto::Entity::default()
        };
        state.storage.insert_entity(&entity).await.unwrap();

        let body = serde_json::json!({ "query": "MATCH (s:ship) RETURN s.id" });
        let resp = app
            .oneshot(
                Request::post("/api/v1/query")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_query_invalid_syntax_400() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({ "query": "NOT VALID ORP-QL !!!" });
        let resp = app
            .oneshot(
                Request::post("/api/v1/query")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── Graph ────────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_graph_query_empty_400() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({ "query": "" });
        let resp = app
            .oneshot(
                Request::post("/api/v1/graph")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── Connectors ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_connectors_200() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(
                Request::get("/api/v1/connectors")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_create_connector_201() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({
            "name": "test-ais",
            "connector_type": "ais",
            "entity_type": "ship",
            "trust_score": 0.9,
        });
        let resp = app
            .oneshot(
                Request::post("/api/v1/connectors")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_delete_connector_not_found_404() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(
                Request::delete("/api/v1/connectors/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_update_connector_not_found_404() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({ "name": "updated" });
        let resp = app
            .oneshot(
                Request::put("/api/v1/connectors/nonexistent")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── Media streams ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_media_stream_redacts_credentials_and_projects_entity() {
        let (app, state) = make_test_app().await;
        let body = serde_json::json!({
            "id": "gate-cam",
            "name": "Gate Camera",
            "source_url": "rtsp://admin:secret@10.0.0.5:554/main",
            "allow_private_network": true,
            "expected_codec": "h264",
            "klv_metadata": true,
            "labels": ["perimeter"]
        });
        let resp = app
            .oneshot(
                Request::post("/api/v1/media/streams")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["data"]["protocol"], "rtsp");
        assert_eq!(
            json["data"]["source_url_redacted"],
            "rtsp://<redacted>@10.0.0.5:554/main"
        );
        assert!(json["data"].get("source_url").is_none());

        let entity = state
            .storage
            .get_entity("media:gate-cam")
            .await
            .unwrap()
            .expect("media stream entity");
        assert_eq!(entity.entity_type, "media_stream");
        assert_eq!(
            entity.properties["source_url"],
            "rtsp://<redacted>@10.0.0.5:554/main"
        );
    }

    #[tokio::test]
    async fn test_create_media_stream_private_ip_requires_opt_in() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({
            "id": "lan-cam",
            "name": "LAN Camera",
            "source_url": "rtsp://192.168.1.20/live"
        });
        let resp = app
            .oneshot(
                Request::post("/api/v1/media/streams")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn test_delete_media_stream_removes_graph_entity() {
        let (app, state) = make_test_app().await;
        let body = serde_json::json!({
            "id": "yard-cam",
            "name": "Yard Camera",
            "source_url": "rtsp://203.0.113.20/live"
        });
        let create = app
            .clone()
            .oneshot(
                Request::post("/api/v1/media/streams")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);

        let delete = app
            .oneshot(
                Request::delete("/api/v1/media/streams/yard-cam")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(delete.status(), StatusCode::NO_CONTENT);
        let entity = state
            .storage
            .get_entity("media:yard-cam")
            .await
            .unwrap()
            .expect("soft-deleted media stream entity remains addressable");
        assert!(!entity.is_active);
    }

    #[tokio::test]
    async fn test_media_relay_proxies_jpeg_bytes() {
        let origin = spawn_media_origin().await;
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({
            "id": "snapshot-cam",
            "name": "Snapshot Camera",
            "source_url": format!("{origin}/snapshot.jpg"),
            "allow_private_network": true
        });
        let create = app
            .clone()
            .oneshot(
                Request::post("/api/v1/media/streams")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);

        let resp = app
            .oneshot(
                Request::get("/api/v1/media/streams/snapshot-cam/relay")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "image/jpeg"
        );
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(&bytes[..], &[0xff, 0xd8, 0xff, 0xd9]);
    }

    #[tokio::test]
    async fn test_hls_playlist_relay_rewrites_segments() {
        let origin = spawn_media_origin().await;
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({
            "id": "hls-cam",
            "name": "HLS Camera",
            "source_url": format!("{origin}/index.m3u8"),
            "allow_private_network": true
        });
        let create = app
            .clone()
            .oneshot(
                Request::post("/api/v1/media/streams")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);

        let playlist = app
            .clone()
            .oneshot(
                Request::get("/api/v1/media/streams/hls-cam/playlist.m3u8")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(playlist.status(), StatusCode::OK);
        let body = axum::body::to_bytes(playlist.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = std::str::from_utf8(&body).unwrap();
        assert!(text.contains("/api/v1/media/streams/hls-cam/hls/fetch?url="));
        assert!(text.contains("%2Fseg-1.ts"));

        let segment = app
            .oneshot(
                Request::get(format!(
                    "/api/v1/media/streams/hls-cam/hls/fetch?url={origin}/seg-1.ts"
                ))
                .body(Body::empty())
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(segment.status(), StatusCode::OK);
        assert_eq!(
            segment.headers().get(header::CONTENT_TYPE).unwrap(),
            "video/mp2t"
        );
    }

    #[tokio::test]
    async fn test_rtsp_relay_reports_not_implemented() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({
            "id": "rtsp-cam",
            "name": "RTSP Camera",
            "source_url": "rtsp://203.0.113.20/live"
        });
        let create = app
            .clone()
            .oneshot(
                Request::post("/api/v1/media/streams")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(create.status(), StatusCode::CREATED);

        let resp = app
            .oneshot(
                Request::get("/api/v1/media/streams/rtsp-cam/relay")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_IMPLEMENTED);
    }

    // ── Monitors ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_monitors_200() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(
                Request::get("/api/v1/monitors")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_create_monitor_201() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({
            "name": "Speed Alert",
            "entity_type": "ship",
            "condition": {
                "type": "property_threshold",
                "property": "speed",
                "operator": ">",
                "value": 25.0,
            },
            "severity": "warning",
        });
        let resp = app
            .oneshot(
                Request::post("/api/v1/monitors")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_get_monitor_not_found_404() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(
                Request::get("/api/v1/monitors/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn test_delete_monitor_not_found_404() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(
                Request::delete("/api/v1/monitors/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── Alerts ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_alerts_200() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(Request::get("/api/v1/alerts").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_acknowledge_alert_not_found_404() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(
                Request::post("/api/v1/alerts/nonexistent/acknowledge")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── API Keys ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_api_keys_200() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(
                Request::get("/api/v1/api-keys")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_create_api_key_201() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({
            "name": "test-key",
            "scopes": ["entities:read"],
        });
        let resp = app
            .oneshot(
                Request::post("/api/v1/api-keys")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    #[tokio::test]
    async fn test_delete_api_key_not_found_404() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(
                Request::delete("/api/v1/api-keys/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    }

    // ── Events (global) ──────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_list_events_global_200() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(Request::get("/api/v1/events").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn test_list_events_with_entity_filter() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(
                Request::get("/api/v1/events?entity_id=ship-1&limit=10")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Metrics ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_metrics_200() {
        let (app, _) = make_test_app().await;
        let resp = app
            .oneshot(Request::get("/api/v1/metrics").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = std::str::from_utf8(&body).unwrap();
        assert!(text.contains("orp_entities_total"));
    }

    // ── Entity with geometry ─────────────────────────────────────────────────

    #[tokio::test]
    async fn test_create_entity_with_geometry() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({
            "id": "ship-geo-1",
            "type": "ship",
            "name": "Geo Ship",
            "geometry": { "coordinates": [4.47, 51.92] },
        });
        let resp = app
            .oneshot(
                Request::post("/api/v1/entities")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(json["geometry"].is_object());
    }

    #[tokio::test]
    async fn test_create_entity_invalid_lon_400() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({
            "id": "ship-bad-lon",
            "type": "ship",
            "geometry": { "coordinates": [999.0, 51.0] },
        });
        let resp = app
            .oneshot(
                Request::post("/api/v1/entities")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
    }

    // ── Create entity with properties ────────────────────────────────────────

    #[tokio::test]
    async fn test_create_entity_with_properties() {
        let (app, _) = make_test_app().await;
        let body = serde_json::json!({
            "id": "ship-props-1",
            "type": "ship",
            "properties": {
                "speed": 15.0,
                "heading": 245.0,
                "mmsi": "123456789",
            },
        });
        let resp = app
            .oneshot(
                Request::post("/api/v1/entities")
                    .header("content-type", "application/json")
                    .body(json_body(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::CREATED);
    }

    // ── Search with text ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn test_search_text() {
        let (app, state) = make_test_app().await;
        let entity = orp_proto::Entity {
            entity_id: "ship-search-1".to_string(),
            entity_type: "ship".to_string(),
            name: Some("Ever Given".to_string()),
            ..orp_proto::Entity::default()
        };
        state.storage.insert_entity(&entity).await.unwrap();

        let resp = app
            .oneshot(
                Request::get("/api/v1/entities/search?text_search=Ever")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
    }

    // ── Rate limiter unit tests ──────────────────────────────────────────────

    #[tokio::test]
    async fn test_rate_limiter_allows_requests() {
        let limiter = RateLimiter::new(10, 10);
        assert!(limiter.check("127.0.0.1").await.is_ok());
    }

    #[tokio::test]
    async fn test_rate_limiter_exhausts_tokens() {
        let limiter = RateLimiter::new(3, 0);
        assert!(limiter.check("1.2.3.4").await.is_ok());
        assert!(limiter.check("1.2.3.4").await.is_ok());
        assert!(limiter.check("1.2.3.4").await.is_ok());
        assert!(limiter.check("1.2.3.4").await.is_err());
    }

    #[tokio::test]
    async fn test_rate_limiter_different_ips() {
        let limiter = RateLimiter::new(1, 0);
        assert!(limiter.check("1.2.3.4").await.is_ok());
        assert!(limiter.check("5.6.7.8").await.is_ok());
        assert!(limiter.check("1.2.3.4").await.is_err());
    }

    // ── Connector health surfacing ───────────────────────────────────────────

    /// Stub adapter that lets tests pump arbitrary event/error counts into
    /// `Connector::stats()` without spinning up a real network listener.
    struct StubConnector {
        cfg: orp_connector::ConnectorConfig,
        events: u64,
        errors: u64,
    }

    impl StubConnector {
        fn new(id: &str, kind: &str, events: u64, errors: u64) -> Self {
            Self {
                cfg: orp_connector::ConnectorConfig {
                    connector_id: id.to_string(),
                    connector_type: kind.to_string(),
                    url: None,
                    entity_type: "test".to_string(),
                    enabled: true,
                    trust_score: 1.0,
                    properties: std::collections::HashMap::new(),
                },
                events,
                errors,
            }
        }
    }

    #[async_trait::async_trait]
    impl orp_connector::Connector for StubConnector {
        fn connector_id(&self) -> &str {
            &self.cfg.connector_id
        }
        async fn start(
            &self,
            _tx: tokio::sync::mpsc::Sender<orp_connector::SourceEvent>,
        ) -> Result<(), orp_connector::ConnectorError> {
            Ok(())
        }
        async fn stop(&self) -> Result<(), orp_connector::ConnectorError> {
            Ok(())
        }
        async fn health_check(&self) -> Result<(), orp_connector::ConnectorError> {
            Ok(())
        }
        fn config(&self) -> &orp_connector::ConnectorConfig {
            &self.cfg
        }
        fn stats(&self) -> orp_connector::ConnectorStats {
            orp_connector::ConnectorStats {
                events_processed: self.events,
                errors: self.errors,
                last_event_timestamp: None,
                uptime_seconds: 0,
            }
        }
    }

    async fn read_health(app: Router) -> serde_json::Value {
        let resp = app
            .oneshot(Request::get("/api/v1/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn health_endpoint_includes_connectors_field() {
        let (app, state) = make_test_app().await;
        {
            let mut guard = state.connectors.lock().await;
            guard.push(Arc::new(StubConnector::new("ais-1", "ais", 10, 0)));
            guard.push(Arc::new(StubConnector::new("adsb-1", "adsb", 20, 0)));
        }

        let json = read_health(app).await;
        let connectors = json["components"]["connectors"]
            .as_array()
            .expect("connectors array missing");
        assert_eq!(connectors.len(), 2);
        // Healthy when error rate is zero.
        assert_eq!(connectors[0]["status"], "healthy");
        assert_eq!(connectors[1]["status"], "healthy");
        assert_eq!(json["status"], "healthy");
    }

    #[tokio::test]
    async fn health_status_degraded_when_connector_error_rate_high() {
        // 80 errors / (20 events + 80 errors) = 80% → unhealthy at the
        // connector level, which propagates to a top-level "degraded".
        // A degraded threshold (>10%, ≤50%) is exercised separately below
        // by tweaking events/errors without crossing 50%.
        let (app, state) = make_test_app().await;
        {
            let mut guard = state.connectors.lock().await;
            // 6 errors / (50 + 6) ≈ 10.7% — > 10% so this is "degraded".
            guard.push(Arc::new(StubConnector::new("ais-1", "ais", 50, 6)));
        }

        let json = read_health(app).await;
        let entry = &json["components"]["connectors"][0];
        assert_eq!(entry["status"], "degraded");
        // No unhealthy connector → top-level stays healthy.
        assert_eq!(json["status"], "healthy");
    }

    #[tokio::test]
    async fn health_status_unhealthy_when_connector_error_rate_critical() {
        // 99 / (1 + 99) = 99% → unhealthy. Top-level should become "degraded"
        // (we never fail /health outright — dashboards still get JSON).
        let (app, state) = make_test_app().await;
        {
            let mut guard = state.connectors.lock().await;
            guard.push(Arc::new(StubConnector::new("ais-1", "ais", 1, 99)));
        }

        let json = read_health(app).await;
        let entry = &json["components"]["connectors"][0];
        assert_eq!(entry["status"], "unhealthy");
        assert_eq!(entry["events_processed"], 1);
        assert_eq!(entry["errors"], 99);
        assert_eq!(json["status"], "degraded");
    }
}
