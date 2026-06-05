//! String helpers for Concerto names and namespaces. No dependencies, just
//! `&str` juggling.
//!
//! Concerto names a declaration with a fully-qualified name like
//! `namespace.ShortName`, and the namespace can carry a `@version` (so
//! `org.example@1.0.0.Person`). The functions here just split those apart and
//! put them back together. The actual type resolution happens over in the
//! introspect layer.

use crate::error::{ConcertoError, Result};

/// Concerto's six primitives. Everything else is a declared type.
const PRIMITIVE_TYPES: &[&str] = &["Boolean", "String", "DateTime", "Double", "Integer", "Long"];

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

/// A namespace pulled apart into its name and (maybe) a version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Namespace {
    /// The bare namespace, no version, e.g. `org.example`.
    pub name: String,
    /// The version (`1.0.0`) if there was one.
    pub version: Option<String>,
}

/// Splits a namespace like `org.example@1.0.0` into name and version.
///
/// A second `@` is nonsense, so that's an [`ConcertoError::IllegalModel`].
///
/// ```
/// # use concerto_core::model_util::parse_namespace;
/// let ns = parse_namespace("org.example@1.0.0").unwrap();
/// assert_eq!(ns.name, "org.example");
/// assert_eq!(ns.version.as_deref(), Some("1.0.0"));
/// ```
pub fn parse_namespace(namespace: &str) -> Result<Namespace> {
    let mut parts = namespace.splitn(3, '@');
    let name = parts.next().unwrap_or("").to_string();
    match (parts.next(), parts.next()) {
        (None, _) => Ok(Namespace {
            name,
            version: None,
        }),
        (Some(version), None) => Ok(Namespace {
            name,
            version: Some(version.to_string()),
        }),
        (Some(_), Some(_)) => Err(ConcertoError::IllegalModel {
            message: format!("invalid namespace (multiple '@'): {namespace}"),
            file_name: None,
            location: None,
        }),
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
    fn parse_namespace_handles_version() {
        let ns = parse_namespace("org.example@1.0.0").unwrap();
        assert_eq!(ns.name, "org.example");
        assert_eq!(ns.version.as_deref(), Some("1.0.0"));

        let ns = parse_namespace("org.example").unwrap();
        assert_eq!(ns.name, "org.example");
        assert!(ns.version.is_none());

        assert!(parse_namespace("a@1@2").is_err());
    }

    #[test]
    fn primitive_types_are_recognised() {
        for t in ["Boolean", "String", "DateTime", "Double", "Integer", "Long"] {
            assert!(is_primitive_type(t));
        }
        assert!(!is_primitive_type("Concept"));
    }
}
