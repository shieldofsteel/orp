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
    /// Cached discovery document
    discovery: Option<OidcDiscovery>,
    /// Optional local JWT service — if set, we issue our own JWTs wrapping provider claims
    jwt_service: Option<Arc<JwtService>>,
}

impl OidcClient {
    /// Create a new OIDC client.
    pub fn new(config: OidcConfig) -> Self {
        Self {
            config,
            http: Client::new(),
            discovery: None,
            jwt_service: None,
        }
    }

    /// Create with an attached JWT service for local token issuance.
    pub fn with_jwt(config: OidcConfig, jwt: Arc<JwtService>) -> Self {
        let mut client = Self::new(config);
        client.jwt_service = Some(jwt);
        client
    }

    /// Fetch and cache the OIDC discovery document.
    pub async fn discover(&mut self) -> Result<OidcDiscovery, OidcError> {
        if !self.config.enabled {
            return Err(OidcError::NotConfigured);
        }

        let url = format!(
            "{}/.well-known/openid-configuration",
            self.config.provider_url.trim_end_matches('/')
        );

        let discovery: OidcDiscovery = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| OidcError::DiscoveryFailed(e.to_string()))?
            .json()
            .await
            .map_err(|e| OidcError::DiscoveryFailed(e.to_string()))?;

        self.discovery = Some(discovery.clone());
        Ok(discovery)
    }

    /// Get the authorization redirect URL to send the browser to.
    pub fn authorization_url(&self, state: &str) -> Result<String, OidcError> {
        if !self.config.enabled {
            return Err(OidcError::NotConfigured);
        }

        let discovery = self
            .discovery
            .as_ref()
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
            return jwt
                .validate_token(token)
                .map_err(|e| match e {
                    crate::jwt::JwtError::TokenExpired => SecurityError::TokenExpired,
                    other => SecurityError::TokenInvalid(other.to_string()),
                });
        }

        if !self.config.enabled {
            // Permissive dev mode
            Ok(Claims {
                sub: "anonymous".to_string(),
                email: None,
                name: Some("Anonymous User (dev mode)".to_string()),
                iat: Utc::now().timestamp(),
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
    pub fn build_auth_cookie(
        &self,
        token: &str,
        expires_in: u64,
    ) -> String {
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
async fn handle_login(
    State(state): State<AuthRouterState>,
) -> Response {
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
            cookies
                .split(';')
                .find_map(|c| {
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
fn generate_csrf_state() -> String {
    use rand::Rng;
    let bytes: Vec<u8> = (0..16).map(|_| rand::thread_rng().gen::<u8>()).collect();
    hex::encode(bytes)
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

        let jwt_svc = Arc::new(JwtService::new(crate::jwt::JwtConfig {
            hs256_secret: Some(b"test-secret-for-unit-tests-32bytes!".to_vec()),
            ..crate::jwt::JwtConfig::default()
        }).unwrap());
        let token = jwt_svc
            .issue_token("user-1", Some("test@example.com"), None, None, vec!["entities:read".to_string()])
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
        assert_eq!(s1.len(), 32); // 16 bytes → 32 hex chars
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
}
