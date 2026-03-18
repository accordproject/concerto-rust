use regex::Regex;
use std::sync::LazyLock;

use crate::{types::ModelAst, ConcertoError};

/// Parses a potentially versioned namespace into
/// its name and version parts. The version of the namespace
/// (if present) is parsed using semver.parse.
pub fn parse_namespace_and_version(ast: &ModelAst) -> Result<(String, String), ConcertoError> {
    let x = ast.0.namespace.as_str();
    let parts: Vec<&str> = x.split('@').collect();
    // For now, assume all NS will have to have namespace@version
    if parts.len() != 2 {
        Err(ConcertoError::InvalidNS(x.to_string()))
    } else {
        Ok((parts[0].to_string(), parts[1].to_string()))
    }
}

const ID_REGEX: &str = r"^(?:\p{Lu}|\p{Ll}|\p{Lt}|\p{Lm}|\p{Lo}|\p{Nl}|\$|_|\u{005C}u[0-9A-Fa-f]{4})(?:\p{Lu}|\p{Ll}|\p{Lt}|\p{Lm}|\p{Lo}|\p{Nl}|\$|_|\u{005C}u[0-9A-Fa-f]{4}|\p{Mn}|\p{Mc}|\p{Nd}|\p{Pc}|\u{200C}|\u{200D})*$";

static IDENTIFIER_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(ID_REGEX).expect("Invalid ID regex"));

/// Checks if a string is a valid identifier name in Concerto
/// Identifiers must start with a letter and contain only letters, numbers, or underscores
pub fn is_valid_identifier(name: &str) -> bool {
    IDENTIFIER_REGEX.is_match(name)
}

#[cfg(test)]
mod test {
    use crate::types::ModelAst;
    use concerto_metamodel::concerto_metamodel_1_0_0::Model;

    #[allow(unused)]
    fn make_ast() -> ModelAst {
        let basic_ast = include_str!("../examples/assets/concerto_models/basic.json");
        let parsed: Model = serde_json::from_str(basic_ast).expect("Cannot parse basic.json");
        ModelAst(parsed)
    }

    #[test]
    fn test_parse_ns() {
        let ast = make_ast();
        if let Ok((ns, version)) = super::parse_namespace_and_version(&ast) {
            assert_eq!(ns, "com.example.foo".to_string(), "Can parse namespace");
            assert_eq!(version, "1.0.0".to_string(), "Can parse version");
        } else {
            assert!(false);
        }
    }

    #[test]
    fn test_valid_identifiers() {
        assert!(super::is_valid_identifier("simple_var"));
        assert!(super::is_valid_identifier("$dollar"));
        assert!(super::is_valid_identifier("π_ratio"));
        assert!(super::is_valid_identifier("变量"));
        assert!(super::is_valid_identifier(r"a\u0061")); // Literal backslash-u-hex
    }

    #[test]
    fn test_invalid_identifiers() {
        assert!(!super::is_valid_identifier("1stVariable")); // Starts with digit
        assert!(!super::is_valid_identifier("var-name")); // Contains hyphen
        assert!(!super::is_valid_identifier("no space")); // Contains space
        assert!(!super::is_valid_identifier("heavy+metal")); // Contains plus sign
        assert!(!super::is_valid_identifier("🚀_star")); // Contains emoji
    }
}
