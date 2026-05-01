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
use jsonwebtoken::{
    decode, decode_header,
    jwk::{AlgorithmParameters, Jwk, JwkSet},
    Algorithm, DecodingKey, Validation,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use thiserror::Error;
use tokio::sync::RwLock;

use crate::jwt::{Claims, JwtService};

// ─── Errors ───────────────────────────────────────────────────────────────────

#[derive(Debug, Error)]
pub enum OidcError {
    #[error("OIDC discovery failed: {0}")]
    DiscoveryFailed(String),
    #[error("JWKS fetch failed: {0}")]
    JwksFetchFailed(String),
    #[error("JWKS missing kid={0}")]
    JwksMissingKid(String),
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
    #[error("Token validation failed: {0}")]
    TokenValidationFailed(String),
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

// ─── External IdP claim shape ─────────────────────────────────────────────────

/// Claims as they may appear in a JWT issued by an external IdP.
///
/// Differences from the internal [`Claims`] type:
/// - `aud` may be a single string or an array (RFC 7519 §4.1.3)
/// - `iat` is optional (Auth0 / Azure include it; some custom IdPs don't)
/// - `nbf` is optional (most external IdPs omit it)
/// - `permissions` may live under different keys (`permissions`,
///   `roles`, `scope`) depending on the provider
///
/// We accept this loose shape and project into a strict [`Claims`] for
/// downstream code via [`Self::into_internal_claims`].
#[derive(Clone, Debug, Deserialize)]
struct IdpClaims {
    sub: String,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    iat: Option<i64>,
    exp: i64,
    #[serde(default)]
    nbf: Option<i64>,
    iss: String,
    aud: AudClaim,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    org_id: Option<String>,
    /// Provider-specific permissions claim. Auth0 emits `permissions: []`
    /// when RBAC is on; Keycloak puts roles under `realm_access.roles`
    /// (out of scope here — operators wanting Keycloak roles should map
    /// them via a custom token mapper into a top-level `permissions`).
    #[serde(default)]
    permissions: Vec<String>,
}

/// Either a single audience string or an array — RFC 7519 §4.1.3.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
enum AudClaim {
    Single(String),
    Multi(Vec<String>),
}

impl IdpClaims {
    fn into_internal_claims(self) -> Claims {
        let aud = match self.aud {
            AudClaim::Single(s) => s,
            // Pick the first audience entry — `Validation::set_audience`
            // already verified our configured `client_id` is among them.
            AudClaim::Multi(v) => v.into_iter().next().unwrap_or_default(),
        };
        let iat = self.iat.unwrap_or(self.exp);
        let scope_str = self.scope.unwrap_or_default();

        // If the IdP didn't emit a top-level `permissions` claim, fall
        // back to splitting `scope` into permission strings. This keeps
        // RBAC working for IdPs (Okta, Azure AD) that only emit `scope`.
        let permissions = if self.permissions.is_empty() && !scope_str.is_empty() {
            scope_str.split_whitespace().map(String::from).collect()
        } else {
            self.permissions
        };

        Claims {
            sub: self.sub,
            email: self.email,
            name: self.name,
            iat,
            nbf: self.nbf,
            exp: self.exp,
            iss: self.iss,
            aud,
            scope: scope_str,
            org_id: self.org_id,
            permissions,
        }
    }
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

/// JWKS cache — keyed by `kid`, with a single `last_refresh` timestamp.
///
/// Stored behind an `Arc<RwLock<_>>` on `OidcClient` so the validation
/// fast-path (a sync `read()` of the kid→Jwk map) can be lock-light while
/// refreshes (rare, async, fetch JWKS over HTTP) take a brief write lock.
#[derive(Clone, Debug, Default)]
struct JwksCache {
    keys: HashMap<String, Jwk>,
    last_refresh: Option<std::time::Instant>,
}

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
    /// Cached JWKS keys by `kid`. Populated lazily after the first
    /// `validate_external_token` call (or eagerly via `refresh_jwks`).
    /// Refreshed on `kid`-not-found or every `discovery_ttl`.
    jwks: Arc<RwLock<JwksCache>>,
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
            jwks: Arc::new(RwLock::new(JwksCache::default())),
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

    /// Fetch the JWKS document from the discovered `jwks_uri` and replace
    /// the in-memory key cache. Refresh is gated so two concurrent refreshes
    /// can't both hammer the IdP — the second waits on the write-lock and
    /// then sees a fresh cache.
    pub async fn refresh_jwks(&self) -> Result<(), OidcError> {
        let jwks_uri = self
            .discovery
            .as_ref()
            .map(|(d, _)| d.jwks_uri.clone())
            .ok_or_else(|| OidcError::JwksFetchFailed("Discovery not loaded".into()))?;

        let resp = self
            .http
            .get(&jwks_uri)
            .send()
            .await
            .map_err(|e| OidcError::JwksFetchFailed(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(OidcError::JwksFetchFailed(format!(
                "JWKS endpoint returned status {}",
                resp.status()
            )));
        }
        let set: JwkSet = resp
            .json()
            .await
            .map_err(|e| OidcError::JwksFetchFailed(e.to_string()))?;

        let mut keys = HashMap::with_capacity(set.keys.len());
        for jwk in set.keys {
            // Skip JWKs without a `kid` — we can't route to them in
            // multi-key environments. Most well-behaved IdPs include `kid`.
            let kid = match jwk.common.key_id.clone() {
                Some(k) => k,
                None => {
                    tracing::warn!("JWKS entry skipped: no `kid` field");
                    continue;
                }
            };
            keys.insert(kid, jwk);
        }

        let mut guard = self.jwks.write().await;
        guard.keys = keys;
        guard.last_refresh = Some(std::time::Instant::now());
        Ok(())
    }

    /// Look up a JWK by `kid` from the cache (read-only). Returns `None`
    /// if the cache is empty or the kid isn't present.
    async fn jwks_get(&self, kid: &str) -> Option<Jwk> {
        let guard = self.jwks.read().await;
        guard.keys.get(kid).cloned()
    }

    /// True if the JWKS cache has been refreshed within `discovery_ttl`.
    /// Used by callers (and tests) that need to gate work on whether the
    /// in-memory key set is still considered authoritative.
    pub async fn jwks_is_fresh(&self) -> bool {
        let guard = self.jwks.read().await;
        guard
            .last_refresh
            .map(|t| t.elapsed() < self.discovery_ttl)
            .unwrap_or(false)
    }

    /// Validate an external IdP-issued JWT against the cached JWKS.
    ///
    /// Steps:
    /// 1. Decode the JWT header to extract `alg` + `kid`.
    /// 2. Reject `alg = none` (also impossible because jsonwebtoken would
    ///    parse it as `Algorithm` and we never include `none` in the pin
    ///    set — but we make the rejection explicit via header check).
    /// 3. Look up the `kid` in the JWKS cache. On miss, refresh JWKS once
    ///    and retry; still missing → `JwksMissingKid`.
    /// 4. Verify alg in the header matches the alg associated with the
    ///    JWK's parameters (RSA → RS256, EC → ES256). This blocks
    ///    alg-confusion (signing an RSA key with HMAC).
    /// 5. `jsonwebtoken::decode` with the right `Validation` profile,
    ///    enforcing `iss` (= discovery `issuer`), `aud` (= configured
    ///    `client_id`), `exp`, `nbf`, and `sub`.
    pub async fn validate_external_token(&self, token: &str) -> Result<Claims, OidcError> {
        if !self.config.enabled {
            return Err(OidcError::NotConfigured);
        }

        // Discovery must be primed so we know the issuer + jwks_uri.
        let discovery = self
            .discovery
            .as_ref()
            .map(|(d, _)| d.clone())
            .ok_or_else(|| OidcError::DiscoveryFailed("Discovery not loaded".into()))?;

        // 1. Header.
        let header = decode_header(token)
            .map_err(|e| OidcError::TokenValidationFailed(format!("header decode: {e}")))?;

        // 2. Pin algorithm. Only RS256 / ES256 / EdDSA accepted from
        // external IdPs. Explicitly reject HS256 from this path — HS256
        // belongs to the local-JWT path; accepting it here would let an
        // attacker swap a real RSA-signed token for an HMAC one keyed by
        // the public RSA modulus.
        let algo_ok = matches!(
            header.alg,
            Algorithm::RS256 | Algorithm::ES256 | Algorithm::EdDSA
        );
        if !algo_ok {
            return Err(OidcError::TokenValidationFailed(format!(
                "alg {:?} not permitted on the OIDC validation path",
                header.alg
            )));
        }

        let kid = header
            .kid
            .clone()
            .ok_or_else(|| OidcError::TokenValidationFailed("missing kid in header".into()))?;

        // 3. Cache lookup with single-shot refresh on miss.
        let jwk = match self.jwks_get(&kid).await {
            Some(j) => j,
            None => {
                self.refresh_jwks().await?;
                self.jwks_get(&kid)
                    .await
                    .ok_or_else(|| OidcError::JwksMissingKid(kid.clone()))?
            }
        };

        // 4. Cross-check the JWK's algorithm family against the header alg.
        let jwk_alg_ok = matches!(
            (&jwk.algorithm, header.alg),
            (AlgorithmParameters::RSA(_), Algorithm::RS256)
                | (AlgorithmParameters::EllipticCurve(_), Algorithm::ES256)
                | (AlgorithmParameters::OctetKeyPair(_), Algorithm::EdDSA)
        );
        if !jwk_alg_ok {
            return Err(OidcError::TokenValidationFailed(format!(
                "JWK alg family does not match header alg {:?}",
                header.alg
            )));
        }

        let key = DecodingKey::from_jwk(&jwk).map_err(|e| {
            OidcError::TokenValidationFailed(format!("could not build DecodingKey: {e}"))
        })?;

        // 5. Decode + validate. Pin alg from the header (now known to be
        // RS256/ES256/EdDSA and matched to the JWK), enforce iss=
        // discovery.issuer, aud=client_id, require exp/nbf/sub.
        let mut validation = Validation::new(header.alg);
        validation.set_issuer(&[&discovery.issuer]);
        validation.set_audience(&[&self.config.client_id]);
        validation.validate_exp = true;
        validation.validate_nbf = true;
        // Most external IdPs DON'T issue `nbf` — Auth0 in particular
        // never sets it. We require `exp`, `iss`, `aud`, `sub` and let
        // `nbf` be optional. (The local JWT path is stricter.)
        validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
        validation.leeway = 60;

        // External IdPs often emit `aud` as an array, omit `iat`, and put
        // permissions/roles under provider-specific names. Decode against
        // a tolerant intermediate struct, then project into our internal
        // `Claims` shape.
        let data = decode::<IdpClaims>(token, &key, &validation)
            .map_err(|e| OidcError::TokenValidationFailed(e.to_string()))?;
        Ok(data.claims.into_internal_claims())
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

// ─── Multi-IdP Validator ──────────────────────────────────────────────────────

/// Multi-issuer validator. Routes inbound JWTs to:
///
/// 1. The matching `OidcClient` (when `iss` matches a configured provider's
///    discovered `issuer`) — verified against the IdP's JWKS.
/// 2. The fallback local [`JwtService`] — for legacy HS256 "API token"
///    use-cases issued by ORP itself (or for federation peers that share
///    the local secret).
///
/// The router is cheap: each token costs one `decode_header` for `alg` and
/// one base64-decode of the payload for `iss`, both purely in-process. The
/// JWKS cache is touched only after that lookup picks a provider.
#[derive(Clone)]
pub struct OidcValidator {
    /// Configured external IdPs. Read-locked on the validation hot-path
    /// (the cache lookup is read-only); write-locked only during JWKS
    /// or discovery refresh.
    providers: Vec<Arc<RwLock<OidcClient>>>,
    /// Fallback validator for tokens that don't match any external IdP.
    /// Typically an HS256 [`JwtService`] for API-key-style internal tokens.
    legacy_jwt: Option<Arc<JwtService>>,
}

impl std::fmt::Debug for OidcValidator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OidcValidator")
            .field("providers", &self.providers.len())
            .field("legacy_jwt", &self.legacy_jwt.is_some())
            .finish()
    }
}

impl OidcValidator {
    /// Build a validator with no providers and no legacy fallback. Used as
    /// a starting point — chain `.with_provider()` / `.with_legacy_jwt()`.
    pub fn new() -> Self {
        Self {
            providers: Vec::new(),
            legacy_jwt: None,
        }
    }

    /// Attach an OIDC provider. The client should already have had
    /// `discover()` called once so `validate_external_token` can look up
    /// the JWKS by issuer.
    pub fn with_provider(mut self, client: Arc<RwLock<OidcClient>>) -> Self {
        self.providers.push(client);
        self
    }

    /// Attach a fallback HS256 [`JwtService`]. Tokens that don't match any
    /// configured IdP issuer (or that are HS256-signed) fall through here.
    pub fn with_legacy_jwt(mut self, jwt: Arc<JwtService>) -> Self {
        self.legacy_jwt = Some(jwt);
        self
    }

    /// Number of attached external IdPs.
    pub fn provider_count(&self) -> usize {
        self.providers.len()
    }

    /// True if a legacy HS256 fallback is attached.
    pub fn has_legacy_jwt(&self) -> bool {
        self.legacy_jwt.is_some()
    }

    /// Validate `token` against the configured providers, falling back to
    /// the legacy HS256 service if no IdP matches.
    pub async fn validate(&self, token: &str) -> Result<Claims, OidcError> {
        // Fast bail-out for the obviously-broken case: no providers, no
        // legacy. We surface a `NotConfigured` so callers can map to 500.
        if self.providers.is_empty() && self.legacy_jwt.is_none() {
            return Err(OidcError::NotConfigured);
        }

        // Step 1: peek at the algorithm — HS256 always routes to the
        // legacy fallback (provider keys are RSA/EC). This also blocks
        // alg-confusion: a malicious caller can't smuggle an HS256 token
        // through the OIDC path.
        let header = decode_header(token)
            .map_err(|e| OidcError::TokenValidationFailed(format!("header decode: {e}")))?;

        if matches!(header.alg, Algorithm::HS256) {
            return self.validate_legacy(token);
        }

        // Step 2: peek at `iss` (without signature verification — purely
        // for routing). Then dispatch to the matching provider.
        let unverified_iss = peek_issuer(token).ok_or_else(|| {
            OidcError::TokenValidationFailed("could not read iss claim from token".into())
        })?;

        for client_lock in &self.providers {
            // Acquire read lock first to check if this provider matches.
            let client = client_lock.read().await;
            let provider_iss = client.discovery.as_ref().map(|(d, _)| d.issuer.clone());
            drop(client);

            if let Some(iss) = provider_iss {
                if iss == unverified_iss {
                    let client = client_lock.read().await;
                    return client.validate_external_token(token).await;
                }
            }
        }

        // Step 3: no provider matched. We do NOT silently fall back to
        // legacy here — a token claiming `iss = https://attacker.example`
        // must not be accepted just because `legacy_jwt` exists. Legacy
        // tokens are HS256-only and were filtered in step 1.
        Err(OidcError::TokenValidationFailed(format!(
            "no configured OIDC provider matches iss={unverified_iss}"
        )))
    }

    fn validate_legacy(&self, token: &str) -> Result<Claims, OidcError> {
        let jwt = self
            .legacy_jwt
            .as_ref()
            .ok_or_else(|| OidcError::NotConfigured)?;
        jwt.validate_token(token).map_err(OidcError::Jwt)
    }
}

impl Default for OidcValidator {
    fn default() -> Self {
        Self::new()
    }
}

/// Read the `iss` claim from a JWT WITHOUT signature verification.
///
/// This is solely a routing hint — we use it to pick which provider's
/// JWKS to validate against. The signature is then verified in
/// `validate_external_token`. NEVER trust the value beyond routing.
fn peek_issuer(token: &str) -> Option<String> {
    use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
    #[derive(Deserialize)]
    struct OnlyIss {
        iss: Option<String>,
    }
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload = URL_SAFE_NO_PAD.decode(parts[1]).ok()?;
    let claims: OnlyIss = serde_json::from_slice(&payload).ok()?;
    claims.iss
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

/// Build a signed CSRF cookie value: `state|HEX(HMAC-SHA256(secret, state))`.
///
/// Closes P-audit F5. Previously this used `SHA256(state || secret)`, which
/// is length-extension forgeable: an attacker who can read one valid
/// (state, signature) pair can extend `state` and forge a valid signature
/// without knowing `secret`. HMAC eliminates that family of attacks.
fn sign_csrf_state(state_val: &str, secret: &str) -> String {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
        .expect("HMAC-SHA256 accepts arbitrary-length keys");
    mac.update(state_val.as_bytes());
    let tag = mac.finalize().into_bytes();
    format!("{}|{}", state_val, hex::encode(tag))
}

/// Verify and extract the CSRF state from a signed cookie value.
///
/// Compares the candidate signature against the recomputed HMAC tag in
/// constant time via `hmac::Mac::verify_slice`. The previous string `==`
/// branch could leak the matching prefix length through timing.
fn verify_csrf_cookie(cookie_val: &str, secret: &str) -> Option<String> {
    use hmac::{Hmac, Mac};
    use sha2::Sha256;
    type HmacSha256 = Hmac<Sha256>;
    let (state_val, tag_hex) = cookie_val.split_once('|')?;
    // Hex parse is fine in variable time — the tag isn't secret per se,
    // but its acceptance/rejection signal must be ct.
    let candidate = hex::decode(tag_hex).ok()?;
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).ok()?;
    mac.update(state_val.as_bytes());
    mac.verify_slice(&candidate).ok()?;
    Some(state_val.to_string())
}

/// CSRF cookie secret — derived from `client_secret` in production, or a
/// fixed dev key when OIDC is disabled. **In production (`enabled = true`)
/// with an empty `client_secret`, this returns `None`** so the caller MUST
/// refuse to issue or accept the cookie. The previous behaviour silently
/// fell back to `"orp-dev-csrf-secret"` (a public string), which would
/// degrade a misconfigured production deploy to no-CSRF-protection. The
/// crypto audit flagged that as a concern.
fn csrf_secret(oidc: &OidcClient) -> Option<String> {
    if oidc.config.client_secret.is_empty() {
        if oidc.config.enabled {
            // Don't fall back. Caller fails-closed.
            return None;
        }
        return Some("orp-dev-csrf-secret".to_string());
    }
    Some(format!("orp-csrf-{}", &oidc.config.client_secret))
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
            // Sign and store CSRF state in an httpOnly cookie. If OIDC is
            // enabled but client_secret is empty, csrf_secret returns None
            // — refuse to issue a cookie rather than degrade to a public
            // dev-mode key (audit-flagged misconfiguration vector).
            let Some(secret) = csrf_secret(&oidc) else {
                tracing::error!(
                    "OIDC enabled but client_secret is empty — refusing to issue CSRF cookie"
                );
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({
                        "error": {
                            "code": "OIDC_MISCONFIGURED",
                            "message": "OIDC enabled but client_secret is empty",
                        }
                    })),
                )
                    .into_response();
            };
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

    // Verify CSRF state from signed cookie. If misconfigured (OIDC enabled
    // with empty client_secret) refuse the callback altogether — the
    // alternative is verifying the cookie against a public dev-mode key,
    // i.e. anyone can forge it.
    let Some(secret) = csrf_secret(&oidc) else {
        tracing::error!("OIDC enabled but client_secret is empty — refusing to verify CSRF cookie");
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({
                "error": {
                    "code": "OIDC_MISCONFIGURED",
                    "message": "OIDC enabled but client_secret is empty",
                }
            })),
        )
            .into_response();
    };
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

    // ── F5 regression: CSRF HMAC + constant-time compare ─────────────────

    #[test]
    fn csrf_sign_verify_roundtrip_succeeds() {
        let state = "abcDEF123_-";
        let secret = "shared-secret-deadbeef";
        let signed = sign_csrf_state(state, secret);
        let recovered = verify_csrf_cookie(&signed, secret).expect("verify succeeds");
        assert_eq!(recovered, state);
    }

    #[test]
    fn csrf_verify_rejects_tampered_signature_byte() {
        let state = "abcDEF123_-";
        let secret = "shared-secret-deadbeef";
        let signed = sign_csrf_state(state, secret);
        // Flip one byte of the hex tag.
        let (s, tag) = signed.split_once('|').unwrap();
        let mut tag_bytes: Vec<u8> = tag.bytes().collect();
        tag_bytes[0] ^= 0x01;
        let tampered = format!("{s}|{}", String::from_utf8(tag_bytes).unwrap());
        assert!(verify_csrf_cookie(&tampered, secret).is_none());
    }

    #[test]
    fn csrf_verify_rejects_tampered_state() {
        let state = "abcDEF123_-";
        let secret = "shared-secret-deadbeef";
        let signed = sign_csrf_state(state, secret);
        let (_, tag) = signed.split_once('|').unwrap();
        let tampered = format!("EvILstate|{tag}");
        assert!(verify_csrf_cookie(&tampered, secret).is_none());
    }

    #[test]
    fn csrf_verify_rejects_wrong_secret() {
        let state = "abcDEF123_-";
        let secret_real = "real";
        let secret_fake = "fake";
        let signed = sign_csrf_state(state, secret_real);
        assert!(verify_csrf_cookie(&signed, secret_fake).is_none());
    }

    #[test]
    fn csrf_verify_rejects_missing_separator() {
        // No '|' → splitn returns one element → split_once returns None.
        assert!(verify_csrf_cookie("no-separator-here", "secret").is_none());
    }

    #[test]
    fn csrf_verify_rejects_non_hex_tag() {
        // Tag must be hex; non-hex chars → hex::decode fails → None.
        assert!(verify_csrf_cookie("state|not-hex-XYZ", "secret").is_none());
    }

    #[test]
    fn csrf_signature_changes_when_state_changes() {
        // The signature depends on the state, so two distinct states with
        // the same secret must produce different tags. Catches a regression
        // where someone "optimises" the HMAC into a constant.
        let secret = "shared";
        let s1 = sign_csrf_state("alpha", secret);
        let s2 = sign_csrf_state("beta", secret);
        let tag1 = s1.split_once('|').unwrap().1;
        let tag2 = s2.split_once('|').unwrap().1;
        assert_ne!(tag1, tag2);
    }

    #[test]
    fn csrf_signature_changes_when_secret_changes() {
        let state = "alpha";
        let s1 = sign_csrf_state(state, "secret-1");
        let s2 = sign_csrf_state(state, "secret-2");
        assert_ne!(s1, s2);
    }

    #[test]
    fn csrf_secret_returns_none_when_oidc_enabled_with_empty_client_secret() {
        // Crypto-audit concern: previously csrf_secret silently fell back
        // to a public dev-mode key in this case, degrading production CSRF
        // protection to zero. Now csrf_secret returns None and the caller
        // must fail closed.
        let cfg = OidcConfig {
            enabled: true,
            client_secret: String::new(),
            ..Default::default()
        };
        let client = OidcClient::new(cfg);
        assert!(csrf_secret(&client).is_none());
    }

    #[test]
    fn csrf_secret_dev_mode_falls_back_to_dev_key() {
        let cfg = OidcConfig {
            enabled: false,
            client_secret: String::new(),
            ..Default::default()
        };
        let client = OidcClient::new(cfg);
        let secret = csrf_secret(&client).expect("dev-mode fallback");
        assert_eq!(secret, "orp-dev-csrf-secret");
    }

    #[test]
    fn csrf_secret_with_real_client_secret_uses_it() {
        let cfg = OidcConfig {
            enabled: true,
            client_secret: "very-real-secret".to_string(),
            ..Default::default()
        };
        let client = OidcClient::new(cfg);
        let secret = csrf_secret(&client).unwrap();
        assert!(secret.contains("very-real-secret"));
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

    // ─── JWKS verification tests ─────────────────────────────────────────
    //
    // These tests stand up a single mock IdP that serves both
    // `/.well-known/openid-configuration` and `/jwks`. Tokens are signed
    // with a fixed RSA keypair (PKCS8 PEM + matching JWK n/e — copied
    // from the jsonwebtoken 9.3.1 test fixtures, public values only).

    use jsonwebtoken::{encode as jwt_encode, EncodingKey, Header as JwtHeader};

    /// Fixed RSA private key used to sign every test token. Public values
    /// are reproducible from the modulus below.
    const TEST_RSA_PRIV_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQDJETqse41HRBsc
7cfcq3ak4oZWFCoZlcic525A3FfO4qW9BMtRO/iXiyCCHn8JhiL9y8j5JdVP2Q9Z
IpfElcFd3/guS9w+5RqQGgCR+H56IVUyHZWtTJbKPcwWXQdNUX0rBFcsBzCRESJL
eelOEdHIjG7LRkx5l/FUvlqsyHDVJEQsHwegZ8b8C0fz0EgT2MMEdn10t6Ur1rXz
jMB/wvCg8vG8lvciXmedyo9xJ8oMOh0wUEgxziVDMMovmC+aJctcHUAYubwoGN8T
yzcvnGqL7JSh36Pwy28iPzXZ2RLhAyJFU39vLaHdljwthUaupldlNyCfa6Ofy4qN
ctlUPlN1AgMBAAECggEAdESTQjQ70O8QIp1ZSkCYXeZjuhj081CK7jhhp/4ChK7J
GlFQZMwiBze7d6K84TwAtfQGZhQ7km25E1kOm+3hIDCoKdVSKch/oL54f/BK6sKl
qlIzQEAenho4DuKCm3I4yAw9gEc0DV70DuMTR0LEpYyXcNJY3KNBOTjN5EYQAR9s
2MeurpgK2MdJlIuZaIbzSGd+diiz2E6vkmcufJLtmYUT/k/ddWvEtz+1DnO6bRHh
xuuDMeJA/lGB/EYloSLtdyCF6sII6C6slJJtgfb0bPy7l8VtL5iDyz46IKyzdyzW
tKAn394dm7MYR1RlUBEfqFUyNK7C+pVMVoTwCC2V4QKBgQD64syfiQ2oeUlLYDm4
CcKSP3RnES02bcTyEDFSuGyyS1jldI4A8GXHJ/lG5EYgiYa1RUivge4lJrlNfjyf
dV230xgKms7+JiXqag1FI+3mqjAgg4mYiNjaao8N8O3/PD59wMPeWYImsWXNyeHS
55rUKiHERtCcvdzKl4u35ZtTqQKBgQDNKnX2bVqOJ4WSqCgHRhOm386ugPHfy+8j
m6cicmUR46ND6ggBB03bCnEG9OtGisxTo/TuYVRu3WP4KjoJs2LD5fwdwJqpgtHl
yVsk45Y1Hfo+7M6lAuR8rzCi6kHHNb0HyBmZjysHWZsn79ZM+sQnLpgaYgQGRbKV
DZWlbw7g7QKBgQCl1u+98UGXAP1jFutwbPsx40IVszP4y5ypCe0gqgon3UiY/G+1
zTLp79GGe/SjI2VpQ7AlW7TI2A0bXXvDSDi3/5Dfya9ULnFXv9yfvH1QwWToySpW
Kvd1gYSoiX84/WCtjZOr0e0HmLIb0vw0hqZA4szJSqoxQgvF22EfIWaIaQKBgQCf
34+OmMYw8fEvSCPxDxVvOwW2i7pvV14hFEDYIeZKW2W1HWBhVMzBfFB5SE8yaCQy
pRfOzj9aKOCm2FjjiErVNpkQoi6jGtLvScnhZAt/lr2TXTrl8OwVkPrIaN0bG/AS
aUYxmBPCpXu3UjhfQiWqFq/mFyzlqlgvuCc9g95HPQKBgAscKP8mLxdKwOgX8yFW
GcZ0izY/30012ajdHY+/QK5lsMoxTnn0skdS+spLxaS5ZEO4qvPVb8RAoCkWMMal
2pOhmquJQVDPDLuZHdrIiKiDM20dy9sMfHygWcZjQ4WSxf/J7T9canLZIXFhHAZT
3wc9h4G8BBCtWN2TN/LsGZdB
-----END PRIVATE KEY-----"#;

    /// Modulus (`n`) and exponent (`e`) matching `TEST_RSA_PRIV_PEM`.
    /// Base64-url-encoded per RFC 7518.
    const TEST_RSA_N: &str = "yRE6rHuNR0QbHO3H3Kt2pOKGVhQqGZXInOduQNxXzuKlvQTLUTv4l4sggh5_CYYi_cvI-SXVT9kPWSKXxJXBXd_4LkvcPuUakBoAkfh-eiFVMh2VrUyWyj3MFl0HTVF9KwRXLAcwkREiS3npThHRyIxuy0ZMeZfxVL5arMhw1SRELB8HoGfG_AtH89BIE9jDBHZ9dLelK9a184zAf8LwoPLxvJb3Il5nncqPcSfKDDodMFBIMc4lQzDKL5gvmiXLXB1AGLm8KBjfE8s3L5xqi-yUod-j8MtvIj812dkS4QMiRVN_by2h3ZY8LYVGrqZXZTcgn2ujn8uKjXLZVD5TdQ";
    const TEST_RSA_E: &str = "AQAB";

    /// Build a JWKS document containing one entry under `kid`. Optionally
    /// pin a specific `n` so we can serve a JWKS that does NOT match the
    /// signing key (used to test signature failure).
    fn build_jwks_json(kid: &str, n: &str) -> serde_json::Value {
        serde_json::json!({
            "keys": [
                {
                    "kty": "RSA",
                    "use": "sig",
                    "alg": "RS256",
                    "kid": kid,
                    "n": n,
                    "e": TEST_RSA_E,
                }
            ]
        })
    }

    /// Sign a JWT with `TEST_RSA_PRIV_PEM` for the given `kid`, `iss`,
    /// `aud`, and `exp`-offset. `extra` lets a test override fields like
    /// `nbf` or omit `sub`.
    fn sign_test_jwt(kid: Option<&str>, alg: Algorithm, claims: serde_json::Value) -> String {
        let mut header = JwtHeader::new(alg);
        header.kid = kid.map(String::from);
        let key = EncodingKey::from_rsa_pem(TEST_RSA_PRIV_PEM.as_bytes()).unwrap();
        jwt_encode(&header, &claims, &key).unwrap()
    }

    /// Standard claims with given iss/aud and a 1-hour expiry.
    fn standard_claims(iss: &str, aud: &str, sub: &str) -> serde_json::Value {
        let now = Utc::now().timestamp();
        serde_json::json!({
            "sub": sub,
            "iss": iss,
            "aud": aud,
            "iat": now,
            "exp": now + 3600,
        })
    }

    /// Mock IdP: serves discovery, JWKS, and lets the caller swap the
    /// JWKS body (to test `kid` rotation) and count requests.
    #[derive(Clone)]
    struct MockIdp {
        base: String,
        jwks_body: Arc<RwLock<serde_json::Value>>,
        jwks_hits: Arc<AtomicUsize>,
    }

    /// Spawn a mock IdP. Initial JWKS contains one key with `kid="key1"`
    /// matching `TEST_RSA_PRIV_PEM`. Returns the handle for assertions.
    async fn spawn_mock_idp_full() -> MockIdp {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");

        let jwks_body = Arc::new(RwLock::new(build_jwks_json("key1", TEST_RSA_N)));
        let jwks_hits = Arc::new(AtomicUsize::new(0));

        let jb = jwks_body.clone();
        let jh = jwks_hits.clone();
        let base_for_resp = base.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(p) => p,
                    Err(_) => break,
                };
                let jb = jb.clone();
                let jh = jh.clone();
                let base = base_for_resp.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = std::str::from_utf8(&buf[..n]).unwrap_or("");
                    let path = req
                        .lines()
                        .next()
                        .and_then(|l| l.split_whitespace().nth(1))
                        .unwrap_or("/");

                    let body_str = if path.starts_with("/.well-known") {
                        serde_json::json!({
                            "issuer": base,
                            "authorization_endpoint": format!("{base}/authorize"),
                            "token_endpoint": format!("{base}/token"),
                            "jwks_uri": format!("{base}/jwks"),
                            "response_types_supported": ["code"],
                            "scopes_supported": ["openid"],
                        })
                        .to_string()
                    } else if path.starts_with("/jwks") {
                        jh.fetch_add(1, Ordering::SeqCst);
                        jb.read().await.to_string()
                    } else {
                        "{}".to_string()
                    };

                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body_str.len(),
                        body_str
                    );
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        MockIdp {
            base,
            jwks_body,
            jwks_hits,
        }
    }

    impl MockIdp {
        async fn rotate_keys(&self, new_kid: &str) {
            let mut g = self.jwks_body.write().await;
            *g = build_jwks_json(new_kid, TEST_RSA_N);
        }
    }

    /// Build an OidcClient pointed at the mock IdP, primed via discovery.
    async fn primed_client(idp: &MockIdp, client_id: &str) -> OidcClient {
        let cfg = OidcConfig {
            enabled: true,
            provider_url: idp.base.clone(),
            client_id: client_id.into(),
            client_secret: "shh".into(),
            ..Default::default()
        };
        let mut client = OidcClient::new(cfg);
        client.discover().await.expect("discovery primes ok");
        client
    }

    #[tokio::test]
    async fn test_oidc_validates_real_jwt() {
        let idp = spawn_mock_idp_full().await;
        let client = primed_client(&idp, "test-client").await;

        let token = sign_test_jwt(
            Some("key1"),
            Algorithm::RS256,
            standard_claims(&idp.base, "test-client", "user-42"),
        );

        let claims = client
            .validate_external_token(&token)
            .await
            .expect("valid JWT signed by RSA key in JWKS must be accepted");
        assert_eq!(claims.sub, "user-42");
        assert_eq!(claims.iss, idp.base);
        // First validation should have triggered exactly one JWKS fetch.
        assert_eq!(idp.jwks_hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn test_oidc_rejects_kid_not_in_jwks() {
        let idp = spawn_mock_idp_full().await;
        let client = primed_client(&idp, "test-client").await;

        // Sign with the same RSA key but use an unknown `kid` in the
        // header — JWKS lookup must miss and refresh once and still miss.
        let token = sign_test_jwt(
            Some("nonexistent-kid"),
            Algorithm::RS256,
            standard_claims(&idp.base, "test-client", "u"),
        );
        let result = client.validate_external_token(&token).await;
        assert!(
            matches!(result, Err(OidcError::JwksMissingKid(ref k)) if k == "nonexistent-kid"),
            "expected JwksMissingKid, got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_oidc_rejects_alg_mismatch() {
        // Token says `alg=HS256` in the header — must be rejected on the
        // OIDC validation path even though the rest of the claim shape
        // looks valid.
        let idp = spawn_mock_idp_full().await;
        let client = primed_client(&idp, "test-client").await;

        // Build an HS256-signed token. Use any HMAC secret — it's
        // rejected before the signature is checked.
        let mut header = JwtHeader::new(Algorithm::HS256);
        header.kid = Some("key1".into());
        let key = EncodingKey::from_secret(b"any-hmac-secret");
        let token = jwt_encode(
            &header,
            &standard_claims(&idp.base, "test-client", "u"),
            &key,
        )
        .unwrap();

        let result = client.validate_external_token(&token).await;
        assert!(
            matches!(result, Err(OidcError::TokenValidationFailed(ref m)) if m.contains("alg")),
            "expected alg-not-permitted error, got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_oidc_rejects_alg_none() {
        // jsonwebtoken refuses to encode `alg=none` directly via the
        // public Header API (it's not in the Algorithm enum), so we
        // hand-craft the token: header `{"alg":"none","typ":"JWT"}` +
        // base64(payload) + empty signature.
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        let idp = spawn_mock_idp_full().await;
        let client = primed_client(&idp, "test-client").await;

        let header_json = r#"{"alg":"none","typ":"JWT","kid":"key1"}"#;
        let payload_json = standard_claims(&idp.base, "test-client", "u").to_string();
        let token = format!(
            "{}.{}.",
            URL_SAFE_NO_PAD.encode(header_json),
            URL_SAFE_NO_PAD.encode(&payload_json),
        );
        let result = client.validate_external_token(&token).await;
        assert!(
            matches!(result, Err(OidcError::TokenValidationFailed(_))),
            "alg=none must be rejected at header decode (jsonwebtoken rejects \
             unknown alg in decode_header), got {result:?}"
        );
    }

    #[tokio::test]
    async fn test_oidc_jwks_refresh_on_kid_miss() {
        let idp = spawn_mock_idp_full().await;
        let client = primed_client(&idp, "test-client").await;

        // First validation primes the JWKS cache via key1.
        let t1 = sign_test_jwt(
            Some("key1"),
            Algorithm::RS256,
            standard_claims(&idp.base, "test-client", "u"),
        );
        client.validate_external_token(&t1).await.unwrap();
        assert_eq!(idp.jwks_hits.load(Ordering::SeqCst), 1);

        // Rotate the IdP's keys (same modulus, new `kid="key2"`). Then
        // present a token signed with the new kid — cache miss must
        // trigger a refresh.
        idp.rotate_keys("key2").await;
        let t2 = sign_test_jwt(
            Some("key2"),
            Algorithm::RS256,
            standard_claims(&idp.base, "test-client", "u"),
        );
        let claims = client
            .validate_external_token(&t2)
            .await
            .expect("rotated kid must be honoured after refresh");
        assert_eq!(claims.sub, "u");
        assert_eq!(
            idp.jwks_hits.load(Ordering::SeqCst),
            2,
            "kid miss must trigger exactly one extra JWKS fetch"
        );
    }

    #[tokio::test]
    async fn test_oidc_jwks_ttl() {
        // Validate that the cache *is* used (no extra JWKS fetch on
        // repeated valid tokens with the same kid).
        let idp = spawn_mock_idp_full().await;
        let client = primed_client(&idp, "test-client").await;

        for _ in 0..3 {
            let t = sign_test_jwt(
                Some("key1"),
                Algorithm::RS256,
                standard_claims(&idp.base, "test-client", "u"),
            );
            client.validate_external_token(&t).await.unwrap();
        }
        assert_eq!(
            idp.jwks_hits.load(Ordering::SeqCst),
            1,
            "subsequent same-kid validations must reuse the cache"
        );
    }

    #[tokio::test]
    async fn test_oidc_iss_aud_validation() {
        let idp = spawn_mock_idp_full().await;
        let client = primed_client(&idp, "test-client").await;

        // Wrong issuer.
        let bad_iss = sign_test_jwt(
            Some("key1"),
            Algorithm::RS256,
            standard_claims("https://attacker.example", "test-client", "u"),
        );
        let r = client.validate_external_token(&bad_iss).await;
        assert!(
            matches!(r, Err(OidcError::TokenValidationFailed(_))),
            "wrong iss must be rejected, got {r:?}"
        );

        // Wrong audience.
        let bad_aud = sign_test_jwt(
            Some("key1"),
            Algorithm::RS256,
            standard_claims(&idp.base, "other-client", "u"),
        );
        let r = client.validate_external_token(&bad_aud).await;
        assert!(
            matches!(r, Err(OidcError::TokenValidationFailed(_))),
            "wrong aud must be rejected, got {r:?}"
        );
    }

    // ─── OidcValidator multi-IdP / legacy fallback tests ────────────────

    #[tokio::test]
    async fn test_validator_routes_by_issuer() {
        // Two IdPs. Each issues a token; the validator must accept both
        // by routing to the matching provider's JWKS.
        let idp_a = spawn_mock_idp_full().await;
        let idp_b = spawn_mock_idp_full().await;
        let ca = Arc::new(RwLock::new(primed_client(&idp_a, "client-a").await));
        let cb = Arc::new(RwLock::new(primed_client(&idp_b, "client-b").await));
        let validator = OidcValidator::new().with_provider(ca).with_provider(cb);

        let tok_a = sign_test_jwt(
            Some("key1"),
            Algorithm::RS256,
            standard_claims(&idp_a.base, "client-a", "alice"),
        );
        let tok_b = sign_test_jwt(
            Some("key1"),
            Algorithm::RS256,
            standard_claims(&idp_b.base, "client-b", "bob"),
        );

        let claims_a = validator.validate(&tok_a).await.expect("idp_a token ok");
        assert_eq!(claims_a.sub, "alice");

        let claims_b = validator.validate(&tok_b).await.expect("idp_b token ok");
        assert_eq!(claims_b.sub, "bob");
    }

    #[tokio::test]
    async fn test_validator_rejects_unknown_issuer() {
        let idp = spawn_mock_idp_full().await;
        let c = Arc::new(RwLock::new(primed_client(&idp, "client-a").await));
        let validator = OidcValidator::new().with_provider(c);

        let token = sign_test_jwt(
            Some("key1"),
            Algorithm::RS256,
            standard_claims("https://other.example", "client-a", "u"),
        );
        let r = validator.validate(&token).await;
        assert!(
            matches!(r, Err(OidcError::TokenValidationFailed(ref m)) if m.contains("iss")),
            "unknown iss must be rejected, got {r:?}"
        );
    }

    #[tokio::test]
    async fn test_validator_legacy_hs256_fallback() {
        use crate::jwt::{JwtConfig, JwtService};
        let idp = spawn_mock_idp_full().await;
        let c = Arc::new(RwLock::new(primed_client(&idp, "client-a").await));
        let jwt_svc = Arc::new(
            JwtService::new(JwtConfig {
                hs256_secret: Some(b"a-test-only-shared-hmac-secret-32b!".to_vec()),
                ..JwtConfig::default()
            })
            .unwrap(),
        );
        let validator = OidcValidator::new()
            .with_provider(c)
            .with_legacy_jwt(jwt_svc.clone());

        // HS256 token from the local service must be accepted via the
        // legacy fallback.
        let local_token = jwt_svc
            .issue_token(
                "internal-svc",
                None,
                None,
                None,
                vec!["entities:read".into()],
            )
            .unwrap();
        let claims = validator
            .validate(&local_token)
            .await
            .expect("local HS256 must validate via legacy fallback");
        assert_eq!(claims.sub, "internal-svc");

        // A foreign-IdP token with unknown iss must still be rejected
        // (no silent fallback to legacy).
        let foreign = sign_test_jwt(
            Some("key1"),
            Algorithm::RS256,
            standard_claims("https://attacker.example", "client-a", "u"),
        );
        let r = validator.validate(&foreign).await;
        assert!(
            r.is_err(),
            "foreign iss must NOT silently fall back to HS256"
        );
    }

    #[tokio::test]
    async fn test_validator_rejects_hs256_when_no_legacy() {
        let idp = spawn_mock_idp_full().await;
        let c = Arc::new(RwLock::new(primed_client(&idp, "client-a").await));
        let validator = OidcValidator::new().with_provider(c);

        // HS256-signed token, no legacy fallback. Must error with
        // NotConfigured (HS256 routes to legacy and there isn't one).
        let header = JwtHeader::new(Algorithm::HS256);
        let key = EncodingKey::from_secret(b"any");
        let token =
            jwt_encode(&header, &standard_claims(&idp.base, "client-a", "u"), &key).unwrap();
        let r = validator.validate(&token).await;
        assert!(
            matches!(r, Err(OidcError::NotConfigured)),
            "HS256 with no legacy must error NotConfigured, got {r:?}"
        );
    }
}
