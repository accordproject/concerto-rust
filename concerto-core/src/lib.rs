//! # concerto-core
//!
//! The heart of the Rust Concerto implementation. This crate holds the
//! in-memory picture of a Concerto schema, the type lookups built on top of
//! it, and the semantic validation that checks a loaded model is consistent.
//!
//! Everything sits on top of the generated [`concerto_metamodel`] types. We
//! wrap those in our own enums rather than redefining the schema by hand.
//!
//! ## Features
//!
//! - `js-compat` (off by default): the JS-compatibility layer the WASM
//!   binding (`concerto-wasm`) is built on. It makes public the JS object
//!   model (`instance::{value, dayjs, serializer, factory, populator,
//!   generator, resource, resource_id}`), the `$$` tag encoding of JS values
//!   in `instance::validate`, the TS exception-class mapping
//!   (`ErrorKind::ts_class`), the collaborator fallback
//!   (`model_manager::{ResolutionContext, ValidatedElement, Node}`), the
//!   `process` family the TS views are built with, and the decorator command
//!   sets (`dcs`). None of it carries a stability promise, and none of it is
//!   in the default public API (docs/public-api.md section 4).

// P6-01 (accordproject/concerto-rust#83): every public item is documented.
#![warn(missing_docs)]

// The derives name the traits by their `::concerto_core` paths, which also
// have to resolve inside this crate.
extern crate self as concerto_core;

/// Declares one item that is `pub` with the `js-compat` feature and
/// `pub(crate)` without it: part of the JS-compatibility layer that the WASM
/// binding needs and the native API does not show (docs/public-api.md
/// section 4.6). The item is written once, with `pub`.
macro_rules! js_compat_pub {
    ($(#[$attr:meta])* pub $($item:tt)*) => {
        #[cfg(feature = "js-compat")]
        $(#[$attr])*
        pub $($item)*

        #[cfg(not(feature = "js-compat"))]
        #[allow(dead_code)]
        $(#[$attr])*
        pub(crate) $($item)*
    };
}

/// The derive macros for this crate's traits (plan decision D5), from the
/// `concerto-macros` crate.
pub use concerto_macros as derive;

// The decorator command sets are outside the D11 surface for now
// (docs/public-api.md Q3): public only for the WASM binding.
#[cfg(feature = "js-compat")]
pub mod dcs;
#[cfg(not(feature = "js-compat"))]
#[allow(dead_code, unused_imports)]
#[allow(clippy::enum_variant_names, clippy::wrong_self_convention)]
pub(crate) mod dcs;
mod ecma;
pub mod error;
pub mod instance;
pub mod introspect;
pub mod model_manager;
pub mod model_util;
pub mod rootmodel;
mod semver_range;
pub mod validation;

/// The introspection traits, for code that is generic over the element
/// types: `use concerto_core::prelude::*;`. The types also have the same
/// methods as inherent methods (`name`, `type_name`, `decorators`,
/// `declaration_kind`), so a caller that is not generic needs no import.
pub mod prelude {
    pub use crate::introspect::{DeclarationKind, Decorated, Named, Typed};
}

pub use error::{Error, Result};
pub use introspect::{
    ClassDeclaration, ClassKind, Declaration, DeclarationKind, Decorated, Decorator,
    DecoratorArgument, DecoratorValidationOptions, Import, ModelFile, Named, Property,
    ScalarDeclaration, TypeReferenceArgument, Typed,
};
#[cfg(feature = "js-compat")]
pub use introspect::{FullyQualified, HasValidators, Validate};
pub use model_manager::ModelManager;
