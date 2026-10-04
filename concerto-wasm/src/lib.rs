//! The WASM binding of `concerto-core` for concerto-core's TS engine
//! (PORTING.md section 4). Everything JS-shaped lives here, never in core.
//!
//! **Transports.** Each `ModelManager` is a [`ModelManagerHandle`] over its
//! arena; model files, declarations and properties cross as their dense `u32`
//! handles, and their state as one JSON snapshot per element or file, which
//! a TS view caches until the engine's own `EngineState.version` moves (the
//! handle's epoch stamps only the handle's own caches). A model file is staged
//! once at construction (`staging`), from JSON text as UTF-8 or the compact
//! binary layout, and later calls refer to the stage or file by id.
//! Instances cross as the serializer's wire encoding, as JSON text or the
//! compact layout (`serializer`, `validate_resource`, `validate_instance`).
//! Arguments are coerced as the TS member uses them (3.5). An error leaves
//! as the payload `{kind, code, params, message, location, errorType,
//! modelFile}` the host's error factory turns into the TS exception
//! (`host`), with an instance error's `details` and the fast path's
//! `fastPathUnsupported` flag. Strings cross as UTF-8, so a lone surrogate
//! becomes U+FFFD.
//!
//! **Modules.** `host` (the error mapping), `js_values`, `model_util`,
//! `validator_bindings`, `properties`, `declarations`, `decorators`, `arena`
//! (the BC-52 arena answers), `handle`, `serializer`, `validate_instance`,
//! `validate_resource`, `model_file`, `dcs_bindings` and `dcs_memo` (the
//! DecoratorManager and its extract memo), `staging`, `caches` (the
//! process-global state) and `hash_seed`.
//!
//! **Export names.** The JS names are the interface the TS views call: a
//! free function binding a TS member is `<tsClass><Method>`
//! (`modelUtilGetNamespace`); a handle method binding a `ModelManager`
//! member is the TS method name (`resolveType`), and one binding another
//! class's member by handle is `<tsClass><Method>` (`modelFileGetTypeName`);
//! an engine operation with no TS member is an engine verb (`stage*`,
//! `commit*`, `dcs*`). The Rust name is the JS name in snake case, except
//! `DcsManagerHandle::extract_with_action` (`extract`).

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
use concerto_core::json;
use concerto_core::json::Value;
use concerto_core::model_manager::{DeclId, ModelFileId, ModelFileSource, Node, PropId};
use concerto_core::model_manager::{ResolutionContext, ValidatedElement};
use concerto_core::model_util as mu;
use concerto_core::{Error as CoreError, ModelFile, ModelManager};
use concerto_core_js::{Instance, InstanceKind, JsValue as CoreValue};
use concerto_core_js::{Serializer, SerializerOptions, populator};
use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use js_sys::{Array, Function, JSON, Object, Reflect};
use serde::Serialize;
use wasm_bindgen::prelude::*;

// The bindings, split along lib.rs's section banners.
mod arena;
mod dcs_bindings;
mod declarations;
mod decorators;
mod handle;
#[cfg(feature = "hashdos-probe")]
mod hashdos_probe;
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

// `ValidatedResource.validate()`, `setPropertyValue` and `addArrayValue` in
// one engine call each.
mod validate_resource;

// The process-global state, in one module.
mod caches;

// Seeds the hasher of the untrusted-keyed maps (`JsObject`,
// concerto-core's `SeededState` tables) from the host's entropy at
// instantiation.
mod hash_seed;

// A handle's staging slot.
mod staging;

// A handle's per-epoch DCS extract memo.
mod dcs_memo;
#[cfg(test)]
use dcs_memo::DcsExtractKept;
use dcs_memo::{DcsExtractMemo, compacted_extract_js};
use staging::{STAGE_CHECKED, STAGE_COMPACT, StagedModelFiles};

#[cfg(test)]
mod tests;
