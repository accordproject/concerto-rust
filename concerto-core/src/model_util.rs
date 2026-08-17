//! String helpers for Concerto names and namespaces. No dependencies, just
//! `&str` juggling.
//!
//! Concerto names a declaration with a fully-qualified name like
//! `namespace.ShortName`, and the namespace carries a `@version` (so
//! `org.example@1.0.0.Person`). The functions here just split those apart and
//! put them back together. The actual type resolution happens over in the
//! introspect layer.

use crate::error::{ConcertoError, Result};

/// Concerto's six primitives. Everything else is a declared type.
const PRIMITIVE_TYPES: &[&str] = &["Boolean", "String", "DateTime", "Double", "Integer", "Long"];

/// The property names Concerto reserves for itself. A model may not declare a
/// field with any of these names. Identifiers are otherwise allowed to start
/// with a dollar sign, so this is a fixed set rather than a prefix rule.
const RESERVED_PROPERTIES: &[&str] = &[
    // Included in serialization.
    "$class",
    "$identifier",
    "$timestamp",
    "$id",
    // Internal use.
    "$classDeclaration",
    "$namespace",
    "$type",
    "$modelManager",
    "$validator",
    "$identifierFieldName",
    "$imports",
    "$superTypes",
];

/// The short name: whatever comes after the last `.`.
///
/// ```
/// # use concerto_core::model_util::short_name;
/// assert_eq!(short_name("org.example@1.0.0.Person"), "Person");
/// assert_eq!(short_name("Person"), "Person");
/// ```
pub fn short_name(fqn: &str) -> &str {
    match fqn.rfind('.') {
        Some(i) => &fqn[i + 1..],
        None => fqn,
    }
}

/// The namespace: everything before the last `.`. Empty string if the name
/// isn't qualified.
///
/// ```
/// # use concerto_core::model_util::namespace_of;
/// assert_eq!(namespace_of("org.example@1.0.0.Person"), "org.example@1.0.0");
/// assert_eq!(namespace_of("Person"), "");
/// ```
pub fn namespace_of(fqn: &str) -> &str {
    match fqn.rfind('.') {
        Some(i) => &fqn[..i],
        None => "",
    }
}

/// Sticks a namespace and short name back together. An empty namespace just
/// gives you the short name back, which is what we want for primitives.
///
/// ```
/// # use concerto_core::model_util::qualify;
/// assert_eq!(qualify("org.example@1.0.0", "Person"), "org.example@1.0.0.Person");
/// assert_eq!(qualify("", "String"), "String");
/// ```
pub fn qualify(namespace: &str, short: &str) -> String {
    if namespace.is_empty() {
        short.to_string()
    } else {
        format!("{namespace}.{short}")
    }
}

/// A namespace pulled apart into its name and version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespace {
    /// The bare namespace, no version, e.g. `org.example`.
    pub name: String,
    /// The version, e.g. `1.0.0`.
    pub version: String,
}

/// Splits a namespace like `org.example@1.0.0` into name and version.
///
/// Namespace versions are mandatory in Concerto v4, so a namespace without a
/// `@version`, with a second `@`, or with an empty name or version on either
/// side of the `@`, is rejected as an [`ConcertoError::IllegalModel`]. Each dot
/// separated segment of the name has to be a valid identifier.
///
/// ```
/// # use concerto_core::model_util::parse_namespace;
/// let ns = parse_namespace("org.example@1.0.0").unwrap();
/// assert_eq!(ns.name, "org.example");
/// assert_eq!(ns.version, "1.0.0");
/// assert!(parse_namespace("org.example").is_err());
/// ```
pub fn parse_namespace(namespace: &str) -> Result<Namespace> {
    let illegal = || ConcertoError::IllegalModel {
        message: format!("invalid namespace: {namespace}"),
        file_name: None,
        location: None,
    };
    let mut parts = namespace.splitn(3, '@');
    let name = parts.next().unwrap_or("").to_string();
    match (parts.next(), parts.next()) {
        (Some(version), None) => {
            if name.is_empty() || version.is_empty() {
                return Err(illegal());
            }
            if !name.split('.').all(is_valid_identifier) {
                return Err(illegal());
            }
            Ok(Namespace {
                name,
                version: version.to_string(),
            })
        }
        _ => Err(illegal()),
    }
}

/// True if this is one of Concerto's six primitive type names.
///
/// ```
/// # use concerto_core::model_util::is_primitive_type;
/// assert!(is_primitive_type("String"));
/// assert!(!is_primitive_type("Person"));
/// ```
pub fn is_primitive_type(type_name: &str) -> bool {
    PRIMITIVE_TYPES.contains(&type_name)
}

/// True if this is a legal Concerto identifier.
///
/// An identifier starts with a letter, a dollar sign or an underscore, and
/// continues with those or a digit. Names of declarations, properties and
/// namespace segments all have to satisfy this.
///
/// ```
/// # use concerto_core::model_util::is_valid_identifier;
/// assert!(is_valid_identifier("Person"));
/// assert!(!is_valid_identifier("1Person"));
/// ```
pub fn is_valid_identifier(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    let leading = first.is_alphabetic() || first == '$' || first == '_';
    leading && characters.all(|c| c.is_alphanumeric() || c == '$' || c == '_')
}

/// True if this property name is reserved by Concerto and so cannot be
/// declared on a type.
///
/// ```
/// # use concerto_core::model_util::is_system_property;
/// assert!(is_system_property("$class"));
/// assert!(!is_system_property("name"));
/// ```
pub fn is_system_property(property_name: &str) -> bool {
    RESERVED_PROPERTIES.contains(&property_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_name_splits_on_last_dot() {
        assert_eq!(short_name("org.example@1.0.0.Person"), "Person");
        assert_eq!(short_name("a.b.c.D"), "D");
        assert_eq!(short_name("Person"), "Person");
    }

    #[test]
    fn namespace_of_returns_prefix() {
        assert_eq!(
            namespace_of("org.example@1.0.0.Person"),
            "org.example@1.0.0"
        );
        assert_eq!(namespace_of("Person"), "");
    }

    #[test]
    fn qualify_round_trips() {
        let fqn = qualify("org.example@1.0.0", "Person");
        assert_eq!(fqn, "org.example@1.0.0.Person");
        assert_eq!(namespace_of(&fqn), "org.example@1.0.0");
        assert_eq!(short_name(&fqn), "Person");
        assert_eq!(qualify("", "String"), "String");
    }

    #[test]
    fn parse_namespace_requires_version() {
        let ns = parse_namespace("org.example@1.0.0").unwrap();
        assert_eq!(ns.name, "org.example");
        assert_eq!(ns.version, "1.0.0");

        assert!(parse_namespace("org.example").is_err());
        assert!(parse_namespace("a@1@2").is_err());
        assert!(parse_namespace("@1.0.0").is_err());
        assert!(parse_namespace("org.example@").is_err());
        // Every segment of the name has to be an identifier.
        assert!(parse_namespace("org.1bad@1.0.0").is_err());
        assert!(parse_namespace("1org.bad@1.0.0").is_err());
        assert!(parse_namespace("org.a.b.c@1.0.0").is_ok());
    }

    #[test]
    fn identifiers_must_start_with_a_letter_or_sign() {
        for name in ["Person", "_private", "$system", "a1", "Ünicode"] {
            assert!(is_valid_identifier(name), "{name} should be valid");
        }
        for name in ["1Person", "", "with space", "with-dash", "with.dot"] {
            assert!(!is_valid_identifier(name), "{name} should be invalid");
        }
    }

    #[test]
    fn reserved_property_names_are_recognised() {
        for name in ["$class", "$identifier", "$timestamp", "$namespace"] {
            assert!(is_system_property(name));
        }
        // A dollar sign is legal in an identifier, so only the reserved names
        // are rejected.
        assert!(!is_system_property("$other"));
        assert!(!is_system_property("name"));
    }

    #[test]
    fn primitive_types_are_recognised() {
        for t in ["Boolean", "String", "DateTime", "Double", "Integer", "Long"] {
            assert!(is_primitive_type(t));
        }
        assert!(!is_primitive_type("Concept"));
    }
}
