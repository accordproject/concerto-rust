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
//! - `js-compat` (off by default): the seam the JS-compatibility layer is
//!   built on. The JS object model itself (the TS `Resource` objects and the
//!   JS values they hold, the `Serializer`, `Factory`, `JSONPopulator` and
//!   `JSONGenerator`) is in the `concerto-core-js` crate, which enables this
//!   feature, as the WASM binding (`concerto-wasm`) does. The feature makes
//!   public what that layer needs from core: `Serializer.fromJSON` over plain
//!   JSON and the `Factory` checks (`instance::from_json`), the
//!   `ResourceValidator` port with the `$$` tag encoding of JS values
//!   (`instance::validate`), the `DateTime` and relationship URI semantics
//!   (`instance::{dayjs, resource_id}`), the TS exception-class mapping
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

/// Checking a model's JSON AST against the Concerto metamodel
/// (`concerto.metamodel@1.0.0`), before it is loaded.
pub mod metamodel {
    /// The metamodel's namespace, `concerto.metamodel@1.0.0`.
    pub use crate::instance::metamodel::METAMODEL_NAMESPACE as NAMESPACE;
    /// The version check and the structural check, as TS
    /// `BaseModelManager.validateAst` runs them.
    pub use crate::instance::metamodel::validate_ast;
    /// The structural check alone (TS `MetaModelUtil.validateMetaModel`).
    pub use crate::instance::metamodel::validate_metamodel as validate_structure;
}

/// The introspection traits, for code that is generic over the element
/// types: `use concerto_core::prelude::*;`. The types also have the same
/// methods as inherent methods (`name`, `type_name`, `decorators`,
/// `declaration_kind`), so a caller that is not generic needs no import.
pub mod prelude {
    pub use crate::introspect::{DeclarationKind, Decorated, Named, Typed};
}

#[allow(deprecated)]
pub use error::ConcertoError;
pub use error::{Error, ErrorKind, Result};
pub use introspect::{
    ClassDeclaration, ClassKind, Declaration, DeclarationKind, Decorated, Decorator,
    DecoratorArgument, DecoratorValidationOptions, Import, ModelFile, Named, Property,
    ScalarDeclaration, TypeReferenceArgument, Typed,
};
#[cfg(feature = "js-compat")]
pub use introspect::{FullyQualified, HasValidators, Validate};
pub use model_manager::ModelManager;

/// Guarantee 6 of docs/public-api.md section 2: the manager, a model file
/// and the error type can cross threads, and the error type is a standard
/// `'static` error.
const _: () = {
    const fn send_sync<T: Send + Sync>() {}
    const fn std_error<T: std::error::Error + Send + Sync + 'static>() {}
    send_sync::<ModelManager>();
    send_sync::<ModelFile>();
    std_error::<Error>();
};
