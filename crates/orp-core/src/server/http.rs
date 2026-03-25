use crate::server::handlers;
use crate::server::websocket;
use anyhow::Result;
use axum::{
    routing::{delete, get, post, put},
    Router,
};
use orp_query::QueryExecutor;
use orp_storage::traits::Storage;
use orp_stream::StreamProcessor;
use std::sync::Arc;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;

pub struct AppState {
    pub storage: Arc<dyn Storage>,
    pub query_executor: Arc<QueryExecutor>,
    pub processor: Arc<StreamProcessor>,
}

pub async fn start_server(
    storage: Arc<dyn Storage>,
    query_executor: Arc<QueryExecutor>,
    processor: Arc<StreamProcessor>,
    port: u16,
) -> Result<()> {
    let state = Arc::new(AppState {
        storage,
        query_executor,
        processor,
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let app = Router::new()
        // Health
        .route("/api/v1/health", get(handlers::health_check))
        .route("/api/v1/metrics", get(handlers::metrics))
        // Entities
        .route("/api/v1/entities", get(handlers::list_entities))
        .route("/api/v1/entities", post(handlers::create_entity))
        .route("/api/v1/entities/search", get(handlers::search_entities))
        .route("/api/v1/entities/{id}", get(handlers::get_entity))
        .route("/api/v1/entities/{id}", put(handlers::update_entity))
        .route("/api/v1/entities/{id}", delete(handlers::delete_entity))
        .route(
            "/api/v1/entities/{id}/relationships",
            get(handlers::get_entity_relationships),
        )
        .route(
            "/api/v1/entities/{id}/events",
            get(handlers::get_entity_events),
        )
        // Query
        .route("/api/v1/query", post(handlers::execute_query))
        // Graph
        .route("/api/v1/graph", post(handlers::graph_query))
        // Connectors
        .route("/api/v1/connectors", get(handlers::list_connectors))
        // Monitors
        .route("/api/v1/monitors", get(handlers::list_monitors))
        // WebSocket
        .route("/ws/updates", get(websocket::ws_handler))
        // Frontend (serve static files)
        .fallback(handlers::serve_frontend)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    tracing::info!("ORP server listening on port {}", port);

    axum::serve(listener, app).await?;

    Ok(())
}
