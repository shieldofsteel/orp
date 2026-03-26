//! ORP Entity Resolution crate.
//!
//! Provides the [`EntityResolver`] trait and a structural implementation
//! ([`StructuralEntityResolver`]) for Phase 1.

pub mod resolver;

pub use resolver::{
    EntityResolver,
    MatchType,
    ResolutionMatch,
    StructuralEntityResolver,
    name_similarity,
};
