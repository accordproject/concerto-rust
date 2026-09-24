//! Instance-level Concerto: the pieces that operate on data conforming to a
//! model, as opposed to the model itself.
//!
//! Per PORTING.md's target layout this module eventually also holds the
//! serializer, `JSONPopulator`, `JSONGenerator`, `ResourceValidator`,
//! `InstanceGenerator.findConcreteSubclass` and the `Factory` model checks
//! (P3-01). Only [`resource_id`] exists so far: its four members
//! (`parseUri`, the `ResourceId` constructor, `fromURI`, `toURI`) are
//! ledger-scoped to P2-01 (`SEAM_LEDGER.tsv` `planned_task`
//! `P2-01+P4-03`) and do not depend on the rest of the instance layer.

pub mod resource_id;
