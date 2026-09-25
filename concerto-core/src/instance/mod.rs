//! Instance-level Concerto: the pieces that operate on data conforming to a
//! model, as opposed to the model itself.
//!
//! Per PORTING.md's target layout this module eventually also holds the
//! serializer, `JSONPopulator`, `JSONGenerator`,
//! `InstanceGenerator.findConcreteSubclass` and the `Factory` model checks.
//! [`resource_id`]'s four members (`parseUri`, the `ResourceId` constructor,
//! `fromURI`, `toURI`) are ledger-scoped to P2-01 (`SEAM_LEDGER.tsv`
//! `planned_task` `P2-01+P4-03`) and do not depend on the rest of the
//! instance layer. [`validate`] is task P3-01
//! (`accordproject/concerto-rust#56`): the instance validator, a port of
//! `ResourceValidator` that also folds in `concerto-validate-rs` (plan
//! decision D3).

pub mod resource_id;
pub mod validate;

pub use validate::{ValidateOptions, validate_instance};
