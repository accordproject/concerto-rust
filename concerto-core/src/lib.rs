//! # Concerto
//!
//! Concerto is a lightweight data modeling (schema) language and runtime for business concepts.
//!
//! Refer to the [language documentation](https://concerto.accordproject.org/) for more information.
//!
//! This crate is the "core" implementation of Concerto in Rust language.
//!
pub mod error;
pub mod introspect;
pub mod model_manager;
pub mod model_util;
pub mod rootmodel;
pub mod validation;
