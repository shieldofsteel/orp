use crate::server::http::AppState;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Json},
};
use orp_proto::{Entity, GeoPoint};
#[allow(unused_imports)]
use orp_storage::traits::Storage;
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
    timestamp: String,
}

fn error_response(code: &str, status: StatusCode, message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: ErrorBody {
                code: code.to_string(),
                status: status.as_u16(),
                message: message.to_string(),
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
    components: HealthComponents,
}

#[derive(Serialize)]
pub struct HealthComponents {
    database: ComponentHealth,
    stream_processor: ComponentHealth,
    api_server: ComponentHealth,
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

    Json(HealthResponse {
        status: "healthy".to_string(),
        timestamp: chrono::Utc::now().to_rfc3339(),
        version: env!("CARGO_PKG_VERSION").to_string(),
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
        },
    })
}

pub async fn metrics(State(state): State<Arc<AppState>>) -> String {
    let stats = state.storage.get_stats().await.unwrap_or_default();
    let proc_stats = state.processor.stats().await;

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
         orp_stream_events_deduplicated {}\n",
        stats.total_entities,
        stats.total_events,
        stats.total_relationships,
        proc_stats.events_processed,
        proc_stats.events_deduplicated,
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
        created_at: e.last_updated.to_rfc3339(),
        updated_at: e.last_updated.to_rfc3339(),
    }
}

pub async fn list_entities(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListParams>,
) -> impl IntoResponse {
    let page = params.page.unwrap_or(1);
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
            let total_pages = (total as f64 / limit as f64).ceil() as u64;
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
        Err(e) => error_response("INTERNAL_ERROR", StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
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
        Err(e) => error_response("INTERNAL_ERROR", StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
            .into_response(),
    }
}

pub async fn get_entity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.storage.get_entity(&id).await {
        Ok(Some(entity)) => Json(entity_to_response(&entity)).into_response(),
        Ok(None) => {
            error_response("NOT_FOUND", StatusCode::NOT_FOUND, &format!("Entity '{}' not found", id))
                .into_response()
        }
        Err(e) => error_response("INTERNAL_ERROR", StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
            .into_response(),
    }
}

pub async fn update_entity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    // Get existing entity
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
        Ok(None) => {
            error_response("NOT_FOUND", StatusCode::NOT_FOUND, &format!("Entity '{}' not found", id))
                .into_response()
        }
        Err(e) => error_response("INTERNAL_ERROR", StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
            .into_response(),
    }
}

pub async fn delete_entity(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.storage.delete_entity(&id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => error_response("INTERNAL_ERROR", StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
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
                        return Json(serde_json::json!({
                            "data": data,
                            "search_time_ms": 0,
                        }))
                        .into_response();
                    }
                    Err(e) => {
                        return error_response(
                            "INTERNAL_ERROR",
                            StatusCode::INTERNAL_SERVER_ERROR,
                            &e.to_string(),
                        )
                        .into_response()
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
            Json(serde_json::json!({
                "data": data,
                "search_time_ms": 0,
            }))
            .into_response()
        }
        Err(e) => error_response("INTERNAL_ERROR", StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
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
            }))
            .into_response()
        }
        Err(e) => error_response("INTERNAL_ERROR", StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
            .into_response(),
    }
}

pub async fn get_entity_events(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    match state.storage.get_events_for_entity(&id, 100).await {
        Ok(events) => {
            let data: Vec<_> = events
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "id": e.event_id,
                        "entity_id": e.entity_id,
                        "event_type": e.event_type,
                        "timestamp": e.event_timestamp.to_rfc3339(),
                        "source": e.source_id,
                        "data": e.data,
                    })
                })
                .collect();
            Json(serde_json::json!({ "data": data })).into_response()
        }
        Err(e) => error_response("INTERNAL_ERROR", StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
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
    match state.query_executor.execute(&body.query).await {
        Ok(result) => Json(serde_json::json!({
            "status": "success",
            "results": result.rows,
            "metadata": {
                "execution_time_ms": result.execution_time_ms,
                "rows_returned": result.row_count,
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
    let query = body
        .get("query")
        .and_then(|q| q.as_str())
        .unwrap_or("");

    match state.storage.graph_query(query).await {
        Ok(results) => Json(serde_json::json!({
            "status": "success",
            "results": results,
            "metadata": {
                "execution_time_ms": 0,
                "rows_returned": results.len(),
            }
        }))
        .into_response(),
        Err(e) => error_response("INTERNAL_ERROR", StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
            .into_response(),
    }
}

// ---- Connectors ----

pub async fn list_connectors(
    State(state): State<Arc<AppState>>,
) -> impl IntoResponse {
    match state.storage.get_data_sources().await {
        Ok(sources) => Json(serde_json::json!({ "data": sources })).into_response(),
        Err(e) => error_response("INTERNAL_ERROR", StatusCode::INTERNAL_SERVER_ERROR, &e.to_string())
            .into_response(),
    }
}

// ---- Monitors ----

pub async fn list_monitors() -> impl IntoResponse {
    Json(serde_json::json!({
        "data": []
    }))
}

// ---- Frontend ----

pub async fn serve_frontend() -> impl IntoResponse {
    Html(include_str!("../../frontend.html").to_string())
}
