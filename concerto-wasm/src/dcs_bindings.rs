//! The DecoratorManager, DCS converter and extractor bindings (P4-09).
//!
//! Split out of `lib.rs` (P5-104, review M7); the crate root glob-imports it.

use super::*;

// ---------------------------------------------------------------------------
// DecoratorManager, DCS converter and extractor (src/decoratormanager.ts,
// src/decoratorextractor.ts) — P4-09
// ---------------------------------------------------------------------------
//
// The DecoratorManager operations run on the source ModelManager's own
// handle (`dcsDecorateModels`, `dcsExtract`, `dcsValidate`) or on a resident
// `DcsManagerHandle` built from the model ASTs a view reads off its
// `ModelManager` with `getAst` (P5-27, P5-55); P5-103 removed the per-call
// bindings that built a throwaway native `ModelManager` on every call. The
// result is staged into the new ModelManager's handle. `dcsconverter.ts`
// stays out of this: the seam ledger classifies every one of its members TS
// ("YAML (de)serialisation via the `yaml` npm lib ... no model semantics"),
// so `DecoratorManager.jsonToYaml`/`yamlToJson` only need `validate` below —
// the YAML conversion itself is unchanged TS on both sides of the view.

/// `new ModelManager()` (`src/modelmanager.ts`), then the ASTs of the
/// `models` array (anything but an array loads nothing) added the way
/// `fromAst` does, each moved into its model file
/// ([`ModelManager::add_owned_model_with_definitions`]) rather than copied
/// (P5-40, F-B).
pub(crate) fn model_manager_from_owned_asts(models: Value) -> Result<ModelManager> {
    let mut mm = ModelManager::new()?;
    if let Value::Array(models) = models {
        for model in models {
            mm.add_owned_model_with_definitions(model, None, None)?;
        }
    }
    Ok(mm)
}

/// A native `ModelManager`'s own models (the system ones included, in load
/// order) as `{ $class, models }` — the shape
/// `BaseModelManager.getAst`/`fromAst` (`src/basemodelmanager.ts`) use. The
/// view's own `fromAst` filters the system ones back out (`EXCLUDE_NS`)
/// exactly as it already does for the ts-mode `decorateModels`/`extract*`
/// bodies, so this need not filter them here.
pub(crate) fn model_manager_to_ast(mm: &ModelManager) -> Value {
    let models: Vec<Value> = mm.model_files().map(|mf| mf.ast().clone()).collect();
    json!({
        "$class": "concerto.metamodel@1.0.0.Models",
        "models": models,
    })
}

/// An `Option<bool>` the way [`dcs::DecorateOptions`]' `disable_*` fields
/// read a JS option: `Some(b)` only for a literal JS boolean, `None` for
/// anything else (absent, `null`, `undefined`, or a non-boolean value),
/// matching TS's `=== false`/truthy-assignment use of the same fields.
pub(crate) fn opt_bool(options: &Value, key: &str) -> Option<bool> {
    match options.get(key) {
        Some(Value::Bool(b)) => Some(*b),
        _ => None,
    }
}

/// [`dcs::DecorateOptions`] from `DecoratorManager.decorateModels`'s
/// `options` object.
pub(crate) fn decorate_options_from_js(options: &Value) -> dcs::DecorateOptions {
    dcs::DecorateOptions {
        migrate: options
            .get("migrate")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        validate: options
            .get("validate")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        validate_commands: options
            .get("validateCommands")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        default_namespace: options
            .get("defaultNamespace")
            .cloned()
            .filter(|v| !v.is_null()),
        skip_validation_and_resolution: options
            .get("skipValidationAndResolution")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        disable_metamodel_resolution: opt_bool(options, "disableMetamodelResolution"),
        disable_metamodel_validation: opt_bool(options, "disableMetamodelValidation"),
    }
}

/// [`dcs::ExtractOptions`] from `DecoratorManager.extractDecorators`'s (and
/// its `extractVocabularies`/`extractNonVocabDecorators` siblings') `options`
/// object; TS defaults `removeDecoratorsFromModel` to `false` and `locale`
/// to `'en'` the same way before either ever reads it.
pub(crate) fn extract_options_from_js(options: &Value) -> dcs::ExtractOptions {
    dcs::ExtractOptions {
        remove_decorators_from_model: options
            .get("removeDecoratorsFromModel")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        locale: options
            .get("locale")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| "en".to_string()),
    }
}

/// P5-41 (F-C, accordproject/concerto-rust#351): the `{ "$class", "models" }`
/// AST [`model_manager_to_ast`] builds, serialised straight from the
/// manager's own model ASTs, without cloning them into a new `Value`.
pub(crate) struct ModelManagerAstView<'a>(&'a ModelManager);

impl serde::Serialize for ModelManagerAstView<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(2))?;
        map.serialize_entry("$class", "concerto.metamodel@1.0.0.Models")?;
        map.serialize_entry("models", &ModelAstsView(self.0))?;
        map.end()
    }
}

/// The `models` array of [`ModelManagerAstView`].
pub(crate) struct ModelAstsView<'a>(&'a ModelManager);

impl serde::Serialize for ModelAstsView<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        s.collect_seq(self.0.model_files().map(ModelFile::ast))
    }
}

/// TS: `DecoratorManager.falsyOrEqual`. `values` is always a plain string
/// array (every call site passes one).
#[wasm_bindgen(js_name = decoratorManagerFalsyOrEqual)]
pub fn decorator_manager_falsy_or_equal(test: JsValue, values: JsValue) -> JsResult<bool> {
    run(|| {
        let test_json = to_json(&test)?;
        let values_json = to_json(&values)?.unwrap_or(Value::Array(Vec::new()));
        let values_vec: Vec<String> = values_json
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let values_refs: Vec<&str> = values_vec.iter().map(String::as_str).collect();
        Ok(dcs::falsy_or_equal(test_json.as_ref(), &values_refs))
    })
}

/// TS: `DecoratorManager.migrateTo` (the unused `version` parameter is
/// dropped, as [`dcs::migrate_to`]'s doc comment explains). Mutates a clone
/// of `decorator_command_set` and returns it; the view assigns the result
/// back onto its own variable exactly as the TS body's `return
/// decoratorCommandSet` does.
#[wasm_bindgen(js_name = decoratorManagerMigrateTo)]
pub fn decorator_manager_migrate_to(decorator_command_set: JsValue) -> JsResult<JsValue> {
    run(|| {
        let mut value = to_json(&decorator_command_set)?.unwrap_or(Value::Null);
        dcs::migrate_to(&mut value)?;
        Ok(to_js(&value))
    })
}

/// TS: `DecoratorManager.executePropertyCommand`, which mutates `property`
/// in place and returns nothing; the view copies the mutated fields this
/// returns back onto its own `property` object.
#[wasm_bindgen(js_name = decoratorManagerExecutePropertyCommand)]
pub fn decorator_manager_execute_property_command(
    property: JsValue,
    command: JsValue,
) -> JsResult<JsValue> {
    run(|| {
        let mut prop = to_json(&property)?.unwrap_or(Value::Null);
        let cmd = to_json(&command)?.unwrap_or(Value::Null);
        dcs::execute_property_command(&mut prop, &cmd)?;
        Ok(to_js(&prop))
    })
}

// ---------------------------------------------------------------------------
// P5-27 (F6, accordproject/concerto-rust#332): a resident DCS manager with
// staged-handle results. Additive: the `decoratorManager*` bindings above
// are unchanged.
//
// The bindings above rebuild the input manager from the source models' AST
// on every call, and hand back the result models as AST only. The view then
// loads that AST into a new ModelManager (`fromAst`), which sends every
// model back into Rust (`stageModelFile`), reads each header across the
// boundary (`modelFileFromAstHeader`) and validates the whole set again
// (`validateModelFiles`), although Rust has just loaded and validated those
// same models.
//
// [`DcsManagerHandle`] keeps the input manager resident, so the view builds
// it once per source ModelManager and reuses it while that manager's epoch
// and model files are unchanged (engine/views.ts `dcsManagerFor`). Each of
// its operations stages the result's model files into the new
// ModelManager's own handle (`target`) and returns, with the result AST,
// each file's stage id and header ([`stage_shared`], in the flat layout since P5-101), and whether the
// result was validated. The view builds each ModelFile from its stage id and
// header without sending the AST again, and skips its own
// `validateModelFiles` when Rust has already validated the same files.
// ---------------------------------------------------------------------------

/// TS `EXCLUDE_NS` (src/basemodelmanager.ts): the system namespaces
/// `fromAst` skips, since the new manager already has them.
pub(crate) const DCS_EXCLUDE_NS: [&str; 3] =
    ["concerto@1.0.0", "concerto", "concerto.decorator@1.0.0"];

/// P5-101 (D-4, D-13; accordproject/concerto-rust#455): stages each of a
/// DecoratorManager result's model files, with its header, into `target`'s
/// staging slot, and returns one entry per file, in
/// [`model_manager_to_ast`]'s order: `null` for a system file `fromAst`
/// skips ([`DCS_EXCLUDE_NS`]), otherwise the stage in the flat layout every
/// staging path returns ([`FlatStaged`]). The one helper both
/// [`stage_result`] and [`dcs_memo::DcsExtractKept::stage`] stage through. Staging
/// never changes `target`'s manager or epoch, and has one capacity policy:
/// past [`StagedModelFiles::CAPACITY`] the oldest stage is evicted
/// ([`StagedModelFiles::insert_shared`]), and its file falls back to
/// sending its AST, as any evicted stage does.
///
/// P5-77 (accordproject/concerto-rust#419): each file is staged shared with
/// the result (`ModelManager::shared_model_files`), not deep-copied.
pub(crate) fn stage_shared<'h, 'a: 'h>(
    target: &mut ModelManagerHandle,
    files: impl Iterator<Item = (&'h std::sync::Arc<ModelFile>, Option<&'h StagedHeader<'a>>)>,
) -> Vec<Value> {
    files
        .map(|(mf, header)| {
            if DCS_EXCLUDE_NS.contains(&mf.namespace()) {
                return Value::Null;
            }
            let id = target.staged.insert_shared(std::sync::Arc::clone(mf));
            serde_json::to_value(FlatStaged { id, header }).unwrap_or(Value::Null)
        })
        .collect()
}

/// [`stage_shared`] for a result the caller drops (or keeps unchanged)
/// once the call returns, each header read from its file's AST
/// ([`staged_header_from_parts`]).
pub(crate) fn stage_result(target: &mut ModelManagerHandle, result: &ModelManager) -> Vec<Value> {
    let headers: Vec<Option<StagedHeader<'_>>> = result
        .model_files()
        .map(|mf| staged_header_from_parts(mf.namespace(), mf.ast().get("imports")))
        .collect();
    stage_shared(
        target,
        result
            .shared_model_files()
            .zip(headers.iter().map(Option::as_ref)),
    )
}

/// The input manager of the `DecoratorManager` operations (P5-27, F6), for
/// a source ModelManager whose own handle cannot stand for it: the source
/// models, as the view reads them off `modelManager.getAst(resolve,
/// false).models`, loaded once ([`model_manager_from_owned_asts`]). The
/// view builds one per operation and frees it (P5-103 removed the copy it
/// kept per source manager, which only a manager the source handle serves
/// could use). The operations never change it.
#[wasm_bindgen]
pub struct DcsManagerHandle {
    manager: ModelManager,
}

#[wasm_bindgen]
impl DcsManagerHandle {
    /// Loads `models` (a JSON array of model ASTs, none of them the system
    /// ones), throwing what `new ModelManager().fromAst` throws for them.
    #[wasm_bindgen(constructor)]
    pub fn new(models: JsValue) -> JsResult<DcsManagerHandle> {
        run(|| {
            let models_json = to_json(&models)?.unwrap_or(Value::Array(Vec::new()));
            let manager = model_manager_from_owned_asts(models_json)?;
            Ok(Self { manager })
        })
    }

    /// TS: `DecoratorManager.decorateModels` on the resident manager, with
    /// the result staged into `target` (the new ModelManager's handle, as
    /// the view's `clearModelFiles` left it). Returns `{ast, staged,
    /// validated}`: `ast` is the decorated models' AST, `staged` is
    /// [`stage_result`]'s entries for `ast.models`, and `validated` is
    /// whether the result manager was validated (every model but the system
    /// ones).
    ///
    /// P5-54 (accordproject/concerto-rust#375): the result is validated
    /// with `target`'s `decoratorValidation`, as TS validates it in
    /// `new ModelManager({decoratorValidation: modelManager
    /// .getDecoratorValidation()}).fromAst(…)`: the view builds `target`
    /// with the source manager's option, and [`dcs::decorate_models`] gives
    /// its result the input manager's, so the resident manager takes
    /// `target`'s before it runs. A fresh resident manager has the default
    /// (disabled) option, so without this the view, which trusts
    /// `validated`, skipped the decorator checks.
    #[wasm_bindgen(js_name = decorateModels)]
    pub fn decorate_models(
        &mut self,
        target: &mut ModelManagerHandle,
        decorator_command_sets: JsValue,
        options: JsValue,
    ) -> JsResult<JsValue> {
        self.manager
            .set_decorator_validation(target.manager.decorator_validation().clone());
        run(|| staged_decorate_models(&self.manager, target, &decorator_command_sets, &options))
    }

    /// P5-101 (D-10, accordproject/concerto-rust#455): the three extract
    /// operations as one binding, `action` selecting which
    /// ([`extract_action`]: 0 `extractDecorators`, 1 `extractVocabularies`,
    /// 2 `extractNonVocabDecorators`), the result staged into `target`.
    /// P5-103 removed the three per-action bindings.
    #[wasm_bindgen(js_name = extract)]
    pub fn extract_with_action(
        &self,
        target: &mut ModelManagerHandle,
        options: JsValue,
        action: u32,
    ) -> JsResult<JsValue> {
        let action = run(|| extract_action(action))?;
        self.extract(target, &options, action)
    }
}

#[wasm_bindgen]
impl ModelManagerHandle {
    /// TS: `DecoratorManager.validate`'s structural check
    /// (`serializer.fromJSON(decoratorCommandSet)`), against this handle's
    /// own resident manager (P5-27, F6). The view calls it on the
    /// `validationModelManager` it has just built and returns (the
    /// metamodel, the caller's model files and the DCS model), once that
    /// manager's rustHandle mirrors its model files; so it neither sends
    /// the model files again nor rebuilds a manager from them, and
    /// [`dcs::validate_against`] throws what [`dcs::validate`] throws at the
    /// same step. Never changes the manager.
    #[wasm_bindgen(js_name = dcsValidate)]
    pub fn dcs_validate(&self, decorator_command_set: JsValue) -> JsResult<()> {
        run(|| {
            let command_set = to_json(&decorator_command_set)?.unwrap_or(Value::Null);
            dcs::validate_against(&self.manager, &command_set)?;
            Ok(())
        })
    }
}

/// P5-101 (D-10): the extract operation the `action` argument of
/// `dcsExtract`/`extract` names: 0 `ExtractAll` (`extractDecorators`), 1
/// `ExtractVocab` (`extractVocabularies`), 2 `ExtractNonVocab`
/// (`extractNonVocabDecorators`). Any other number is a plain `Error`, which
/// the view never passes.
pub(crate) fn extract_action(action: u32) -> Result<dcs::extractor::Action> {
    match action {
        0 => Ok(dcs::extractor::Action::ExtractAll),
        1 => Ok(dcs::extractor::Action::ExtractVocab),
        2 => Ok(dcs::extractor::Action::ExtractNonVocab),
        other => Err(ContractError::pre_port(
            ErrorKind::InvalidArgument,
            format!("an unknown extract action {other}"),
            None,
        )
        .into()),
    }
}

impl DcsManagerHandle {
    /// One extract operation, staged into `target`.
    pub(crate) fn extract(
        &self,
        target: &mut ModelManagerHandle,
        options: &JsValue,
        action: dcs::extractor::Action,
    ) -> JsResult<JsValue> {
        run(|| staged_extract(&self.manager, target, options, action))
    }
}

/// [`DcsManagerHandle::decorate_models`]'s body, on `manager` (whose
/// `decoratorValidation` the caller has already set to `target`'s): the
/// result staged into `target`, and `{ast, staged, validated}` returned.
pub(crate) fn staged_decorate_models(
    manager: &ModelManager,
    target: &mut ModelManagerHandle,
    decorator_command_sets: &JsValue,
    options: &JsValue,
) -> Result<JsValue> {
    let mut sets = owned_array(to_json(decorator_command_sets)?);

    let options_json = to_json(options)?.unwrap_or_else(|| json!({}));
    let mut opts = decorate_options_from_js(&options_json);

    // `dcs::decorate_models` validates the result unless the command sets
    // are empty or `disable_metamodel_validation` (as the options stand
    // once `skip_validation_and_resolution` has set it) is `Some(true)`.
    let applied = !sets.is_empty();
    let decorated = dcs::decorate_models(manager, &mut sets, &mut opts)?;
    let validated = applied && opts.disable_metamodel_validation != Some(true);
    // P5-102 (D-5, C-3): extract's writer. `{ast, staged, validated}` is
    // written as text straight from the result's model ASTs
    // ([`ModelManagerAstView`]), then parsed once; the intermediate `Value`
    // (every AST deep-copied by `model_manager_to_ast`, then `to_js`) is
    // only the fallback. The result's ASTs are not compacted as extract's
    // are (P5-77): a decorated manager is usually read again (extracted
    // from, validated, serialised), and re-parsing every compacted AST in
    // WASM then cost far more than compaction saved (P5-102 measured the
    // `extract_cold` row 3.7x slower on the synthetic-large set).
    let staged = stage_result(target, &decorated);
    Ok(decorate_result_js(&decorated, staged, validated))
}

/// A JS array argument's elements, moved out of its `Value` rather than
/// copied (P5-102): anything but an array (`undefined` included) gives
/// none, as `as_array().cloned().unwrap_or_default()` read it.
pub(crate) fn owned_array(value: Option<Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    }
}

/// The JSON text of `{ast, staged, validated}` for a decorate result:
/// byte for byte `serde_json`'s text of [`staged_decorate_models`]'s former
/// intermediate `Value`, written without copying any AST.
pub(crate) fn decorate_result_text(
    decorated: &ModelManager,
    staged: &[Value],
    validated: bool,
) -> serde_json::Result<String> {
    let mut out = Vec::new();
    out.extend_from_slice(b"{\"ast\":");
    serde_json::to_writer(&mut out, &ModelManagerAstView(decorated))?;
    out.extend_from_slice(b",\"staged\":");
    serde_json::to_writer(&mut out, staged)?;
    out.extend_from_slice(if validated {
        b",\"validated\":true}"
    } else {
        b",\"validated\":false}"
    });
    String::from_utf8(out).map_err(serde::ser::Error::custom)
}

/// The JS value of a decorate result: [`decorate_result_text`], parsed, or
/// the intermediate-`Value` fallback, which gives the same value.
pub(crate) fn decorate_result_js(
    decorated: &ModelManager,
    staged: Vec<Value>,
    validated: bool,
) -> JsValue {
    if let Some(js) = decorate_result_text(decorated, &staged, validated)
        .ok()
        .and_then(|text| JSON::parse(&text).ok())
    {
        return js;
    }
    to_js(&json!({
        "ast": model_manager_to_ast(decorated),
        "staged": staged,
        "validated": validated,
    }))
}

/// [`DcsManagerHandle::extract`]'s body, on `manager`: one extract
/// operation, its result's model files staged into `target`.
pub(crate) fn staged_extract(
    manager: &ModelManager,
    target: &mut ModelManagerHandle,
    options: &JsValue,
    action: dcs::extractor::Action,
) -> Result<JsValue> {
    let options_json = to_json(options)?.unwrap_or_else(|| json!({}));
    let opts = extract_options_from_js(&options_json);
    let result = dcs::extract(manager, &opts, action, false)?;
    // P5-77: staged shared, with the result's ASTs kept as text.
    Ok(compacted_extract_js(target, result).0)
}

// ---------------------------------------------------------------------------
// P5-55 (T1, F-A1, accordproject/concerto-rust#376): the DecoratorManager
// operations on the source ModelManager's own rustHandle. Additive: the
// `decoratorManager*` bindings and [`DcsManagerHandle`] are unchanged and
// stay the view's fallbacks.
//
// The view's source ModelManager already mirrors its model files into its
// rustHandle (P4-08, P5-34), so the handle holds exactly the models a
// [`DcsManagerHandle`] would be built from: the same ASTs, loaded the same
// way, with the same system models. [`dcs::decorate_models`] and
// [`dcs::extract`] resolve those models themselves
// (`ModelManager::models_ast`), so running them on the handle's own manager
// skips the copy (`getAst`, then JsValue to `Value`, then the load) that a
// cold [`DcsManagerHandle`] costs. Each operation is
// [`DcsManagerHandle`]'s, staged into `target` the same way.
// ---------------------------------------------------------------------------

#[wasm_bindgen]
impl ModelManagerHandle {
    /// [`DcsManagerHandle::decorate_models`] on this handle's own manager.
    /// `target` is the new ModelManager's handle, never this one. The result
    /// is validated with `target`'s `decoratorValidation`, as
    /// [`DcsManagerHandle::decorate_models`] validates it (P5-54): this
    /// manager's own option is set to it for the call and restored after,
    /// so the call never changes this manager (nor its epoch).
    #[wasm_bindgen(js_name = dcsDecorateModels)]
    pub fn dcs_decorate_models(
        &mut self,
        target: &mut ModelManagerHandle,
        decorator_command_sets: JsValue,
        options: JsValue,
    ) -> JsResult<JsValue> {
        let own = self.manager.decorator_validation().clone();
        self.manager
            .set_decorator_validation(target.manager.decorator_validation().clone());
        let result = run(|| {
            staged_decorate_models(&self.manager, target, &decorator_command_sets, &options)
        });
        self.manager.set_decorator_validation(own);
        result
    }

    /// P5-101 (D-10): [`DcsManagerHandle::extract_with_action`] on this
    /// handle's own manager, through the per-epoch memo, as the three
    /// `dcsExtract*` bindings run it. Additive: those are unchanged.
    #[wasm_bindgen(js_name = dcsExtract)]
    pub fn dcs_extract(
        &self,
        target: &mut ModelManagerHandle,
        options: JsValue,
        action: u32,
    ) -> JsResult<JsValue> {
        run(|| self.memo_extract(target, &options, extract_action(action)?))
    }
}
