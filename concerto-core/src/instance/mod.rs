//! Instance-level Concerto: the pieces that operate on data conforming to a
//! model, as opposed to the model itself.
//!
//! Per PORTING.md's target layout this module holds the serializer,
//! `JSONPopulator`, `JSONGenerator`, the `Factory` model checks and,
//! eventually, `InstanceGenerator.findConcreteSubclass`.
//! [`resource_id`]'s four members (`parseUri`, the `ResourceId` constructor,
//! `fromURI`, `toURI`) are ledger-scoped to P2-01 (`SEAM_LEDGER.tsv`
//! `planned_task` `P2-01+P4-03`) and do not depend on the rest of the
//! instance layer. [`validate`] is task P3-01
//! (`accordproject/concerto-rust#56`): the instance validator, a port of
//! `ResourceValidator` that also folds in `concerto-validate-rs` (plan
//! decision D3). The serializer is task P3-01b
//! (`accordproject/concerto-rust#124`): [`value`] (the instances and the JS
//! values they hold), [`dayjs`], [`factory`] (`Factory`, whose model checks
//! #32 point 4 moves to Rust), [`populator`] (`JSONPopulator`),
//! [`generator`] (`JSONGenerator`), [`resource`] (the members of the
//! `Resource` family that change or check an instance) and [`serializer`]
//! (`Serializer`, option B's whole-document calls). [`deserialize`] is task
//! P3-02 (`accordproject/concerto-rust#57`): accordproject/concerto#1273's
//! `DeserializeOptions` and `STRICT_VALIDATE_OPTIONS`.

pub mod dayjs;
pub mod deserialize;
pub mod factory;
pub mod generator;
pub(crate) mod model;
pub mod populator;
pub mod resource;
pub mod resource_id;
pub mod serializer;
pub mod validate;
pub mod value;

pub use deserialize::{DeserializeOptions, STRICT_VALIDATE_OPTIONS};
pub use factory::InstanceEnv;
pub use serializer::{Serializer, SerializerOptions};
pub use validate::{ValidateOptions, validate_instance};
pub use value::{Instance, InstanceKind, JsValue};
