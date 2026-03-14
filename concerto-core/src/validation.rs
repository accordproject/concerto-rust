use crate::error::ConcertoError;

/// A trait for validating Concerto components
pub trait Validate {
    /// Validates the component
    /// Returns Ok(()) if valid, or an error if invalid
    fn validate(&self) -> Result<(), ConcertoError>;
}
