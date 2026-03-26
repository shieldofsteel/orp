//! `orp-security` — Authentication, authorization, and access control for ORP.
//!
//! ## Modules
//!
//! - [`jwt`] — JWT token creation and validation (HS256/RS256)
//! - [`middleware`] — Axum extractor that injects [`middleware::AuthContext`]
//! - [`api_keys`] — API key generation (`orpk_prod_xxx`), scoped permissions, rate limiting
//! - [`abac`] — Attribute-Based Access Control engine (policy evaluation, deny-overrides)
//! - [`oidc`] — OIDC client: discovery, authorization code flow, token exchange, refresh
//!
//! ## Quick Start
//!
//! ### Development (permissive mode)
//!
//! ```rust,no_run
//! use orp_security::middleware::{AuthState, AuthContext};
//! use std::sync::Arc;
//!
//! // All requests pass through with admin context
//! let auth = Arc::new(AuthState::dev());
//! ```
//!
//! ### Production
//!
//! ```rust,no_run
//! use orp_security::{
//!     jwt::{JwtConfig, JwtService, JwtAlgorithm},
//!     api_keys::ApiKeyService,
//!     middleware::AuthState,
//! };
//! use std::sync::Arc;
//!
//! let jwt_svc = Arc::new(JwtService::new(JwtConfig {
//!     algorithm: JwtAlgorithm::RS256,
//!     rs256_public_key_pem: Some(std::env::var("JWT_PUBLIC_KEY").unwrap()),
//!     ..Default::default()
//! }).unwrap());
//!
//! let api_key_svc = Arc::new(ApiKeyService::new());
//! let auth = Arc::new(AuthState::production(jwt_svc, api_key_svc));
//! ```

pub mod abac;
pub mod api_keys;
pub mod jwt;
pub mod middleware;
pub mod oidc;
pub mod rbac;

// ─── Convenience re-exports ───────────────────────────────────────────────────

pub use abac::{
    AbacEngine, AbacError, AbacPolicy, EvaluationContext, EvaluationResult, Permission,
    PolicyDecision, PolicyEffect, PrincipalSpec, Resource, ResourceSpec, Subject,
};

pub use api_keys::{
    ApiKeyError, ApiKeyRecord, ApiKeyService, ApiKeyValidationResult, CreateApiKeyRequest,
    CreateApiKeyResponse,
};

pub use jwt::{Claims, JwtAlgorithm, JwtConfig, JwtError, JwtService};

pub use middleware::{AuthContext, AuthError, AuthMethod, AuthState};

pub use oidc::{OidcClient, OidcConfig, OidcError, SecurityError, TokenResponse};

pub use rbac::{
    check_role_permission, check_role_permission_str, register_rbac_policies,
    Action as RbacAction, Role, RolePermissions,
};
