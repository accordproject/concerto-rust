/// Provides introspection capabilities for Concerto models
/// Maps from various introspect-related JavaScript classes
pub mod introspect {
    use crate::error::ConcertoError;

    /// Checks if a type name is valid in Concerto
    pub fn is_valid_type_name(name: &str) -> bool {
        if name.is_empty() {
            return false;
        }

        // Check if name starts with a letter and contains only alphanumeric or underscore
        name.chars().next().map_or(false, |c| c.is_ascii_alphabetic()) &&
            name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
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
            return Err(ConcertoError::ValidationError(format!("Invalid namespace: {}", namespace)));
        }

        if !is_valid_type_name(name) {
            return Err(ConcertoError::ValidationError(format!("Invalid type name: {}", name)));
        }

        Ok(format!("{}.{}", namespace, name))
    }
}
