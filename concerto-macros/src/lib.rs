//! # concerto-macros
//!
//! Derive macros for the traits of `concerto-core`. Use them through their
//! re-export, `concerto_core::derive`; the code they generate names the traits
//! by their `::concerto_core` paths.
//!
//! Each derive writes the `match` that the trait's method would otherwise
//! repeat by hand over every variant of a sum type. A variant is either a
//! newtype over a node that carries the value itself, or a wrapper over
//! another type that already implements the trait:
//!
//! - [`Named`](macro@Named) implements `concerto_core::introspect::Named`.
//!   By default it reads the `name` field of the wrapped node: `&node.name`
//!   for a tuple variant or tuple struct, the `name` field of a struct variant
//!   or struct.
//! - [`DeclarationKind`](macro@DeclarationKind) implements
//!   `concerto_core::introspect::DeclarationKind`. Each variant (or the whole
//!   type) names its metamodel `$class` short name with
//!   `#[concerto(kind = "…")]`.
//!
//! Both read the same helper attribute:
//!
//! - `#[concerto(delegate)]`, on the enum or on one variant: the variant wraps
//!   a type that implements the trait, and the method is forwarded to it.
//! - `#[concerto(kind = "EnumDeclaration")]`, on the type or on one variant:
//!   the declaration kind of the type, or of that variant.
//!
//! ```ignore
//! use concerto_core::derive::{DeclarationKind, Named};
//!
//! #[derive(Named, DeclarationKind)]
//! #[concerto(delegate)]
//! pub enum Declaration {
//!     Class(ClassDeclaration),
//!     Enum(EnumDeclaration),
//!     Scalar(ScalarDeclaration),
//!     Map(MapDeclaration),
//! }
//! ```

use proc_macro::TokenStream;
use syn::{DeriveInput, parse_macro_input};

mod expand;

/// Derives `concerto_core::introspect::Named`. See the [crate docs](crate).
#[proc_macro_derive(Named, attributes(concerto))]
pub fn derive_named(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand::named(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Derives `concerto_core::introspect::DeclarationKind`. See the
/// [crate docs](crate).
#[proc_macro_derive(DeclarationKind, attributes(concerto))]
pub fn derive_declaration_kind(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    expand::declaration_kind(&input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
