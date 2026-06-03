//! Error types for `concerto-core`.
//!
//! There are really two kinds of failure here, and we keep them apart. A hard
//! error means the model is broken and we can't go on (a missing type, a bad
//! namespace). Those are the [`ConcertoError`] cases below. The everyday
//! "this instance doesn't match its type" failures are a different thing: the
//! validation layer gathers those up and hands them back as a result, not a
//! panic-y error.

use thiserror::Error;

/// Shorthand `Result` used all over `concerto-core`.
pub type Result<T> = std::result::Result<T, ConcertoError>;

/// Something went wrong while loading a model or looking up a type.
#[derive(Debug, Error)]
pub enum ConcertoError {
    /// We looked for a type and no loaded model has it.
    #[error("type not found: {type_name}")]
    TypeNotFound {
        /// The name we couldn't resolve (qualified or short).
        type_name: String,
    },

    /// A namespace was referenced before it was loaded.
    #[error("namespace not found: {namespace}")]
    NamespaceNotFound {
        /// The namespace we couldn't find.
        namespace: String,
    },

    /// The model JSON is malformed, or breaks one of Concerto's rules.
    #[error("illegal model: {message}")]
    IllegalModel {
        /// What went wrong, in plain English.
        message: String,
        /// Which file it came from, when we know.
        file_name: Option<String>,
        /// Where in the file, when we know.
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
