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
//! Each variant of these enums is a newtype over the generated `mm::*` struct
//! for its `$class`. The variant is picked from the node's `$class` while the
//! model is loaded, which is the one place the raw JSON is read: a `$class`
//! may be given fully qualified or as its bare short name, and a few shapes
//! the generated unions cannot hold are still accepted (see [`Property`],
//! [`Import`] and [`declaration::MapDeclaration`]). A [`ModelFile`] groups the
//! declarations and imports of one namespace and keeps the JSON AST it was
//! given, unchanged, as [`ModelFile::ast`]; resolving types and inheritance
//! *across* namespaces is the job of the
//! [`ModelManager`](crate::model_manager::ModelManager).
//!
//! What the families share, such as a name ([`Named`]), a declaration kind
//! ([`DeclarationKind`]) or decorators ([`Decorated`]), is a trait, derived
//! where it is the same over every variant (see [`crate::derive`]).

pub mod declaration;
pub mod decorator;
pub mod field;
pub mod import;
pub mod model_file;
pub mod property;
pub mod scalar;
mod traits;
pub mod validators;

pub use declaration::{ClassDeclaration, ClassKind, Declaration, MapDeclaration};
pub use decorator::{
    Decorated, Decorator, DecoratorArgument, DecoratorValidationOptions, TypeReferenceArgument,
};
pub use import::Import;
pub use model_file::ModelFile;
pub use property::Property;
pub use scalar::ScalarDeclaration;
pub use traits::{DeclarationKind, FullyQualified, HasValidators, Named, Typed, Validate};

/// Returns the `$class` discriminator of an AST node, or `""` if it is absent.
/// The sum types in this module select their variant from this value.
pub(crate) fn declared_class(value: &serde_json::Value) -> &str {
    value.get("$class").and_then(|v| v.as_str()).unwrap_or("")
}

/// The namespace every metamodel `$class` belongs to.
const METAMODEL_NAMESPACE: &str = "concerto.metamodel@1.0.0";

/// The fully-qualified metamodel `$class` for a short name, such as
/// `concerto.metamodel@1.0.0.StringMapKeyType` for `StringMapKeyType`.
pub(crate) fn qualified_class(short: &str) -> String {
    format!("{METAMODEL_NAMESPACE}.{short}")
}
