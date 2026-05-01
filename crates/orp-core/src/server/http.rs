use crate::server::federation::{self, PeerRegistry};
use crate::server::handlers;
use crate::server::ingest;
use crate::server::layers;
use crate::server::websocket;
use anyhow::{Context, Result};
use axum::{
    extract::{ConnectInfo, Request, State},
    http::{HeaderName, HeaderValue, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Router,
};
use orp_audit::crypto::EventSigner;
use orp_connector::Connector;
use orp_query::QueryExecutor;
use orp_security::{AbacEngine, ApiKeyService, AuthState};
use orp_storage::traits::Storage;
use orp_stream::{FederationOutbox, MonitorEngine, StreamProcessor};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::set_header::SetResponseHeaderLayer;
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

/// TLS configuration for the inbound HTTP server. When `Some`, the server
/// terminates TLS via `rustls`; when `None`, it serves plain HTTP.
#[derive(Clone, Debug)]
pub struct TlsConfig {
    /// Path to the PEM-encoded server certificate (chain).
    pub cert_path: PathBuf,
    /// Path to the PEM-encoded server private key.
    pub key_path: PathBuf,
    /// Optional path to a PEM bundle of trusted client CAs. When set, the
    /// server requires clients to present a certificate signed by one of
    /// these CAs (mTLS).
    pub client_ca_path: Option<PathBuf>,
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
    /// Optional TLS configuration. When `Some`, the server serves HTTPS; when
    /// `None`, it serves plain HTTP and emits a startup warning.
    pub tls: Option<TlsConfig>,
    /// When TLS is active and this is `Some`, spawn a second listener on the
    /// given port that 301-redirects every request to the HTTPS origin.
    pub redirect_http_port: Option<u16>,
}

pub async fn start_server(config: ServerConfig) -> Result<()> {
    let (broadcast_tx, _) = broadcast::channel::<websocket::BroadcastEvent>(4096);

    let audit_signer = config
        .audit_signer
        .unwrap_or_else(|| Arc::new(EventSigner::new()));

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
        federation_outbox,
        connectors: Arc::new(Mutex::new(Vec::new())),
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
        // WebSocket
        .route("/ws/updates", get(websocket::ws_handler));

    // HSTS layer — only attached when TLS is active. The header tells
    // compliant browsers to refuse HTTP for one year, mitigating downgrade
    // attacks. We set it unconditionally on TLS responses (no
    // `includeSubDomains` / `preload` by default — operators add those
    // explicitly via reverse proxy when they own the apex domain).
    let hsts_layer = config.tls.as_ref().map(|_| {
        SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("strict-transport-security"),
            HeaderValue::from_static("max-age=31536000"),
        )
    });

    // In headless mode we skip static file serving entirely — no frontend/dist
    // required, making ORP deployable on Raspberry Pi, servers, and embedded.
    let base_router: Router<Arc<AppState>> = if config.headless {
        tracing::info!("Headless mode: static frontend disabled");
        api_routes
    } else {
        api_routes.fallback_service(
            ServeDir::new("frontend/dist")
                .not_found_service(ServeFile::new("frontend/dist/index.html")),
        )
    };

    let stateful: Router<Arc<AppState>> = base_router
        .layer(axum::middleware::from_fn_with_state(
            rate_limiter,
            rate_limit_middleware,
        ))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            inject_auth_state,
        ))
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    let mut app: Router = stateful.with_state(state);

    if let Some(layer) = hsts_layer {
        app = app.layer(layer);
    }

    // Nest the layers sub-router if registry is available. It's a stateless
    // `Router<()>` so this only works once the main router has had its state
    // resolved via `with_state(state)` above.
    if let Some(subrouter) = layers_subrouter {
        app = app.nest("/api/v1", subrouter);
    }

    let addr: SocketAddr = format!("0.0.0.0:{}", config.port)
        .parse()
        .context("invalid bind address")?;

    match config.tls.as_ref() {
        Some(tls) => {
            tracing::info!(
                cert = %tls.cert_path.display(),
                key  = %tls.key_path.display(),
                mtls = tls.client_ca_path.is_some(),
                "Starting HTTPS server on {}",
                addr,
            );
            let rustls = build_rustls_config(tls).await?;

            // Optional plain-HTTP redirector. Returns 301 to the HTTPS origin
            // for every request. Spawned as a background task so the main
            // listener controls process lifetime.
            if let Some(redirect_port) = config.redirect_http_port {
                spawn_http_to_https_redirect(redirect_port, config.port);
            }

            axum_server::bind_rustls(addr, rustls)
                .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                .await
                .context("HTTPS server error")?;
        }
        None => {
            tracing::warn!(
                "Starting plain HTTP server on {} — TLS is DISABLED. Pass --tls-cert and \
                 --tls-key to enable HTTPS, or run `orp gen-cert` for a dev cert.",
                addr,
            );
            let listener = tokio::net::TcpListener::bind(addr).await?;
            axum::serve(
                listener,
                app.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .await?;
        }
    }

    Ok(())
}

/// Build a `rustls`-backed `axum_server` config from PEM files on disk. When
/// `client_ca_path` is provided, the resulting config requires clients to
/// present a certificate signed by one of the CAs in that bundle (mTLS).
pub async fn build_rustls_config(tls: &TlsConfig) -> Result<axum_server::tls_rustls::RustlsConfig> {
    use axum_server::tls_rustls::RustlsConfig;

    // Ensure a default CryptoProvider is installed. axum-server-rustls picks
    // the process-wide provider; calling install on every start is idempotent
    // (Err means a provider is already installed, which is fine).
    let _ = rustls::crypto::ring::default_provider().install_default();

    if let Some(ca_path) = tls.client_ca_path.as_ref() {
        let server_config = build_mtls_server_config(&tls.cert_path, &tls.key_path, ca_path)
            .context("failed to build mTLS rustls server config")?;
        Ok(RustlsConfig::from_config(Arc::new(server_config)))
    } else {
        RustlsConfig::from_pem_file(&tls.cert_path, &tls.key_path)
            .await
            .with_context(|| {
                format!(
                    "failed to load TLS cert={} key={}",
                    tls.cert_path.display(),
                    tls.key_path.display()
                )
            })
    }
}

/// Build a rustls `ServerConfig` that requires clients to present a
/// certificate signed by one of the CAs in `client_ca_path`. Used when mTLS
/// is enabled.
fn build_mtls_server_config(
    cert_path: &std::path::Path,
    key_path: &std::path::Path,
    client_ca_path: &std::path::Path,
) -> Result<rustls::ServerConfig> {
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};
    use std::io::BufReader;

    let cert_file = std::fs::File::open(cert_path)
        .with_context(|| format!("failed to open cert {}", cert_path.display()))?;
    let mut cert_reader = BufReader::new(cert_file);
    let cert_chain: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut cert_reader)
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("failed to parse cert PEM {}", cert_path.display()))?;
    if cert_chain.is_empty() {
        anyhow::bail!("no certificates found in {}", cert_path.display());
    }

    let key_file = std::fs::File::open(key_path)
        .with_context(|| format!("failed to open key {}", key_path.display()))?;
    let mut key_reader = BufReader::new(key_file);
    let key: PrivateKeyDer<'static> = rustls_pemfile::private_key(&mut key_reader)
        .with_context(|| format!("failed to parse key PEM {}", key_path.display()))?
        .ok_or_else(|| anyhow::anyhow!("no private key found in {}", key_path.display()))?;

    let ca_file = std::fs::File::open(client_ca_path)
        .with_context(|| format!("failed to open client CA {}", client_ca_path.display()))?;
    let mut ca_reader = BufReader::new(ca_file);
    let mut roots = rustls::RootCertStore::empty();
    for ca in rustls_pemfile::certs(&mut ca_reader) {
        let ca = ca.with_context(|| {
            format!("failed to parse client CA PEM {}", client_ca_path.display())
        })?;
        roots
            .add(ca)
            .context("failed to add client CA to root store")?;
    }
    if roots.is_empty() {
        anyhow::bail!(
            "no certificates found in client CA bundle {}",
            client_ca_path.display()
        );
    }

    let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(roots))
        .build()
        .context("failed to build client cert verifier")?;

    rustls::ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(cert_chain, key)
        .context("failed to assemble rustls ServerConfig")
}

/// Spawn a tiny background server on `port` that 301-redirects every request
/// to the same path on the HTTPS origin. Used when the operator passes
/// `--redirect-http`.
fn spawn_http_to_https_redirect(port: u16, https_port: u16) {
    use axum::extract::Host;
    use axum::http::Uri;
    use axum::response::Redirect;

    let redirect_addr: SocketAddr = match format!("0.0.0.0:{}", port).parse() {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, "Invalid HTTP-to-HTTPS redirect address; skipping");
            return;
        }
    };

    let app = Router::new().fallback(move |Host(host): Host, uri: Uri| async move {
        // Strip any port the client sent and append the HTTPS port.
        let host_no_port = host.split(':').next().unwrap_or(&host).to_string();
        let path = uri
            .path_and_query()
            .map(|p| p.as_str())
            .unwrap_or("/")
            .to_string();
        let target = if https_port == 443 {
            format!("https://{}{}", host_no_port, path)
        } else {
            format!("https://{}:{}{}", host_no_port, https_port, path)
        };
        Redirect::permanent(&target)
    });

    tokio::spawn(async move {
        match tokio::net::TcpListener::bind(redirect_addr).await {
            Ok(listener) => {
                tracing::info!(
                    "HTTP-to-HTTPS redirector listening on {} → :{}",
                    redirect_addr,
                    https_port
                );
                if let Err(e) = axum::serve(listener, app).await {
                    tracing::warn!(error = %e, "HTTP redirector exited");
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, addr = %redirect_addr, "HTTP redirector bind failed");
            }
        }
    });
}

#[cfg(test)]
mod tls_tests {
    //! Inbound TLS tests. We exercise `build_rustls_config` against a real
    //! loopback `axum_server` listener with rcgen-generated certs.
    use super::*;
    use axum::routing::get;
    use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair, SanType};
    use std::path::Path;

    /// rcgen helper — write a self-signed cert+key into `dir`. Returns the
    /// PEM cert and the on-disk paths.
    fn write_self_signed(dir: &Path, cn: &str) -> (String, PathBuf, PathBuf) {
        let mut params = CertificateParams::new(Vec::<String>::new()).unwrap();
        let mut dn = DistinguishedName::new();
        dn.push(DnType::CommonName, cn.to_string());
        params.distinguished_name = dn;
        params
            .subject_alt_names
            .push(SanType::DnsName(cn.to_string().try_into().unwrap()));
        params
            .subject_alt_names
            .push(SanType::IpAddress("127.0.0.1".parse().unwrap()));
        let key = KeyPair::generate().unwrap();
        let cert = params.self_signed(&key).unwrap();
        let cert_pem = cert.pem();
        let key_pem = key.serialize_pem();
        let cert_path = dir.join(format!("{cn}-cert.pem"));
        let key_path = dir.join(format!("{cn}-key.pem"));
        std::fs::write(&cert_path, &cert_pem).unwrap();
        std::fs::write(&key_path, &key_pem).unwrap();
        (cert_pem, cert_path, key_path)
    }

    /// Spawn an axum-server listener on an ephemeral loopback port. Returns
    /// the bound address. The listener stops when the test exits.
    async fn spawn_https_server(
        rustls_cfg: axum_server::tls_rustls::RustlsConfig,
        app: Router,
    ) -> SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = axum_server::Handle::new();
        let handle_for_task = handle.clone();
        tokio::spawn(async move {
            let _ = axum_server::from_tcp_rustls(listener, rustls_cfg)
                .handle(handle_for_task)
                .serve(app.into_make_service_with_connect_info::<SocketAddr>())
                .await;
        });
        handle.listening().await;
        addr
    }

    fn hello_app() -> Router {
        Router::new()
            .route("/health", get(|| async { "ok" }))
            .layer(SetResponseHeaderLayer::if_not_present(
                HeaderName::from_static("strict-transport-security"),
                HeaderValue::from_static("max-age=31536000"),
            ))
    }

    #[tokio::test]
    async fn test_tls_server_serves_https() {
        let dir = tempfile::tempdir().unwrap();
        let (_pem, cert_path, key_path) = write_self_signed(dir.path(), "localhost");
        let cfg = build_rustls_config(&TlsConfig {
            cert_path,
            key_path,
            client_ca_path: None,
        })
        .await
        .expect("rustls config");
        let addr = spawn_https_server(cfg, hello_app()).await;

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();
        let resp = client
            .get(format!("https://{addr}/health"))
            .send()
            .await
            .expect("https request");
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        assert_eq!(resp.text().await.unwrap(), "ok");
    }

    #[tokio::test]
    async fn test_tls_rejects_http() {
        let dir = tempfile::tempdir().unwrap();
        let (_pem, cert_path, key_path) = write_self_signed(dir.path(), "localhost");
        let cfg = build_rustls_config(&TlsConfig {
            cert_path,
            key_path,
            client_ca_path: None,
        })
        .await
        .unwrap();
        let addr = spawn_https_server(cfg, hello_app()).await;

        let result = reqwest::Client::builder()
            .build()
            .unwrap()
            .get(format!("http://{addr}/health"))
            .timeout(std::time::Duration::from_secs(5))
            .send()
            .await;
        assert!(
            result.is_err(),
            "plain HTTP must fail against a TLS listener, got {:?}",
            result
        );
    }

    #[tokio::test]
    async fn test_hsts_header() {
        let dir = tempfile::tempdir().unwrap();
        let (_pem, cert_path, key_path) = write_self_signed(dir.path(), "localhost");
        let cfg = build_rustls_config(&TlsConfig {
            cert_path,
            key_path,
            client_ca_path: None,
        })
        .await
        .unwrap();
        let addr = spawn_https_server(cfg, hello_app()).await;

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .build()
            .unwrap();
        let resp = client
            .get(format!("https://{addr}/health"))
            .send()
            .await
            .unwrap();
        let hsts = resp
            .headers()
            .get("strict-transport-security")
            .expect("HSTS header present on TLS responses")
            .to_str()
            .unwrap();
        assert!(
            hsts.starts_with("max-age=31536000"),
            "unexpected HSTS value: {hsts}"
        );
    }

    #[tokio::test]
    async fn test_mtls_requires_client_cert() {
        let dir = tempfile::tempdir().unwrap();
        let (_server_pem, server_cert, server_key) = write_self_signed(dir.path(), "localhost");
        // Use a separate self-signed cert as the trusted client-CA bundle.
        // The point is that the test client presents *no* cert at all, so
        // the handshake fails regardless of which CAs are trusted.
        let (ca_pem, _ca_cert, _ca_key) = write_self_signed(dir.path(), "test-client-ca");
        let ca_bundle = dir.path().join("client-ca.pem");
        std::fs::write(&ca_bundle, &ca_pem).unwrap();

        let cfg = build_rustls_config(&TlsConfig {
            cert_path: server_cert,
            key_path: server_key,
            client_ca_path: Some(ca_bundle),
        })
        .await
        .expect("mTLS rustls config");
        let addr = spawn_https_server(cfg, hello_app()).await;

        let client = reqwest::Client::builder()
            .danger_accept_invalid_certs(true)
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap();
        let result = client.get(format!("https://{addr}/health")).send().await;
        assert!(
            result.is_err(),
            "mTLS server must reject clients without a cert, got {:?}",
            result
        );
    }
}
