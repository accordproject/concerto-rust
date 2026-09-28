//! Instance-level Concerto: the pieces that operate on data conforming to a
//! model, as opposed to the model itself.
//!
//! Per PORTING.md's target layout this module holds the serializer,
//! `JSONPopulator`, `JSONGenerator`, the `Factory` model checks and,
//! eventually, `InstanceGenerator.findConcreteSubclass`.
//! `resource_id`'s four members (`parseUri`, the `ResourceId` constructor,
//! `fromURI`, `toURI`) are ledger-scoped to P2-01 (`SEAM_LEDGER.tsv`
//! `planned_task` `P2-01+P4-03`) and do not depend on the rest of the
//! instance layer. [`validate`] is task P3-01
//! (`accordproject/concerto-rust#56`): the instance validator, a port of
//! `ResourceValidator` that also folds in `concerto-validate-rs` (plan
//! decision D3). The serializer is task P3-01b
//! (`accordproject/concerto-rust#124`): `value` (the instances and the JS
//! values they hold), `dayjs`, `factory` (`Factory`, whose model checks
//! #32 point 4 moves to Rust), `populator` (`JSONPopulator`),
//! `generator` (`JSONGenerator`), `resource` (the members of the
//! `Resource` family that change or check an instance) and `serializer`
//! (`Serializer`, option B's whole-document calls). [`deserialize`] is task
//! P3-02 (`accordproject/concerto-rust#57`): accordproject/concerto#1273's
//! `DeserializeOptions` and `STRICT_VALIDATE_OPTIONS`. [`diagnostic`] is task
//! P3-03 (`accordproject/concerto-rust#58`): accordproject/concerto#1239's
//! `Diagnostic`/`ValidationResult` foundation — a Rust-only collect-all
//! validation mode alongside [`validate`]'s first-error walk. [`metamodel`] is
//! task P3-04 (`accordproject/concerto-rust#59`): `BaseModelManager.validateAst`,
//! rebuilt on [`validate`] with [`deserialize::STRICT_VALIDATE_OPTIONS`] as
//! its default strictness.

//!
//! # The JS object model
//!
//! `dayjs`, `factory`, `generator`, `populator`, `resource`,
//! `resource_id`, `serializer` and `value` model the TS `Resource`
//! objects and the JS values they hold (`undefined`, a JS `Map`, a dayjs
//! object). They exist for the WASM binding, so they are public only with
//! the `js-compat` feature, and carry no stability promise
//! (docs/public-api.md section 4). Without the feature they are compiled as
//! crate-private modules, because [`metamodel::validate_ast`] still reads
//! its input through the `Serializer` (docs/public-api.md F8). Crate-private,
//! they lose clippy's exemption for exported names, which the JS-facing
//! names (`Serializer::from_json`) keep through the `allow`s below.

#[cfg(feature = "js-compat")]
pub mod dayjs;
#[cfg(not(feature = "js-compat"))]
#[allow(dead_code, unused_imports)]
#[allow(clippy::enum_variant_names, clippy::wrong_self_convention)]
pub(crate) mod dayjs;
pub mod deserialize;
pub mod diagnostic;
#[cfg(feature = "js-compat")]
pub mod factory;
#[cfg(not(feature = "js-compat"))]
#[allow(dead_code, unused_imports)]
#[allow(clippy::enum_variant_names, clippy::wrong_self_convention)]
pub(crate) mod factory;
#[cfg(feature = "js-compat")]
pub mod generator;
#[cfg(not(feature = "js-compat"))]
#[allow(dead_code, unused_imports)]
#[allow(clippy::enum_variant_names, clippy::wrong_self_convention)]
pub(crate) mod generator;
pub mod metamodel;
pub(crate) mod model;
#[cfg(feature = "js-compat")]
pub mod populator;
#[cfg(not(feature = "js-compat"))]
#[allow(dead_code, unused_imports)]
#[allow(clippy::enum_variant_names, clippy::wrong_self_convention)]
pub(crate) mod populator;
#[cfg(feature = "js-compat")]
pub mod resource;
#[cfg(not(feature = "js-compat"))]
#[allow(dead_code, unused_imports)]
#[allow(clippy::enum_variant_names, clippy::wrong_self_convention)]
pub(crate) mod resource;
#[cfg(feature = "js-compat")]
pub mod resource_id;
#[cfg(not(feature = "js-compat"))]
#[allow(dead_code, unused_imports)]
#[allow(clippy::enum_variant_names, clippy::wrong_self_convention)]
pub(crate) mod resource_id;
#[cfg(feature = "js-compat")]
pub mod serializer;
#[cfg(not(feature = "js-compat"))]
#[allow(dead_code, unused_imports)]
#[allow(clippy::enum_variant_names, clippy::wrong_self_convention)]
pub(crate) mod serializer;
pub mod validate;
#[cfg(feature = "js-compat")]
pub mod value;
#[cfg(not(feature = "js-compat"))]
#[allow(dead_code, unused_imports)]
#[allow(clippy::enum_variant_names, clippy::wrong_self_convention)]
pub(crate) mod value;

pub use deserialize::{DeserializeOptions, STRICT_VALIDATE_OPTIONS};
pub use diagnostic::{Diagnostic, DiagnosticCode, Severity, ValidationResult};
#[cfg(feature = "js-compat")]
pub use factory::InstanceEnv;
pub use metamodel::{
    METAMODEL_NAMESPACE, model_manager_from_meta_model, validate_ast, validate_meta_model_instance,
    validate_metamodel,
};
#[cfg(feature = "js-compat")]
pub use serializer::{Serializer, SerializerOptions};
pub use validate::{ValidateOptions, validate_instance};
#[cfg(feature = "js-compat")]
pub use value::{Instance, InstanceKind, JsValue};
