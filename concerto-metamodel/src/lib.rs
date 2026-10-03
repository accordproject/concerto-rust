//! Rust types for the Concerto metamodel.
//!
//! The types under `src/generated` are generated from the model ASTs in
//! `vendor/` by concerto-codegen's Rust target, at the version recorded in
//! `codegen.version`, through `codegen/generate.js`: a `RustVisitor`
//! subclass that overrides `visitClassDeclaration`, `visitField`,
//! `visitEnumDeclaration` and `toRustType` and post-processes the
//! `FileWriter` output (see there for why). `tests/drift.rs` fails when the
//! generated sources are not what that script produced from the
//! checked-in inputs. Abstract types are enums tagged by
//! `$class`, so deserialising an AST keeps each node's concrete type.

/// A node's `$class`, as a generated struct keeps it: a borrowed static
/// string when it names a type declared in one of the models this crate is
/// generated from (every `$class` of a well-formed AST), so reading one
/// allocates nothing ([`utils::deserialize_class`]); an owned copy for any
/// other string. Build one with `.into()` from a `&'static str` or a
/// `String`, or with [`utils::class_name`].
pub type ClassName = std::borrow::Cow<'static, str>;

/// Types for the `concerto@1.0.0` namespace.
// Generated from the vendored ASTs, which carry no doc comments.
#[allow(unused_imports, missing_docs)]
#[path = "generated/concerto_1_0_0.rs"]
pub mod concerto_1_0_0;

/// Types for the `concerto.decorator@1.0.0` namespace.
// Generated from the vendored ASTs, which carry no doc comments.
#[allow(unused_imports, missing_docs)]
#[path = "generated/concerto_decorator_1_0_0.rs"]
pub mod concerto_decorator_1_0_0;

/// Types for the `concerto.metamodel@1.0.0` namespace.
// Generated from the vendored ASTs, which carry no doc comments.
#[allow(unused_imports, missing_docs)]
#[path = "generated/concerto_metamodel_1_0_0.rs"]
pub mod concerto_metamodel_1_0_0;

/// Types for the `org.accordproject.decoratorcommands@0.4.0` namespace.
// Generated from the vendored ASTs, which carry no doc comments.
#[allow(unused_imports, missing_docs)]
#[path = "generated/org_accordproject_decoratorcommands_0_4_0.rs"]
pub mod org_accordproject_decoratorcommands_0_4_0;

/// concerto-codegen's serde helpers, re-exported by [`utils`]. Generated
/// as is, so the lints its code trips are allowed here.
#[allow(
    missing_docs,
    clippy::needless_borrow,
    clippy::ptr_arg,
    clippy::type_complexity
)]
#[path = "generated/utils.rs"]
mod codegen_utils;

mod name;
pub mod utils;

pub use name::{Name, with_source};
