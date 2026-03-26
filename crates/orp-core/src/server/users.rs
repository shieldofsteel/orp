//! Multi-user management for ORP operations rooms.
//!
//! Provides a user registry backed by DuckDB, REST endpoints for CRUD,
//! and password hashing (SHA-256 + salt — no external dependency required).
//!
//! ## REST API
//!
//! | Method | Path                         | Role Required | Description              |
//! |--------|------------------------------|---------------|--------------------------|
//! | POST   | /api/v1/users                | Admin+        | Create a new user        |
//! | GET    | /api/v1/users                | Admin+        | List all users           |
//! | PUT    | /api/v1/users/{id}/role      | Admin+        | Change user role         |
//! | DELETE | /api/v1/users/{id}           | Admin+        | Deactivate user          |
//! | GET    | /api/v1/users/me             | Any auth      | Current user profile     |
//!
//! ## Password Hashing
//!
//! Uses SHA-256 + random 32-byte salt stored as `$sha256$<hex_salt>$<hex_hash>`.
//! This is a minimum-viable implementation. When the `argon2` crate is added
//! as a dependency, replace `hash_password` / `verify_password` with argon2id.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Json},
    routing::{delete, get, post, put},
    Router,
};
use chrono::{DateTime, Utc};
use duckdb::{params, Connection};
use orp_security::{
    middleware::AuthContext,
    rbac::{check_role_permission, Action, Role},
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

// ─── Schema ───────────────────────────────────────────────────────────────────

pub const USERS_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS users (
    user_id      VARCHAR PRIMARY KEY,
    username     VARCHAR NOT NULL UNIQUE,
    email        VARCHAR NOT NULL UNIQUE,
    display_name VARCHAR NOT NULL DEFAULT '',
    password_hash VARCHAR NOT NULL,
    role         VARCHAR NOT NULL DEFAULT 'viewer',
    is_active    BOOLEAN NOT NULL DEFAULT TRUE,
    created_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at   TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_login   TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_users_email    ON users(email);
CREATE INDEX IF NOT EXISTS idx_users_username ON users(username);
CREATE INDEX IF NOT EXISTS idx_users_role     ON users(role);
CREATE INDEX IF NOT EXISTS idx_users_active   ON users(is_active);
"#;

// ─── Models ───────────────────────────────────────────────────────────────────

/// A registered ORP user.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct User {
    pub user_id: String,
    pub username: String,
    pub email: String,
    pub display_name: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub role: String,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login: Option<DateTime<Utc>>,
}

/// Public view of a user — no password hash.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UserProfile {
    pub user_id: String,
    pub username: String,
    pub email: String,
    pub display_name: String,
    pub role: String,
    pub is_active: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login: Option<DateTime<Utc>>,
}

impl From<User> for UserProfile {
    fn from(u: User) -> Self {
        Self {
            user_id: u.user_id,
            username: u.username,
            email: u.email,
            display_name: u.display_name,
            role: u.role,
            is_active: u.is_active,
            created_at: u.created_at,
            updated_at: u.updated_at,
            last_login: u.last_login,
        }
    }
}

// ─── Request / Response DTOs ──────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub email: String,
    pub display_name: Option<String>,
    pub password: String,
    pub role: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ChangeRoleRequest {
    pub role: String,
}

#[derive(Debug, Serialize)]
pub struct UserListResponse {
    pub users: Vec<UserProfile>,
    pub total: usize,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: String,
    status: u16,
    message: String,
    request_id: String,
    timestamp: String,
}

fn error_resp(code: &str, status: StatusCode, message: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: ErrorBody {
                code: code.to_string(),
                status: status.as_u16(),
                message: message.to_string(),
                request_id: Uuid::new_v4().to_string(),
                timestamp: Utc::now().to_rfc3339(),
            },
        }),
    )
}

// ─── Password hashing ─────────────────────────────────────────────────────────

/// Hash a password with a random 32-byte salt using SHA-256.
///
/// Output format: `$sha256$<hex_salt>$<hex_hash>`
///
/// NOTE: Replace with argon2id when `argon2` crate is available for production
/// deployments. SHA-256 is acceptable for internal ops rooms without external
/// access, but argon2id provides better brute-force resistance.
pub fn hash_password(password: &str) -> String {
    let mut salt = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut salt);
    let hash = sha256_hash_with_salt(password, &salt);
    format!("$sha256${}${}", hex::encode(salt), hex::encode(hash))
}

/// Verify a password against a stored hash.
pub fn verify_password(password: &str, stored: &str) -> bool {
    let parts: Vec<&str> = stored.splitn(4, '$').collect();
    // Format: ["", "sha256", "<salt_hex>", "<hash_hex>"]
    if parts.len() != 4 || parts[1] != "sha256" {
        return false;
    }
    let Ok(salt) = hex::decode(parts[2]) else { return false; };
    let Ok(expected_hash) = hex::decode(parts[3]) else { return false; };
    let actual_hash = sha256_hash_with_salt(password, &salt);
    // Constant-time comparison
    constant_time_eq(&actual_hash, &expected_hash)
}

fn sha256_hash_with_salt(password: &str, salt: &[u8]) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(password.as_bytes());
    hasher.finalize().to_vec()
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ─── UserRegistry ─────────────────────────────────────────────────────────────

/// Thread-safe user registry backed by DuckDB.
#[derive(Clone)]
pub struct UserRegistry {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, thiserror::Error)]
pub enum UserError {
    #[error("User not found: {0}")]
    NotFound(String),
    #[error("User already exists: {0}")]
    AlreadyExists(String),
    #[error("Invalid role: {0}")]
    InvalidRole(String),
    #[error("Database error: {0}")]
    Db(#[from] duckdb::Error),
    #[error("Lock poisoned")]
    Lock,
}

impl UserRegistry {
    /// Create a registry against an existing DuckDB connection.
    pub fn new(conn: Arc<Mutex<Connection>>) -> Result<Self, UserError> {
        {
            let c = conn.lock().map_err(|_| UserError::Lock)?;
            c.execute_batch(USERS_SCHEMA).map_err(UserError::Db)?;
        }
        Ok(Self { conn })
    }

    /// Create an in-memory registry (useful for testing and embedded deployments).
    pub fn in_memory() -> Result<Self, UserError> {
        let conn = Connection::open_in_memory().map_err(UserError::Db)?;
        let registry = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        {
            let c = registry.conn.lock().map_err(|_| UserError::Lock)?;
            c.execute_batch(USERS_SCHEMA).map_err(UserError::Db)?;
        }
        Ok(registry)
    }

    // ── CRUD ──────────────────────────────────────────────────────────────────

    /// Create a new user. Returns the created user profile.
    pub fn create(&self, req: CreateUserRequest) -> Result<UserProfile, UserError> {
        // Validate role
        let role_str = req.role.as_deref().unwrap_or("viewer");
        Role::from_str(role_str)
            .ok_or_else(|| UserError::InvalidRole(role_str.to_string()))?;

        let user_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let display_name = req
            .display_name
            .unwrap_or_else(|| req.username.clone());
        let password_hash = hash_password(&req.password);

        let c = self.conn.lock().map_err(|_| UserError::Lock)?;
        c.execute(
            r#"INSERT INTO users
               (user_id, username, email, display_name, password_hash, role, is_active, created_at, updated_at)
               VALUES (?, ?, ?, ?, ?, ?, TRUE, ?, ?)"#,
            params![
                user_id,
                req.username,
                req.email,
                display_name,
                password_hash,
                role_str,
                now.to_rfc3339(),
                now.to_rfc3339(),
            ],
        )
        .map_err(|e| {
            let msg = e.to_string();
            if msg.contains("Constraint Error") || msg.contains("UNIQUE") {
                UserError::AlreadyExists(req.username.clone())
            } else {
                UserError::Db(e)
            }
        })?;

        Ok(UserProfile {
            user_id,
            username: req.username,
            email: req.email,
            display_name,
            role: role_str.to_string(),
            is_active: true,
            created_at: now,
            updated_at: now,
            last_login: None,
        })
    }

    /// List all users (admin view).
    pub fn list(&self) -> Result<Vec<UserProfile>, UserError> {
        let c = self.conn.lock().map_err(|_| UserError::Lock)?;
        let mut stmt = c.prepare(
            "SELECT user_id, username, email, display_name, role, is_active,
                    CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR),
                    CAST(last_login AS VARCHAR)
             FROM users ORDER BY created_at ASC",
        )?;

        let users = stmt
            .query_map([], |row| {
                Ok(UserProfile {
                    user_id: row.get(0)?,
                    username: row.get(1)?,
                    email: row.get(2)?,
                    display_name: row.get(3)?,
                    role: row.get(4)?,
                    is_active: row.get(5)?,
                    created_at: parse_ts(row.get::<_, String>(6)?),
                    updated_at: parse_ts(row.get::<_, String>(7)?),
                    last_login: row
                        .get::<_, Option<String>>(8)?
                        .map(parse_ts),
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(users)
    }

    /// Get a single user by ID.
    pub fn get(&self, user_id: &str) -> Result<UserProfile, UserError> {
        let c = self.conn.lock().map_err(|_| UserError::Lock)?;
        let mut stmt = c.prepare(
            "SELECT user_id, username, email, display_name, role, is_active,
                    CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR),
                    CAST(last_login AS VARCHAR)
             FROM users WHERE user_id = ?",
        )?;

        let mut rows = stmt.query_map(params![user_id], |row| {
            Ok(UserProfile {
                user_id: row.get(0)?,
                username: row.get(1)?,
                email: row.get(2)?,
                display_name: row.get(3)?,
                role: row.get(4)?,
                is_active: row.get(5)?,
                created_at: parse_ts(row.get::<_, String>(6)?),
                updated_at: parse_ts(row.get::<_, String>(7)?),
                last_login: row.get::<_, Option<String>>(8)?.map(parse_ts),
            })
        })?;

        rows.next()
            .ok_or_else(|| UserError::NotFound(user_id.to_string()))?
            .map_err(UserError::Db)
    }

    /// Find a user by email (for authentication).
    pub fn find_by_email(&self, email: &str) -> Result<User, UserError> {
        let c = self.conn.lock().map_err(|_| UserError::Lock)?;
        let mut stmt = c.prepare(
            "SELECT user_id, username, email, display_name, password_hash, role, is_active,
                    CAST(created_at AS VARCHAR), CAST(updated_at AS VARCHAR),
                    CAST(last_login AS VARCHAR)
             FROM users WHERE email = ? AND is_active = TRUE",
        )?;

        let mut rows = stmt.query_map(params![email], |row| {
            Ok(User {
                user_id: row.get(0)?,
                username: row.get(1)?,
                email: row.get(2)?,
                display_name: row.get(3)?,
                password_hash: row.get(4)?,
                role: row.get(5)?,
                is_active: row.get(6)?,
                created_at: parse_ts(row.get::<_, String>(7)?),
                updated_at: parse_ts(row.get::<_, String>(8)?),
                last_login: row.get::<_, Option<String>>(9)?.map(parse_ts),
            })
        })?;

        rows.next()
            .ok_or_else(|| UserError::NotFound(email.to_string()))?
            .map_err(UserError::Db)
    }

    /// Change a user's role.
    pub fn change_role(&self, user_id: &str, new_role: &str) -> Result<UserProfile, UserError> {
        Role::from_str(new_role)
            .ok_or_else(|| UserError::InvalidRole(new_role.to_string()))?;

        let now = Utc::now();
        let c = self.conn.lock().map_err(|_| UserError::Lock)?;
        let affected = c.execute(
            "UPDATE users SET role = ?, updated_at = ? WHERE user_id = ? AND is_active = TRUE",
            params![new_role, now.to_rfc3339(), user_id],
        )?;

        if affected == 0 {
            return Err(UserError::NotFound(user_id.to_string()));
        }

        drop(c);
        self.get(user_id)
    }

    /// Deactivate a user (soft delete — preserves audit trail).
    pub fn deactivate(&self, user_id: &str) -> Result<(), UserError> {
        let now = Utc::now();
        let c = self.conn.lock().map_err(|_| UserError::Lock)?;
        let affected = c.execute(
            "UPDATE users SET is_active = FALSE, updated_at = ? WHERE user_id = ?",
            params![now.to_rfc3339(), user_id],
        )?;

        if affected == 0 {
            return Err(UserError::NotFound(user_id.to_string()));
        }

        Ok(())
    }

    /// Update last login timestamp.
    pub fn record_login(&self, user_id: &str) -> Result<(), UserError> {
        let now = Utc::now();
        let c = self.conn.lock().map_err(|_| UserError::Lock)?;
        c.execute(
            "UPDATE users SET last_login = ?, updated_at = ? WHERE user_id = ?",
            params![now.to_rfc3339(), now.to_rfc3339(), user_id],
        )?;
        Ok(())
    }
}

fn parse_ts(s: String) -> DateTime<Utc> {
    s.parse::<DateTime<Utc>>()
        .or_else(|_| {
            // DuckDB may return TIMESTAMP as "YYYY-MM-DD HH:MM:SS.ffffff +00:00"
            DateTime::parse_from_str(&s, "%Y-%m-%d %H:%M:%S%.f %z")
                .map(|dt| dt.with_timezone(&Utc))
        })
        .unwrap_or_else(|_| Utc::now())
}

// ─── Auth requirement helpers ─────────────────────────────────────────────────

/// Require a minimum role from the AuthContext.
/// Returns Err(403) if the caller's role is insufficient.
fn require_role(
    auth: &AuthContext,
    action: &Action,
) -> Result<(), (StatusCode, Json<ErrorResponse>)> {
    // SuperAdmin / Admin bypass: check the "admin" permission first
    if auth.has_permission("admin") {
        return Ok(());
    }

    // Resolve the caller's role from their permissions
    let caller_role = role_from_auth(auth);

    if check_role_permission(caller_role, action) {
        Ok(())
    } else {
        Err(error_resp(
            "FORBIDDEN",
            StatusCode::FORBIDDEN,
            &format!(
                "Role '{}' is not permitted to perform '{}'",
                caller_role.as_str(),
                action.as_scope()
            ),
        ))
    }
}

/// Resolve a `Role` from an `AuthContext` by inspecting its permissions.
///
/// Falls back to `Guest` if no recognizable role permissions are present.
fn role_from_auth(auth: &AuthContext) -> Role {
    if auth.has_permission("admin") {
        return Role::SuperAdmin;
    }
    // Heuristic: look for a `role:<name>` permission or check capability levels
    if auth.permissions.iter().any(|p| p == "users:create") {
        return Role::Admin;
    }
    if auth.permissions.iter().any(|p| p == "alerts:acknowledge") {
        return Role::Operator;
    }
    if auth.permissions.iter().any(|p| p == "query:execute") {
        return Role::Analyst;
    }
    if auth.permissions.iter().any(|p| p == "entities:read") {
        return Role::Viewer;
    }
    Role::Guest
}

// ─── State for handlers ───────────────────────────────────────────────────────

/// Minimal state needed by user management handlers.
#[derive(Clone)]
pub struct UserState {
    pub registry: Arc<UserRegistry>,
}

// ─── Handlers ────────────────────────────────────────────────────────────────

/// POST /api/v1/users — create user (Admin+)
pub async fn create_user(
    State(state): State<UserState>,
    auth: AuthContext,
    Json(req): Json<CreateUserRequest>,
) -> impl IntoResponse {
    if let Err(e) = require_role(&auth, &Action::UsersCreate) {
        return e.into_response();
    }

    if req.username.trim().is_empty() || req.email.trim().is_empty() || req.password.is_empty() {
        return error_resp("VALIDATION_ERROR", StatusCode::BAD_REQUEST, "username, email, and password are required")
            .into_response();
    }

    match state.registry.create(req) {
        Ok(user) => (StatusCode::CREATED, Json(user)).into_response(),
        Err(UserError::AlreadyExists(name)) => error_resp(
            "CONFLICT",
            StatusCode::CONFLICT,
            &format!("User already exists: {name}"),
        )
        .into_response(),
        Err(UserError::InvalidRole(r)) => error_resp(
            "VALIDATION_ERROR",
            StatusCode::BAD_REQUEST,
            &format!("Invalid role: {r}"),
        )
        .into_response(),
        Err(e) => {
            tracing::error!("create_user error: {e}");
            error_resp("INTERNAL_ERROR", StatusCode::INTERNAL_SERVER_ERROR, "Failed to create user")
                .into_response()
        }
    }
}

/// GET /api/v1/users — list users (Admin+)
pub async fn list_users(
    State(state): State<UserState>,
    auth: AuthContext,
) -> impl IntoResponse {
    if let Err(e) = require_role(&auth, &Action::UsersView) {
        return e.into_response();
    }

    match state.registry.list() {
        Ok(users) => {
            let total = users.len();
            Json(UserListResponse { users, total }).into_response()
        }
        Err(e) => {
            tracing::error!("list_users error: {e}");
            error_resp("INTERNAL_ERROR", StatusCode::INTERNAL_SERVER_ERROR, "Failed to list users")
                .into_response()
        }
    }
}

/// PUT /api/v1/users/{id}/role — change role (Admin+)
pub async fn change_user_role(
    State(state): State<UserState>,
    auth: AuthContext,
    Path(user_id): Path<String>,
    Json(req): Json<ChangeRoleRequest>,
) -> impl IntoResponse {
    if let Err(e) = require_role(&auth, &Action::UsersChangeRole) {
        return e.into_response();
    }

    // Prevent privilege escalation: non-SuperAdmin cannot promote to SuperAdmin
    if req.role == "super_admin" && !auth.has_permission("admin") {
        return error_resp(
            "FORBIDDEN",
            StatusCode::FORBIDDEN,
            "Only SuperAdmin can assign the SuperAdmin role",
        )
        .into_response();
    }

    // Prevent self-role changes
    if user_id == auth.subject {
        return error_resp(
            "FORBIDDEN",
            StatusCode::FORBIDDEN,
            "Cannot change your own role",
        )
        .into_response();
    }

    match state.registry.change_role(&user_id, &req.role) {
        Ok(user) => Json(user).into_response(),
        Err(UserError::NotFound(_)) => error_resp(
            "NOT_FOUND",
            StatusCode::NOT_FOUND,
            &format!("User not found: {user_id}"),
        )
        .into_response(),
        Err(UserError::InvalidRole(r)) => error_resp(
            "VALIDATION_ERROR",
            StatusCode::BAD_REQUEST,
            &format!("Invalid role: {r}"),
        )
        .into_response(),
        Err(e) => {
            tracing::error!("change_user_role error: {e}");
            error_resp("INTERNAL_ERROR", StatusCode::INTERNAL_SERVER_ERROR, "Failed to change role")
                .into_response()
        }
    }
}

/// DELETE /api/v1/users/{id} — deactivate user (Admin+)
pub async fn deactivate_user(
    State(state): State<UserState>,
    auth: AuthContext,
    Path(user_id): Path<String>,
) -> impl IntoResponse {
    if let Err(e) = require_role(&auth, &Action::UsersDelete) {
        return e.into_response();
    }

    // Prevent self-deletion
    if user_id == auth.subject {
        return error_resp(
            "FORBIDDEN",
            StatusCode::FORBIDDEN,
            "Cannot deactivate your own account",
        )
        .into_response();
    }

    match state.registry.deactivate(&user_id) {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(UserError::NotFound(_)) => error_resp(
            "NOT_FOUND",
            StatusCode::NOT_FOUND,
            &format!("User not found: {user_id}"),
        )
        .into_response(),
        Err(e) => {
            tracing::error!("deactivate_user error: {e}");
            error_resp(
                "INTERNAL_ERROR",
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to deactivate user",
            )
            .into_response()
        }
    }
}

/// GET /api/v1/users/me — current user profile
pub async fn get_current_user(
    State(state): State<UserState>,
    auth: AuthContext,
) -> impl IntoResponse {
    match state.registry.get(&auth.subject) {
        Ok(user) => Json(user).into_response(),
        Err(UserError::NotFound(_)) => {
            // User is authenticated but not in local DB (e.g. JWT-only setup)
            // Return a synthetic profile from the JWT claims
            Json(UserProfile {
                user_id: auth.subject.clone(),
                username: auth
                    .email
                    .as_deref()
                    .and_then(|e| e.split('@').next())
                    .unwrap_or(&auth.subject)
                    .to_string(),
                email: auth.email.clone().unwrap_or_default(),
                display_name: auth.name.clone().unwrap_or_else(|| auth.subject.clone()),
                role: role_from_auth(&auth).as_str().to_string(),
                is_active: true,
                created_at: Utc::now(),
                updated_at: Utc::now(),
                last_login: None,
            })
            .into_response()
        }
        Err(e) => {
            tracing::error!("get_current_user error: {e}");
            error_resp(
                "INTERNAL_ERROR",
                StatusCode::INTERNAL_SERVER_ERROR,
                "Failed to retrieve user profile",
            )
            .into_response()
        }
    }
}

// ─── Router ───────────────────────────────────────────────────────────────────

/// Build the users sub-router.
///
/// Mount under `/api/v1` in the main router:
/// ```rust,no_run
/// let app = Router::new()
///     .nest("/api/v1", users_router(registry));
/// ```
pub fn users_router(registry: Arc<UserRegistry>) -> Router {
    let state = UserState { registry };
    Router::new()
        .route("/users", post(create_user).get(list_users))
        .route("/users/me", get(get_current_user))
        .route("/users/{id}/role", put(change_user_role))
        .route("/users/{id}", delete(deactivate_user))
        .with_state(state)
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn test_registry() -> UserRegistry {
        UserRegistry::in_memory().expect("in-memory registry")
    }

    fn create_req(username: &str, email: &str, role: &str) -> CreateUserRequest {
        CreateUserRequest {
            username: username.to_string(),
            email: email.to_string(),
            display_name: Some(format!("{} Display", username)),
            password: "TestPass123!".to_string(),
            role: Some(role.to_string()),
        }
    }

    // ── Password hashing ──────────────────────────────────────────────────────

    #[test]
    fn password_hash_roundtrip() {
        let hash = hash_password("MySecret123");
        assert!(verify_password("MySecret123", &hash));
    }

    #[test]
    fn wrong_password_fails_verification() {
        let hash = hash_password("correct");
        assert!(!verify_password("wrong", &hash));
    }

    #[test]
    fn two_hashes_of_same_password_differ() {
        let h1 = hash_password("same");
        let h2 = hash_password("same");
        assert_ne!(h1, h2, "Different salts should produce different hashes");
    }

    #[test]
    fn hash_format_starts_with_sha256() {
        let h = hash_password("test");
        assert!(h.starts_with("$sha256$"));
    }

    // ── CRUD operations ──────────────────────────────────────────────────────

    #[test]
    fn create_user_success() {
        let reg = test_registry();
        let req = create_req("alice", "alice@ops.example", "operator");
        let user = reg.create(req).expect("create user");
        assert_eq!(user.username, "alice");
        assert_eq!(user.role, "operator");
        assert!(user.is_active);
    }

    #[test]
    fn create_user_default_role_is_viewer() {
        let reg = test_registry();
        let req = CreateUserRequest {
            username: "bob".to_string(),
            email: "bob@ops.example".to_string(),
            display_name: None,
            password: "pass".to_string(),
            role: None,
        };
        let user = reg.create(req).expect("create");
        assert_eq!(user.role, "viewer");
    }

    #[test]
    fn create_duplicate_user_fails() {
        let reg = test_registry();
        let req1 = create_req("dup", "dup@example.com", "viewer");
        reg.create(req1).expect("first create");

        let req2 = create_req("dup", "dup2@example.com", "viewer");
        let err = reg.create(req2).expect_err("should fail");
        assert!(matches!(err, UserError::AlreadyExists(_)));
    }

    #[test]
    fn create_user_invalid_role_fails() {
        let reg = test_registry();
        let req = CreateUserRequest {
            username: "badrol".to_string(),
            email: "br@example.com".to_string(),
            display_name: None,
            password: "pass".to_string(),
            role: Some("wizard".to_string()),
        };
        let err = reg.create(req).expect_err("should fail");
        assert!(matches!(err, UserError::InvalidRole(_)));
    }

    #[test]
    fn list_users_returns_all() {
        let reg = test_registry();
        reg.create(create_req("u1", "u1@ex.com", "viewer")).unwrap();
        reg.create(create_req("u2", "u2@ex.com", "analyst")).unwrap();
        let users = reg.list().expect("list");
        assert_eq!(users.len(), 2);
    }

    #[test]
    fn get_user_by_id() {
        let reg = test_registry();
        let created = reg.create(create_req("getme", "getme@ex.com", "viewer")).unwrap();
        let fetched = reg.get(&created.user_id).expect("get");
        assert_eq!(fetched.user_id, created.user_id);
        assert_eq!(fetched.email, "getme@ex.com");
    }

    #[test]
    fn get_nonexistent_user_fails() {
        let reg = test_registry();
        let err = reg.get("nonexistent-id").expect_err("should fail");
        assert!(matches!(err, UserError::NotFound(_)));
    }

    #[test]
    fn change_role_success() {
        let reg = test_registry();
        let user = reg.create(create_req("rolechange", "rc@ex.com", "viewer")).unwrap();
        let updated = reg.change_role(&user.user_id, "analyst").expect("change role");
        assert_eq!(updated.role, "analyst");
    }

    #[test]
    fn change_role_invalid_fails() {
        let reg = test_registry();
        let user = reg.create(create_req("badrc", "brc@ex.com", "viewer")).unwrap();
        let err = reg.change_role(&user.user_id, "god").expect_err("should fail");
        assert!(matches!(err, UserError::InvalidRole(_)));
    }

    #[test]
    fn deactivate_user_success() {
        let reg = test_registry();
        let user = reg.create(create_req("deact", "deact@ex.com", "viewer")).unwrap();
        reg.deactivate(&user.user_id).expect("deactivate");
        // Re-fetch — user should be inactive
        let fetched = reg.get(&user.user_id).expect("still findable");
        assert!(!fetched.is_active);
    }

    #[test]
    fn deactivate_nonexistent_fails() {
        let reg = test_registry();
        let err = reg.deactivate("nope").expect_err("should fail");
        assert!(matches!(err, UserError::NotFound(_)));
    }

    #[test]
    fn find_by_email_success() {
        let reg = test_registry();
        let created = reg.create(create_req("emailfind", "ef@ex.com", "analyst")).unwrap();
        let user = reg.find_by_email("ef@ex.com").expect("find by email");
        assert_eq!(user.user_id, created.user_id);
    }

    #[test]
    fn record_login_updates_timestamp() {
        let reg = test_registry();
        let user = reg.create(create_req("loginrec", "lr@ex.com", "viewer")).unwrap();
        assert!(user.last_login.is_none());
        reg.record_login(&user.user_id).expect("record login");
        let updated = reg.get(&user.user_id).expect("get");
        assert!(updated.last_login.is_some());
    }

    // ── RBAC integration ─────────────────────────────────────────────────────

    #[test]
    fn role_from_auth_with_admin_permission() {
        let auth = AuthContext {
            subject: "sa".to_string(),
            permissions: vec!["admin".to_string()],
            email: None,
            name: None,
            org_id: None,
            scopes: vec![],
            auth_method: orp_security::middleware::AuthMethod::DevMode,
        };
        assert_eq!(role_from_auth(&auth), Role::SuperAdmin);
    }

    #[test]
    fn role_from_auth_operator() {
        let auth = AuthContext {
            subject: "op".to_string(),
            permissions: vec!["entities:read".to_string(), "alerts:acknowledge".to_string()],
            email: None,
            name: None,
            org_id: None,
            scopes: vec![],
            auth_method: orp_security::middleware::AuthMethod::DevMode,
        };
        assert_eq!(role_from_auth(&auth), Role::Operator);
    }
}
