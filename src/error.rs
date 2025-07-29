use thiserror::Error;

/// Error types for Concerto operations
#[derive(Error, Debug)]
pub enum ConcertoError {
    /// Error during parsing
    #[error("Parse error: {0}")]
    ParseError(String),

    /// Error during validation
    #[error("Validation error: {0}")]
    ValidationError(String),

    /// Declaration not found
    #[error("Declaration not found: {0}")]
    DeclarationNotFound(String),

    /// Namespace not found
    #[error("Namespace not found: {0}")]
    NamespaceNotFound(String),

    /// I/O error
    #[error("I/O error: {0}")]
    IoError(String),

    /// Generic error
    #[error("Error: {0}")]
    GenericError(String),
}
