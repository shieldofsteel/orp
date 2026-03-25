use crate::server::http::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Json},
};
use orp_proto::{Entity, GeoPoint, Relationship};
use orp_stream::monitor::{
    AlertSeverity, GeofenceTrigger, MonitorAction, MonitorCondition, MonitorRule, ThresholdOp,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

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
    stream_processor: ComponentHealth,
    api_server: ComponentHealth,
    monitor_engine: ComponentHealth,
}

#[derive(Serialize)]
pub struct ComponentHealth {
    status: String,
    latency_ms: Option<f64>,
}

pub async fn health_check(State(state): State<Arc<AppState>>) -> Json<HealthResponse> {
    let start = std::time::Instant::now();
    let db_status = match state.storage.health_check().await {
        Ok(()) => "healthy",
        Err(_) => "error",
    };
    let db_latency = start.elapsed().as_secs_f64() * 1000.0;

    let _proc_stats = state.processor.stats().await;
    let uptime = state.started_at.elapsed().as_secs();

    Json(HealthResponse {
        status: "healthy".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: uptime,
        components: HealthComponents {
            database: ComponentHealth {
                status: db_status.to_string(),
                latency_ms: Some(db_latency),
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
        },
    })
}

pub async fn metrics(State(state): State<Arc<AppState>>) -> String {
    let stats = state.storage.get_stats().await.unwrap_or_default();
    let proc_stats = state.processor.stats().await;
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
    confidence: f32,
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
        created_at: e.last_updated.to_rfc3339(),
        updated_at: e.last_updated.to_rfc3339(),
    }
}

pub async fn list_entities(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
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
    confidence: Option<f32>,
}

#[derive(Deserialize)]
struct CreateGeoJson {
    coordinates: [f64; 2],
}

pub async fn create_entity(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateEntityRequest>,
) -> impl IntoResponse {
    if body.id.is_empty() {
        return error_response(
            "VALIDATION_ERROR",
            StatusCode::BAD_REQUEST,
            "Entity id cannot be empty",
        )
        .into_response();
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
        Ok(()) => (StatusCode::CREATED, Json(entity_to_response(&entity))).into_response(),
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

pub async fn get_entity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
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
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
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
                Ok(()) => Json(entity_to_response(&entity)).into_response(),
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
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.storage.get_entity(&id).await {
        Ok(Some(_)) => match state.storage.delete_entity(&id).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
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
    State(state): State<Arc<AppState>>,
    Query(params): Query<SearchParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(100).min(1000);
    let start = std::time::Instant::now();

    // Parse "near" parameter: "lat,lon,radius_km"
    if let Some(ref near) = params.near {
        let parts: Vec<&str> = near.split(',').collect();
        if parts.len() == 3 {
            if let (Ok(lat), Ok(lon), Ok(radius)) = (
                parts[0].parse::<f64>(),
                parts[1].parse::<f64>(),
                parts[2].parse::<f64>(),
            ) {
                match state
                    .storage
                    .get_entities_in_radius(lat, lon, radius, params.entity_type.as_deref())
                    .await
                {
                    Ok(entities) => {
                        let data: Vec<EntityResponse> =
                            entities.iter().take(limit).map(entity_to_response).collect();
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
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
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
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(params): Query<EventsParams>,
) -> impl IntoResponse {
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

// ---- Relationships ----

#[derive(Deserialize)]
pub struct CreateRelationshipRequest {
    source_id: String,
    target_id: String,
    #[serde(rename = "type")]
    rel_type: String,
    properties: Option<HashMap<String, serde_json::Value>>,
    confidence: Option<f32>,
}

pub async fn create_relationship(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateRelationshipRequest>,
) -> impl IntoResponse {
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
        Ok(()) => (
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
            .into_response(),
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
    State(state): State<Arc<AppState>>,
    Json(body): Json<QueryRequest>,
) -> impl IntoResponse {
    if body.query.trim().is_empty() {
        return error_response(
            "INVALID_QUERY",
            StatusCode::BAD_REQUEST,
            "Query string cannot be empty",
        )
        .into_response();
    }

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
        Err(e) => error_response("INVALID_QUERY", StatusCode::BAD_REQUEST, &e.to_string())
            .into_response(),
    }
}

pub async fn graph_query(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
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
        Err(e) => error_response(
            "INTERNAL_ERROR",
            StatusCode::INTERNAL_SERVER_ERROR,
            &e.to_string(),
        )
        .into_response(),
    }
}

// ---- Connectors ----

pub async fn list_connectors(State(state): State<Arc<AppState>>) -> impl IntoResponse {
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
    trust_score: Option<f32>,
}

pub async fn create_connector(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateConnectorRequest>,
) -> impl IntoResponse {
    let source = orp_proto::DataSource {
        source_id: format!("{}-{}", body.connector_type, uuid::Uuid::new_v4()),
        source_name: body.name,
        source_type: body.connector_type,
        trust_score: body.trust_score.unwrap_or(0.8),
        events_ingested: 0,
        enabled: true,
    };

    match state.storage.register_data_source(&source).await {
        Ok(()) => (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "data": source,
            })),
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

// ---- Monitors ----

pub async fn list_monitors(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let rules = state.monitor_engine.get_rules().await;
    Json(serde_json::json!({
        "data": rules,
        "count": rules.len(),
    }))
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
    Geofence {
        lat: f64,
        lon: f64,
        radius_km: f64,
    },
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

pub async fn create_monitor(
    State(state): State<Arc<AppState>>,
    Json(body): Json<CreateMonitorRequest>,
) -> impl IntoResponse {
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

    let rule = MonitorRule {
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
    };

    state.monitor_engine.add_rule(rule.clone()).await;

    (
        StatusCode::CREATED,
        Json(serde_json::json!({
            "data": rule,
        })),
    )
        .into_response()
}

pub async fn get_monitor(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
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

pub async fn delete_monitor(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if state.monitor_engine.remove_rule(&id).await {
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
    State(state): State<Arc<AppState>>,
    Query(params): Query<AlertsParams>,
) -> impl IntoResponse {
    let limit = params.limit.unwrap_or(100).min(1000);
    let alerts = state.monitor_engine.get_alerts(limit).await;
    Json(serde_json::json!({
        "data": alerts,
        "count": alerts.len(),
    }))
}

pub async fn acknowledge_alert(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    if state.monitor_engine.acknowledge_alert(&id).await {
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

// Frontend is now served via ServeDir in http.rs (frontend/dist/)
