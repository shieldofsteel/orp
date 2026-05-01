//! OIDC (OpenID Connect) client — authorization code flow with token exchange.
//!
//! Implements the ORP spec Section 6.1 flow:
//! 1. GET  /auth/login     → redirect to provider authorization URL
//! 2. GET  /auth/callback  → exchange code, set httpOnly cookie, redirect to /dashboard
//! 3. POST /auth/refresh   → exchange refresh token for new access token
//! 4. POST /auth/logout    → clear session
//!
//! Supports both Keycloak and generic OIDC providers via discovery endpoint.

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use chrono::{Duration, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use thiserror::Error;

use crate::jwt::{Claims, JwtService};

// ─── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum OidcError {
    #[error("OIDC discovery failed: {0}")]
    DiscoveryFailed(String),
    #[error("Token exchange failed: {0}")]
    TokenExchangeFailed(String),
    #[error("Token refresh failed: {0}")]
    RefreshFailed(String),
    #[error("Invalid token response: {0}")]
    InvalidTokenResponse(String),
    #[error("Provider not configured")]
    NotConfigured,
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("JWT error: {0}")]
    Jwt(#[from] crate::jwt::JwtError),
    #[error("State mismatch — possible CSRF attack")]
    StateMismatch,
}

// ─── Configuration ────────────────────────────────────────────────────────────

/// Security error type for public API surface.
#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("Token expired")]
    TokenExpired,
    #[error("Token invalid: {0}")]
    TokenInvalid(String),
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
    #[error("Forbidden: {0}")]
    Forbidden(String),
}

/// OIDC provider configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OidcConfig {
    /// Whether OIDC is enabled. If false, dev/permissive mode is used.
    pub enabled: bool,
    /// Provider discovery URL (e.g. `https://accounts.google.com`)
    pub provider_url: String,
    /// Client ID registered with the provider
    pub client_id: String,
    /// Client secret
    #[serde(default)]
    pub client_secret: String,
    /// Where the provider redirects after auth
    pub redirect_uri: String,
    /// Extra scopes to request (beyond `openid profile email`)
    #[serde(default)]
    pub extra_scopes: Vec<String>,
    /// Cookie name to use for the access token
    #[serde(default = "default_cookie_name")]
    pub cookie_name: String,
    /// JWT signing config (if we issue our own JWTs instead of using provider tokens)
    pub jwt_issuer: String,
}

fn default_cookie_name() -> String {
    "orp_token".to_string()
}

impl Default for OidcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            provider_url: String::new(),
            client_id: String::new(),
            client_secret: String::new(),
            redirect_uri: "http://localhost:9090/auth/callback".to_string(),
            extra_scopes: vec![],
            cookie_name: default_cookie_name(),
            jwt_issuer: "http://localhost:9090/auth".to_string(),
        }
    }
}

// ─── OIDC Discovery Document ─────────────────────────────────────────────────

/// Subset of the OIDC discovery document (`/.well-known/openid-configuration`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OidcDiscovery {
    pub issuer: String,
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub userinfo_endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_session_endpoint: Option<String>,
    pub jwks_uri: String,
    pub response_types_supported: Vec<String>,
    pub scopes_supported: Vec<String>,
}

// ─── Token Response ──────────────────────────────────────────────────────────

/// Token endpoint response from the OIDC provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TokenResponse {
    pub access_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    pub token_type: String,
    pub expires_in: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id_token: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

/// Token info returned to the frontend after login.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthTokenInfo {
    pub access_token: String,
    pub expires_in: u64,
    pub token_type: String,
}

// ─── Callback / Login params ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CallbackParams {
    pub code: String,
    #[serde(default)]
    pub state: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RefreshRequest {
    pub refresh_token: String,
}

// ─── OIDC Client ─────────────────────────────────────────────────────────────

/// Real OIDC client with discovery, authorization code flow, and token exchange.
#[derive(Clone, Debug)]
pub struct OidcClient {
    pub config: OidcConfig,
    http: Client,
    /// Cached discovery document with the timestamp of the last successful
    /// fetch. The cache is honoured up to `discovery_ttl`; beyond that we
    /// re-fetch so identity-provider key/endpoint rotations are picked up
    /// without a server restart.
    discovery: Option<(OidcDiscovery, std::time::Instant)>,
    discovery_ttl: std::time::Duration,
    /// Hard upper bound on how stale a cached discovery doc may be served
    /// when refresh is failing. Beyond this we fail closed rather than
    /// continue trusting (potentially-retired) keys forever. Default
    /// `discovery_ttl * 24`; configurable via
    /// `ORP_OIDC_DISCOVERY_MAX_STALENESS_SECS`.
    discovery_max_staleness: std::time::Duration,
    /// Optional local JWT service — if set, we issue our own JWTs wrapping provider claims
    jwt_service: Option<Arc<JwtService>>,
}

impl OidcClient {
    /// Create a new OIDC client.
    pub fn new(config: OidcConfig) -> Self {
        // Default 1 hour. Override via `with_discovery_ttl` or via the
        // `ORP_OIDC_DISCOVERY_TTL_SECS` env var (read in `new`).
        let discovery_ttl = std::env::var("ORP_OIDC_DISCOVERY_TTL_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .map(std::time::Duration::from_secs)
            .unwrap_or_else(|| std::time::Duration::from_secs(3600));
        // Default `discovery_ttl * 24`. Override via
        // `with_discovery_max_staleness` or via
        // `ORP_OIDC_DISCOVERY_MAX_STALENESS_SECS`.
        let discovery_max_staleness = std::env::var("ORP_OIDC_DISCOVERY_MAX_STALENESS_SECS")
            .ok()
            .and_then(|s| s.parse().ok())
            .map(std::time::Duration::from_secs)
            .unwrap_or(discovery_ttl * 24);
        Self {
            config,
            http: Client::new(),
            discovery: None,
            discovery_ttl,
            discovery_max_staleness,
            jwt_service: None,
        }
    }

    /// Override the discovery cache TTL (mostly for tests).
    pub fn with_discovery_ttl(mut self, ttl: std::time::Duration) -> Self {
        self.discovery_ttl = ttl;
        self
    }

    /// Override the discovery staleness cap (mostly for tests).
    ///
    /// Cached discovery is served up to this duration after the last
    /// successful fetch when refresh is failing; beyond it we fail closed.
    pub fn with_discovery_max_staleness(mut self, max: std::time::Duration) -> Self {
        self.discovery_max_staleness = max;
        self
    }

    /// Create with an attached JWT service for local token issuance.
    pub fn with_jwt(config: OidcConfig, jwt: Arc<JwtService>) -> Self {
        let mut client = Self::new(config);
        client.jwt_service = Some(jwt);
        client
    }

    /// Fetch and cache the OIDC discovery document.
    ///
    /// Honours the cache while it's within `discovery_ttl`. After the TTL
    /// expires we re-fetch; on fetch failure we fall back to the (still
    /// cached) document and log a warning rather than failing closed.
    pub async fn discover(&mut self) -> Result<OidcDiscovery, OidcError> {
        if !self.config.enabled {
            return Err(OidcError::NotConfigured);
        }

        if let Some((doc, fetched_at)) = &self.discovery {
            if fetched_at.elapsed() < self.discovery_ttl {
                return Ok(doc.clone());
            }
        }

        let url = format!(
            "{}/.well-known/openid-configuration",
            self.config.provider_url.trim_end_matches('/')
        );

        match self
            .http
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
        {
            Ok(resp) => match resp.json::<OidcDiscovery>().await {
                Ok(discovery) => {
                    self.discovery = Some((discovery.clone(), std::time::Instant::now()));
                    Ok(discovery)
                }
                Err(e) => self.fall_back_or_fail(OidcError::DiscoveryFailed(e.to_string())),
            },
            Err(e) => self.fall_back_or_fail(OidcError::DiscoveryFailed(e.to_string())),
        }
    }

    /// On refresh failure, return the still-cached discovery doc (with a
    /// warning) rather than failing closed — but only while the cache is
    /// within `discovery_max_staleness`. Beyond that we surface the
    /// original error: an IdP that's been unreachable for many TTLs may
    /// have rotated keys, and continuing to trust the cached doc would
    /// keep accepting tokens signed by retired keys forever.
    fn fall_back_or_fail(&self, err: OidcError) -> Result<OidcDiscovery, OidcError> {
        if let Some((doc, fetched_at)) = &self.discovery {
            let age = fetched_at.elapsed();
            if age < self.discovery_max_staleness {
                tracing::warn!(
                    error = %err,
                    age_secs = age.as_secs(),
                    "OIDC discovery refresh failed; using cached document"
                );
                Ok(doc.clone())
            } else {
                tracing::error!(
                    error = %err,
                    age_secs = age.as_secs(),
                    max_staleness_secs = self.discovery_max_staleness.as_secs(),
                    "OIDC discovery cache exceeded max-staleness cap; failing closed"
                );
                Err(err)
            }
        } else {
            Err(err)
        }
    }

    /// Get the authorization redirect URL to send the browser to.
    pub fn authorization_url(&self, state: &str) -> Result<String, OidcError> {
        if !self.config.enabled {
            return Err(OidcError::NotConfigured);
        }

        let discovery = self
            .discovery
            .as_ref()
            .map(|(doc, _)| doc)
            .ok_or_else(|| OidcError::DiscoveryFailed("Discovery not loaded".into()))?;

        let mut scopes = vec!["openid", "profile", "email"];
        for s in &self.config.extra_scopes {
            scopes.push(s.as_str());
        }
        let scope_str = scopes.join(" ");

        let url = format!(
            "{}?response_type=code&client_id={}&redirect_uri={}&scope={}&state={}",
            discovery.authorization_endpoint,
            urlencoding_simple(&self.config.client_id),
            urlencoding_simple(&self.config.redirect_uri),
            urlencoding_simple(&scope_str),
            urlencoding_simple(state),
        );

        Ok(url)
    }

    /// Exchange an authorization code for tokens.
    pub async fn exchange_code(&self, code: &str) -> Result<TokenResponse, OidcError> {
        let discovery = self
            .discovery
            .as_ref()
            .map(|(doc, _)| doc)
            .ok_or_else(|| OidcError::DiscoveryFailed("Discovery not loaded".into()))?;

        let params = [
            ("grant_type", "authorization_code"),
            ("code", code),
            ("redirect_uri", &self.config.redirect_uri),
            ("client_id", &self.config.client_id),
            ("client_secret", &self.config.client_secret),
        ];

        let resp = self
            .http
            .post(&discovery.token_endpoint)
            .form(&params)
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OidcError::TokenExchangeFailed(body));
        }

        let token_response: TokenResponse = resp
            .json()
            .await
            .map_err(|e| OidcError::InvalidTokenResponse(e.to_string()))?;

        Ok(token_response)
    }

    /// Refresh an access token using a refresh token.
    pub async fn refresh_token(&self, refresh_token: &str) -> Result<TokenResponse, OidcError> {
        let discovery = self
            .discovery
            .as_ref()
            .map(|(doc, _)| doc)
            .ok_or_else(|| OidcError::DiscoveryFailed("Discovery not loaded".into()))?;

        let params = [
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
            ("client_id", &self.config.client_id),
            ("client_secret", &self.config.client_secret),
        ];

        let resp = self
            .http
            .post(&discovery.token_endpoint)
            .form(&params)
            .send()
            .await?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OidcError::RefreshFailed(body));
        }

        let token_response: TokenResponse = resp
            .json()
            .await
            .map_err(|e| OidcError::InvalidTokenResponse(e.to_string()))?;

        Ok(token_response)
    }

    /// Validate a bearer token.
    ///
    /// If a local `JwtService` is configured, validates locally (fast path).
    /// Otherwise returns permissive dev claims when OIDC is disabled.
    pub fn validate_token(&self, token: &str) -> Result<Claims, SecurityError> {
        if let Some(jwt) = &self.jwt_service {
            return jwt.validate_token(token).map_err(|e| match e {
                crate::jwt::JwtError::TokenExpired => SecurityError::TokenExpired,
                other => SecurityError::TokenInvalid(other.to_string()),
            });
        }

        if !self.config.enabled {
            // Permissive dev mode
            let now = Utc::now().timestamp();
            Ok(Claims {
                sub: "anonymous".to_string(),
                email: None,
                name: Some("Anonymous User (dev mode)".to_string()),
                iat: now,
                nbf: Some(now),
                exp: (Utc::now() + Duration::hours(1)).timestamp(),
                iss: self.config.jwt_issuer.clone(),
                aud: "orp-client".to_string(),
                scope: "api:read api:write entities:read entities:write graph:read monitors:read monitors:write query:execute admin".to_string(),
                org_id: None,
                permissions: vec![
                    "entities:read".to_string(),
                    "entities:write".to_string(),
                    "graph:read".to_string(),
                    "monitors:read".to_string(),
                    "monitors:write".to_string(),
                    "query:execute".to_string(),
                    "admin".to_string(),
                ],
            })
        } else {
            Err(SecurityError::TokenInvalid(
                "OIDC provider configured but no local JWT service available for validation. \
                 Use an OIDC JWT validator or configure a JwtService."
                    .to_string(),
            ))
        }
    }

    /// Build the httpOnly cookie Set-Cookie header value.
    pub fn build_auth_cookie(&self, token: &str, expires_in: u64) -> String {
        format!(
            "{}={}; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age={}",
            self.config.cookie_name, token, expires_in
        )
    }

    /// Build the logout cookie (clears the auth cookie).
    pub fn clear_auth_cookie(&self) -> String {
        format!(
            "{}=; HttpOnly; Secure; SameSite=Lax; Path=/; Max-Age=0",
            self.config.cookie_name
        )
    }
}

// ─── Axum Route Handlers ──────────────────────────────────────────────────────

/// Shared state for auth routes.
#[derive(Clone)]
pub struct AuthRouterState {
    pub oidc: Arc<tokio::sync::RwLock<OidcClient>>,
}

/// Build the auth sub-router — attach to the main Axum app.
///
/// ```rust,ignore
/// let app = Router::new()
///     .merge(oidc_router(auth_state))
///     .with_state(app_state);
/// ```
pub fn oidc_router(state: AuthRouterState) -> Router {
    Router::new()
        .route("/auth/login", get(handle_login))
        .route("/auth/callback", get(handle_callback))
        .route("/auth/refresh", axum::routing::post(handle_refresh))
        .route("/auth/logout", axum::routing::post(handle_logout))
        .with_state(state)
}

/// Build a signed CSRF cookie value: state|HMAC(state, secret).
fn sign_csrf_state(state_val: &str, secret: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(state_val.as_bytes());
    hasher.update(secret.as_bytes());
    let sig = hex::encode(hasher.finalize());
    format!("{}|{}", state_val, sig)
}

/// Verify and extract the CSRF state from a signed cookie value.
fn verify_csrf_cookie(cookie_val: &str, secret: &str) -> Option<String> {
    let parts: Vec<&str> = cookie_val.splitn(2, '|').collect();
    if parts.len() != 2 {
        return None;
    }
    let state_val = parts[0];
    let expected = sign_csrf_state(state_val, secret);
    if expected == cookie_val {
        Some(state_val.to_string())
    } else {
        None
    }
}

/// CSRF cookie secret — derived from client_secret or a fixed dev key.
fn csrf_secret(oidc: &OidcClient) -> String {
    if oidc.config.client_secret.is_empty() {
        "orp-dev-csrf-secret".to_string()
    } else {
        format!("orp-csrf-{}", &oidc.config.client_secret)
    }
}

/// GET /auth/login — redirect to OIDC provider.
async fn handle_login(State(state): State<AuthRouterState>) -> Response {
    let oidc = state.oidc.read().await;

    if !oidc.config.enabled {
        return (
            StatusCode::OK,
            Json(serde_json::json!({
                "message": "OIDC disabled — dev mode. Use any Bearer token.",
                "dev_mode": true
            })),
        )
            .into_response();
    }

    // Generate a random state parameter (CSRF protection)
    let csrf_state = generate_csrf_state();

    match oidc.authorization_url(&csrf_state) {
        Ok(url) => {
            // Sign and store CSRF state in an httpOnly cookie
            let secret = csrf_secret(&oidc);
            let signed = sign_csrf_state(&csrf_state, &secret);
            let csrf_cookie = format!(
                "orp_csrf={}; HttpOnly; Secure; SameSite=Lax; Path=/auth; Max-Age=600",
                signed
            );

            let mut headers = HeaderMap::new();
            if let Ok(val) = csrf_cookie.parse() {
                headers.insert(header::SET_COOKIE, val);
            }
            if let Ok(val) = url.parse() {
                headers.insert(header::LOCATION, val);
            }

            (StatusCode::FOUND, headers).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": {
                    "code": "INTERNAL_ERROR",
                    "status": 500,
                    "message": format!("Auth configuration error: {e}"),
                    "timestamp": Utc::now().to_rfc3339()
                }
            })),
        )
            .into_response(),
    }
}

/// GET /auth/callback?code=xxx&state=xxx — exchange code for tokens.
async fn handle_callback(
    State(state): State<AuthRouterState>,
    headers_map: HeaderMap,
    Query(params): Query<CallbackParams>,
) -> Response {
    let oidc = state.oidc.read().await;

    if !oidc.config.enabled {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": {
                    "code": "INVALID_REQUEST",
                    "status": 400,
                    "message": "OIDC is disabled",
                    "timestamp": Utc::now().to_rfc3339()
                }
            })),
        )
            .into_response();
    }

    // Verify CSRF state from signed cookie
    let secret = csrf_secret(&oidc);
    let csrf_cookie_val = headers_map
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|cookies| {
            cookies.split(';').find_map(|c| {
                let c = c.trim();
                c.strip_prefix("orp_csrf=").map(|v| v.to_string())
            })
        });

    if let Some(ref callback_state) = params.state {
        match csrf_cookie_val {
            Some(ref cookie_val) => {
                let verified_state = verify_csrf_cookie(cookie_val, &secret);
                match verified_state {
                    Some(ref stored_state) if stored_state == callback_state => {
                        // CSRF verified — proceed
                    }
                    _ => {
                        return (
                            StatusCode::FORBIDDEN,
                            Json(serde_json::json!({
                                "error": {
                                    "code": "CSRF_MISMATCH",
                                    "status": 403,
                                    "message": "CSRF state mismatch — possible CSRF attack",
                                    "timestamp": Utc::now().to_rfc3339()
                                }
                            })),
                        )
                            .into_response();
                    }
                }
            }
            None => {
                return (
                    StatusCode::FORBIDDEN,
                    Json(serde_json::json!({
                        "error": {
                            "code": "CSRF_MISSING",
                            "status": 403,
                            "message": "Missing CSRF cookie — possible CSRF attack",
                            "timestamp": Utc::now().to_rfc3339()
                        }
                    })),
                )
                    .into_response();
            }
        }
    }

    match oidc.exchange_code(&params.code).await {
        Ok(tokens) => {
            let cookie = oidc.build_auth_cookie(&tokens.access_token, tokens.expires_in);
            // Clear the CSRF cookie
            let clear_csrf = "orp_csrf=; HttpOnly; Secure; SameSite=Lax; Path=/auth; Max-Age=0";
            let mut headers = HeaderMap::new();
            if let Ok(val) = cookie.parse() {
                headers.insert(header::SET_COOKIE, val);
            }
            if let Ok(val) = clear_csrf.parse() {
                headers.append(header::SET_COOKIE, val);
            }
            (
                StatusCode::FOUND,
                headers,
                [(header::LOCATION, "/dashboard")],
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": {
                    "code": "UNAUTHORIZED",
                    "status": 401,
                    "message": format!("Token exchange failed: {e}"),
                    "timestamp": Utc::now().to_rfc3339()
                }
            })),
        )
            .into_response(),
    }
}

/// POST /auth/refresh — refresh the access token.
async fn handle_refresh(
    State(state): State<AuthRouterState>,
    Json(body): Json<RefreshRequest>,
) -> Response {
    let oidc = state.oidc.read().await;

    match oidc.refresh_token(&body.refresh_token).await {
        Ok(tokens) => {
            let cookie = oidc.build_auth_cookie(&tokens.access_token, tokens.expires_in);
            let mut headers = HeaderMap::new();
            if let Ok(val) = cookie.parse() {
                headers.insert(header::SET_COOKIE, val);
            }
            (
                StatusCode::OK,
                headers,
                Json(AuthTokenInfo {
                    access_token: tokens.access_token,
                    expires_in: tokens.expires_in,
                    token_type: tokens.token_type,
                }),
            )
                .into_response()
        }
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({
                "error": {
                    "code": "UNAUTHORIZED",
                    "status": 401,
                    "message": format!("Token refresh failed: {e}"),
                    "timestamp": Utc::now().to_rfc3339()
                }
            })),
        )
            .into_response(),
    }
}

/// POST /auth/logout — clear auth cookie.
async fn handle_logout(State(state): State<AuthRouterState>) -> Response {
    let oidc = state.oidc.read().await;
    let clear_cookie = oidc.clear_auth_cookie();
    let mut headers = HeaderMap::new();
    if let Ok(val) = clear_cookie.parse() {
        headers.insert(header::SET_COOKIE, val);
    }
    (
        StatusCode::OK,
        headers,
        Json(serde_json::json!({ "message": "Logged out" })),
    )
        .into_response()
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Generate a random CSRF state string.
///
/// Uses `OsRng` (the OS CSPRNG) — never `thread_rng()`. The CSRF state guards
/// `/auth/callback` against forged callbacks; if an attacker can predict the
/// state value, they can mount a CSRF attack on the authorization-code flow.
/// `thread_rng()` is seeded from the OS but its internal state is recoverable
/// from a few outputs, so it must not be used here. We draw 32 bytes and
/// encode as URL-safe base64 (no padding) → 43 chars, ~256 bits of entropy.
fn generate_csrf_state() -> String {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
    use rand::{rngs::OsRng, RngCore};
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Minimal URL encoding (replaces only the most important characters).
fn urlencoding_simple(s: &str) -> String {
    s.replace(' ', "%20")
        .replace(':', "%3A")
        .replace('/', "%2F")
        .replace('@', "%40")
        .replace('=', "%3D")
        .replace('&', "%26")
        .replace('+', "%2B")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let cfg = OidcConfig::default();
        assert!(!cfg.enabled);
        assert_eq!(cfg.cookie_name, "orp_token");
    }

    #[test]
    fn test_validate_token_dev_mode() {
        let cfg = OidcConfig::default();
        let client = OidcClient::new(cfg);
        let claims = client.validate_token("any-token").unwrap();
        assert_eq!(claims.sub, "anonymous");
        assert!(claims.permissions.contains(&"admin".to_string()));
        assert!(claims.is_valid());
    }

    #[test]
    fn test_validate_token_enabled_no_jwt_service_fails() {
        let cfg = OidcConfig {
            enabled: true,
            provider_url: "https://example.com".to_string(),
            ..Default::default()
        };
        let client = OidcClient::new(cfg);
        let result = client.validate_token("fake-token");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_token_with_jwt_service() {
        use crate::jwt::JwtService;

        let jwt_svc = Arc::new(
            JwtService::new(crate::jwt::JwtConfig {
                hs256_secret: Some(b"test-secret-for-unit-tests-32bytes!".to_vec()),
                ..crate::jwt::JwtConfig::default()
            })
            .unwrap(),
        );
        let token = jwt_svc
            .issue_token(
                "user-1",
                Some("test@example.com"),
                None,
                None,
                vec!["entities:read".to_string()],
            )
            .unwrap();

        let cfg = OidcConfig::default();
        let client = OidcClient::with_jwt(cfg, jwt_svc);
        let claims = client.validate_token(&token).unwrap();
        assert_eq!(claims.sub, "user-1");
    }

    #[test]
    fn test_build_auth_cookie() {
        let cfg = OidcConfig::default();
        let client = OidcClient::new(cfg);
        let cookie = client.build_auth_cookie("tok123", 3600);
        assert!(cookie.contains("orp_token=tok123"));
        assert!(cookie.contains("HttpOnly"));
        assert!(cookie.contains("Secure"));
        assert!(cookie.contains("Max-Age=3600"));
    }

    #[test]
    fn test_clear_auth_cookie() {
        let cfg = OidcConfig::default();
        let client = OidcClient::new(cfg);
        let cookie = client.clear_auth_cookie();
        assert!(cookie.contains("Max-Age=0"));
        assert!(cookie.contains("orp_token="));
    }

    #[test]
    fn test_generate_csrf_state_unique() {
        let s1 = generate_csrf_state();
        let s2 = generate_csrf_state();
        assert_ne!(s1, s2);
        // 32 random bytes → URL-safe base64 (no pad) = 43 chars
        assert_eq!(s1.len(), 43);
    }

    /// CSRF state must come from a CSPRNG (`OsRng`) and have full entropy.
    /// We generate 1000 states and assert (a) no collisions and (b) every
    /// state decodes to ≥32 bytes of base64url. A non-cryptographic RNG
    /// (e.g. `thread_rng()` configured with a small state) would either
    /// produce shorter outputs or — far more importantly — be predictable
    /// from a handful of observations, which is the actual attack we're
    /// blocking. We can't easily test "non-predictable" directly, so the
    /// regression guard is the explicit `OsRng` import in the function
    /// body and the format assertion below.
    #[test]
    fn test_csrf_state_uses_osrng_high_entropy() {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        use std::collections::HashSet;
        let mut seen: HashSet<String> = HashSet::new();
        for _ in 0..1000 {
            let s = generate_csrf_state();
            // URL-safe base64 (no pad) of 32 bytes = 43 chars
            assert_eq!(s.len(), 43, "state must be 43 chars: {s:?}");
            // Every char must be valid url-safe-base64
            assert!(
                s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
                "non-url-safe-base64 char in state: {s:?}"
            );
            // And it must round-trip to ≥ 32 bytes
            let raw = URL_SAFE_NO_PAD.decode(&s).expect("decodes");
            assert_eq!(raw.len(), 32, "state must encode 32 bytes");
            assert!(seen.insert(s), "duplicate CSRF state — RNG is broken");
        }
    }

    #[test]
    fn test_authorization_url_requires_discovery() {
        let cfg = OidcConfig {
            enabled: true,
            ..Default::default()
        };
        let client = OidcClient::new(cfg);
        // No discovery loaded
        let result = client.authorization_url("state-123");
        assert!(result.is_err());
    }

    #[test]
    fn test_authorization_url_disabled() {
        let cfg = OidcConfig::default();
        let client = OidcClient::new(cfg);
        let result = client.authorization_url("state-123");
        assert!(matches!(result, Err(OidcError::NotConfigured)));
    }

    // ─── Discovery cache / staleness tests ───────────────────────────────
    //
    // These tests stand up a tiny TCP listener that speaks just enough HTTP
    // to answer `/.well-known/openid-configuration`. We avoid pulling in
    // wiremock/hyper-server to stay within the LoC budget.

    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Response policy for the mock server. Each connection consumes one
    /// entry; we wrap with a counter so tests can assert request counts.
    #[derive(Clone, Copy)]
    enum MockResp {
        OkDoc,
        Status503,
    }

    /// Spawn a mock OIDC discovery server on a random port. Returns the
    /// `http://127.0.0.1:port` base URL plus a counter the caller can read
    /// to verify request counts. Each entry in `script` is replayed in
    /// order; once exhausted, the last entry repeats.
    async fn spawn_mock_idp(script: Vec<MockResp>) -> (String, Arc<AtomicUsize>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let counter = Arc::new(AtomicUsize::new(0));
        let counter_c = counter.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(p) => p,
                    Err(_) => break,
                };
                let idx = counter_c.fetch_add(1, Ordering::SeqCst);
                let resp = script
                    .get(idx)
                    .copied()
                    .unwrap_or_else(|| *script.last().unwrap_or(&MockResp::Status503));
                tokio::spawn(async move {
                    // Drain the request just enough to know the client
                    // sent something. We don't bother parsing it.
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    let body = match resp {
                        MockResp::OkDoc => serde_json::json!({
                            "issuer": "http://idp.test",
                            "authorization_endpoint": "http://idp.test/authorize",
                            "token_endpoint": "http://idp.test/token",
                            "jwks_uri": "http://idp.test/jwks",
                            "response_types_supported": ["code"],
                            "scopes_supported": ["openid"],
                        })
                        .to_string(),
                        MockResp::Status503 => String::new(),
                    };
                    let (status_line, body_to_send) = match resp {
                        MockResp::OkDoc => ("HTTP/1.1 200 OK", body.as_str()),
                        MockResp::Status503 => ("HTTP/1.1 503 Service Unavailable", ""),
                    };
                    let response = format!(
                        "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body_to_send.len(),
                        body_to_send
                    );
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (format!("http://127.0.0.1:{port}"), counter)
    }

    fn enabled_cfg(provider_url: String) -> OidcConfig {
        OidcConfig {
            enabled: true,
            provider_url,
            client_id: "test-client".into(),
            client_secret: "test-secret".into(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_discover_caches_within_ttl() {
        let (base, counter) = spawn_mock_idp(vec![MockResp::OkDoc, MockResp::OkDoc]).await;
        let mut client = OidcClient::new(enabled_cfg(base))
            .with_discovery_ttl(std::time::Duration::from_millis(500));
        client.discover().await.expect("first fetch ok");
        client.discover().await.expect("second fetch ok (cached)");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            1,
            "second call must be served from cache"
        );
    }

    #[tokio::test]
    async fn test_discover_refetches_after_ttl() {
        let (base, counter) =
            spawn_mock_idp(vec![MockResp::OkDoc, MockResp::OkDoc, MockResp::OkDoc]).await;
        let mut client = OidcClient::new(enabled_cfg(base))
            .with_discovery_ttl(std::time::Duration::from_millis(50));
        client.discover().await.expect("first fetch ok");
        tokio::time::sleep(std::time::Duration::from_millis(120)).await;
        client.discover().await.expect("refetch ok");
        assert_eq!(
            counter.load(Ordering::SeqCst),
            2,
            "TTL expiry must trigger a re-fetch"
        );
    }

    #[tokio::test]
    async fn test_discover_falls_back_on_5xx_after_first_success() {
        let (base, counter) = spawn_mock_idp(vec![MockResp::OkDoc, MockResp::Status503]).await;
        let mut client = OidcClient::new(enabled_cfg(base))
            .with_discovery_ttl(std::time::Duration::from_millis(20))
            // Generous staleness cap so the fallback is allowed.
            .with_discovery_max_staleness(std::time::Duration::from_secs(60));
        let first = client.discover().await.expect("first fetch ok");
        assert_eq!(first.issuer, "http://idp.test");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Second fetch hits the 503; fall_back_or_fail should serve cache.
        let second = client.discover().await.expect("must fall back to cache");
        assert_eq!(second.issuer, "http://idp.test");
        assert!(counter.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn test_discover_fails_closed_when_no_cache_and_5xx() {
        let (base, _counter) = spawn_mock_idp(vec![MockResp::Status503]).await;
        let mut client = OidcClient::new(enabled_cfg(base));
        let result = client.discover().await;
        assert!(
            matches!(result, Err(OidcError::DiscoveryFailed(_))),
            "must fail closed without a cached doc, got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_discover_max_staleness_cap() {
        let (base, _counter) = spawn_mock_idp(vec![MockResp::OkDoc, MockResp::Status503]).await;
        // TTL 10ms; staleness cap 30ms. After we sleep 100ms (well past
        // cap) and the IdP returns 503, `discover` must Err.
        let mut client = OidcClient::new(enabled_cfg(base))
            .with_discovery_ttl(std::time::Duration::from_millis(10))
            .with_discovery_max_staleness(std::time::Duration::from_millis(30));
        client
            .discover()
            .await
            .expect("first fetch primes the cache");
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        let result = client.discover().await;
        assert!(
            matches!(result, Err(OidcError::DiscoveryFailed(_))),
            "staleness cap must fire and fail closed, got {result:?}"
        );
    }
}
