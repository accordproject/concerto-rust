//! Poking at the structure of a Concerto model.
//!
//! This is our take on Concerto's `introspect` API. The generated
//! [`concerto_metamodel`] structs are awkward to match on directly, so we read
//! a model's JSON AST into a handful of enums instead, [`Declaration`] and
//! [`Property`], each with the accessors we actually need.

pub mod declaration;
pub mod property;

pub use declaration::{ClassDeclaration, ClassKind, Declaration, ScalarDeclaration};
pub use property::Property;

/// Pulls the `$class` string off an AST node, or `""` if it's missing.
/// Almost everything in here switches on this value to pick a variant.
pub(crate) fn declared_class(value: &serde_json::Value) -> &str {
    value.get("$class").and_then(|v| v.as_str()).unwrap_or("")
}
