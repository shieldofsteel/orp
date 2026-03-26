pub mod ast;
pub mod executor;
pub mod parser;

pub use executor::{QueryExecutor, QueryResult, QueryType};
pub use parser::parse_orpql;

use crate::ast::Query;
use orp_storage::traits::StorageError;

/// Trait representing a fully capable ORP-QL query engine.
/// Implementors can validate, explain, and execute queries.
#[async_trait::async_trait]
pub trait QueryEngine: Send + Sync {
    /// Parse and validate query syntax without executing it.
    /// Returns Ok(()) if the query is syntactically/semantically valid.
    async fn validate(&self, query_str: &str) -> Result<(), StorageError>;

    /// Return a human-readable query execution plan.
    async fn explain(&self, query_str: &str) -> Result<String, StorageError>;

    /// Classify the query type.
    fn query_type(&self, query_str: &str) -> QueryType;

    /// Execute an ORP-QL query string and return results.
    async fn execute(&self, query_str: &str) -> Result<QueryResult, StorageError>;

    /// Execute a parsed AST query.
    async fn execute_ast(&self, query: &Query) -> Result<QueryResult, StorageError>;
}
