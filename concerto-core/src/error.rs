//! Error types for `concerto-core`.
//!
//! [`ConcertoError`] covers the hard failures that stop a model from being
//! used: a type or namespace that cannot be resolved, or model JSON that does
//! not satisfy the metamodel. Each variant carries enough context to report
//! what went wrong and, where known, where.

use thiserror::Error;

/// Shorthand `Result` used all over `concerto-core`.
pub type Result<T> = std::result::Result<T, ConcertoError>;

/// A hard failure raised while loading a model or resolving a type.
#[derive(Debug, Error)]
pub enum ConcertoError {
    /// A fully-qualified type could not be resolved in any loaded model.
    #[error("type not found: {type_name}")]
    TypeNotFound {
        /// The name that failed to resolve (qualified or short).
        type_name: String,
    },

    /// A namespace was referenced before any model declared it.
    #[error("namespace not found: {namespace}")]
    NamespaceNotFound {
        /// The namespace that could not be found.
        namespace: String,
    },

    /// The model JSON is malformed or violates a metamodel rule.
    #[error("illegal model: {message}")]
    IllegalModel {
        /// A description of the problem.
        message: String,
        /// The originating file, if known.
        file_name: Option<String>,
        /// The source location, if known.
        location: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn type_not_found_displays_name() {
        let err = ConcertoError::TypeNotFound {
            type_name: "org.acme@1.0.0.Foo".into(),
        };
        assert!(err.to_string().contains("org.acme@1.0.0.Foo"));
    }

    #[test]
    fn illegal_model_displays_message() {
        let err = ConcertoError::IllegalModel {
            message: "missing 'namespace'".into(),
            file_name: Some("model.json".into()),
            location: None,
        };
        assert!(err.to_string().contains("missing 'namespace'"));
    }
}
