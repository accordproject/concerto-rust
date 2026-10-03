//! The WASM binding of `concerto-core` for the concerto-core TS views
//! (PORTING.md section 4).
//!
//! It binds:
//! - the **handle API** (P4-01): one exported object per `ModelManager`,
//!   [`ModelManagerHandle`], over the P1-04 arena. Model files, declarations
//!   and properties cross the boundary as their dense `u32` handles
//!   (`ModelFileId`, `DeclId`, `PropId`), plain JS numbers; their state
//!   crosses as one JSON snapshot per element, which a view caches until
//!   `epoch()` moves (spike REPORT §3, "Input to P1-04"; the epoch rule is
//!   on `ModelManagerHandle::epoch`, P5-101 D-7);
//! - the three P0-04b trial units, `ModelUtil`, `NumberValidator` and
//!   `ScalarDeclaration`, whose views still hand their JS objects back (the
//!   model graph they meet is TS until P4-06 … P4-08);
//! - `Decorator` and `Decorated` (P4-05): `Decorator.process` delegates to
//!   the P2-07 port ([`Decorator::from_ast`]) directly, needing no
//!   collaborator call; `Decorator.validate`'s argument and type-reference
//!   checks, and `Decorated.validate`'s duplicate-decorator check, are new
//!   code here rather than a binding of the existing (concrete-`ModelManager`)
//!   `Decorator::validate`, following the TS source directly rather than
//!   the native method's `ModelManager`-specific shortcuts. Since P5-106
//!   (BC-52) `Decorator.validate` reads the model from the arena, by the
//!   handle of the decorator's model file (`decoratorValidate`, "Arena
//!   answers" below). `Decorated.process`'s `DecoratorFactory` selection is
//!   not bound: that stays TS (decorator.rs module doc);
//! - `ModelFile` (P4-08c): `getImports`, `isLocalType`, `filter` and
//!   `validate`, keyed by the same `ModelFileId` handle every other by-file
//!   lookup here already uses. P5-103 removed the bindings concerto-core no
//!   longer calls (`getVersion`, `isSystemModelFile`, the detached
//!   `modelFileFromAst`, and the arena's per-declaration handles).
//!
//! Everything JS-shaped lives here, never in core (PORTING.md 4):
//! - **argument coercion** (3.5): each binding converts its JS arguments the
//!   way the TS member uses them, and says what it does not model;
//! - **JS collaborator calls** (1.4): a binding that is handed JS objects
//!   reads them back through their methods. P5-106 (BC-52) retired the
//!   JS-callback [`ResolutionContext`] (`JsContext`): the members it served
//!   take handles and answer from the arena ("Arena answers", below);
//! - **the error mapping** (2.3): an error leaves as the payload
//!   `{kind, code, params, message, location, errorType, modelFile}`, which
//!   the error factory the shim registers at load turns into the TS exception;
//!   an error about an instance (`serializerFromJsonCompact`, `validateInstance`)
//!   also carries its diagnostics as `details` (P5-89, #1325), and a value
//!   the serializer fast path's wire codec cannot carry carries
//!   `fastPathUnsupported: true` (P5-101, E-11), the TS side's fallback
//!   signal;
//! - **JS object construction**: none is left here that calls back into a
//!   host function; P5-103 removed `semver.parse`'s registration with the
//!   `modelUtilParseNamespace` binding that used it.
//!
//! Views are snapshot-based (spike REPORT §3): a call that builds an object
//! (`ScalarDeclaration.process`, the `NumberValidator` constructor) returns
//! its snapshot as JSON, the view caches it in the object's fields. The trial
//! units' later calls hand the snapshot back; a view over the arena holds a
//! handle instead.
//!
//! Strings cross the boundary as UTF-8, so a lone UTF-16 surrogate becomes
//! U+FFFD. No oracle fixture or unit test passes one.
//!
//! # Export names (P5-104, D-11)
//!
//! The JS names are an interface the TS views call, so they stay as they
//! are; new bindings follow the rule each existing group follows:
//! - a free function binding a TS member is `<tsClass><Method>`
//!   (`modelUtilGetNamespace`, `classDeclarationProcess`);
//! - a handle method binding a `ModelManager` member is the TS method name
//!   (`getNamespaces`, `resolveType`, `isAssignableTo`); one binding a
//!   member of another TS class, by handle, is `<tsClass><Method>`
//!   (`modelFileGetTypeName`, `modelManagerGetModelFileByFileName`);
//! - an engine operation with no TS member is an engine verb (`stage*`,
//!   `commit*`, `dcs*`).
//!
//! The Rust name is the snake case of the JS name (an acronym in lower
//! case, `resource_id_from_uri`), except `DcsManagerHandle::extract_with_action`
//! (`extract`), whose Rust name `extract` is its body's.
//!
//! # Layout
//!
//! One module per section of the binding (P5-104, review M7): `host`
//! (the error mapping), `js_values`, `model_util`,
//! `validator_bindings`, `properties`, `declarations`,
//! `decorators`, `arena`, `handle`, `serializer`,
//! `validate_instance`, `model_file` and `dcs_bindings`, next to the
//! process-global state (`caches`), the staging slot (`staging`), the
//! extract memo (`dcs_memo`) and `validate_resource`.

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use concerto_core::dcs;
use concerto_core::error::{ContractError, ErrorKind};
use concerto_core::instance::dayjs::{Dayjs, UtcOffset};
use concerto_core::instance::from_json::FromJsonOptions;
use concerto_core::instance::resource_id::ResourceId;
use concerto_core::instance::{
    Diagnostic, InstanceEnv, Severity, ValidateOptions, diagnose, diagnose_read,
};
use concerto_core::introspect::FullyQualified;
use concerto_core::introspect::decorator::{
    self, Decorator, DecoratorArgument, DecoratorValidationOptions,
};
use concerto_core::introspect::field;
use concerto_core::introspect::property;
use concerto_core::introspect::scalar::{ScalarDeclaration, ScalarValidator};
use concerto_core::introspect::validators;
use concerto_core::introspect::validators::{
    CollectionSizeValidator, NumberValidator, StringValidator, Validator,
};
use concerto_core::model_manager::{DeclId, ModelFileId, ModelFileSource, Node, PropId};
use concerto_core::model_manager::{ResolutionContext, ValidatedElement};
use concerto_core::model_util as mu;
use concerto_core::{Error as CoreError, ModelFile, ModelManager};
use concerto_core_js::{Instance, InstanceKind, JsValue as CoreValue};
use concerto_core_js::{Serializer, SerializerOptions, populator};
use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use js_sys::{Array, Function, JSON, Object, Reflect};
use serde::Serialize;
use serde_json::{Value, json};
use wasm_bindgen::prelude::*;

// P5-104 (review M7): the bindings, split along lib.rs's section banners.
mod arena;
mod dcs_bindings;
mod declarations;
mod decorators;
mod handle;
mod host;
mod js_values;
mod model_file;
mod model_util;
mod properties;
mod serializer;
mod validate_instance;
mod validator_bindings;

use dcs_bindings::*;

// The two exported handle types stay at the crate root, where the docs
// link them.
pub use dcs_bindings::DcsManagerHandle;
use declarations::*;
use decorators::*;
pub use handle::ModelManagerHandle;
use handle::*;
use host::*;
use js_values::*;
use model_file::*;
use model_util::*;
use properties::*;
use serializer::*;
use validate_instance::*;

// P5-12c (accordproject/concerto-rust#293): `ValidatedResource.validate()`,
// `setPropertyValue` and `addArrayValue` in one engine call each.
mod validate_resource;

// P5-101 (D-7): the process-global state, in one module.
mod caches;

// P5-110: seeds the hasher of the untrusted-keyed maps (`JsObject`,
// concerto-core's `SeededState` tables) from the host's entropy at
// instantiation.
mod hash_seed;

// P5-101 (D-7, D-13): a handle's staging slot.
mod staging;

// P5-101 (D-7): a handle's per-epoch DCS extract memo (P5-56).
mod dcs_memo;
#[cfg(test)]
use dcs_memo::DcsExtractKept;
use dcs_memo::{DcsExtractMemo, compacted_extract_js};
use staging::{STAGE_CHECKED, STAGE_COMPACT, StagedModelFiles};

#[cfg(test)]
mod tests;
