use crate::server::handlers;
use crate::server::websocket;
use anyhow::Result;
use axum::{
    extract::{Request, State},
    middleware::Next,
    response::Response,
    routing::{delete, get, post, put},
    Router,
};
use orp_query::QueryExecutor;
use orp_security::{AbacEngine, AuthState};
use orp_storage::traits::Storage;
use orp_stream::{MonitorEngine, StreamProcessor};
use std::sync::Arc;
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

pub struct AppState {
    pub storage: Arc<dyn Storage>,
    pub query_executor: Arc<QueryExecutor>,
    pub processor: Arc<dyn StreamProcessor>,
    pub monitor_engine: Arc<MonitorEngine>,
    pub auth_state: Arc<AuthState>,
    pub abac_engine: Arc<AbacEngine>,
    pub started_at: std::time::Instant,
}

/// Build CORS layer from ORP_CORS_ORIGINS env var (comma-separated).
/// Falls back to http://localhost:3000 if unset. Never uses `Any`.
fn build_cors_layer() -> CorsLayer {
    use axum::http::{HeaderValue, Method};
    use tower_http::cors::AllowOrigin;

    let origins_str =
        std::env::var("ORP_CORS_ORIGINS").unwrap_or_else(|_| "http://localhost:3000".to_string());

    let origins: Vec<HeaderValue> = origins_str
        .split(',')
        .filter_map(|s| {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                trimmed.parse::<HeaderValue>().ok()
            }
        })
        .collect();

    CorsLayer::new()
        .allow_origin(AllowOrigin::list(origins))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::DELETE,
            Method::PATCH,
            Method::OPTIONS,
        ])
        .allow_headers(tower_http::cors::Any)
}

/// Middleware that injects Arc<AuthState> into request extensions so the
/// AuthContext extractor (in orp-security) can find it.
async fn inject_auth_state(
    State(state): axum::extract::State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    request
        .extensions_mut()
        .insert(state.auth_state.clone());
    next.run(request).await
}

/// Configuration for starting the HTTP server.
pub struct ServerConfig {
    pub storage: Arc<dyn Storage>,
    pub query_executor: Arc<QueryExecutor>,
    pub processor: Arc<dyn StreamProcessor>,
    pub monitor_engine: Arc<MonitorEngine>,
    pub auth_state: Arc<AuthState>,
    pub abac_engine: Arc<AbacEngine>,
    pub port: u16,
}

pub async fn start_server(config: ServerConfig) -> Result<()> {
    let state = Arc::new(AppState {
        storage: config.storage,
        query_executor: config.query_executor,
        processor: config.processor,
        monitor_engine: config.monitor_engine,
        auth_state: config.auth_state,
        abac_engine: config.abac_engine,
        started_at: std::time::Instant::now(),
    });

    let cors = build_cors_layer();

    let app = Router::new()
        // Health (no auth required)
        .route("/api/v1/health", get(handlers::health_check))
        // Metrics (auth required — handler extracts AuthContext)
        .route("/api/v1/metrics", get(handlers::metrics))
        // Events (global)
        .route("/api/v1/events", get(handlers::list_events_global))
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
        // Relationships
        .route(
            "/api/v1/relationships",
            post(handlers::create_relationship),
        )
        // Query
        .route("/api/v1/query", post(handlers::execute_query))
        // Graph
        .route("/api/v1/graph", post(handlers::graph_query))
        // Connectors
        .route("/api/v1/connectors", get(handlers::list_connectors))
        .route("/api/v1/connectors", post(handlers::create_connector))
        .route(
            "/api/v1/connectors/{id}",
            put(handlers::update_connector),
        )
        .route(
            "/api/v1/connectors/{id}",
            delete(handlers::delete_connector),
        )
        // Monitors
        .route("/api/v1/monitors", get(handlers::list_monitors))
        .route("/api/v1/monitors", post(handlers::create_monitor))
        .route("/api/v1/monitors/{id}", get(handlers::get_monitor))
        .route(
            "/api/v1/monitors/{id}",
            put(handlers::update_monitor),
        )
        .route("/api/v1/monitors/{id}", delete(handlers::delete_monitor))
        // Alerts
        .route("/api/v1/alerts", get(handlers::list_alerts))
        .route(
            "/api/v1/alerts/{id}/acknowledge",
            post(handlers::acknowledge_alert),
        )
        // WebSocket
        .route("/ws/updates", get(websocket::ws_handler))
        // Frontend — serve built Vite assets from frontend/dist/
        .fallback_service(
            ServeDir::new("frontend/dist")
                .not_found_service(ServeFile::new("frontend/dist/index.html")),
        )
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            inject_auth_state,
        ))
        .layer(cors)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port)).await?;
    tracing::info!("ORP server listening on port {}", config.port);

    axum::serve(listener, app).await?;

    Ok(())
}
