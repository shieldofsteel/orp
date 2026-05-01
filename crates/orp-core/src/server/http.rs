use crate::server::federation::{self, PeerRegistry};
use crate::server::federation_tls::{FederationTlsConfig, LocalSigner, OutboundSeq, ReplayTracker};
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
use orp_audit::AuditLogger;
use orp_connector::Connector;
use orp_query::QueryExecutor;
use orp_security::{AbacEngine, ApiKeyService, AuthState};
use orp_storage::traits::Storage;
use orp_stream::{FederationOutbox, MonitorEngine, StreamProcessor};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
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
    /// Ed25519 signer for audit log cryptographic integrity. Retained on the
    /// state so out-of-band consumers (notification signing, federation
    /// outbox proofs) can borrow the same key the audit log signs with.
    /// Currently no in-tree consumer reads it directly; the field is part of
    /// the v0.2.0 public surface and removing it would be a breaking change.
    #[allow(dead_code)]
    pub audit_signer: Arc<EventSigner>,
    /// Persistent (or in-memory) audit log. Production code uses
    /// [`PersistentAuditLog`]; tests and `--in-memory` use
    /// [`InMemoryAuditLog`]. Both implement [`AuditLogger`] so handlers
    /// only see the trait.
    pub audit_log: Arc<dyn AuditLogger>,
    pub broadcast_tx: broadcast::Sender<websocket::BroadcastEvent>,
    pub started_at: std::time::Instant,
    /// Layer registry for intelligence overlays (optional — None if DB unavailable).
    pub layer_registry: Option<Arc<layers::LayerRegistry>>,
    /// Federation peer registry (optional — None if federation is disabled).
    pub federation_registry: Option<Arc<PeerRegistry>>,
    /// Disk-backed outbox for outbound federation events. Survives process
    /// restarts so events queued while a peer is unreachable are replayed on
    /// reconnect. None when federation is disabled or when the outbox path is
    /// not openable (e.g. read-only filesystem) — callers should treat as
    /// best-effort.
    pub federation_outbox: Option<Arc<FederationOutbox>>,
    /// Live connector registry. Each running adapter exposes `stats()` so the
    /// `/api/v1/health` endpoint can surface error rates instead of silently
    /// reporting "running" while the connector 100%-fails. Coexists with the
    /// existing processor flow — connectors push events to the processor; this
    /// registry is purely for observability.
    pub connectors: Arc<Mutex<Vec<Arc<dyn Connector>>>>,
    /// This node's stable identifier as the federation **sender** in signed
    /// envelopes. Defaults to a process-local UUID; operators set
    /// `ORP_NODE_ID` in production so peers can pin one ID per cluster.
    pub local_node_id: String,
    /// Local Ed25519 signing key for outbound federation pushes. Optional
    /// only because federation can be disabled entirely; when federation is
    /// enabled this is always populated (ephemeral if no key file is set).
    pub federation_signer: Arc<LocalSigner>,
    /// Per-receiver outbound sequence allocator. Only used on the push path.
    pub federation_seq: Arc<OutboundSeq>,
    /// Per-sender highest-seq tracker for inbound replay protection. None
    /// when federation is disabled.
    pub federation_replay: Option<Arc<ReplayTracker>>,
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

/// Middleware that injects `Arc<AuthState>` into request extensions so the
/// `AuthContext` extractor (in `orp-security`) can find it.
async fn inject_auth_state(
    State(state): axum::extract::State<Arc<AppState>>,
    mut request: Request,
    next: Next,
) -> Response {
    request.extensions_mut().insert(state.auth_state.clone());
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
    /// Optional pre-built audit logger. If `None` an [`InMemoryAuditLog`]
    /// using `audit_signer` is created — that's the path the v0.2.0 test
    /// harness relies on. `run_start` always passes a `PersistentAuditLog`.
    pub audit_log: Option<Arc<dyn AuditLogger>>,
    /// Optional layer registry for intelligence overlays.
    pub layer_registry: Option<Arc<layers::LayerRegistry>>,
    /// Optional federation peer registry. Pass `Some(PeerRegistry::new())` to enable federation.
    pub federation_registry: Option<Arc<PeerRegistry>>,
    pub port: u16,
    /// Headless mode: serve API + WebSocket only, skip static frontend files.
    /// Enables deployment on servers, Raspberry Pi, embedded, and CI environments
    /// where the web UI build artefacts are absent or unwanted.
    pub headless: bool,
    /// Optional dedicated mTLS listener for the federation push endpoint.
    /// When `enabled` and complete (cert/key/ca all set), a separate
    /// rustls-backed `axum-server` listener is spawned on
    /// `tls_config.listen_addr` (default `0.0.0.0:9443`) that requires every
    /// connecting peer to present a client certificate signed by the
    /// configured CA. The plaintext port (this struct's `port`) continues
    /// serving the frontend + non-federation REST API.
    pub federation_tls: FederationTlsConfig,
    /// Path to the local Ed25519 signing key (32-byte raw seed or 64-char
    /// hex). When unset, an ephemeral key is generated at startup and a
    /// warning is logged — fine for dev, not fine for production.
    pub federation_signing_key_path: Option<std::path::PathBuf>,
    /// Stable identifier for this node when it appears as the *sender* in
    /// signed federation envelopes. Defaults to a fresh UUID per process.
    /// Operators set this to a stable value (e.g. `cluster-east`) in
    /// production so receivers can pin pubkeys per logical node, not per
    /// process restart.
    pub local_node_id: Option<String>,
}

pub async fn start_server(config: ServerConfig) -> Result<()> {
    let (broadcast_tx, _) = broadcast::channel::<websocket::BroadcastEvent>(4096);

    let audit_signer = config
        .audit_signer
        .unwrap_or_else(|| Arc::new(EventSigner::new()));

    // Default to an in-memory backend when the caller hasn't wired one up —
    // tests get a working logger without pulling in DuckDB; production code
    // (`run_start`) always supplies the persistent backend.
    let audit_log: Arc<dyn AuditLogger> = config.audit_log.unwrap_or_else(|| {
        Arc::new(orp_audit::InMemoryAuditLog::with_signer(
            audit_signer.clone(),
        ))
    });

    // Open the federation outbox if federation is enabled. The path is
    // `ORP_FED_OUTBOX_PATH` or `~/.local/share/orp/federation-outbox` by
    // default. Failure to open is non-fatal — federation degrades to "no
    // buffering" and `pending_count` always reads as 0.
    let federation_outbox: Option<Arc<FederationOutbox>> = if config.federation_registry.is_some() {
        let path = std::env::var("ORP_FED_OUTBOX_PATH")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
                std::path::PathBuf::from(home).join(".local/share/orp/federation-outbox")
            });
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match FederationOutbox::open(&path) {
            Ok(o) => {
                tracing::info!(path = %path.display(), "Federation outbox opened");
                Some(Arc::new(o))
            }
            Err(e) => {
                tracing::warn!(
                    path = %path.display(),
                    error = %e,
                    "Failed to open federation outbox; outbound buffering disabled"
                );
                None
            }
        }
    } else {
        None
    };

    // Federation crypto state — only meaningful when the registry is wired,
    // but we always allocate a signer so callers can introspect the local
    // pubkey via `state.federation_signer.pubkey_hex()`.
    let federation_signer = Arc::new(LocalSigner::load_or_ephemeral(
        config.federation_signing_key_path.as_deref(),
    ));
    let federation_seq = OutboundSeq::new();
    let federation_replay = if config.federation_registry.is_some() {
        Some(ReplayTracker::new())
    } else {
        None
    };
    let local_node_id = config
        .local_node_id
        .clone()
        .or_else(|| std::env::var("ORP_NODE_ID").ok())
        .unwrap_or_else(|| format!("node-{}", uuid::Uuid::new_v4()));

    let state = Arc::new(AppState {
        storage: config.storage,
        query_executor: config.query_executor,
        processor: config.processor,
        monitor_engine: config.monitor_engine,
        auth_state: config.auth_state,
        abac_engine: config.abac_engine,
        api_key_service: config.api_key_service,
        audit_signer,
        audit_log,
        broadcast_tx,
        started_at: std::time::Instant::now(),
        layer_registry: config.layer_registry,
        federation_registry: config.federation_registry.clone(),
        federation_outbox,
        connectors: Arc::new(Mutex::new(Vec::new())),
        local_node_id,
        federation_signer,
        federation_seq,
        federation_replay,
    });

    // Spawn federation background sync if registry provided. Also spawn the
    // outbound outbox pump that drains buffered events to peers as they come
    // back online.
    if config.federation_registry.is_some() {
        federation::spawn_federation_sync(state.clone());
        federation::spawn_outbox_pump(state.clone());
    }

    let cors = build_cors_layer();
    let rate_limiter = RateLimiter::new(100, 100); // 100 tokens, refill 100/sec

    // Build the optional layers sub-router
    let layers_subrouter = state
        .layer_registry
        .as_ref()
        .map(|registry| layers::layers_router(Arc::clone(registry)));

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
        .route("/api/v1/relationships", post(handlers::create_relationship))
        // Query
        .route("/api/v1/query", post(handlers::execute_query))
        // Graph
        .route("/api/v1/graph", post(handlers::graph_query))
        // Connectors
        .route("/api/v1/connectors", get(handlers::list_connectors))
        .route("/api/v1/connectors", post(handlers::create_connector))
        .route("/api/v1/connectors/{id}", put(handlers::update_connector))
        .route(
            "/api/v1/connectors/{id}",
            delete(handlers::delete_connector),
        )
        // Monitors
        .route("/api/v1/monitors", get(handlers::list_monitors))
        .route("/api/v1/monitors", post(handlers::create_monitor))
        .route("/api/v1/monitors/{id}", get(handlers::get_monitor))
        .route("/api/v1/monitors/{id}", put(handlers::update_monitor))
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
        // Inbound signed-push endpoint — terminates the receiver side of
        // the mTLS + Ed25519 envelope flow. No auth middleware: the trust
        // is the client cert and the envelope signature, not a JWT.
        .route(
            "/api/v1/federation/push",
            post(federation::receive_signed_push),
        )
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

    // Federation mTLS listener (separate port). Spawned only when:
    //   1. Federation is enabled (registry present)
    //   2. `tls_config.enabled` is true
    //   3. cert/key/ca are all configured
    // Otherwise we log and continue with the plaintext listener only —
    // backward compatible with v0.2.0.
    if config.federation_registry.is_some() {
        if config.federation_tls.enabled {
            if config.federation_tls.is_complete() {
                let app_for_tls = app.clone();
                let tls_cfg = config.federation_tls.clone();
                tokio::spawn(async move {
                    if let Err(e) = serve_federation_tls(app_for_tls, tls_cfg).await {
                        tracing::error!(error = %e, "Federation mTLS listener exited");
                    }
                });
            } else {
                tracing::warn!(
                    "federation.tls.enabled=true but cert/key/ca not all set; \
                     mTLS listener NOT started — federation will accept plaintext only"
                );
            }
        } else {
            tracing::warn!(
                "Federation enabled with TLS DISABLED — peers can spoof signed pushes \
                 if you also have not pinned signing pubkeys. Set federation.tls.enabled=true \
                 in production."
            );
        }
    }

    let listener = tokio::net::TcpListener::bind(format!("0.0.0.0:{}", config.port)).await?;
    tracing::info!("ORP server listening on port {}", config.port);

    axum::serve(listener, app).await?;

    Ok(())
}

/// Bring up the dedicated federation mTLS listener. Pinned to rustls (no
/// native-tls anywhere in the workspace, per security policy) and configured
/// to require client authentication — connections without a valid client
/// cert signed by `tls_cfg.ca_path` are dropped before any HTTP frames are
/// served.
async fn serve_federation_tls(app: Router, tls_cfg: FederationTlsConfig) -> Result<()> {
    use axum_server::tls_rustls::RustlsConfig;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use rustls::server::WebPkiClientVerifier;
    use rustls::RootCertStore;

    // Install ring crypto provider once. Idempotent — if another component
    // already installed it, install_default returns Err which we deliberately
    // ignore.
    let _ = rustls::crypto::ring::default_provider().install_default();

    let cert_path = tls_cfg
        .cert_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("cert_path missing"))?;
    let key_path = tls_cfg
        .key_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("key_path missing"))?;
    let ca_path = tls_cfg
        .ca_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("ca_path missing"))?;

    let server_certs: Vec<CertificateDer<'static>> = {
        let pem = std::fs::read(cert_path)?;
        rustls_pemfile::certs(&mut pem.as_slice())
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| anyhow::anyhow!("parse server cert: {}", e))?
    };
    if server_certs.is_empty() {
        return Err(anyhow::anyhow!(
            "no certificates found in {}",
            cert_path.display()
        ));
    }

    let server_key: PrivateKeyDer<'static> = {
        let pem = std::fs::read(key_path)?;
        rustls_pemfile::private_key(&mut pem.as_slice())
            .map_err(|e| anyhow::anyhow!("parse server key: {}", e))?
            .ok_or_else(|| anyhow::anyhow!("no private key found in {}", key_path.display()))?
    };

    let mut roots = RootCertStore::empty();
    {
        let pem = std::fs::read(ca_path)?;
        for cert in rustls_pemfile::certs(&mut pem.as_slice()) {
            let cert = cert.map_err(|e| anyhow::anyhow!("parse CA cert: {}", e))?;
            roots.add(cert)?;
        }
    }
    if roots.is_empty() {
        return Err(anyhow::anyhow!(
            "no CA certificates found in {}",
            ca_path.display()
        ));
    }

    let client_verifier = WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .map_err(|e| anyhow::anyhow!("build client verifier: {}", e))?;

    let tls_server_cfg = rustls::ServerConfig::builder()
        .with_client_cert_verifier(client_verifier)
        .with_single_cert(server_certs, server_key)
        .map_err(|e| anyhow::anyhow!("rustls server config: {}", e))?;

    let tls_config = RustlsConfig::from_config(Arc::new(tls_server_cfg));

    let addr: std::net::SocketAddr = tls_cfg
        .listen_addr
        .parse()
        .map_err(|e| anyhow::anyhow!("invalid federation TLS listen_addr: {}", e))?;

    tracing::info!(
        addr = %addr,
        cert = %cert_path.display(),
        ca = %ca_path.display(),
        "Federation mTLS listener starting"
    );

    axum_server::bind_rustls(addr, tls_config)
        .serve(app.into_make_service_with_connect_info::<std::net::SocketAddr>())
        .await
        .map_err(|e| anyhow::anyhow!("federation mTLS serve: {}", e))?;
    Ok(())
}
