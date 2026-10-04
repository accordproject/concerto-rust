//! The DecoratorManager, DCS converter and extractor bindings.
//!
//! Split out of `lib.rs`; the crate root glob-imports it.

use super::*;

// ---------------------------------------------------------------------------
// DecoratorManager, DCS converter and extractor (src/decoratormanager.ts,
// src/decoratorextractor.ts)
// ---------------------------------------------------------------------------
//
// The operations run on the source ModelManager's own handle
// (`dcsDecorateModels`, `dcsExtract`, `dcsValidate`) or on a resident
// `DcsManagerHandle` built from the model ASTs a view reads with `getAst`.
// The result is staged into the new ModelManager's handle. `dcsconverter.ts`
// (YAML conversion, no model semantics) stays in TS.

/// `new ModelManager()` (`src/modelmanager.ts`), then the ASTs of the
/// `models` array (anything but an array loads nothing) added the way
/// `fromAst` does, each moved into its model file
/// ([`ModelManager::add_owned_model_with_definitions`]) rather than
/// copied.
pub(crate) fn model_manager_from_owned_asts(models: Value) -> Result<ModelManager> {
    let mut mm = ModelManager::new()?;
    if let Value::Array(models) = models {
        for model in models {
            mm.add_owned_model_with_definitions(model, None, None)?;
        }
    }
    Ok(mm)
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

/// The `{ "$class", "models" }` AST of a manager's own models (the system
/// ones included, in load order), serialised straight from its model ASTs,
/// without cloning them into a new `Value`.
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
// A resident DCS manager with staged-handle results.
//
// [`DcsManagerHandle`] keeps the input manager resident. Each operation
// stages the result's model files into the new ModelManager's handle
// (`target`) and returns the result AST, each file's stage id and header
// ([`stage_shared`], flat layout) and whether the result was validated, so
// the view neither sends the AST again nor re-runs `validateModelFiles`.
// ---------------------------------------------------------------------------

/// TS `EXCLUDE_NS` (src/basemodelmanager.ts): the system namespaces
/// `fromAst` skips, since the new manager already has them.
pub(crate) const DCS_EXCLUDE_NS: [&str; 3] =
    ["concerto@1.0.0", "concerto", "concerto.decorator@1.0.0"];

/// Stages each file of a DecoratorManager result, shared and with its
/// header, into `target`'s staging slot: one entry per file in load order,
/// `null` for a system file `fromAst` skips ([`DCS_EXCLUDE_NS`]), else a
/// [`FlatStaged`]. The one helper [`stage_result`] and
/// [`dcs_memo::DcsExtractKept::stage`] stage through; it never moves
/// `target`'s epoch, and evicts as [`StagedModelFiles::insert_shared`] does.
pub(crate) fn stage_shared<'h, 'a: 'h>(
    target: &mut ModelManagerHandle,
    files: impl Iterator<Item = (&'h Arc<ModelFile>, Option<&'h StagedHeader<'a>>)>,
) -> Vec<Value> {
    files
        .map(|(mf, header)| {
            if DCS_EXCLUDE_NS.contains(&mf.namespace()) {
                return Value::Null;
            }
            let id = target.staged.insert_shared(Arc::clone(mf));
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

/// The input manager of the `DecoratorManager` operations, for a source
/// ModelManager whose own handle cannot stand for it: the source models
/// (`modelManager.getAst(resolve, false).models`) loaded once
/// (`model_manager_from_owned_asts`). The view builds one per operation
/// and frees it, so `decorateModels` setting its `decoratorValidation` to
/// `target`'s is harmless.
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

    /// TS: `DecoratorManager.decorateModels` on the resident manager, staging
    /// the result into `target`. Returns `{ast, staged, validated}`. The result
    /// is validated with `target`'s `decoratorValidation`, as TS's
    /// `new ModelManager({decoratorValidation: ...}).fromAst(...)` does;
    /// otherwise the view, which trusts `validated`, would skip those checks.
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

    /// The three extract operations as one binding, `action` selecting
    /// which (`extract_action`: 0 `extractDecorators`, 1
    /// `extractVocabularies`, 2 `extractNonVocabDecorators`), the result
    /// staged into `target`.
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
    /// TS: `DecoratorManager.validate`'s structural check, on this handle's
    /// manager: the view calls it on the `validationModelManager` it has just
    /// built, once its rustHandle mirrors the files, so nothing is sent or
    /// rebuilt. [`dcs::validate_against`] throws what [`dcs::validate`] throws
    /// at the same step. Never changes the manager.
    #[wasm_bindgen(js_name = dcsValidate)]
    pub fn dcs_validate(&self, decorator_command_set: JsValue) -> JsResult<()> {
        run(|| {
            let command_set = to_json(&decorator_command_set)?.unwrap_or(Value::Null);
            dcs::validate_against(&self.manager, &command_set)?;
            Ok(())
        })
    }
}

/// The extract operation the `action` argument of `dcsExtract`/`extract`
/// names: 0 `ExtractAll` (`extractDecorators`), 1 `ExtractVocab`
/// (`extractVocabularies`), 2 `ExtractNonVocab`
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
    // Extract's writer: `{ast, staged, validated}` is written as text
    // straight from the result's model ASTs ([`ModelManagerAstView`]) and
    // parsed once. The ASTs are not compacted as extract's are: a decorated
    // manager is usually read again, and re-parsing compacted ASTs in WASM
    // costs more than compaction saves.
    let staged = stage_result(target, &decorated);
    decorate_result_js(&decorated, &staged, validated)
}

/// A JS array argument's elements, moved out of its `Value` rather than
/// copied: anything but an array (`undefined` included) gives none, as
/// `as_array().cloned().unwrap_or_default()` read it.
pub(crate) fn owned_array(value: Option<Value>) -> Vec<Value> {
    match value {
        Some(Value::Array(items)) => items,
        _ => Vec::new(),
    }
}

/// The JSON text of `{ast, staged, validated}` for a decorate result,
/// written without copying any AST.
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

/// The JS value of a decorate result: [`decorate_result_text`], parsed.
/// The text is written from values serde built, so neither step fails.
pub(crate) fn decorate_result_js(
    decorated: &ModelManager,
    staged: &[Value],
    validated: bool,
) -> Result<JsValue> {
    let text = decorate_result_text(decorated, staged, validated).map_err(internal)?;
    JSON::parse(&text).map_err(Error::Js)
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
    // Staged shared, with the result's ASTs kept as text.
    Ok(compacted_extract_js(target, result)?.0)
}

// ---------------------------------------------------------------------------
// The DecoratorManager operations on the source ModelManager's own
// rustHandle, which already mirrors the source's model files. Running them
// on the handle's own manager skips the `getAst` copy and load a cold
// [`DcsManagerHandle`] costs; results are staged into `target` the same way.
// ---------------------------------------------------------------------------

#[wasm_bindgen]
impl ModelManagerHandle {
    /// [`DcsManagerHandle::decorate_models`] on this handle's own manager.
    /// `target` is the new ModelManager's handle, never this one. The result
    /// is validated with `target`'s `decoratorValidation`, as
    /// [`DcsManagerHandle::decorate_models`] validates it: this manager's
    /// own option is set to it for the call and restored after, so the call
    /// never changes this manager (nor its epoch).
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

    /// [`DcsManagerHandle::extract_with_action`] on this handle's own
    /// manager, through the per-epoch memo.
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
