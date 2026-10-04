//! Checking a Concerto AST document against the metamodel itself.
//!
//! - [`validate_metamodel`]: `BaseModelManager.validateAst`'s structural
//!   check, the instance validator ([`super::from_json`]) run with
//!   accordproject/concerto#1273's `STRICT_VALIDATE_OPTIONS`; [`validate_ast`]
//!   adds the version check in front of it. `ModelManager::validate_ast` runs
//!   the same check over a caller's own manager (`metamodelValidation`).
//! - [`validate_meta_model_instance`] and [`model_manager_from_meta_model`]:
//!   TS `validateMetaModel` and `modelManagerFromMetaModel`
//!   (src/introspect/metamodel.ts).
//! - [`check_ast_shape`]: the strict AST shape check at model load (BC-19).
//!
//! Every check runs on one resident metamodel manager per thread
//! ([`with_resident_metamodel_manager`]).

use std::cell::RefCell;

use crate::json::Value;

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

/// The fully-qualified name of the metamodel declaration `$short`, as a
/// `&'static str` literal built at compile time: `metamodel_class!("MapDeclaration")`
/// is `"concerto.metamodel@1.0.0.MapDeclaration"`. Only the decorator command
/// sets (`dcs`, the `js-compat` feature) use it.
#[cfg(feature = "js-compat")]
macro_rules! metamodel_class {
    ($short:literal) => {
        concat!("concerto.metamodel@1.0.0.", $short)
    };
}
#[cfg(feature = "js-compat")]
pub(crate) use metamodel_class;

/// The metamodel's own AST, `MetaModelUtil.metaModelAst`: the vendored
/// `concerto-core/src/metamodel.json`, identical to
/// `concerto-metamodel/vendor/concerto.metamodel@1.0.0.json`. Read only by
/// [`metamodel_model_file`].
const METAMODEL_AST_JSON: &str = include_str!("../metamodel.json");

/// A fresh [`ModelManager`] holding the system models and the metamodel file
/// ([`metamodel_model_file`], shared), validated: the manager
/// [`with_resident_metamodel_manager`] keeps per thread. It gives the same
/// answer as TS's `validateAst`, which adds the metamodel only for the
/// duration of the check.
fn metamodel_model_manager() -> Result<ModelManager> {
    let mut mm = ModelManager::new()?;
    mm.add_shared_model_file(metamodel_model_file()?)?;
    mm.validate_models()?;
    Ok(mm)
}

/// Runs `f` on the crate's one resident, per-thread metamodel manager, built
/// on first use and kept warm. It is only ever read, so every call gets the
/// result a fresh manager would; a build error is returned, not cached. Used
/// by metamodel validation, `validate_ast_value`, the DCS validation manager
/// and concerto-wasm's `validateMetaModelInstance` (hence `pub` under
/// `js-compat`).
pub fn with_resident_metamodel_manager<R>(
    f: impl FnOnce(&ModelManager) -> Result<R>,
) -> Result<R> {
    thread_local! {
        static RESIDENT: RefCell<Option<ModelManager>> =
            const { RefCell::new(None) };
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

/// The `Serializer.fromJSON` options a metamodel instance is checked with,
/// one per caller of [`with_resident_metamodel_manager`], so the
/// `validateMetaModelInstance` binding can take the preset as an argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaModelPreset {
    /// accordproject/concerto#1273's `STRICT_VALIDATE_OPTIONS`:
    /// `validateAst`'s check ([`validate_metamodel`]).
    Strict,
    /// The manager's serializer defaults, `baseDefaultOptions`
    /// (`{validate: true, utcOffset}`): `validateAst`'s check over the
    /// caller's own manager (`deserialize_ast`).
    Default,
    /// A `new Serializer(factory, modelManager)`'s defaults:
    /// `validateMetaModel`'s check ([`validate_meta_model_instance`]).
    /// The same options as [`Self::Default`], named apart for its caller.
    #[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
    Serializer,
}

impl MetaModelPreset {
    /// The options this preset reads as. `utcOffset` is `0` in each: the
    /// metamodel declares no `DateTime` property, so no offset can change a
    /// metamodel document's outcome.
    pub fn from_json_options(self) -> FromJsonOptions {
        match self {
            Self::Strict => FromJsonOptions {
                reject_unknown_keys: true,
                reject_required_null: true,
                ..FromJsonOptions::default()
            },
            Self::Default | Self::Serializer => FromJsonOptions::default(),
        }
    }
}

/// The text a TS `catch (err)` would see on `err.message`: the exception's
/// own, already-constructed message (`ContractError::final_message` for a
/// catalogue error).
fn ts_message(err: &Error) -> String {
    err.contract().final_message()
}

/// `BaseModelManager.validateAst`'s structural check:
/// `this.getSerializer().fromJSON(modelFile.getAst())`, run with
/// accordproject/concerto#1273's `STRICT_VALIDATE_OPTIONS`, so an unknown
/// property or a required property set to `null` is rejected too. Any
/// failure (no `$class`, an unresolvable type, a structural mismatch) is
/// re-thrown as `MetamodelException(error.message)`, as TS's `catch` does.
pub fn validate_metamodel(ast: &Value) -> Result<()> {
    let options = MetaModelPreset::Strict.from_json_options();
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

/// `BaseModelManager`'s `this.metamodelModelFile`: `new ModelFile(this,
/// MetaModelUtil.metaModelAst, undefined, MetaModelNamespace)`, so its file
/// name is the namespace and it has no CTO definitions. Loaded once per
/// thread and shared (an `Arc`); a load error is returned, not cached.
pub(crate) fn metamodel_model_file() -> Result<std::sync::Arc<ModelFile>> {
    thread_local! {
        static METAMODEL_MODEL_FILE: RefCell<Option<std::sync::Arc<ModelFile>>> =
            const { RefCell::new(None) };
    }
    if let Some(model_file) = METAMODEL_MODEL_FILE.with(|cache| cache.borrow().clone()) {
        return Ok(model_file);
    }
    let metamodel: Value =
        serde_json::from_str(METAMODEL_AST_JSON).expect("the vendored metamodel AST is JSON");
    let model_file = std::sync::Arc::new(ModelFile::from_owned_json_with_definitions(
        metamodel,
        None,
        Some(METAMODEL_NAMESPACE.to_string()),
    )?);
    METAMODEL_MODEL_FILE.with(|cache| *cache.borrow_mut() = Some(std::sync::Arc::clone(&model_file)));
    Ok(model_file)
}

/// `validateAst`'s structural check against the caller's own manager `mm`
/// (which must already hold the metamodel), with the manager's serializer
/// defaults (`{validate: true, utcOffset}`), not the strict preset:
/// concerto-core 5.0.0 passes no options here. Any failure is re-thrown as
/// `MetamodelException(error.message)`. No other serializer option the
/// manager could carry changes a metamodel document's outcome.
pub(crate) fn deserialize_ast(mm: &ModelManager, ast: &Value) -> Result<()> {
    from_json(
        mm,
        ast,
        &MetaModelPreset::Default.from_json_options(),
        &mut FixedEnv,
    )
    .map(|_resource| ())
    .map_err(|err| wrapped(&err))
}

/// `BaseModelManager.validateAst(modelFile)` over the AST itself: the version
/// check (`check_version`), then the structural check
/// ([`validate_metamodel`]). A missing or non-string `$class` fails the
/// version check, as in TS.
pub fn validate_ast(ast: &Value) -> Result<()> {
    check_version(ast)?;
    validate_metamodel(ast)
}

/// TS `validateMetaModel(input)` (src/introspect/metamodel.ts):
/// `serializer.fromJSON(input)` over `newMetaModelManager()` with a default
/// `Serializer`. TS returns `input` unchanged.
///
/// Unlike [`validate_metamodel`], this runs the default options, and a
/// failure is the serializer's own error, unwrapped, as TS throws it. It runs
/// on the resident metamodel manager, which holds the same models.
#[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
pub fn validate_meta_model_instance(input: &Value) -> Result<()> {
    let options = MetaModelPreset::Serializer.from_json_options();
    with_resident_metamodel_manager(|mm| {
        from_json(mm, input, &options, &mut FixedEnv).map(|_resource| ())
    })
}

/// TS `modelManagerFromMetaModel(metaModel, validate = true)`: with
/// `validate`, [`validate_meta_model_instance`] first; then a fresh
/// [`ModelManager`] gets each model in order, as `new ModelFile` (with
/// [`check_ast_shape`], BC-19) and a validating `addModelFile`, then
/// `validateModelFiles()`. A non-array `models` is V8's `TypeError`, as in TS.
#[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
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
        // BC-19: `new ModelFile(modelManager, mm, null, null)` on a `new
        // ModelManager()`, whose default is the strict shape check, after
        // the constructor's own argument checks (a falsy or non-object AST
        // is a plain `Error`).
        ModelFile::check_constructor_arguments(Some(model), None, None)?;
        check_ast_shape(model)?;
        let model_file = ModelFile::from_json_with_definitions(model, None, None)?;
        if mm.model_file(model_file.namespace()).is_none() {
            mm.validate_detached_model_file(&model_file)?;
        }
        mm.add_model_file(model_file)?;
    }
    mm.validate_models()?;
    Ok(mm)
}

/// The strict AST shape check at model load (BC-19, with BC-17 and BC-20):
/// TS `new ModelFile(modelManager, ast)` runs it after its own argument
/// checks, unless the manager was built with `metamodelValidation: false`.
/// So every load path rejects an AST that does not have the metamodel's
/// shape with an `IllegalModelException`, before any part of it is walked.
/// TS 5.0.0 loads many such ASTs, or crashes on them (BC-18).
///
/// In order, stopping at the first problem:
///
/// 1. BC-17 and BC-20, over every node in document order: a `decorators`
///    that is present, not `null` and not an array
///    (`modelfile-load-decoratorsnotarray`); a `superType` whose `name` is
///    not a non-empty string (`modelfile-load-supertypename`); any other
///    `name` that is not a string (`modelfile-load-namenotstring`); an
///    `identified`, `sizeValidator`, `lengthValidator` or `validator` that is
///    present, not `null` and not an object with a string `$class`
///    (`modelfile-load-nodenotobject`).
/// 2. `validateAst`'s strict check ([`validate_ast`]), its error re-thrown
///    as an `IllegalModelException` after a fixed prefix
///    (`modelfile-load-astshape`), but for its metamodel version check's
///    `MetamodelException`, thrown as it is, as TS 5.0.0's `validateAst`
///    throws it (`basemodelmanager-validateast-versionmismatch`).
///
/// **Two tolerances.** concerto-cto 5.0.0 writes a string `defaultValue` on
/// a `DateTimeProperty`, which the metamodel does not declare; so that such
/// a model still loads, that value is left out of step 2. And
/// `ModelFile.filter` builds the filtered file from its declarations'
/// ASTs, as TS 5.0.0 does, which carry the default super type of an asset,
/// participant, transaction or event with the `$class` `TypeIdentified`
/// (not in the metamodel); so that the filtered file loads, step 2 checks
/// that super type as a `TypeIdentifier`.
///
/// The check reads only `ast`. It is folded into the typed read every load
/// runs (`introspect::shape`): only an AST that read cannot vouch for is
/// checked by the steps above as written (`check_ast_shape_exact`), so the
/// verdict and the error are always theirs.
#[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
pub fn check_ast_shape(ast: &Value) -> Result<()> {
    if crate::introspect::shape::ast_conforms(ast) {
        return Ok(());
    }
    check_ast_shape_exact(ast)
}

/// [`check_ast_shape`]'s steps as written, over the whole `Value`: the node
/// rules (step 1), then `validateAst`'s strict check (step 2). The verdict
/// and the error of every AST the typed read cannot vouch for.
pub(crate) fn check_ast_shape_exact(ast: &Value) -> Result<()> {
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
        // R2F-5: a metamodel version mismatch stays TS 5.0.0's
        // `MetamodelException`.
        if err.code() == "basemodelmanager-validateast-versionmismatch" {
            return err;
        }
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
fn has_parser_default(map: &crate::json::Map<String, Value>) -> bool {
    map.get("$class").and_then(Value::as_str) == Some(DATE_TIME_PROPERTY)
        && map.get("defaultValue").is_some_and(Value::is_string)
}

/// Whether `map` is a declaration whose `superType` is exactly the default
/// one TS 5.0.0 gives an asset, participant, transaction or event view
/// (`ModelFile._declarationView`, with the `$class` `TypeIdentified`
/// written there), which `ModelFile.filter` copies into the filtered file:
/// [`check_ast_shape`] checks it as a `TypeIdentifier`.
fn has_default_super_type(map: &crate::json::Map<String, Value>) -> bool {
    let Some(super_type) = map.get("superType").and_then(Value::as_object) else {
        return false;
    };
    let Some(name) = map
        .get("$class")
        .and_then(Value::as_str)
        .and_then(|class| {
            crate::introspect::model_file::default_super_type(&crate::json!({ "$class": class }))
        })
    else {
        return false;
    };
    super_type.len() == 2
        && super_type.get("$class").and_then(Value::as_str)
            == Some(crate::introspect::model_file::DEFAULT_SUPER_TYPE_CLASS)
        && super_type.get("name").and_then(Value::as_str) == Some(name)
}

/// Removes every value [`has_parser_default`] matches from `node`, and
/// gives every super type [`has_default_super_type`] matches the
/// metamodel's `TypeIdentifier` class.
fn strip_parser_extras(node: &mut Value) {
    match node {
        Value::Object(map) => {
            if has_parser_default(map) {
                map.remove("defaultValue");
            }
            if has_default_super_type(map) {
                map["superType"]["$class"] =
                    Value::String("concerto.metamodel@1.0.0.TypeIdentifier".to_string());
            }
            map.values_mut().for_each(strip_parser_extras);
        }
        Value::Array(items) => items.iter_mut().for_each(strip_parser_extras),
        _ => {}
    }
}

/// The keys whose value, when present and not `null`, must be an object
/// with a string `$class` ([`check_ast_shape`], step 1); the metamodel check
/// alone accepts a value with no own keys there, or an object without a
/// `$class`.
const NODE_KEYS: [&str; 4] = ["identified", "sizeValidator", "lengthValidator", "validator"];

/// [`check_ast_shape`]'s first step, for `node` and everything under it.
/// `super_type` is true for the node under a `superType` key;
/// `parser_extras` is set when a node matches [`has_parser_default`].
fn check_node_shapes(node: &Value, super_type: bool, parser_extras: &mut bool) -> Result<()> {
    match node {
        Value::Object(map) => {
            *parser_extras |= has_parser_default(map) || has_default_super_type(map);
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
            for key in NODE_KEYS {
                if let Some(value) = map.get(key)
                    && !value.is_null()
                    && !value.get("$class").is_some_and(Value::is_string)
                {
                    return Err(ContractError::new(
                        ErrorKind::IllegalModel,
                        "modelfile-load-nodenotobject",
                        vec![("key", key.to_string()), ("value", value.to_string())],
                    )
                    .into());
                }
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
/// The `ModelFile` constructor does not require a `$class`, so any JSON
/// value can reach `ModelUtil.getNamespace(fqn)`: a falsy one throws
/// "FQN is invalid." (`modelutil-getnamespace-nofnq`); an array has
/// namespace `''` without a `"."` element (`parseNamespace` throws) and is a
/// V8 `TypeError` with one; any other non-string is a V8 `TypeError`. Each
/// fails before the metamodel is added, as in TS.
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
/// namespace gives `None` rather than `parse_namespace`'s error (BC-02): it
/// is still rejected, by the caller's version mismatch, with the
/// `MetamodelException` TS 5.0.0 threw for it.
fn namespace_version(ns: &str) -> Result<Option<String>> {
    Ok(model_util::split_namespace(ns)?.1.map(str::to_string))
}

#[cfg(test)]
mod tests;
