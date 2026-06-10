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
//!
//! Deserializing straight into the generated types is lossy: the base
//! `Property` struct, for instance, drops subtype-specific fields such as
//! validators and the referenced type. Each node is therefore re-read from its
//! raw JSON into the enums above, which keep exactly what the runtime needs to
//! inspect a model.

pub mod declaration;
pub mod property;

pub use declaration::{ClassDeclaration, ClassKind, Declaration, ScalarDeclaration};
pub use property::Property;

/// Returns the `$class` discriminator of an AST node, or `""` if it is absent.
/// The sum types in this module select their variant from this value.
pub(crate) fn declared_class(value: &serde_json::Value) -> &str {
    value.get("$class").and_then(|v| v.as_str()).unwrap_or("")
}
