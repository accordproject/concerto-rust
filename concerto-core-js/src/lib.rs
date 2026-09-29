//! # concerto-core-js
//!
//! The JS object model of `concerto-core` (task P6-01,
//! accordproject/concerto-rust#83, docs/public-api.md sections 4.4 and 4.6,
//! step 5): the state of the TS `Resource` objects and of the JS values they
//! hold, and the TS classes that build and read them (`Serializer`,
//! `Factory`, `JSONPopulator`, `JSONGenerator`), for the WASM binding
//! (`concerto-wasm`).
//!
//! A native caller does not need any of it: `concerto-core` validates plain
//! JSON instances itself (`ModelManager::validate_instance`). This crate is
//! built on core's `js-compat` seam, carries no stability promise, and is
//! not published.

#![warn(missing_docs)]

pub use deserialize::{DeserializeOptions, STRICT_VALIDATE_OPTIONS};
pub use serializer::{FromJsonOptions, Serializer, SerializerOptions};
pub use value::{Instance, InstanceKind, JsObject, JsValue};

pub mod deserialize;
pub mod factory;
pub mod generator;
pub mod populator;
pub mod resource;
pub mod serializer;
pub mod value;
