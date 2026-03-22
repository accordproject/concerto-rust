use std::sync::LazyLock;

use regex::Regex;

// Unicode-aware identifier regex, mirrors the JS ID_REGEX.
// Rust's `regex` crate supports Unicode categories with \p{…}.
static ID_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^[\p{Lu}\p{Ll}\p{Lt}\p{Lm}\p{Lo}\p{Nl}$_][\p{Lu}\p{Ll}\p{Lt}\p{Lm}\p{Lo}\p{Nl}$_\p{Mn}\p{Mc}\p{Nd}\p{Pc}\x{200C}\x{200D}]*$",
    )
    .expect("ID_REGEX is valid")
});

const PRIMITIVE_TYPES: &[&str] = &["Boolean", "String", "DateTime", "Double", "Integer", "Long"];

const RESERVED_PROPERTIES: &[&str] = &[
    "$class",
    "$identifier",
    "$timestamp",
    // private / internal
    "$classDeclaration",
    "$namespace",
    "$type",
    "$modelManager",
    "$validator",
    "$identifierFieldName",
    "$imports",
    "$superTypes",
    "$id",
];

const PRIVATE_RESERVED_PROPERTIES: &[&str] = &[
    "$classDeclaration",
    "$namespace",
    "$type",
    "$modelManager",
    "$validator",
    "$identifierFieldName",
    "$imports",
    "$superTypes",
    "$id",
];

/// Returns everything after the last dot of the fully-qualified name.
///
/// ```
/// # use concerto_core::model_util::get_short_name;
/// assert_eq!(get_short_name("org.example.Asset"), "Asset");
/// assert_eq!(get_short_name("Asset"), "Asset");
/// ```
pub fn get_short_name(fqn: &str) -> &str {
    match fqn.rfind('.') {
        Some(idx) => &fqn[idx + 1..],
        None => fqn,
    }
}

/// Returns the namespace portion of a fully-qualified name (everything
/// before the last dot). Returns an empty string if there is no dot.
///
/// ```
/// # use concerto_core::model_util::get_namespace;
/// assert_eq!(get_namespace("org.example.Asset"), "org.example");
/// assert_eq!(get_namespace("Asset"), "");
/// ```
pub fn get_namespace(fqn: &str) -> &str {
    match fqn.rfind('.') {
        Some(idx) => &fqn[..idx],
        None => "",
    }
}

/// Builds a fully-qualified name from a namespace and a short type name.
///
/// ```
/// # use concerto_core::model_util::fully_qualified_name;
/// assert_eq!(fully_qualified_name("org.example", "Asset"), "org.example.Asset");
/// assert_eq!(fully_qualified_name("", "Asset"), "Asset");
/// ```
pub fn fully_qualified_name(namespace: &str, type_name: &str) -> String {
    if namespace.is_empty() {
        type_name.to_string()
    } else {
        format!("{namespace}.{type_name}")
    }
}

/// Result of parsing a potentially-versioned namespace string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedNamespace {
    /// The namespace name without the version.
    pub name: String,
    /// The version string (e.g. `"1.0.0"`), if present.
    pub version: Option<String>,
}

/// Parses a namespace that may contain a `@version` suffix.
///
/// ```
/// # use concerto_core::model_util::parse_namespace;
/// let ns = parse_namespace("org.example@1.0.0").unwrap();
/// assert_eq!(ns.name, "org.example");
/// assert_eq!(ns.version.as_deref(), Some("1.0.0"));
///
/// let ns = parse_namespace("org.example").unwrap();
/// assert_eq!(ns.name, "org.example");
/// assert_eq!(ns.version, None);
/// ```
pub fn parse_namespace(ns: &str) -> crate::error::Result<ParsedNamespace> {
    let parts: Vec<&str> = ns.split('@').collect();
    match parts.len() {
        1 => Ok(ParsedNamespace {
            name: parts[0].to_string(),
            version: None,
        }),
        2 => Ok(ParsedNamespace {
            name: parts[0].to_string(),
            version: Some(parts[1].to_string()),
        }),
        _ => Err(crate::error::ConcertoError::IllegalModel {
            message: format!("Invalid namespace {ns}"),
            file_name: None,
            location: None,
        }),
    }
}

/// Returns `true` if `type_name` is one of the Concerto primitive types.
pub fn is_primitive_type(type_name: &str) -> bool {
    PRIMITIVE_TYPES.contains(&type_name)
}

/// Returns `true` if `name` is a reserved system property (e.g. `$class`).
pub fn is_system_property(name: &str) -> bool {
    RESERVED_PROPERTIES.contains(&name)
}

/// Returns `true` if `name` is a private (internal-only) system property.
pub fn is_private_system_property(name: &str) -> bool {
    PRIVATE_RESERVED_PROPERTIES.contains(&name)
}

/// Returns `true` if `name` is a valid Concerto identifier.
pub fn is_valid_identifier(name: &str) -> bool {
    ID_REGEX.is_match(name)
}

/// Strips the `@version` part from the namespace portion of a
/// fully-qualified type name. Primitive types are returned unchanged.
pub fn remove_namespace_version(fqn: &str) -> String {
    if is_primitive_type(fqn) {
        return fqn.to_string();
    }
    let ns = get_namespace(fqn);
    let parsed = match parse_namespace(ns) {
        Ok(p) => p,
        Err(_) => return fqn.to_string(),
    };
    let type_name = get_short_name(fqn);
    fully_qualified_name(&parsed.name, type_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_short_name() {
        assert_eq!(get_short_name("org.example@1.0.0.Asset"), "Asset");
        assert_eq!(get_short_name("Asset"), "Asset");
        assert_eq!(get_short_name("a.b.c.D"), "D");
    }

    #[test]
    fn test_get_namespace() {
        assert_eq!(get_namespace("org.example.Asset"), "org.example");
        assert_eq!(get_namespace("Asset"), "");
    }

    #[test]
    fn test_fully_qualified_name() {
        assert_eq!(
            fully_qualified_name("org.example", "Asset"),
            "org.example.Asset"
        );
        assert_eq!(fully_qualified_name("", "Asset"), "Asset");
    }

    #[test]
    fn test_parse_namespace() {
        let ns = parse_namespace("org.example@1.0.0").unwrap();
        assert_eq!(ns.name, "org.example");
        assert_eq!(ns.version.as_deref(), Some("1.0.0"));

        let ns = parse_namespace("org.example").unwrap();
        assert_eq!(ns.name, "org.example");
        assert!(ns.version.is_none());

        assert!(parse_namespace("a@b@c").is_err());
    }

    #[test]
    fn test_is_primitive_type() {
        assert!(is_primitive_type("String"));
        assert!(is_primitive_type("Boolean"));
        assert!(is_primitive_type("DateTime"));
        assert!(is_primitive_type("Double"));
        assert!(is_primitive_type("Integer"));
        assert!(is_primitive_type("Long"));
        assert!(!is_primitive_type("Concept"));
    }

    #[test]
    fn test_is_system_property() {
        assert!(is_system_property("$class"));
        assert!(is_system_property("$identifier"));
        assert!(!is_system_property("name"));
    }

    #[test]
    fn test_is_valid_identifier() {
        assert!(is_valid_identifier("foo"));
        assert!(is_valid_identifier("_bar"));
        assert!(is_valid_identifier("$baz"));
        assert!(is_valid_identifier("MyType"));
        assert!(!is_valid_identifier("123abc"));
        assert!(!is_valid_identifier(""));
    }

    #[test]
    fn test_remove_namespace_version() {
        assert_eq!(
            remove_namespace_version("org.example@1.0.0.Asset"),
            "org.example.Asset"
        );
        assert_eq!(remove_namespace_version("String"), "String");
        assert_eq!(
            remove_namespace_version("org.example.Asset"),
            "org.example.Asset"
        );
    }
}
