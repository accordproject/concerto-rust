// Main library file for concerto_core

// Define modules
pub mod error;
pub mod introspect;
pub mod metamodel_validation;
pub mod model_manager;
pub mod traits;
pub mod types;
pub mod validation;

// Re-export the main components
pub use model_manager::ModelManager;
pub use traits::*;

// Metamodel types exports
pub use concerto_metamodel::concerto_metamodel_1_0_0::{
    Declaration, Decorator, Import, Model, Property, TypeIdentifier,
};

pub use error::ConcertoError;
pub use validation::Validate;

/// Version of the Concerto core library
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Provides introspection capabilities for Concerto models
/// Maps from various introspect-related JavaScript classes
pub mod introspection {
    use crate::error::ConcertoError;

    /// Checks if a type name is valid in Concerto
    pub fn is_valid_type_name(name: &str) -> bool {
        if name.is_empty() {
            return false;
        }

        // Check if name starts with a letter and contains only alphanumeric or underscore
        name.chars()
            .next()
            .map_or(false, |c| c.is_ascii_alphabetic())
            && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    }

    /// Checks if a namespace is valid in Concerto
    pub fn is_valid_namespace(namespace: &str) -> bool {
        if namespace.is_empty() {
            return false;
        }

        // Check if namespace follows dot notation pattern
        namespace.split('.').all(|part| is_valid_type_name(part))
    }

    /// Gets the fully qualified name for a declaration
    pub fn get_fully_qualified_name(namespace: &str, name: &str) -> Result<String, ConcertoError> {
        if !is_valid_namespace(namespace) {
            return Err(ConcertoError::ValidationError(format!(
                "Invalid namespace: {}",
                namespace
            )));
        }

        if !is_valid_type_name(name) {
            return Err(ConcertoError::ValidationError(format!(
                "Invalid type name: {}",
                name
            )));
        }

        Ok(format!("{}.{}", namespace, name))
    }
}

/// Utility functions for Concerto
pub mod utility {
    use semver::Version;

    /// Checks if a string is a valid semver version
    pub fn is_valid_semantic_version(version: &str) -> bool {
        Version::parse(version).is_ok()
    }

    /// Compares two semver versions
    pub fn compare_versions(version1: &str, version2: &str) -> Result<std::cmp::Ordering, String> {
        let v1 = Version::parse(version1).map_err(|e| e.to_string())?;
        let v2 = Version::parse(version2).map_err(|e| e.to_string())?;

        Ok(v1.cmp(&v2))
    }
}
