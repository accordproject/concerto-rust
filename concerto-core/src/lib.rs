//! # concerto-core
//!
//! The heart of the Rust Concerto implementation. This crate holds the
//! in-memory picture of a Concerto schema and the type lookups built on top of
//! it. Data validation will live here too, but that part isn't written yet.
//!
//! Everything sits on top of the generated [`concerto_metamodel`] types. We
//! wrap those in our own enums rather than redefining the schema by hand.

pub mod error;
pub mod model_util;

pub use error::{ConcertoError, Result};
