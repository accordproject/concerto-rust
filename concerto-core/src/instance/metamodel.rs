//! `BaseModelManager.validateAst` (`src/basemodelmanager.ts`; task P3-04,
//! `accordproject/concerto-rust#59`; `SEAM_LEDGER.tsv` row
//! `src/basemodelmanager.ts BaseModelManager validateAst`, planned task
//! `P3-04+P4-08`): checking a Concerto AST document against the metamodel
//! itself, rebuilt on [`super::validate`] (P3-01, the instance validator
//! that folded in `concerto-validate-rs`'s structural check, plan decision
//! D3) and accordproject/concerto#1273's `STRICT_VALIDATE_OPTIONS` (P3-02,
//! the strictness preset). P6-01 (step 5) runs it on [`super::from_json`],
//! `Serializer.fromJSON` over plain JSON, so that it does not depend on the
//! JS object model (docs/public-api.md F8).
//!
//! **Scope (this task only).** The issue's plan gave `concerto-validate-rs`
//! (D3) as the exit condition — "validate-rs tests pass on the new core" —
//! but the maintainer's later comment on the issue supersedes that: leave
//! `concerto-validate-rs` untouched (it is reference-only, to be archived),
//! and instead port its test cases as tests of the function this module
//! adds (below). [`validate_metamodel`] is that function: the standalone
//! structural check `concerto-validate-rs::validate_metamodel` provided,
//! rebuilt on the instance validator with the strict preset instead of
//! `concerto-validate-rs`'s own bug-ridden hand-rolled one (plan §1.3).
//! [`validate_ast`] adds `validateAst`'s version check in front of it.
//! Wiring either into a caller's own [`ModelManager`] — TS's
//! `options.metamodelValidation`, and the temporary add/remove of
//! `this.metamodelModelFile` so `getType` resolves it — is `SEAM_LEDGER.tsv`'s
//! other half of this row. Task P4-08b (accordproject/concerto-rust#174)
//! added it as [`ModelManager::validate_ast`] (with
//! `ModelManager::set_metamodel_validation`), built on this module's
//! `check_version`, `metamodel_model_file` and `deserialize_ast`.
//!
//! accordproject/concerto-rust#265 adds the two `src/introspect/metamodel.ts`
//! functions the ledger also places here, [`validate_meta_model_instance`]
//! (`validateMetaModel`) and [`model_manager_from_meta_model`]
//! (`modelManagerFromMetaModel`), and [`ModelManager::add_metamodel`] for the
//! constructor's `addMetamodel` option, so the native oracle harness can
//! replay their fixtures.

use serde_json::Value;

use super::from_json::{FixedEnv, FromJsonOptions, from_json};
use super::model::not_a_function;
use crate::ecma;
use crate::error::{ContractError, Error, ErrorKind, Result};
use crate::introspect::model_file::ModelFile;
use crate::model_manager::ModelManager;
use crate::model_util;

/// `MetaModelNamespace` (`@accordproject/concerto-metamodel`), as
/// `basemodelmanager.ts` imports it.
pub const METAMODEL_NAMESPACE: &str = "concerto.metamodel@1.0.0";

/// The metamodel's own AST: the same vendored copy
/// `crate::dcs` includes (`concerto-core/src/dcs/metamodel.json`, identical
/// byte for byte to `concerto-metamodel/vendor/concerto.metamodel@1.0.0.json`
/// — `MetaModelUtil.metaModelAst`, the document `new ModelManager({
/// addMetamodel: true })` adds), so this module vendors no copy of its own.
const METAMODEL_AST_JSON: &str = include_str!("../dcs/metamodel.json");

/// A fresh [`ModelManager`] with the metamodel model itself loaded. TS's
/// `validateAst` adds `this.metamodelModelFile` only for the duration of
/// the check (and only when a metamodel is not already present) rather
/// than caching it; [`validate_metamodel`] runs on
/// [`with_resident_metamodel_manager`]'s per-thread copy of this manager
/// instead (P5-21), which holds the same models and gives the same answer.
fn metamodel_model_manager() -> Result<ModelManager> {
    let mut mm = ModelManager::new()?;
    let metamodel: Value =
        serde_json::from_str(METAMODEL_AST_JSON).expect("the vendored metamodel AST is JSON");
    mm.load_models([(&metamodel, Some(format!("{METAMODEL_NAMESPACE}.cto")))])?;
    Ok(mm)
}

/// Runs `f` on a resident, per-thread [`metamodel_model_manager`] (task
/// P5-21, accordproject/concerto-rust#319), as P5-13's
/// `ModelManager::validate_ast_value` does for its own resident manager:
/// built on the first call on each thread, then kept with its caches warm,
/// so a later call pays for no system-model or metamodel load.
///
/// The manager is only ever read ([`from_json`] takes it by shared
/// reference, and nothing else can reach it), so every call sees the same
/// models a fresh manager would hold, and gets the same result and error.
/// A build error is returned and not cached, exactly as an uncached build
/// returns it. Being per-thread, the cache adds no shared state: nothing
/// here changes what is `Send` or `Sync` (P6-01).
fn with_resident_metamodel_manager<R>(f: impl FnOnce(&ModelManager) -> Result<R>) -> Result<R> {
    thread_local! {
        static RESIDENT: std::cell::RefCell<Option<ModelManager>> =
            const { std::cell::RefCell::new(None) };
    }
    RESIDENT.with(|cell| {
        if cell.try_borrow().is_ok_and(|resident| resident.is_none()) {
            let mm = metamodel_model_manager()?;
            if let Ok(mut slot) = cell.try_borrow_mut() {
                *slot = Some(mm);
            }
        }
        match cell.try_borrow() {
            Ok(resident) => match resident.as_ref() {
                Some(mm) => f(mm),
                None => f(&metamodel_model_manager()?),
            },
            // Unreachable in practice (the closure cannot re-enter this
            // function), but a fresh manager is always a correct answer.
            Err(_) => f(&metamodel_model_manager()?),
        }
    })
}

/// The text a TS `catch (err)` would see on `err.message`: the exception's
/// own, already-constructed message. For a catalogue error that is
/// `ContractError::final_message` (the same text the native oracle harness
/// compares, per its own doc comment); the two pre-port shapes
/// (`Error::type_not_found`, `Error::illegal_model`) are given the message
/// they carry to the binding.
fn ts_message(err: &Error) -> String {
    if let Some(message) = err.unported_illegal_model() {
        return message.to_string();
    }
    if let Some(type_name) = err.unported_type_not_found() {
        return format!("type not found: {type_name}");
    }
    err.contract().final_message()
}

/// `BaseModelManager.validateAst`'s structural check:
/// `this.getSerializer().fromJSON(modelFile.getAst())`
/// (`src/basemodelmanager.ts`), run with accordproject/concerto#1273's
/// `STRICT_VALIDATE_OPTIONS` preset (task P3-02) so an unknown property or a
/// required property explicitly set to `null` is rejected too — the
/// strictness `concerto-validate-rs` never had (plan §1.3's confirmed bugs:
/// no `Long`/`DateTime`/relationship/enum support, only the direct super
/// type's properties merged, abstract and nested `$class` values unchecked).
/// Any failure — no `$class`, an unresolvable type, a structural mismatch —
/// is re-thrown as `MetamodelException(error.message)`, exactly as TS's
/// `catch` block does.
///
/// The metamodel manager is resident per thread (task P5-21,
/// `with_resident_metamodel_manager`), not rebuilt on every call.
pub fn validate_metamodel(ast: &Value) -> Result<()> {
    let options = FromJsonOptions {
        reject_unknown_keys: true,
        reject_required_null: true,
        ..FromJsonOptions::default()
    };
    with_resident_metamodel_manager(|mm| {
        from_json(mm, ast, &options, &mut FixedEnv)
            .map(|_resource| ())
            .map_err(|err| wrapped(&err))
    })
}

/// `throw new MetamodelException(error.message)`: `validateAst`'s `catch`
/// block.
fn wrapped(err: &Error) -> Error {
    ContractError::new(
        ErrorKind::Metamodel,
        "basemodelmanager-validateast-wrapped",
        vec![("message", ts_message(err))],
    )
    .into()
}

/// `BaseModelManager`'s cached `this.metamodelModelFile`: `new
/// ModelFile(this, MetaModelUtil.metaModelAst, undefined,
/// MetaModelNamespace)` (`src/basemodelmanager.ts`'s constructor), so its
/// file name is the namespace itself and it has no CTO definitions.
///
/// Loaded once per thread and cloned on every later call (P5-06:
/// `validateAst` registers it on every call); a load error is returned, and
/// not cached, exactly as an uncached load would return it.
pub(crate) fn metamodel_model_file() -> Result<ModelFile> {
    thread_local! {
        static METAMODEL_MODEL_FILE: std::cell::RefCell<Option<ModelFile>> =
            const { std::cell::RefCell::new(None) };
    }
    if let Some(model_file) = METAMODEL_MODEL_FILE.with(|cache| cache.borrow().clone()) {
        return Ok(model_file);
    }
    let metamodel: Value =
        serde_json::from_str(METAMODEL_AST_JSON).expect("the vendored metamodel AST is JSON");
    let model_file = ModelFile::from_json(&metamodel, Some(METAMODEL_NAMESPACE.to_string()))?;
    METAMODEL_MODEL_FILE.with(|cache| *cache.borrow_mut() = Some(model_file.clone()));
    Ok(model_file)
}

/// `validateAst`'s structural check against the caller's own model manager
/// `mm` (which must already hold the metamodel):
/// `this.getSerializer().fromJSON(modelFile.getAst())`, with the manager's
/// serializer default options (`baseDefaultOptions`, `{validate: true,
/// utcOffset}`), not [`validate_metamodel`]'s strict preset — concerto-core
/// 5.0.0 passes no options here. Any failure is re-thrown as
/// `MetamodelException(error.message)`.
///
/// TS's `Serializer` merges the manager's own constructor options into its
/// defaults too. None of the keys `Serializer.fromJSON` reads
/// (`acceptResourcesForRelationships`, `utcOffset`,
/// `strictQualifiedDateTimes`) can change a metamodel document's outcome —
/// the metamodel declares no relationship and no `DateTime` property — so
/// only a manager option `validate: false` would, and no
/// `ModelManager` in this port carries a serializer option bag.
pub(crate) fn deserialize_ast(mm: &ModelManager, ast: &Value) -> Result<()> {
    from_json(mm, ast, &FromJsonOptions::default(), &mut FixedEnv)
        .map(|_resource| ())
        .map_err(|err| wrapped(&err))
}

/// `BaseModelManager.validateAst(modelFile)` (`src/basemodelmanager.ts`):
/// the version check (`check_version`), then the structural check
/// ([`validate_metamodel`]).
///
/// Unlike the TS reference, this takes the AST directly rather than a
/// `ModelFile` handle — task P3-04's scope is the standalone check
/// `concerto-validate-rs` provided (module doc), not the `ModelFile`/
/// `ModelManager` view integration (task P4-08). A missing or non-string
/// `$class` fails the version check exactly as it does in TS
/// (`check_version`'s doc), before any structural check runs.
pub fn validate_ast(ast: &Value) -> Result<()> {
    check_version(ast)?;
    validate_metamodel(ast)
}

/// TS `validateMetaModel(input)` (`src/introspect/metamodel.ts`;
/// `SEAM_LEDGER.tsv` row `validateMetaModel`, planned task `P3-04+P4-08`;
/// accordproject/concerto-rust#265): `serializer.fromJSON(input)` over a
/// fresh metamodel manager (`newMetaModelManager()`), with a `Serializer`
/// built with no options, so the default `{validate: true}`. TS returns
/// `input` itself unchanged, so a caller that needs the return value keeps
/// its own `input`.
///
/// Not [`validate_metamodel`]: that is `validateAst`'s check, which runs the
/// strict preset and re-throws every failure as a `MetamodelException`.
/// This one runs the default options, and a failure is the serializer's own
/// error, unwrapped, as TS throws it.
///
/// The metamodel manager is `metamodel_model_manager`'s. TS's
/// `newMetaModelManager` names the file `concerto.metamodel` and keeps the
/// metamodel's CTO text as its definitions; neither is observable here
/// beyond an error message's wording (error parity compares the class).
pub fn validate_meta_model_instance(input: &Value) -> Result<()> {
    let mm = metamodel_model_manager()?;
    from_json(&mm, input, &FromJsonOptions::default(), &mut FixedEnv).map(|_resource| ())
}

/// TS `modelManagerFromMetaModel(metaModel, validate = true)`
/// (`src/introspect/metamodel.ts`; `SEAM_LEDGER.tsv` row
/// `modelManagerFromMetaModel`, planned task `P3-04+P4-08`;
/// accordproject/concerto-rust#265):
///
/// 1. when `validate` is set, [`validate_meta_model_instance`] first;
/// 2. a fresh [`ModelManager`] (`new ModelManager()`, no options);
/// 3. for each entry of `metaModel.models`, in order, `new ModelFile(mm,
///    model, null, null)` (which, since BC-19 in R1, runs
///    [`check_ast_shape`] on an object model) and a validating
///    `addModelFile(mf, null, null)`:
///    a namespace already registered is the already-exists error, otherwise
///    the new file alone is validated against the manager as it stands
///    (`ModelManager::validate_detached_model_file`) before it is
///    registered;
/// 4. `validateModelFiles()` over the whole manager.
///
/// `metaModel.models.forEach` on something that is not an array is V8's
/// `TypeError`, as in TS: reading `models` of `null`, `forEach` of a
/// missing or `null` `models`, or `forEach` not being a function.
pub fn model_manager_from_meta_model(meta_model: &Value, validate: bool) -> Result<ModelManager> {
    if validate {
        validate_meta_model_instance(meta_model)?;
    }
    let mut mm = ModelManager::new()?;
    let read_properties = |value: &str, property: &str| -> Error {
        ContractError::new(
            ErrorKind::MalformedInput,
            "engine-typeerror-readproperties",
            vec![
                ("value", value.to_string()),
                ("property", property.to_string()),
            ],
        )
        .into()
    };
    if meta_model.is_null() {
        return Err(read_properties("null", "models"));
    }
    let models = match meta_model.get("models") {
        None => return Err(read_properties("undefined", "forEach")),
        Some(Value::Null) => return Err(read_properties("null", "forEach")),
        Some(Value::Array(models)) => models,
        Some(_) => return Err(not_a_function("mm.models.forEach")),
    };
    for model in models {
        // BC-19 (R1): `new ModelFile(modelManager, mm, null, null)` on a
        // `new ModelManager()`, whose default is the strict shape check. The
        // constructor's own `typeof ast !== 'object'` check comes first, so
        // a model that is not a JS object keeps that error (below).
        if model.is_object() || model.is_array() {
            check_ast_shape(model)?;
        }
        let model_file = ModelFile::from_json_with_definitions(model, None, None)?;
        if mm.model_file(model_file.namespace()).is_none() {
            mm.validate_detached_model_file(&model_file)?;
        }
        mm.add_model_file(model_file)?;
    }
    mm.validate_models()?;
    Ok(mm)
}

/// The strict AST shape check at model load (BREAKING-CHANGES-PLAN.md BC-19,
/// with BC-17 and BC-20; release R1, task P5-49,
/// accordproject/concerto-rust#370): TS `new ModelFile(modelManager, ast)`
/// runs it, after its own argument checks, unless the manager was built
/// with `metamodelValidation: false`. So every load path that builds a
/// `ModelFile` (`fromAst`, `addModel`, `addCTOModel`, `addModelFiles`,
/// `updateModelFile`, and the file `addModelFile` is given) rejects an AST
/// that does not have the metamodel's shape, with an
/// `IllegalModelException`, before any part of the AST is walked. TS 5.0.0
/// loads many such ASTs, or crashes on them with a V8 `TypeError` (BC-18).
///
/// In order, and stopping at the first problem:
///
/// 1. BC-17 and BC-20, over every node of the AST in document order: a
///    `decorators` that is present, not `null` and not an array
///    (`modelfile-load-decoratorsnotarray`); a super type (`superType`)
///    whose `name` is not a non-empty string
///    (`modelfile-load-supertypename`); any other `name` that is not a
///    string (`modelfile-load-namenotstring`).
/// 2. `validateAst`'s strict check ([`validate_ast`]): the version check,
///    then the structure against the metamodel with the strict preset
///    ([`validate_metamodel`]). Its error, whatever its class, is
///    re-thrown as an `IllegalModelException` whose message is the check's
///    own, after a fixed prefix (`modelfile-load-astshape`).
///
/// **One tolerance.** The reference CTO parser (concerto-cto 5.0.0) writes a
/// string `defaultValue` on a `DateTimeProperty` (`o DateTime d
/// default="..."`), which `concerto.metamodel@1.0.0` does not declare, so
/// `validateAst` rejects every such model ("Unexpected properties for type
/// concerto.metamodel@1.0.0.DateTimeProperty: defaultValue"). Every other
/// AST the reference parser writes for the oracle corpus passes the check
/// (P5-49 ran it over all 611 CTO-cache ASTs). So that a model written in
/// CTO still loads, a string `defaultValue` on a `DateTimeProperty` node is
/// left out of step 2; any other `defaultValue` there is checked as before.
///
/// The check reads nothing but `ast`, and runs on the resident metamodel
/// manager ([`validate_metamodel`]'s), so it does not depend on, or change,
/// any caller's manager.
pub fn check_ast_shape(ast: &Value) -> Result<()> {
    let mut parser_extras = false;
    check_node_shapes(ast, false, &mut parser_extras)?;
    let result = if parser_extras {
        let mut stripped = ast.clone();
        strip_parser_extras(&mut stripped);
        validate_ast(&stripped)
    } else {
        validate_ast(ast)
    };
    result.map_err(|err| {
        ContractError::new(
            ErrorKind::IllegalModel,
            "modelfile-load-astshape",
            vec![("message", ts_message(&err))],
        )
        .into()
    })
}

/// The `$class` of the metamodel node that may carry a parser-written
/// `defaultValue` ([`check_ast_shape`]'s tolerance).
const DATE_TIME_PROPERTY: &str = "concerto.metamodel@1.0.0.DateTimeProperty";

/// Whether `map` is a `DateTimeProperty` node with a string `defaultValue`,
/// which [`check_ast_shape`] leaves out of the metamodel check.
fn has_parser_default(map: &serde_json::Map<String, Value>) -> bool {
    map.get("$class").and_then(Value::as_str) == Some(DATE_TIME_PROPERTY)
        && map.get("defaultValue").is_some_and(Value::is_string)
}

/// Removes every value [`has_parser_default`] matches from `node`.
fn strip_parser_extras(node: &mut Value) {
    match node {
        Value::Object(map) => {
            if has_parser_default(map) {
                map.remove("defaultValue");
            }
            map.values_mut().for_each(strip_parser_extras);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_parser_extras),
        _ => {}
    }
}

/// [`check_ast_shape`]'s first step, for `node` and everything under it.
/// `super_type` is true for the node under a `superType` key;
/// `parser_extras` is set when a node matches [`has_parser_default`].
fn check_node_shapes(node: &Value, super_type: bool, parser_extras: &mut bool) -> Result<()> {
    match node {
        Value::Object(map) => {
            *parser_extras |= has_parser_default(map);
            if let Some(decorators) = map.get("decorators")
                && !decorators.is_array()
                && !decorators.is_null()
            {
                return Err(shape_error("modelfile-load-decoratorsnotarray", decorators));
            }
            if super_type {
                match map.get("name") {
                    Some(Value::String(name)) if !name.is_empty() => {}
                    Some(name) => return Err(shape_error("modelfile-load-supertypename", name)),
                    // A missing name is the metamodel check's error.
                    None => {}
                }
            } else if let Some(name) = map.get("name")
                && !name.is_string()
            {
                return Err(shape_error("modelfile-load-namenotstring", name));
            }
            for (key, value) in map {
                check_node_shapes(value, key == "superType" && value.is_object(), parser_extras)?;
            }
            Ok(())
        }
        Value::Array(items) => items
            .iter()
            .try_for_each(|item| check_node_shapes(item, false, parser_extras)),
        _ => Ok(()),
    }
}

/// An `IllegalModelException` from [`check_node_shapes`], rendering the
/// offending value as JSON text.
fn shape_error(code: &'static str, value: &Value) -> Error {
    ContractError::new(
        ErrorKind::IllegalModel,
        code,
        vec![("value", value.to_string())],
    )
    .into()
}

/// `validateAst`'s version check:
/// `ModelUtil.parseNamespace(ModelUtil.getNamespace(modelFile.getAst().$class))`,
/// whose version must be the metamodel's own
/// (`basemodelmanager-validateast-versionmismatch`, rendering an absent
/// version as JS `null`, as the TS template literal does).
///
/// TS's `ModelFile` constructor does not require the AST to carry a
/// `$class`, so on `addModelFile`'s path (`metamodelValidation`) any JSON
/// value can reach `ModelUtil.getNamespace(fqn)`:
///
/// - falsy (missing, `null`, `false`, `0`, `""`): its `!fqn` guard throws
///   `Error` "FQN is invalid." (`modelutil-getnamespace-nofnq`);
/// - an array: `Array.prototype.lastIndexOf('.')` finds a `"."` element only
///   by strict equality; with none the namespace is `''` (and
///   `parseNamespace` throws "Namespace is null or undefined."), with one
///   `fqn.substr` is not a function (V8 `TypeError`);
/// - any other non-string (a non-zero number, `true`, an object):
///   `fqn.lastIndexOf` is not a function (V8 `TypeError`).
///
/// Each fails before the metamodel is added, as in TS.
pub(crate) fn check_version(ast: &Value) -> Result<()> {
    let class = ast.get("$class").unwrap_or(&Value::Null);
    let ns = match class {
        _ if !ecma::is_truthy(class) => model_util::get_namespace(None)?,
        Value::String(class_name) => model_util::get_namespace(Some(class_name))?,
        Value::Array(items) if items.iter().any(|item| item == ".") => {
            return Err(not_a_function("fqn.substr"));
        }
        Value::Array(_) => "",
        _ => return Err(not_a_function("fqn.lastIndexOf")),
    };
    let model_file_version = namespace_version(ns)?;
    let metamodel_version = namespace_version(METAMODEL_NAMESPACE)?;
    if model_file_version != metamodel_version {
        let js = |version: Option<String>| version.unwrap_or_else(|| "null".to_string());
        return Err(ContractError::new(
            ErrorKind::Metamodel,
            "basemodelmanager-validateast-versionmismatch",
            vec![
                ("modelFileVersion", js(model_file_version)),
                ("metamodelVersion", js(metamodel_version)),
            ],
        )
        .into());
    }
    Ok(())
}

/// `ModelUtil.parseNamespace(ns).version`, except that an unversioned
/// namespace gives `None` rather than `parse_namespace`'s error (BC-02,
/// P5-50): it is still rejected, by the caller's version mismatch, with the
/// `MetamodelException` TS 5.0.0 threw for it.
fn namespace_version(ns: &str) -> Result<Option<String>> {
    Ok(model_util::split_namespace(ns)?.1.map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- ported from concerto-validate-rs's src/lib.rs tests (issue
    //      accordproject/concerto-rust#59's exit condition, as narrowed by
    //      the maintainer's issue comment: validate-rs's own tests, ported
    //      as tests of this module, citing the source test) ----

    /// concerto-validate-rs `tests::test_valid_metamodel_validation`: the
    /// vendored metamodel document, validated against itself. Unlike the
    /// source test, this reads the same vendored copy this module already
    /// includes rather than a repo-root `metamodel.json` (that file exists
    /// only in `concerto-validate-rs`, out of scope here — see the module
    /// doc), but it is byte-for-byte the same document (both copies are
    /// `MetaModelUtil.metaModelAst`).
    #[test]
    fn valid_metamodel_validation() {
        let metamodel: Value =
            serde_json::from_str(METAMODEL_AST_JSON).expect("the vendored AST is JSON");
        let result = validate_metamodel(&metamodel);
        assert!(
            result.is_ok(),
            "metamodel validation should succeed: {result:?}"
        );
    }

    /// concerto-validate-rs `tests::test_invalid_json`: malformed JSON text
    /// fails validation. This module's API boundary differs deliberately
    /// from `concerto-validate-rs`'s: `validate_metamodel` takes an already
    /// parsed [`Value`], not a `&str` — JSON parsing is a step upstream of
    /// this module in concerto-rust (`serde_json::from_str`, as every other
    /// entry point in this crate does), not something this port repeats. A
    /// document with no usable `$class` at all is the nearest equivalent
    /// this module's own boundary can express; it still fails, through
    /// `Serializer::from_json`'s own "no `$class`" check.
    #[test]
    fn invalid_json_has_no_usable_class() {
        let result = validate_metamodel(&json!({}));
        assert!(
            result.is_err(),
            "a $class-less document should fail: {result:?}"
        );
    }

    /// concerto-validate-rs `tests::test_invalid_namespace`: a `namespace`
    /// of the wrong JSON type fails validation. The source fixture omits
    /// `imports`/`declarations`, which `STRICT_VALIDATE_OPTIONS` would also
    /// reject as missing required properties; either way the document must
    /// fail, so this keeps both defects to stay close to the source.
    #[test]
    fn invalid_namespace_type() {
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": 123
        });
        let result = validate_metamodel(&ast);
        assert!(
            result.is_err(),
            "a non-string namespace should fail: {result:?}"
        );
    }

    /// concerto-validate-rs `tests::test_missing_class_property`: a document
    /// with no `$class` at all fails validation.
    #[test]
    fn missing_class_property() {
        let ast = json!({
            "namespace": "test.namespace",
            "imports": [],
            "declarations": []
        });
        let result = validate_metamodel(&ast);
        assert!(
            result.is_err(),
            "a $class-less document should fail: {result:?}"
        );
    }

    /// concerto-validate-rs `tests::test_simple_model_validation`: a small,
    /// well-formed model passes validation.
    #[test]
    fn simple_model_validation() {
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "test.namespace@1.0.0",
            "imports": [],
            "declarations": [
                {
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "TestConcept",
                    "isAbstract": false,
                    "properties": [
                        {
                            "$class": "concerto.metamodel@1.0.0.StringProperty",
                            "name": "testField",
                            "isArray": false,
                            "isOptional": false
                        }
                    ]
                }
            ]
        });
        let result = validate_metamodel(&ast);
        assert!(
            result.is_ok(),
            "a simple valid model should pass validation: {result:?}"
        );
    }

    /// concerto-validate-rs `tests::test_extra_properties`: an undeclared
    /// property anywhere in the document fails validation.
    /// `concerto-validate-rs`'s own hand-rolled structural check caught
    /// this only by accident of its shape checks; here it is
    /// `populator::validate_properties` (called unconditionally by
    /// `visitClassDeclaration`, in both TS and this port) that rejects it
    /// — *not* `STRICT_VALIDATE_OPTIONS`. This
    /// document's two extra keys (`isOptional` on the declaration,
    /// `propertyType` on the property) are both non-null, and a non-null
    /// unknown key is rejected by `Serializer.fromJSON`'s default
    /// (non-strict) behaviour too, in TS and in this port alike — see
    /// `super::deserialize`'s own divergence table ("Unknown field =
    /// non-null" is an error under the default). The strict preset's own,
    /// real effect on unknown keys is exercised below by
    /// `null_extra_property_rejected_only_under_the_strict_preset`.
    #[test]
    fn extra_properties_rejected_under_the_strict_preset() {
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "test.namespace@1.0.0",
            "imports": [],
            "declarations": [
                {
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "TestConcept",
                    "isAbstract": false,
                    "isOptional": false,
                    "properties": [
                        {
                            "$class": "concerto.metamodel@1.0.0.StringProperty",
                            "name": "testField",
                            "isArray": false,
                            "isOptional": false,
                            "propertyType": "String"
                        }
                    ]
                }
            ]
        });
        let result = validate_metamodel(&ast);
        assert!(
            result.is_err(),
            "extra properties should fail validation under the strict preset: {result:?}"
        );
    }

    /// The strict preset's actual, provable effect on unknown keys, and the
    /// test that would fail if `validate_metamodel` stopped applying
    /// `STRICT_VALIDATE_OPTIONS` (e.g. `Some(&options)` dropped to `None`).
    /// Per `super::deserialize`'s own divergence table, an unknown property
    /// set to `null` is *ignored* by default (it never reaches
    /// `populator::validate_properties`, since `get_assignable_properties`
    /// drops nullish values before that check runs) and rejected only when
    /// `STRICT_VALIDATE_OPTIONS.reject_unknown_keys` is set — unlike a
    /// non-null unknown property, which `extra_properties_rejected_under_
    /// the_strict_preset` above shows fails either way, strict or not.
    ///
    /// The first assertion calls `validate_metamodel` itself (which always
    /// applies the strict preset) and expects it to reject the document.
    /// The second calls the underlying serializer directly with the
    /// default (non-strict) options, on the identical document, and expects
    /// it to accept it — establishing that the first assertion's failure is
    /// really due to the strict preset, not some other check.
    #[test]
    fn null_extra_property_rejected_only_under_the_strict_preset() {
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "test.namespace@1.0.0",
            "imports": [],
            "declarations": [
                {
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "TestConcept",
                    "isAbstract": false,
                    "unknownKey": null,
                    "properties": []
                }
            ]
        });

        let strict_result = validate_metamodel(&ast);
        assert!(
            strict_result.is_err(),
            "a null unknown property should be rejected by validate_metamodel, \
             which applies the strict preset: {strict_result:?}"
        );

        let mm = metamodel_model_manager().expect("metamodel model manager");
        let default_result = from_json(&mm, &ast, &FromJsonOptions::default(), &mut FixedEnv);
        assert!(
            default_result.is_ok(),
            "the same null unknown property should be ignored under the \
             (non-strict) default options: {default_result:?}"
        );
    }

    // ---- validateAst's own behaviour beyond validate_metamodel (not in
    //      concerto-validate-rs, which has no version check at all) ----

    #[test]
    fn validate_ast_accepts_a_well_formed_model() {
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": []
        });
        assert!(validate_ast(&ast).is_ok());
    }

    #[test]
    fn validate_ast_rejects_an_unknown_metamodel_version() {
        let ast = json!({
            "$class": "concerto.metamodel@99.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "declarations": []
        });
        let err = validate_ast(&ast).expect_err("an unknown metamodel version should fail");
        let Some(contract) = err.ported().cloned() else {
            panic!("expected a Contract error, got {err:?}");
        };
        assert_eq!(contract.kind, ErrorKind::Metamodel);
        assert_eq!(
            contract.message(),
            "Model file version 99.0.0 does not match metamodel version 1.0.0"
        );
    }

    #[test]
    fn validate_ast_rejects_a_bad_metamodel_ast_with_an_undeclared_property() {
        // TS: modelmanager.js "#addModel > should throw for a bad metamodel
        // AST".
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "undeclared": []
        });
        assert!(validate_ast(&ast).is_err());
    }

    // ---- the resident metamodel manager (task P5-21) ----

    /// The structural check on a fresh metamodel manager, as
    /// `validate_metamodel` ran it before P5-21.
    fn validate_metamodel_on_a_fresh_manager(ast: &Value) -> Result<()> {
        let mm = metamodel_model_manager()?;
        let options = FromJsonOptions {
            reject_unknown_keys: true,
            reject_required_null: true,
            ..FromJsonOptions::default()
        };
        from_json(&mm, ast, &options, &mut FixedEnv)
            .map(|_resource| ())
            .map_err(|err| wrapped(&err))
    }

    fn outcome(result: Result<()>) -> Option<(ErrorKind, String)> {
        result.err().map(|err| (err.kind(), err.to_string()))
    }

    #[test]
    fn validate_ast_on_the_resident_manager_matches_a_fresh_one_in_any_order() {
        let metamodel: Value = serde_json::from_str(METAMODEL_AST_JSON).unwrap();
        let cases = [
            metamodel.clone(),
            json!({ "$class": "concerto.metamodel@1.0.0.Model", "namespace": "org.acme@1.0.0", "undeclared": [] }),
            json!({ "$class": "concerto.metamodel@1.0.0.Model", "namespace": "org.acme@1.0.0", "imports": [], "declarations": [] }),
            json!({ "$class": "concerto.metamodel@1.0.0.Model", "namespace": null, "declarations": [] }),
            json!({ "$class": "concerto.metamodel@1.0.0.Nope", "namespace": "org.acme@1.0.0" }),
            json!({ "$class": "concerto.metamodel@1.0.0.Model" }),
            json!({ "namespace": "org.acme@1.0.0" }),
            metamodel,
        ];
        // Twice over, so the second pass runs on a manager every case has
        // already been through.
        for _ in 0..2 {
            for ast in &cases {
                assert_eq!(
                    outcome(validate_metamodel(ast)),
                    outcome(validate_metamodel_on_a_fresh_manager(ast)),
                    "{ast}"
                );
                let fresh = check_version(ast).and_then(|()| validate_metamodel_on_a_fresh_manager(ast));
                assert_eq!(outcome(validate_ast(ast)), outcome(fresh), "{ast}");
            }
        }
    }

    #[test]
    fn validate_ast_on_the_resident_manager_is_per_thread() {
        let ok = json!({ "$class": "concerto.metamodel@1.0.0.Model", "namespace": "org.acme@1.0.0", "imports": [], "declarations": [] });
        let bad = json!({ "$class": "concerto.metamodel@1.0.0.Model", "namespace": "org.acme@1.0.0", "undeclared": [] });
        let expected = outcome(validate_metamodel_on_a_fresh_manager(&bad));
        std::thread::scope(|scope| {
            for _ in 0..4 {
                scope.spawn(|| {
                    for _ in 0..3 {
                        assert!(validate_ast(&ok).is_ok());
                        assert_eq!(outcome(validate_ast(&bad)), expected);
                    }
                });
            }
        });
    }

    // ---- ModelManager::validate_ast: `validateAst` on a caller's own
    //      model manager, the `metamodelValidation` option (task P4-08b) ----

    fn namespaces(mm: &ModelManager) -> Vec<String> {
        mm.model_files()
            .map(|mf| mf.namespace().to_string())
            .collect()
    }

    fn model_file(ast: &Value) -> ModelFile {
        ModelFile::from_json(ast, Some("test.cto".into())).expect("a well-formed model file")
    }

    #[test]
    fn metamodel_validation_is_off_by_default_and_carried_by_scratch_copies() {
        let mut mm = ModelManager::new().unwrap();
        assert!(!mm.metamodel_validation());
        mm.set_metamodel_validation(true);
        assert!(mm.metamodel_validation());
        mm.load_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "imports": [],
                "declarations": []
            }),
            None,
        )
        .unwrap();
        assert!(
            mm.delete_model_file("org.acme@1.0.0")
                .unwrap()
                .metamodel_validation()
        );
    }

    #[test]
    fn manager_validate_ast_accepts_a_well_formed_model_and_removes_the_metamodel() {
        let mut mm = ModelManager::new().unwrap();
        let before = namespaces(&mm);
        let mf = model_file(&json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Person",
                "isAbstract": false,
                "properties": [{
                    "$class": "concerto.metamodel@1.0.0.StringProperty",
                    "name": "name",
                    "isArray": false,
                    "isOptional": false
                }]
            }]
        }));
        mm.validate_ast(&mf).unwrap();
        assert_eq!(namespaces(&mm), before);
        assert!(
            mm.declaration_id("concerto.metamodel@1.0.0.Model")
                .is_none()
        );
    }

    #[test]
    fn manager_validate_ast_failure_leaves_the_metamodel_registered() {
        // TS: `validateAst`'s `deleteModelFile(MetaModelNamespace)` follows
        // the `try`/`catch` that re-throws, so a failed check never reaches it.
        let mut mm = ModelManager::new().unwrap();
        let mf = model_file(&json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": [],
            "undeclared": []
        }));
        let err = mm
            .validate_ast(&mf)
            .expect_err("an undeclared property is invalid");
        let Some(contract) = err.ported().cloned() else {
            panic!("expected a Contract error, got {err:?}");
        };
        assert_eq!(contract.kind, ErrorKind::Metamodel);
        let mut expected: Vec<String> =
            ModelManager::new().map(|fresh| namespaces(&fresh)).unwrap();
        expected.push(METAMODEL_NAMESPACE.to_string());
        assert_eq!(namespaces(&mm), expected);
        assert_eq!(
            mm.model_file(METAMODEL_NAMESPACE).unwrap().file_name(),
            Some(METAMODEL_NAMESPACE)
        );
        // A later check finds it already there and keeps it.
        let valid = model_file(&json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.other@1.0.0",
            "imports": [],
            "declarations": []
        }));
        mm.validate_ast(&valid).unwrap();
        assert_eq!(namespaces(&mm), expected);
    }

    // ---- ModelManager::validate_ast_value (P5-13,
    //      accordproject/concerto-rust#297): the check over the AST alone,
    //      on the resident metamodel manager where that is exact ----

    /// An AST the `ModelFile` constructor rejects (no `namespace`,
    /// `declarations` not an array) reaches the structural check, which
    /// throws TS's `MetamodelException` and leaves the metamodel
    /// registered, as TS 5.0.0's `validateAst` does.
    #[test]
    fn manager_validate_ast_value_reports_a_model_file_shape_error_as_a_metamodel_error() {
        for ast in [
            json!({ "$class": "concerto.metamodel@1.0.0.Model", "imports": [], "declarations": [] }),
            json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "imports": [],
                "declarations": "not an array"
            }),
        ] {
            assert!(ModelFile::from_json(&ast, None).is_err());
            let mut mm = ModelManager::new().unwrap();
            let err = mm.validate_ast_value(&ast).expect_err("the check fails");
            assert_eq!(kind_of(&err), Some(ErrorKind::Metamodel));
            assert!(mm.model_file(METAMODEL_NAMESPACE).is_some());
        }
    }

    /// A pass on the resident metamodel leaves the caller's manager exactly
    /// as it was: no namespace, no mutation counted.
    #[test]
    fn manager_validate_ast_value_pass_leaves_the_manager_unchanged() {
        let mut mm = ModelManager::new().unwrap();
        let (before, generation) = (namespaces(&mm), mm.generation());
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": []
        });
        for _ in 0..2 {
            mm.validate_ast_value(&ast).unwrap();
            assert_eq!(namespaces(&mm), before);
            assert_eq!(mm.generation(), generation);
        }
    }

    /// A document typed by the caller's own model is resolved against the
    /// caller's manager, as TS's `getSerializer().fromJSON` does: it fails
    /// on the resident metamodel manager, which does not hold that type, so
    /// the check runs on the caller's manager, where it passes, and the
    /// metamodel is removed again.
    #[test]
    fn manager_validate_ast_value_resolves_the_callers_own_types() {
        let mut mm = ModelManager::new().unwrap();
        mm.load_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "imports": [],
                "declarations": [{
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "Person",
                    "isAbstract": false,
                    "properties": []
                }]
            }),
            None,
        )
        .unwrap();
        let before = namespaces(&mm);
        mm.validate_ast_value(&json!({ "$class": "org.acme@1.0.0.Person" }))
            .unwrap();
        assert_eq!(namespaces(&mm), before);
    }

    /// A manager without the system models (`ModelManager::default()`) does
    /// not match the resident manager's, so its own check runs, as before
    /// P5-13: the metamodel's declarations cannot resolve their implicit
    /// `Concept` super type there.
    #[test]
    fn manager_validate_ast_value_without_system_models_checks_the_manager_itself() {
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": []
        });
        assert!(ModelManager::new().unwrap().validate_ast_value(&ast).is_ok());
        let mut bare = ModelManager::default();
        let err = bare
            .validate_ast_value(&ast)
            .expect_err("no system models to resolve against");
        assert_eq!(kind_of(&err), Some(ErrorKind::Metamodel));
        assert!(bare.model_file(METAMODEL_NAMESPACE).is_some());
    }

    #[test]
    fn manager_validate_ast_version_mismatch_adds_nothing() {
        let mut mm = ModelManager::new().unwrap();
        let before = namespaces(&mm);
        let mf = model_file(&json!({
            "$class": "concerto.metamodel@99.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": []
        }));
        let err = mm
            .validate_ast(&mf)
            .expect_err("an unknown metamodel version");
        let Some(contract) = err.ported().cloned() else {
            panic!("expected a Contract error, got {err:?}");
        };
        assert_eq!(
            contract.message(),
            "Model file version 99.0.0 does not match metamodel version 1.0.0"
        );
        assert_eq!(namespaces(&mm), before);
    }

    /// `validateAst` on an AST whose `$class` `getNamespace` cannot use:
    /// the error, and nothing added to the manager. Expected values are the
    /// frozen reference's (concerto-core 5.0.0, `new ModelManager({
    /// metamodelValidation: true }).addModelFile(new ModelFile(mm, ast))`).
    fn assert_bad_class(class: Option<Value>, kind: ErrorKind, message: &str) {
        let mut ast = json!({
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": []
        });
        if let Some(class) = class {
            ast["$class"] = class;
        }
        for err in [
            validate_ast(&ast).expect_err("the standalone check fails"),
            {
                let mut mm = ModelManager::new().unwrap();
                let before = namespaces(&mm);
                let err = mm
                    .validate_ast(&model_file(&ast))
                    .expect_err("the manager check fails");
                assert_eq!(namespaces(&mm), before, "nothing is added");
                err
            },
        ] {
            let Some(contract) = err.ported().cloned() else {
                panic!("expected a Contract error, got {err:?}");
            };
            assert_eq!(contract.kind, kind);
            assert_eq!(contract.message(), message);
        }
    }

    #[test]
    fn validate_ast_without_a_class_is_an_invalid_fqn() {
        assert_bad_class(None, ErrorKind::InvalidArgument, "FQN is invalid.");
        assert_bad_class(
            Some(json!(null)),
            ErrorKind::InvalidArgument,
            "FQN is invalid.",
        );
        assert_bad_class(
            Some(json!("")),
            ErrorKind::InvalidArgument,
            "FQN is invalid.",
        );
        assert_bad_class(
            Some(json!(0)),
            ErrorKind::InvalidArgument,
            "FQN is invalid.",
        );
        assert_bad_class(
            Some(json!(false)),
            ErrorKind::InvalidArgument,
            "FQN is invalid.",
        );
    }

    #[test]
    fn validate_ast_with_a_non_string_class_is_a_type_error() {
        for class in [json!(5), json!(true), json!({})] {
            assert_bad_class(
                Some(class),
                ErrorKind::MalformedInput,
                "fqn.lastIndexOf is not a function",
            );
        }
        assert_bad_class(
            Some(json!(["."])),
            ErrorKind::MalformedInput,
            "fqn.substr is not a function",
        );
        assert_bad_class(
            Some(json!(["concerto.metamodel@1.0.0.Model"])),
            ErrorKind::InvalidArgument,
            "Namespace is null or undefined.",
        );
    }

    #[test]
    fn validate_ast_with_an_unversioned_class_reports_version_null() {
        assert_bad_class(
            Some(json!("concerto.metamodel.Model")),
            ErrorKind::Metamodel,
            "Model file version null does not match metamodel version 1.0.0",
        );
    }

    // ---- accordproject/concerto-rust#265: `addMetamodel`,
    //      `validateMetaModel` and `modelManagerFromMetaModel` ----

    fn kind_of(err: &Error) -> Option<ErrorKind> {
        err.ported().map(|contract| contract.kind)
    }

    fn person_models() -> Value {
        json!({
            "$class": "concerto.metamodel@1.0.0.Models",
            "models": [{
                "$class": "concerto.metamodel@1.0.0.Model",
                "decorators": [],
                "namespace": "test.person@1.0.0",
                "imports": [],
                "declarations": [{
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "Person",
                    "isAbstract": false,
                    "properties": [{
                        "$class": "concerto.metamodel@1.0.0.StringProperty",
                        "name": "name",
                        "isArray": false,
                        "isOptional": false
                    }]
                }]
            }]
        })
    }

    #[test]
    fn add_metamodel_registers_the_metamodel_under_its_namespace() {
        // TS: `new ModelManager({ addMetamodel: true })` adds
        // `this.metamodelModelFile` last, named after its namespace.
        for metamodel_validation in [false, true] {
            let mut mm = ModelManager::new().unwrap();
            mm.set_metamodel_validation(metamodel_validation);
            mm.add_metamodel().expect("the metamodel loads");
            assert_eq!(
                namespaces(&mm).last().map(String::as_str),
                Some(METAMODEL_NAMESPACE)
            );
            let file = mm.model_file(METAMODEL_NAMESPACE).unwrap();
            assert_eq!(file.file_name(), Some(METAMODEL_NAMESPACE));
            assert!(mm.get_declaration("concerto.metamodel@1.0.0.Model").is_ok());
        }
    }

    #[test]
    fn add_metamodel_twice_is_the_already_exists_error() {
        let mut mm = ModelManager::new().unwrap();
        mm.add_metamodel().unwrap();
        let before = namespaces(&mm);
        assert!(mm.add_metamodel().is_err());
        assert_eq!(namespaces(&mm), before, "nothing is added");
    }

    #[test]
    fn validate_meta_model_instance_accepts_a_metamodel_document() {
        validate_meta_model_instance(&person_models()).expect("a valid Models document");
        let model = person_models()["models"][0].clone();
        validate_meta_model_instance(&model).expect("a valid Model document");
    }

    #[test]
    fn validate_meta_model_instance_does_not_wrap_the_serializer_error() {
        // TS `validateMetaModel` throws `serializer.fromJSON`'s own error;
        // only `validateAst` re-throws it as a `MetamodelException`.
        let mut bad = person_models();
        bad["models"][0]["namespace"] = json!(42);
        let err = validate_meta_model_instance(&bad).expect_err("a bad namespace fails");
        assert_ne!(kind_of(&err), Some(ErrorKind::Metamodel));
        assert!(validate_metamodel(&bad).is_err());
    }

    #[test]
    fn model_manager_from_meta_model_loads_every_model() {
        for validate in [true, false] {
            let mm = model_manager_from_meta_model(&person_models(), validate).unwrap();
            assert_eq!(
                namespaces(&mm).last().map(String::as_str),
                Some("test.person@1.0.0")
            );
            let file = mm.model_file("test.person@1.0.0").unwrap();
            assert_eq!(file.file_name(), None);
            assert!(mm.get_declaration("test.person@1.0.0.Person").is_ok());
        }
    }

    #[test]
    fn model_manager_from_meta_model_checks_the_shape_even_without_validate() {
        // Structurally invalid (an undeclared property), semantically fine.
        // `validate` runs `validateMetaModel` over the whole document first;
        // without it, BC-19 (R1) still rejects the model when its
        // `ModelFile` is built, with an `IllegalModelException`.
        let mut doc = person_models();
        doc["models"][0]["undeclared"] = json!(true);
        let err = model_manager_from_meta_model(&doc, true).expect_err("validateMetaModel");
        assert_ne!(kind_of(&err), Some(ErrorKind::IllegalModel));
        let err = model_manager_from_meta_model(&doc, false).expect_err("the load check");
        assert_eq!(kind_of(&err), Some(ErrorKind::IllegalModel));
        assert_eq!(
            err.contract().message(),
            "Model AST does not conform to the metamodel: Unexpected properties for type concerto.metamodel@1.0.0.Model: undeclared"
        );
    }

    // ---- P5-49 (BC-19 with BC-17 and BC-20, R1): `check_ast_shape` ----

    fn person_model() -> Value {
        person_models()["models"][0].clone()
    }

    fn shape_code(ast: &Value) -> Option<&'static str> {
        check_ast_shape(ast).err().map(|err| {
            let contract = err.ported().expect("a catalogue error");
            assert_eq!(contract.kind, ErrorKind::IllegalModel, "{ast}");
            contract.code
        })
    }

    #[test]
    fn check_ast_shape_accepts_well_formed_models() {
        assert_eq!(shape_code(&person_model()), None);
        let metamodel: Value = serde_json::from_str(METAMODEL_AST_JSON).unwrap();
        assert_eq!(shape_code(&metamodel), None);
        let mut with_super = person_model();
        with_super["declarations"][0]["superType"] =
            json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Base"});
        with_super["declarations"][0]["decorators"] = json!([]);
        assert_eq!(shape_code(&with_super), None);
    }

    #[test]
    fn check_ast_shape_rejects_a_non_array_decorators_value() {
        // BC-17: TS iterates a string by code unit and ignores a number.
        for decorators in [json!("💥emoji"), json!("x"), json!(""), json!(5), json!(true), json!({})] {
            let mut model = person_model();
            model["declarations"][0]["properties"][0]["decorators"] = decorators.clone();
            assert_eq!(
                shape_code(&model),
                Some("modelfile-load-decoratorsnotarray"),
                "{decorators}"
            );
        }
        let mut model = person_model();
        model["decorators"] = json!("x");
        let err = check_ast_shape(&model).unwrap_err();
        assert_eq!(
            err.contract().message(),
            "Invalid decorators. Expected array. Found \"x\""
        );
    }

    #[test]
    fn check_ast_shape_tolerates_the_parsers_date_time_default() {
        // concerto-cto 5.0.0 writes `defaultValue` on a `DateTimeProperty`,
        // which the metamodel does not declare.
        let date_time = |default: Value| {
            let mut model = person_model();
            model["declarations"][0]["properties"][0] = json!({
                "$class": "concerto.metamodel@1.0.0.DateTimeProperty",
                "name": "born",
                "isArray": false,
                "isOptional": false,
                "defaultValue": default
            });
            model
        };
        let parsed = date_time(json!("2020-01-01T00:00:00Z"));
        assert_eq!(shape_code(&parsed), None);
        assert!(validate_ast(&parsed).is_err(), "validateAst itself rejects it");
        assert_eq!(shape_code(&date_time(json!(5))), Some("modelfile-load-astshape"));
        // Only on a `DateTimeProperty`.
        let mut enum_value = person_model();
        enum_value["declarations"][0]["properties"][0]["$class"] =
            json!("concerto.metamodel@1.0.0.EnumProperty");
        enum_value["declarations"][0]["properties"][0]["defaultValue"] = json!("x");
        assert_eq!(shape_code(&enum_value), Some("modelfile-load-astshape"));
    }

    #[test]
    fn check_ast_shape_rejects_non_string_names() {
        // BC-20: TS coerces a name with `String()`.
        for name in [json!(1e308), json!(0), json!(false), json!(null), json!({})] {
            let mut model = person_model();
            model["declarations"][0]["name"] = name.clone();
            assert_eq!(shape_code(&model), Some("modelfile-load-namenotstring"), "{name}");
        }
    }

    #[test]
    fn check_ast_shape_rejects_an_empty_or_non_string_super_type_name() {
        // BC-20: `superType.name` of `""`, `0` or `false` gives TS's
        // "Could not find super type 0".
        for name in [json!(""), json!(0), json!(false), json!(null)] {
            let mut model = person_model();
            model["declarations"][0]["superType"] =
                json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": name});
            assert_eq!(shape_code(&model), Some("modelfile-load-supertypename"), "{name}");
        }
        // `superType: {}` has no name: the metamodel check's error.
        let mut model = person_model();
        model["declarations"][0]["superType"] = json!({});
        assert_eq!(shape_code(&model), Some("modelfile-load-astshape"));
    }

    #[test]
    fn check_ast_shape_rejects_what_the_metamodel_rejects() {
        // BC-19: an unknown property, a wrong-typed field, a malformed
        // `identified` and another metamodel version.
        let mut unknown = person_model();
        unknown["undeclared"] = json!([]);
        let mut bounds = person_model();
        bounds["declarations"][0]["properties"][0] = json!({
            "$class": "concerto.metamodel@1.0.0.IntegerProperty",
            "name": "age",
            "isArray": false,
            "isOptional": false,
            "validator": {"$class": "concerto.metamodel@1.0.0.IntegerDomainValidator", "lower": "0"}
        });
        let mut identified = person_model();
        identified["declarations"][0]["identified"] = json!("yes");
        let mut version = person_model();
        version["$class"] = json!("concerto.metamodel@99.0.0.Model");
        // DV-017's typeless relationship and DV-018's `null` decorator: the
        // metamodel check rejects both first, so those rows' own errors are
        // raised only with the check off.
        let mut relationship = person_model();
        relationship["declarations"][0]["properties"][0] = json!({
            "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
            "name": "home",
            "isArray": false,
            "isOptional": false
        });
        let mut null_decorator = person_model();
        null_decorator["declarations"][0]["decorators"] = json!([null]);
        for ast in [unknown, bounds, identified, version, relationship, null_decorator] {
            assert_eq!(shape_code(&ast), Some("modelfile-load-astshape"), "{ast}");
        }
        let mut version = person_model();
        version["$class"] = json!("concerto.metamodel@99.0.0.Model");
        assert_eq!(
            check_ast_shape(&version).unwrap_err().contract().message(),
            "Model AST does not conform to the metamodel: Model file version 99.0.0 does not match metamodel version 1.0.0"
        );
    }

    #[test]
    fn check_ast_shape_reports_the_first_problem_in_document_order() {
        let mut model = person_model();
        model["declarations"][0]["name"] = json!(7);
        model["declarations"][0]["properties"][0]["decorators"] = json!("x");
        assert_eq!(shape_code(&model), Some("modelfile-load-namenotstring"));
        // Steps 1 (BC-17, BC-20) before step 2 (the metamodel check).
        let mut model = person_model();
        model["undeclared"] = json!(true);
        model["declarations"][0]["decorators"] = json!(1);
        assert_eq!(shape_code(&model), Some("modelfile-load-decoratorsnotarray"));
    }

    #[test]
    fn model_manager_from_meta_model_keeps_the_error_for_a_non_object_model() {
        // `new ModelFile(mm, null)`: the constructor's own check, not BC-19.
        let doc = json!({"models": [null]});
        let err = model_manager_from_meta_model(&doc, false).expect_err("a null model");
        assert_ne!(kind_of(&err), Some(ErrorKind::IllegalModel));
    }

    #[test]
    fn model_manager_from_meta_model_rejects_a_semantically_invalid_model() {
        // The type `Missing` is never declared: `addModelFile` validates.
        let mut doc = person_models();
        doc["models"][0]["declarations"][0]["properties"] = json!([{
            "$class": "concerto.metamodel@1.0.0.ObjectProperty",
            "name": "other",
            "type": {"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Missing"},
            "isArray": false,
            "isOptional": false
        }]);
        assert!(model_manager_from_meta_model(&doc, false).is_err());
    }

    #[test]
    fn model_manager_from_meta_model_without_models_is_a_type_error() {
        for doc in [json!({}), json!({"models": null}), json!(null)] {
            let err = model_manager_from_meta_model(&doc, false).expect_err("no models array");
            assert_eq!(kind_of(&err), Some(ErrorKind::MalformedInput), "{doc}");
        }
        let err = model_manager_from_meta_model(&json!({"models": "x"}), false)
            .expect_err("models is not an array");
        assert_eq!(kind_of(&err), Some(ErrorKind::MalformedInput));
    }

    #[test]
    fn model_manager_from_meta_model_rejects_a_duplicate_namespace() {
        let mut doc = person_models();
        let model = doc["models"][0].clone();
        doc["models"].as_array_mut().unwrap().push(model);
        assert!(model_manager_from_meta_model(&doc, false).is_err());
    }
}
