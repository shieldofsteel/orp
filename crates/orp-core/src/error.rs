//! Unified ORP error type (BUILD_CORE_ENGINE.md §8.1).
//!
//! All crates that return errors should use `OrpError` at crate boundaries.
//! Leaf crates may use `anyhow` internally but must convert to `OrpError`
//! before returning across the public API.

use std::fmt;
use std::io;

// When axum is available (orp-core binary crate) we expose HTTP status mapping.
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};

/// Unified error type for all ORP subsystems.
///
/// Organised by layer so callers can match at the right level of granularity.
#[derive(Debug)]
pub enum OrpError {
    // ── Storage layer ─────────────────────────────────────────────────────
    /// General storage failure (e.g., DuckDB exec error).
    StorageError(String),
    /// Low-level database error (schema, constraint violations, …).
    DatabaseError(String),
    /// Transaction begin/commit/rollback failure.
    TransactionError(String),

    // ── Connector layer ───────────────────────────────────────────────────
    /// A connector failed to start, read, or write.
    ConnectorError(String),
    /// The requested connector ID was not found in the registry.
    ConnectorNotFound(String),
    /// A data source (external service) returned an error.
    DataSourceError(String),

    // ── Stream processing ─────────────────────────────────────────────────
    /// Stream processor failed to handle an event.
    StreamProcessorError(String),
    /// The deduplication window (RocksDB) encountered an error.
    DeduplicationError(String),

    // ── Entity resolution ─────────────────────────────────────────────────
    /// Entity resolution algorithm produced an unexpected result.
    EntityResolutionError(String),

    // ── Query execution ───────────────────────────────────────────────────
    /// Query execution failed (runtime error, not a syntax error).
    QueryError(String),
    /// Query syntax or semantic validation failed.
    QueryValidationError(String),
    /// Query execution exceeded the time budget.
    QueryTimeoutError,

    // ── Security ─────────────────────────────────────────────────────────
    /// The caller could not be authenticated.
    AuthenticationError(String),
    /// The authenticated caller lacks permission for this action.
    AuthorizationError(String),
    /// An Ed25519 (or other) signature on incoming data did not verify.
    SignatureVerificationError(String),

    // ── Configuration ─────────────────────────────────────────────────────
    /// Configuration could not be parsed or loaded.
    ConfigError(String),
    /// One or more configuration validation rules were violated.
    ValidationError(Vec<String>),

    // ── FFI (DuckDB / Kuzu C++ bindings) ─────────────────────────────────
    /// An error originating in a C/C++ native library (DuckDB, Kuzu, RocksDB).
    FfiError(String),

    // ── Network ───────────────────────────────────────────────────────────
    /// A network operation failed (TCP, HTTP, MQTT, WebSocket, …).
    NetworkError(String),
    /// A network or I/O operation exceeded its deadline.
    TimeoutError,

    // ── System ────────────────────────────────────────────────────────────
    /// An OS-level I/O error.
    IoError(io::Error),
    /// (De)serialization failed (JSON, CBOR, Protobuf, …).
    SerializationError(String),

    // ── Catch-all ────────────────────────────────────────────────────────
    /// An error that doesn't fit any category above.
    Unknown(String),
}

impl fmt::Display for OrpError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StorageError(msg) => write!(f, "Storage error: {msg}"),
            Self::DatabaseError(msg) => write!(f, "Database error: {msg}"),
            Self::TransactionError(msg) => write!(f, "Transaction error: {msg}"),
            Self::ConnectorError(msg) => write!(f, "Connector error: {msg}"),
            Self::ConnectorNotFound(id) => write!(f, "Connector not found: {id}"),
            Self::DataSourceError(msg) => write!(f, "Data source error: {msg}"),
            Self::StreamProcessorError(msg) => write!(f, "Stream processor error: {msg}"),
            Self::DeduplicationError(msg) => write!(f, "Deduplication error: {msg}"),
            Self::EntityResolutionError(msg) => write!(f, "Entity resolution error: {msg}"),
            Self::QueryError(msg) => write!(f, "Query error: {msg}"),
            Self::QueryValidationError(msg) => write!(f, "Query validation error: {msg}"),
            Self::QueryTimeoutError => write!(f, "Query timed out"),
            Self::AuthenticationError(msg) => write!(f, "Authentication error: {msg}"),
            Self::AuthorizationError(msg) => write!(f, "Authorization error: {msg}"),
            Self::SignatureVerificationError(msg) => {
                write!(f, "Signature verification error: {msg}")
            }
            Self::ConfigError(msg) => write!(f, "Configuration error: {msg}"),
            Self::ValidationError(errs) => {
                write!(f, "Validation errors: [{}]", errs.join(", "))
            }
            Self::FfiError(msg) => write!(f, "FFI error: {msg}"),
            Self::NetworkError(msg) => write!(f, "Network error: {msg}"),
            Self::TimeoutError => write!(f, "Operation timed out"),
            Self::IoError(e) => write!(f, "I/O error: {e}"),
            Self::SerializationError(msg) => write!(f, "Serialization error: {msg}"),
            Self::Unknown(msg) => write!(f, "Unknown error: {msg}"),
        }
    }
}

impl std::error::Error for OrpError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::IoError(e) => Some(e),
            _ => None,
        }
    }
}

// ── Standard conversions ─────────────────────────────────────────────────────

impl From<io::Error> for OrpError {
    fn from(e: io::Error) -> Self {
        Self::IoError(e)
    }
}

impl From<serde_json::Error> for OrpError {
    fn from(e: serde_json::Error) -> Self {
        Self::SerializationError(e.to_string())
    }
}

impl From<anyhow::Error> for OrpError {
    fn from(e: anyhow::Error) -> Self {
        Self::Unknown(e.to_string())
    }
}

// ── HTTP status code mapping (Axum) ──────────────────────────────────────────

impl OrpError {
    /// Map this error to an HTTP [`StatusCode`].
    ///
    /// Used by Axum handlers to return the semantically correct HTTP status.
    pub fn http_status(&self) -> StatusCode {
        match self {
            // 400 — client sent a bad request
            Self::QueryError(_)
            | Self::QueryValidationError(_)
            | Self::ValidationError(_)
            | Self::ConfigError(_) => StatusCode::BAD_REQUEST,

            // 401 — no valid credentials
            Self::AuthenticationError(_) => StatusCode::UNAUTHORIZED,

            // 403 — authenticated but not authorised
            Self::AuthorizationError(_) | Self::SignatureVerificationError(_) => {
                StatusCode::FORBIDDEN
            }

            // 404 — the requested resource simply doesn't exist
            Self::ConnectorNotFound(_) => StatusCode::NOT_FOUND,

            // 408 — timed out waiting
            Self::QueryTimeoutError | Self::TimeoutError => StatusCode::REQUEST_TIMEOUT,

            // 503 — backing service is unavailable
            Self::NetworkError(_)
            | Self::DataSourceError(_)
            | Self::ConnectorError(_) => StatusCode::SERVICE_UNAVAILABLE,

            // 500 — our fault
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

/// Axum `IntoResponse` so handlers can use `?` with `OrpError` directly.
impl IntoResponse for OrpError {
    fn into_response(self) -> Response {
        let status = self.http_status();
        let body = serde_json::json!({
            "error": {
                "code": error_code(&self),
                "message": self.to_string(),
                "status": status.as_u16(),
            }
        });
        (status, axum::Json(body)).into_response()
    }
}

/// Machine-readable error code for JSON error bodies.
fn error_code(e: &OrpError) -> &'static str {
    match e {
        OrpError::StorageError(_) => "STORAGE_ERROR",
        OrpError::DatabaseError(_) => "DATABASE_ERROR",
        OrpError::TransactionError(_) => "TRANSACTION_ERROR",
        OrpError::ConnectorError(_) => "CONNECTOR_ERROR",
        OrpError::ConnectorNotFound(_) => "CONNECTOR_NOT_FOUND",
        OrpError::DataSourceError(_) => "DATA_SOURCE_ERROR",
        OrpError::StreamProcessorError(_) => "STREAM_PROCESSOR_ERROR",
        OrpError::DeduplicationError(_) => "DEDUPLICATION_ERROR",
        OrpError::EntityResolutionError(_) => "ENTITY_RESOLUTION_ERROR",
        OrpError::QueryError(_) => "QUERY_ERROR",
        OrpError::QueryValidationError(_) => "QUERY_VALIDATION_ERROR",
        OrpError::QueryTimeoutError => "QUERY_TIMEOUT",
        OrpError::AuthenticationError(_) => "AUTHENTICATION_ERROR",
        OrpError::AuthorizationError(_) => "AUTHORIZATION_ERROR",
        OrpError::SignatureVerificationError(_) => "SIGNATURE_VERIFICATION_ERROR",
        OrpError::ConfigError(_) => "CONFIG_ERROR",
        OrpError::ValidationError(_) => "VALIDATION_ERROR",
        OrpError::FfiError(_) => "FFI_ERROR",
        OrpError::NetworkError(_) => "NETWORK_ERROR",
        OrpError::TimeoutError => "TIMEOUT_ERROR",
        OrpError::IoError(_) => "IO_ERROR",
        OrpError::SerializationError(_) => "SERIALIZATION_ERROR",
        OrpError::Unknown(_) => "UNKNOWN_ERROR",
    }
}

/// Convenient `Result` alias for ORP operations.
pub type OrpResult<T> = std::result::Result<T, OrpError>;

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::StatusCode;

    #[test]
    fn test_io_error_conversion() {
        let io_err = io::Error::new(io::ErrorKind::NotFound, "file missing");
        let orp: OrpError = io_err.into();
        assert!(matches!(orp, OrpError::IoError(_)));
        assert!(orp.to_string().contains("I/O error"));
    }

    #[test]
    fn test_http_status_query_error() {
        let e = OrpError::QueryError("bad syntax".to_string());
        assert_eq!(e.http_status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn test_http_status_auth() {
        assert_eq!(
            OrpError::AuthenticationError("no token".to_string()).http_status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            OrpError::AuthorizationError("read-only".to_string()).http_status(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn test_http_status_timeout() {
        assert_eq!(
            OrpError::QueryTimeoutError.http_status(),
            StatusCode::REQUEST_TIMEOUT
        );
        assert_eq!(
            OrpError::TimeoutError.http_status(),
            StatusCode::REQUEST_TIMEOUT
        );
    }

    #[test]
    fn test_http_status_not_found() {
        assert_eq!(
            OrpError::ConnectorNotFound("ais-01".to_string()).http_status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn test_display_validation_error() {
        let e = OrpError::ValidationError(vec![
            "port must be > 0".to_string(),
            "memory limit too low".to_string(),
        ]);
        let s = e.to_string();
        assert!(s.contains("port must be > 0"));
        assert!(s.contains("memory limit too low"));
    }

    #[test]
    fn test_error_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        // OrpError must be Send+Sync to work across tokio task boundaries
        assert_send_sync::<OrpError>();
    }
}
