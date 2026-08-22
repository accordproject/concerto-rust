//! Introspection over a Concerto model: an in-memory representation of its
//! abstract syntax tree that the rest of the runtime can query.
//!
//! A model arrives as a JSON AST whose nodes are described by the generated
//! [`concerto_metamodel`] types, which follow the metamodel's own inheritance
//! hierarchy: declarations such as concepts, assets and participants all derive
//! from a common declaration, and a declaration's fields all derive from a
//! common property. Rust has no subtyping, so rather than a trait hierarchy
//! each family of AST node is reflected as a sum type and selected by matching
//! on the node's `$class`:
//!
//! - [`Declaration`], a top-level declaration (class-like, enum, scalar or map)
//! - [`Property`], a field of a declaration
//! - [`Import`], a reference to types declared in another namespace
//!
//! Deserializing straight into the generated types is lossy: the base
//! `Property` struct, for instance, drops subtype-specific fields such as
//! validators and the referenced type. Each node is therefore re-read from its
//! raw JSON into the enums above, which keep exactly what the runtime needs to
//! inspect a model. A [`ModelFile`] groups the declarations and imports of one
//! namespace; resolving types and inheritance *across* namespaces is the job of
//! the [`ModelManager`](crate::model_manager::ModelManager).

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::error::{ConcertoError, Result};

pub mod declaration;
pub mod import;
pub mod model_file;
pub mod property;

pub use declaration::{ClassDeclaration, ClassKind, Declaration, ScalarDeclaration};
pub use import::Import;
pub use model_file::ModelFile;
pub use property::Property;

/// Returns the `$class` discriminator of an AST node, or `""` if it is absent.
/// The sum types in this module select their variant from this value.
pub(crate) fn declared_class(value: &serde_json::Value) -> &str {
    value.get("$class").and_then(|v| v.as_str()).unwrap_or("")
}

/// Builds a [`ConcertoError::IllegalModel`] for a malformed validator.
fn illegal(message: String) -> ConcertoError {
    ConcertoError::IllegalModel {
        message,
        file_name: None,
        location: None,
    }
}

/// Checks a numeric domain validator. At least one bound must be given, and a
/// lower bound may not exceed the upper one. A domain with only one bound is
/// left open at the other end.
pub(crate) fn check_domain<T: PartialOrd>(
    owner: &str,
    lower: Option<T>,
    upper: Option<T>,
) -> Result<()> {
    match (lower, upper) {
        (None, None) => Err(illegal(format!(
            "Invalid range on {owner}, lower and-or upper bound must be specified"
        ))),
        (Some(lower), Some(upper)) if lower > upper => Err(illegal(format!(
            "Lower bound must be less than or equal to upper bound on {owner}"
        ))),
        _ => Ok(()),
    }
}

/// Checks that a string regex validator compiles.
///
/// Patterns come from the JavaScript runtime, so the engine here is one that
/// takes the same constructs, lookahead and backreferences among them.
pub(crate) fn check_pattern(owner: &str, validator: &mm::StringRegexValidator) -> Result<()> {
    fancy_regex::Regex::new(&validator.pattern)
        .map_err(|error| illegal(format!("Invalid regular expression on {owner}: {error}")))?;
    Ok(())
}

/// Checks a string length validator. At least one bound must be given, neither
/// bound may be negative, and a minimum may not exceed the maximum.
pub(crate) fn check_length(owner: &str, validator: &mm::StringLengthValidator) -> Result<()> {
    let (min, max) = (validator.min_length, validator.max_length);
    if min.is_none() && max.is_none() {
        return Err(illegal(format!(
            "Invalid string length on {owner}, minLength and-or maxLength must be specified"
        )));
    }
    if min.is_some_and(|value| value < 0) || max.is_some_and(|value| value < 0) {
        return Err(illegal(format!(
            "minLength and-or maxLength must be positive integers on {owner}"
        )));
    }
    if let (Some(min), Some(max)) = (min, max)
        && min > max
    {
        return Err(illegal(format!(
            "minLength must be less than or equal to maxLength on {owner}"
        )));
    }
    Ok(())
}
