//! # concerto-core
//!
//! The heart of the Rust Concerto implementation. This crate holds the
//! in-memory picture of a Concerto schema, the type lookups built on top of
//! it, and the semantic validation that checks a loaded model is consistent.
//!
//! Everything sits on top of the generated [`concerto_metamodel`] types. We
//! wrap those in our own enums rather than redefining the schema by hand.

// The derives name the traits by their `::concerto_core` paths, which also
// have to resolve inside this crate.
extern crate self as concerto_core;

/// The derive macros for this crate's traits (plan decision D5), from the
/// `concerto-macros` crate.
pub use concerto_macros as derive;

mod ecma;
pub mod error;
pub mod instance;
pub mod introspect;
pub mod model_manager;
pub mod model_util;
pub mod rootmodel;
mod validation;

pub use error::{ConcertoError, Result};
pub use introspect::{
    ClassDeclaration, ClassKind, Declaration, DeclarationKind, Decorated, FullyQualified,
    HasValidators, Import, ModelFile, Named, Property, ScalarDeclaration, Typed, Validate,
};
pub use model_manager::ModelManager;
