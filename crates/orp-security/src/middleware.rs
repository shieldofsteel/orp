//! Axum authentication middleware and extractors.
//!
//! Extracts a Bearer token from `Authorization: Bearer <token>` header
//! OR an API key from `X-API-Key: <key>` header, validates it, and injects
//! an [`AuthContext`] into request extensions.

use axum::{
    async_trait,
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Json, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;

use crate::{
    api_keys::{ApiKeyService, ApiKeyValidationResult},
    jwt::{Claims, JwtService},
};

/// Authenticated request context injected into Axum extensions and extractors.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthContext {
    /// The authenticated subject (user ID or API key ID)
    pub subject: String,
    /// Permissions granted to this principal
    pub permissions: Vec<String>,
    /// Optional email (JWT only)
    pub email: Option<String>,
    /// Optional display name (JWT only)
    pub name: Option<String>,
    /// Organization ID (JWT only)
    pub org_id: Option<String>,
    /// Scopes (API key or JWT scope string parsed)
    pub scopes: Vec<String>,
    /// How the request was authenticated
    pub auth_method: AuthMethod,
}

impl AuthContext {
    /// Returns true if the context holds the requested permission.
    pub fn has_permission(&self, perm: &str) -> bool {
        self.permissions.iter().any(|p| p == perm)
            || self.permissions.iter().any(|p| p == "admin")
    }

    /// Returns true if the context holds the requested scope.
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|s| s == scope)
    }

    /// Build an anonymous (unauthenticated) context — used when no credentials are
    /// provided in production mode. Has ZERO permissions for security.
    pub fn anonymous() -> Self {
        Self {
            subject: "anonymous".to_string(),
            permissions: vec![],
            email: None,
            name: Some("Anonymous".to_string()),
            org_id: None,
            scopes: vec![],
            auth_method: AuthMethod::DevMode,
        }
    }

    /// Build an anonymous context with full admin permissions — used in dev/permissive mode
    /// so that unauthenticated requests have unrestricted access for local development.
    pub fn anonymous_dev() -> Self {
        Self {
            subject: "anonymous".to_string(),
            permissions: vec!["admin".to_string()],
            email: None,
            name: Some("Anonymous (dev)".to_string()),
            org_id: None,
            scopes: vec![
                "entities:read".to_string(),
                "entities:write".to_string(),
                "events:read".to_string(),
                "events:write".to_string(),
                "graph:read".to_string(),
                "graph:write".to_string(),
                "monitors:read".to_string(),
                "monitors:write".to_string(),
                "admin".to_string(),
            ],
            auth_method: AuthMethod::DevMode,
        }
    }

    fn from_jwt_claims(claims: Claims) -> Self {
        let scopes: Vec<String> = claims
            .scope
            .split_whitespace()
            .map(|s| s.to_string())
            .collect();
        Self {
            subject: claims.sub,
            permissions: claims.permissions,
            email: claims.email,
            name: claims.name,
            org_id: claims.org_id,
            scopes,
            auth_method: AuthMethod::Jwt,
        }
    }

    fn from_api_key(result: ApiKeyValidationResult) -> Self {
        Self {
            subject: result.key_id,
            permissions: result.scopes.clone(),
            email: None,
            name: result.name,
            org_id: result.org_id,
            scopes: result.scopes,
            auth_method: AuthMethod::ApiKey,
        }
    }
}

/// How a request was authenticated.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum AuthMethod {
    Jwt,
    ApiKey,
    DevMode,
}

/// Authentication state shared across Axum handlers.
#[derive(Clone, Debug)]
#[derive(Default)]
pub struct AuthState {
    /// JWT service — `None` means auth is disabled (dev mode)
    pub jwt_service: Option<Arc<JwtService>>,
    /// API key service — `None` means API keys are disabled
    pub api_key_service: Option<Arc<ApiKeyService>>,
    /// When true, missing/invalid tokens fall through to anonymous context.
    /// Set to false in production.
    pub permissive_mode: bool,
}

impl AuthState {
    /// Production configuration — requires a valid JWT or API key.
    pub fn production(jwt: Arc<JwtService>, api_keys: Arc<ApiKeyService>) -> Self {
        Self {
            jwt_service: Some(jwt),
            api_key_service: Some(api_keys),
            permissive_mode: false,
        }
    }

    /// Development mode — permissive only when ORP_DEV_MODE=true.
    pub fn dev() -> Self {
        let permissive = std::env::var("ORP_DEV_MODE")
            .map(|v| v == "true" || v == "1")
            .unwrap_or(false);
        Self {
            jwt_service: None,
            api_key_service: None,
            permissive_mode: permissive,
        }
    }
}

/// Authentication error returned as an HTTP response.
#[derive(Debug)]
pub struct AuthError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let body = json!({
            "error": {
                "code": self.code,
                "status": self.status.as_u16(),
                "message": self.message,
                "timestamp": chrono::Utc::now().to_rfc3339(),
            }
        });
        (self.status, Json(body)).into_response()
    }
}

/// Axum extractor that validates authentication and provides [`AuthContext`].
///
/// Looks for credentials in this order:
/// 1. Pre-injected `AuthContext` in request extensions (set by middleware layer)
/// 2. `Authorization: Bearer <token>` — JWT
/// 3. `X-API-Key: <key>` — API key
/// 4. Falls through to anonymous if `permissive_mode` is true
///
/// Requires `Arc<AuthState>` to be present in request extensions.
/// Use `AuthState::into_layer()` or manually insert it.
///
/// # Usage in handlers
/// ```rust,ignore
/// async fn my_handler(auth: AuthContext, ...) -> impl IntoResponse { ... }
/// ```
#[async_trait]
impl<S> FromRequestParts<S> for AuthContext
where
    S: Send + Sync,
{
    type Rejection = AuthError;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        // Check if AuthContext was already injected (e.g., by a middleware layer)
        if let Some(ctx) = parts.extensions.get::<AuthContext>() {
            return Ok(ctx.clone());
        }

        // Get AuthState from request extensions
        let auth_state = parts
            .extensions
            .get::<Arc<AuthState>>()
            .cloned()
            .unwrap_or_else(|| Arc::new(AuthState::default()));

        extract_auth(parts, &auth_state).await
    }
}

async fn extract_auth(
    parts: &mut Parts,
    auth_state: &AuthState,
) -> Result<AuthContext, AuthError> {
    // Try to extract Bearer token
    let bearer_token = parts
        .headers
        .get("Authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|s| s.to_string());

    // Try to extract X-API-Key
    let api_key = parts
        .headers
        .get("X-API-Key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    // Validate Bearer JWT
    if let Some(token) = bearer_token {
        return validate_jwt(&token, auth_state).await;
    }

    // Validate API key
    if let Some(key) = api_key {
        return validate_api_key(&key, auth_state).await;
    }

    // No credentials provided
    if auth_state.permissive_mode {
        // Check extensions for pre-injected context (e.g., from layer)
        if let Some(ctx) = parts.extensions.get::<AuthContext>() {
            return Ok(ctx.clone());
        }
        return Ok(AuthContext::anonymous_dev());
    }

    Err(AuthError {
        status: StatusCode::UNAUTHORIZED,
        code: "UNAUTHORIZED",
        message: "Missing authentication credentials. Provide Authorization: Bearer <token> or X-API-Key header.".to_string(),
    })
}

async fn validate_jwt(token: &str, state: &AuthState) -> Result<AuthContext, AuthError> {
    let svc = match &state.jwt_service {
        Some(s) => s.clone(),
        None if state.permissive_mode => return Ok(AuthContext::anonymous_dev()),
        None => {
            return Err(AuthError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "INTERNAL_ERROR",
                message: "JWT service not configured".to_string(),
            })
        }
    };

    match svc.validate_token(token) {
        Ok(claims) => Ok(AuthContext::from_jwt_claims(claims)),
        Err(crate::jwt::JwtError::TokenExpired) => Err(AuthError {
            status: StatusCode::UNAUTHORIZED,
            code: "UNAUTHORIZED",
            message: "Token has expired".to_string(),
        }),
        Err(e) => Err(AuthError {
            status: StatusCode::UNAUTHORIZED,
            code: "UNAUTHORIZED",
            message: format!("Invalid token: {e}"),
        }),
    }
}

async fn validate_api_key(key: &str, state: &AuthState) -> Result<AuthContext, AuthError> {
    let svc = match &state.api_key_service {
        Some(s) => s.clone(),
        None if state.permissive_mode => return Ok(AuthContext::anonymous_dev()),
        None => {
            return Err(AuthError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                code: "INTERNAL_ERROR",
                message: "API key service not configured".to_string(),
            })
        }
    };

    match svc.validate_key(key).await {
        Ok(result) => {
            if result.is_expired {
                return Err(AuthError {
                    status: StatusCode::UNAUTHORIZED,
                    code: "UNAUTHORIZED",
                    message: "API key has expired".to_string(),
                });
            }
            if result.is_revoked {
                return Err(AuthError {
                    status: StatusCode::UNAUTHORIZED,
                    code: "UNAUTHORIZED",
                    message: "API key has been revoked".to_string(),
                });
            }
            Ok(AuthContext::from_api_key(result))
        }
        Err(e) => Err(AuthError {
            status: StatusCode::UNAUTHORIZED,
            code: "UNAUTHORIZED",
            message: format!("Invalid API key: {e}"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_anonymous_context_has_zero_permissions() {
        let ctx = AuthContext::anonymous();
        assert!(!ctx.has_permission("entities:read"));
        assert!(!ctx.has_permission("entities:write"));
        assert!(!ctx.has_permission("admin"));
        assert!(ctx.permissions.is_empty());
        assert!(ctx.scopes.is_empty());
        assert_eq!(ctx.auth_method, AuthMethod::DevMode);
    }

    #[test]
    fn test_anonymous_dev_context_has_admin_permissions() {
        let ctx = AuthContext::anonymous_dev();
        assert!(ctx.has_permission("entities:read"));
        assert!(ctx.has_permission("entities:write"));
        assert!(ctx.has_permission("admin"));
        assert!(!ctx.permissions.is_empty());
        assert!(!ctx.scopes.is_empty());
        assert_eq!(ctx.auth_method, AuthMethod::DevMode);
    }

    #[test]
    fn test_has_permission_admin_wildcard() {
        let ctx = AuthContext {
            subject: "x".into(),
            permissions: vec!["admin".into()],
            email: None,
            name: None,
            org_id: None,
            scopes: vec![],
            auth_method: AuthMethod::Jwt,
        };
        assert!(ctx.has_permission("entities:read"));
        assert!(ctx.has_permission("anything"));
    }

    #[test]
    fn test_has_permission_specific() {
        let ctx = AuthContext {
            subject: "x".into(),
            permissions: vec!["entities:read".into()],
            email: None,
            name: None,
            org_id: None,
            scopes: vec![],
            auth_method: AuthMethod::Jwt,
        };
        assert!(ctx.has_permission("entities:read"));
        assert!(!ctx.has_permission("entities:write"));
    }

    #[test]
    fn test_has_scope() {
        let ctx = AuthContext {
            subject: "x".into(),
            permissions: vec![],
            email: None,
            name: None,
            org_id: None,
            scopes: vec!["api:read".into(), "monitors:read".into()],
            auth_method: AuthMethod::ApiKey,
        };
        assert!(ctx.has_scope("api:read"));
        assert!(!ctx.has_scope("api:write"));
    }

    #[test]
    fn test_from_jwt_claims_maps_correctly() {
        use crate::jwt::Claims;
        let claims = Claims {
            sub: "user-99".into(),
            email: Some("bob@test.com".into()),
            name: Some("Bob".into()),
            iat: 0,
            exp: 9999999999,
            iss: "http://localhost:9090/auth".into(),
            aud: "orp-client".into(),
            scope: "api:read entities:read".into(),
            org_id: Some("org-77".into()),
            permissions: vec!["entities:read".into()],
        };
        let ctx = AuthContext::from_jwt_claims(claims);
        assert_eq!(ctx.subject, "user-99");
        assert_eq!(ctx.email.as_deref(), Some("bob@test.com"));
        assert_eq!(ctx.org_id.as_deref(), Some("org-77"));
        assert!(ctx.has_scope("api:read"));
        assert!(ctx.has_scope("entities:read"));
        assert_eq!(ctx.auth_method, AuthMethod::Jwt);
    }
}
