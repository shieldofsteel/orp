//! Config-specific error types.

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("I/O error: {0}")]
    IoError(#[from] std::io::Error),
    #[error("Parse error: {0}")]
    ParseError(String),
    #[error("Validation errors: {0:?}")]
    ValidationError(Vec<String>),
}
