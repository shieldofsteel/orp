use crate::server::federation::{self, PeerRegistry};
use crate::server::handlers;
use crate::server::ingest;
use crate::server::layers;
use crate::server::websocket;
use anyhow::Result;
use axum::{
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Router,
};
use orp_audit::crypto::EventSigner;
use orp_query::QueryExecutor;
use orp_security::{AbacEngine, ApiKeyService, AuthState};
use orp_storage::traits::Storage;
use orp_stream::{MonitorEngine, StreamProcessor};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
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
    pub api_key_service: Arc<ApiKeyService>,
    /// Ed25519 signer for audit log cryptographic integrity.
    pub audit_signer: Arc<EventSigner>,
    pub broadcast_tx: broadcast::Sender<websocket::BroadcastEvent>,
    pub started_at: std::time::Instant,
    /// Layer registry for intelligence overlays (optional — None if DB unavailable).
    pub layer_registry: Option<Arc<layers::LayerRegistry>>,
    /// Federation peer registry (optional — None if federation is disabled).
    pub federation_registry: Option<Arc<PeerRegistry>>,
}

/// Per-IP rate limiter state — token bucket with 100 req/sec.
#[derive(Clone)]
pub struct RateLimiter {
    /// IP → (token_count, last_refill_instant)
    buckets: Arc<tokio::sync::Mutex<HashMap<String, (u64, std::time::Instant)>>>,
    max_tokens: u64,
    refill_rate: u64, // tokens per second
}

impl RateLimiter {
    pub fn new(max_tokens: u64, refill_rate: u64) -> Self {
        Self {
            buckets: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
            max_tokens,
            refill_rate,
        }
    }

    /// Try to consume a token for the given IP. Returns Ok(()) or Err(retry_after_secs).
    pub async fn check(&self, ip: &str) -> Result<(), u64> {
        let mut buckets = self.buckets.lock().await;
        let now = std::time::Instant::now();

        let (tokens, last_refill) = buckets
            .entry(ip.to_string())
            .or_insert((self.max_tokens, now));

        // Refill tokens based on elapsed time
        let elapsed = now.duration_since(*last_refill);
        let new_tokens = elapsed.as_secs() * self.refill_rate
            + (elapsed.subsec_millis() as u64 * self.refill_rate) / 1000;
        if new_tokens > 0 {
            *tokens = (*tokens + new_tokens).min(self.max_tokens);
            *last_refill = now;
        }

        if *tokens > 0 {
            *tokens -= 1;
            Ok(())
        } else {
            // Retry-After: 1 second (one refill window)
            Err(1u64)
        }
    }
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

/// Rate limiting middleware — 100 req/sec per IP, returns 429 + Retry-After.
async fn rate_limit_middleware(
    State(limiter): State<RateLimiter>,
    request: Request,
    next: Next,
) -> Response {
    // Extract client IP from X-Forwarded-For or ConnectInfo or fallback
    let ip = request
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .or_else(|| {
            request
                .extensions()
                .get::<ConnectInfo<SocketAddr>>()
                .map(|ci| ci.0.ip().to_string())
        })
        .unwrap_or_else(|| "unknown".to_string());

    // Exempt high-throughput and static paths from rate limiting
    let path = request.uri().path();
    if path.starts_with("/api/v1/ingest") 
        || path.starts_with("/assets/")
        || path.starts_with("/api/v1/health")
        || path == "/"
        || path.ends_with(".js")
        || path.ends_with(".css")
        || path.ends_with(".html")
        || path.ends_with(".svg")
        || path.ends_with(".png")
    {
        return next.run(request).await;
    }

    match limiter.check(&ip).await {
        Ok(()) => next.run(request).await,
        Err(retry_after) => {
            let body = serde_json::json!({
                "error": {
                    "code": "RATE_LIMITED",
                    "status": 429,
                    "message": "Too many requests. Please retry later.",
                    "retry_after_seconds": retry_after,
                    "timestamp": chrono::Utc::now().to_rfc3339(),
                }
            });
            (
                StatusCode::TOO_MANY_REQUESTS,
                [("Retry-After", retry_after.to_string().as_str().to_owned())],
                axum::Json(body),
            )
                .into_response()
        }
    }
}

/// Configuration for starting the HTTP server.
pub struct ServerConfig {
    pub storage: Arc<dyn Storage>,
    pub query_executor: Arc<QueryExecutor>,
    pub processor: Arc<dyn StreamProcessor>,
    pub monitor_engine: Arc<MonitorEngine>,
    pub auth_state: Arc<AuthState>,
    pub abac_engine: Arc<AbacEngine>,
    pub api_key_service: Arc<ApiKeyService>,
    /// Optional Ed25519 signer; a fresh one is generated if None.
    pub audit_signer: Option<Arc<EventSigner>>,
    /// Optional layer registry for intelligence overlays.
    pub layer_registry: Option<Arc<layers::LayerRegistry>>,
    /// Optional federation peer registry. Pass `Some(PeerRegistry::new())` to enable federation.
    pub federation_registry: Option<Arc<PeerRegistry>>,
    pub port: u16,
    /// Headless mode: serve API + WebSocket only, skip static frontend files.
    /// Enables deployment on servers, Raspberry Pi, embedded, and CI environments
    /// where the web UI build artefacts are absent or unwanted.
    pub headless: bool,
}

pub async fn start_server(config: ServerConfig) -> Result<()> {
    let (broadcast_tx, _) = broadcast::channel::<websocket::BroadcastEvent>(4096);

    let audit_signer = config
        .audit_signer
        .unwrap_or_else(|| Arc::new(EventSigner::new()));

    let state = Arc::new(AppState {
        storage: config.storage,
        query_executor: config.query_executor,
        processor: config.processor,
        monitor_engine: config.monitor_engine,
        auth_state: config.auth_state,
        abac_engine: config.abac_engine,
        api_key_service: config.api_key_service,
        audit_signer,
        broadcast_tx,
        started_at: std::time::Instant::now(),
        layer_registry: config.layer_registry,
        federation_registry: config.federation_registry.clone(),
    });

    // Spawn federation background sync if registry provided
    if config.federation_registry.is_some() {
        federation::spawn_federation_sync(state.clone());
    }

    let cors = build_cors_layer();
    let rate_limiter = RateLimiter::new(100, 100); // 100 tokens, refill 100/sec

    // Build the optional layers sub-router
    let layers_subrouter = state.layer_registry.as_ref().map(|registry| {
        layers::layers_router(Arc::clone(registry))
    });

    // Core API routes (always present regardless of headless mode)
    let api_routes = Router::new()
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
        // API Keys
        .route("/api/v1/api-keys", post(handlers::create_api_key))
        .route("/api/v1/api-keys", get(handlers::list_api_keys))
        .route("/api/v1/api-keys/{id}", delete(handlers::delete_api_key))
        // Universal ingest — any system can POST JSON, ORP handles the rest
        .route("/api/v1/ingest", post(ingest::ingest_single))
        .route("/api/v1/ingest/batch", post(ingest::ingest_batch))
        // Federation peers
        .route("/api/v1/peers", get(federation::list_peers))
        .route("/api/v1/peers", post(federation::register_peer))
        .route("/api/v1/peers/{id}", delete(federation::remove_peer))
        .route("/api/v1/peers/{id}/sync", post(federation::sync_peer))
        // WebSocket
        .route("/ws/updates", get(websocket::ws_handler));

    // In headless mode we skip static file serving entirely — no frontend/dist
    // required, making ORP deployable on Raspberry Pi, servers, and embedded.
    let mut app: Router = if config.headless {
        tracing::info!("Headless mode: static frontend disabled");
        api_routes
            .layer(axum::middleware::from_fn_with_state(
                rate_limiter,
                rate_limit_middleware,
            ))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                inject_auth_state,
            ))
            .layer(cors)
            .layer(TraceLayer::new_for_http())
            .with_state(state)
    } else {
        api_routes
            // Frontend — serve built Vite assets from frontend/dist/
            .fallback_service(
                ServeDir::new("frontend/dist")
                    .not_found_service(ServeFile::new("frontend/dist/index.html")),
            )
            .layer(axum::middleware::from_fn_with_state(
                rate_limiter,
                rate_limit_middleware,
            ))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                inject_auth_state,
            ))
            .layer(cors)
            .layer(TraceLayer::new_for_http())
            .with_state(state)
    };

    // Nest the layers sub-router if registry is available
    if let Some(subrouter) = layers_subrouter {
        app = app.nest("/api/v1", subrouter);
    }

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port)).await?;
    tracing::info!("ORP server listening on port {}", config.port);

    axum::serve(listener, app).await?;

    Ok(())
}
