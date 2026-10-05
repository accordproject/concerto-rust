//! The handle API: [`ModelManagerHandle`], one object per `ModelManager`.

use super::*;

// ---------------------------------------------------------------------------
// The handle API: one object per ModelManager
// ---------------------------------------------------------------------------

/// A handle that names nothing in this manager, as the arena reports one
/// (`model_manager/mod.rs`, `unknown`): a `TypeNotFound` naming the node.
pub(crate) fn unknown(node: Node) -> Error {
    CoreError::type_not_found(format!("{node:?}")).into()
}

/// A snapshot as the JSON text that crosses the boundary: one string per
/// element, which the view parses once (cheaper than serde-wasm-bindgen or
/// per-field getters for a tree).
pub(crate) fn snapshot(value: &Value) -> Result<String> {
    serde_json::to_string(value).map_err(internal)
}

/// A `ModelManager`, exported to JS as one object. Model files,
/// declarations and properties cross as dense `u32` arena handles
/// (PORTING.md 1.4); state crosses as JSON snapshots a view caches until
/// the manager changes (PORTING.md 1.5; the TS side keys its caches on its
/// own `EngineState.version`). wasm-bindgen
/// registers a `FinalizationRegistry`; after `free()`, every call throws.
#[wasm_bindgen]
pub struct ModelManagerHandle {
    pub(crate) manager: ModelManager,
    /// The epoch ([`Self::epoch`]): **it moves iff `manager` may have changed**
    /// (a file added, replaced or removed, an option set, or a check that may
    /// leave a file registered) and never goes back. Staging, the extract memo
    /// and reads leave it alone. A binding that changes the manager calls
    /// [`Self::bump_epoch`]. It stamps the handle's own caches: the
    /// `validatePropertyById` slots ([`Self::validation_property_slot`]) and
    /// the extract memo. The TS views do not read it: they key their caches
    /// on their own `EngineState.version`.
    pub(crate) epoch: u64,
    /// Model files loaded by [`Self::stage_model_file_bytes`] and not yet
    /// committed or dropped.
    pub(crate) staged: StagedModelFiles,
    /// The per-epoch extract result memo ([`DcsExtractMemo`]), dropped
    /// whenever the epoch moves. A `RefCell`, so the extract bindings take
    /// `&self`.
    pub(crate) dcs_memo: RefCell<Option<DcsExtractMemo>>,
}

impl ModelManagerHandle {
    /// Moves the epoch on ([`Self::epoch`]): the manager has, or may have,
    /// changed. Also drops the extract result memo, which is only ever
    /// valid for the epoch it was built at.
    pub(crate) fn bump_epoch(&mut self) {
        self.epoch += 1;
        *self.dcs_memo.get_mut() = None;
    }
}

#[wasm_bindgen]
impl ModelManagerHandle {
    /// A fresh manager with the `concerto@1.0.0` system model loaded.
    #[wasm_bindgen(constructor)]
    pub fn new() -> JsResult<ModelManagerHandle> {
        run(|| {
            Ok(Self {
                manager: ModelManager::new()?,
                epoch: 0,
                staged: StagedModelFiles::default(),
                dcs_memo: RefCell::new(None),
            })
        })
    }

    /// TS `ModelManager`'s `dangerouslyAllowReservedSystemTypeNamesInUserModels`
    /// option, `false` on a new handle. Set it before
    /// [`Self::add_model_with_definitions`] validates a model that relies on
    /// it.
    #[wasm_bindgen(js_name = setDangerouslyAllowReservedSystemTypeNamesInUserModels)]
    pub fn set_dangerously_allow_reserved_system_type_names_in_user_models(&mut self, allow: bool) {
        self.bump_epoch();
        self.manager
            .set_dangerously_allow_reserved_system_type_names_in_user_models(allow);
    }

    /// Loads a model from its JSON AST, passed as JSON text
    /// (`JSON.stringify(ast)`), and returns the handle of its model file.
    /// Malformed JSON throws a JS `SyntaxError`.
    #[wasm_bindgen(js_name = addModel)]
    pub fn add_model(&mut self, ast: &str, file_name: Option<String>) -> JsResult<u32> {
        self.bump_epoch();
        run(|| {
            let model_file = model_file_from_text(ast, None, file_name)?;
            let namespace = model_file.namespace().to_string();
            self.manager.add_model_file(model_file)?;
            self.file_handle(&namespace)
        })
    }

    /// TS `ModelManagerOptions.decoratorValidation`, disabled on a new handle
    /// (TS's `DEFAULT_DECORATOR_VALIDATION`). `options` is shaped like the TS
    /// option, `{missingDecorator?, invalidDecorator?}`; only a truthy value
    /// enables the check for that field, as in TS.
    #[wasm_bindgen(js_name = setDecoratorValidation)]
    pub fn set_decorator_validation(&mut self, options: &JsValue) -> JsResult<()> {
        self.bump_epoch();
        run(|| {
            let missing_decorator = level_option(options, "missingDecorator")?;
            let invalid_decorator = level_option(options, "invalidDecorator")?;
            self.manager
                .set_decorator_validation(DecoratorValidationOptions {
                    missing_decorator,
                    invalid_decorator,
                });
            Ok(())
        })
    }

    /// `validateAst` over the AST alone, as JSON text
    /// ([`concerto_core::ModelManager::validate_ast_value`]), without building
    /// a model file, whose constructor would reject some malformed ASTs with
    /// an `IllegalModelException` where TS's `validateAst` throws a
    /// `MetamodelException`. Malformed JSON throws a JS `SyntaxError`.
    #[wasm_bindgen(js_name = validateAstValue)]
    pub fn validate_ast_value(&mut self, ast: &str) -> JsResult<()> {
        self.bump_epoch();
        run(|| {
            let value = parse_json(ast)?;
            Ok(self.manager.validate_ast_value(&value)?)
        })
    }

    /// The handle's mutation counter (the rule on the field): it stamps this
    /// handle's own caches and moves iff the manager may have changed. A JS
    /// number (exact up to 2^53), exported for the smoke checks; the TS
    /// views key on `EngineState.version` instead. A fork restarts it at 0.
    pub fn epoch(&self) -> f64 {
        // Precision loss only past 2^53 mutations.
        #[allow(clippy::cast_precision_loss)]
        let epoch = self.epoch as f64;
        epoch
    }

    /// The handle of a declaration, by its exact fully-qualified name;
    /// `undefined` if none. The views pass it to the arena answers (BC-52).
    #[wasm_bindgen(js_name = declarationId)]
    pub fn declaration_id(&self, fqn: &str) -> Option<u32> {
        self.manager.declaration_id(fqn).map(DeclId::index)
    }

    /// The handle of the model file for a namespace; `undefined` if none.
    #[wasm_bindgen(js_name = modelFileId)]
    pub fn model_file_id(&self, namespace: &str) -> Option<u32> {
        self.manager
            .model_file_id(namespace)
            .map(ModelFileId::index)
    }

    /// TS: `BaseModelManager.getModelFileByFileName(fileName)`: the namespace
    /// of the first non-system model file whose `getName()` equals
    /// `file_name`, or `undefined`; the caller looks it up in its own
    /// `this.modelFiles`. An omitted `file_name` matches the first file
    /// loaded with no name, as TS's `getName() === undefined` does.
    #[wasm_bindgen(js_name = modelManagerGetModelFileByFileName)]
    pub fn model_manager_get_model_file_by_file_name(
        &self,
        file_name: Option<String>,
    ) -> Option<String> {
        self.manager
            .model_file_by_optional_file_name(file_name.as_deref())
            .map(|mf| mf.namespace().to_string())
    }

    /// `Serializer.fromJSON`'s fast path: the document's wire encoding
    /// (`json_text`) read and validated as `fromJSON` does with
    /// `options_text`, the resulting resource written in the compact shape
    /// (`CompactInstanceOut`) the view materialises.
    #[wasm_bindgen(js_name = serializerFromJsonCompact)]
    pub fn serializer_from_json_compact(
        &self,
        json_text: &str,
        options_text: &str,
        env: JsValue,
    ) -> JsResult<String> {
        run(|| {
            let resource = self.build_from_json(WireDoc::Text(json_text), options_text, env)?;
            serde_json::to_string(&CompactInstanceOut(&resource)).map_err(internal)
        })
    }

    /// `Serializer.toJSON`'s fast path: `wire_text` is the resource's
    /// `"typed"` wire encoding, `options_text` its merged options or
    /// `"null"`.
    #[wasm_bindgen(js_name = serializerToJson)]
    pub fn serializer_to_json(&self, wire_text: &str, options_text: &str) -> JsResult<String> {
        run(|| {
            // One pass each way, with the serializer reused while the
            // options text is unchanged ([`with_serializer_options`]).
            self.to_json_text(WireDoc::Text(wire_text), options_text)
        })
    }

    /// [`Self::serializer_to_json`] with the wire value in the compact binary
    /// layout (`WireDoc::Bytes`) the TS writer (src/engine/wire.ts) writes
    /// from the live object: the same result and errors as its JSON text.
    #[wasm_bindgen(js_name = serializerToJsonBytes)]
    pub fn serializer_to_json_bytes(&self, bytes: &[u8], options_text: &str) -> JsResult<String> {
        run(|| self.to_json_text(WireDoc::Bytes(bytes), options_text))
    }

    /// [`Self::serializer_from_json_compact`] with the wire value in the
    /// compact binary layout: the same result and errors (diagnostics
    /// included) as its JSON text.
    #[wasm_bindgen(js_name = serializerFromJsonCompactBytes)]
    pub fn serializer_from_json_compact_bytes(
        &self,
        bytes: &[u8],
        options_text: &str,
        env: JsValue,
    ) -> JsResult<String> {
        run(|| {
            let resource = self.build_from_json(WireDoc::Bytes(bytes), options_text, env)?;
            serde_json::to_string(&CompactInstanceOut(&resource)).map_err(internal)
        })
    }

    /// [`Self::add_model`], keeping `definitions` (the CTO source) for
    /// `getDefinitions()`. With `validate` (TS `!disableValidation`) the file
    /// is validated before it is registered
    /// ([`ModelManager::validate_detached_model_file`]); the
    /// duplicate-namespace check comes first either way, as in TS.
    #[wasm_bindgen(js_name = addModelWithDefinitions)]
    pub fn add_model_with_definitions(
        &mut self,
        ast: &str,
        definitions: Option<String>,
        file_name: Option<String>,
        validate: bool,
    ) -> JsResult<u32> {
        self.bump_epoch();
        run(|| {
            // The file is built once, for both the check and the add.
            let model_file = model_file_from_text(ast, definitions, file_name)?;
            let namespace = model_file.namespace().to_string();
            if validate && self.manager.model_file(&namespace).is_none() {
                // Validated and registered in one step, without a scratch
                // copy of the manager.
                return self
                    .manager
                    .validate_and_add_model_file(model_file)
                    .map(ModelFileId::index)
                    .map_err(|(err, _)| err.into());
            }
            self.manager.add_model_file(model_file)?;
            self.file_handle(&namespace)
        })
    }

    /// The staging binding every model file goes through: loads the AST from
    /// JSON text as UTF-8 or, with `STAGE_COMPACT`, the compact layout, with
    /// BC-19's check folded in under `STAGE_CHECKED`, and returns the stage
    /// with its header (`FlatStaged`). Bad bytes throw a `TypeError`, malformed
    /// JSON a `SyntaxError`. Leaves the manager and epoch unchanged.
    #[wasm_bindgen(js_name = stageModelFileBytes)]
    pub fn stage_model_file_bytes(
        &mut self,
        ast: &[u8],
        definitions: Option<String>,
        file_name: Option<String>,
        flags: u32,
    ) -> JsResult<String> {
        let checked = flags & STAGE_CHECKED != 0;
        if flags & STAGE_COMPACT != 0 {
            return run(|| {
                let loaded = if checked {
                    ModelFile::from_compact_checked_with_imports(ast, definitions, file_name)
                } else {
                    ModelFile::from_compact_with_imports(ast, definitions, file_name)
                };
                self.stage_loaded_flat(loaded.map_err(compact_layout_error)??)
            });
        }
        let text = utf8_text(ast)?;
        run(|| {
            let loaded = if checked {
                ModelFile::from_json_text_checked_with_imports(text, definitions, file_name)
            } else {
                ModelFile::from_json_text_with_imports(text, definitions, file_name)
            };
            self.stage_loaded_flat(loaded.map_err(json_syntax)??)
        })
    }

    /// Stages a model file [`Self::stage_model_file_bytes`] has just
    /// loaded, with the AST's own `imports` node, and returns the stage and
    /// its header in the flat layout ([`flat_staged_text`]).
    pub(crate) fn stage_loaded_flat(
        &mut self,
        loaded: (ModelFile, Option<Value>),
    ) -> Result<String> {
        let (file, imports) = loaded;
        let header = staged_header_from_parts(file.namespace(), imports.as_ref());
        let text = flat_staged_text(0, header.as_ref()).map_err(internal)?;
        drop(header);
        let stage = self.staged.insert(file);
        // The text was written for stage 0 (`[0` or `[0,`), so that a write
        // error leaves nothing staged; the id `insert` gave replaces it.
        Ok(format!("[{stage}{}", &text[2..]))
    }

    /// Registers a staged model file, as [`Self::add_model_with_definitions`]
    /// with `validate: false` would register its AST (the same checks and
    /// errors), without sending the AST again. The stage id is consumed.
    /// Returns the file's handle, or `undefined` if the stage id is unknown
    /// (evicted or consumed), and the caller sends the AST.
    #[wasm_bindgen(js_name = commitStagedModelFile)]
    pub fn commit_staged_model_file(&mut self, stage: u32) -> JsResult<Option<u32>> {
        let Some(file) = self.staged.files.remove(&stage) else {
            return Ok(None);
        };
        // A file staged shared from a manager that validated it carries that
        // manager's proof, so `validateModelFiles` may skip it; one `filter`
        // kept whole is read in TS's filtered form.
        let (proof, filtered) = self.staged.take_extras(stage);
        self.bump_epoch();
        run(|| {
            let id = self.manager.add_shared_model_file_with_proof(file, proof)?;
            if filtered {
                self.manager.read_in_filtered_form(id);
            }
            Ok(Some(ModelFileId::index(id)))
        })
    }

    /// [`Self::commit_staged_model_file`] for several stages, in order, in one
    /// call: each entry of `stages` is overwritten with its file's handle and
    /// `true` returned; `false`, changing nothing, for an unknown stage. A
    /// registration error is thrown with the earlier files registered and the
    /// later ones still staged.
    #[wasm_bindgen(js_name = commitStagedModelFiles)]
    pub fn commit_staged_model_files(&mut self, stages: &mut [u32]) -> JsResult<bool> {
        if stages
            .iter()
            .any(|stage| !self.staged.files.contains_key(stage))
        {
            return Ok(false);
        }
        if stages.is_empty() {
            return Ok(true);
        }
        self.bump_epoch();
        for slot in stages.iter_mut() {
            let stage = *slot;
            let Some(file) = self.staged.files.remove(&stage) else {
                // A stage id given twice: the second is already consumed.
                return Err(throw(
                    ContractError::pre_port(
                        ErrorKind::InvalidArgument,
                        format!("the stage {stage} given twice"),
                        None,
                    )
                    .into(),
                    None,
                ));
            };
            let (proof, filtered) = self.staged.take_extras(stage);
            let id = run(|| Ok(self.manager.add_shared_model_file_with_proof(file, proof)?))?;
            if filtered {
                self.manager.read_in_filtered_form(id);
            }
            *slot = ModelFileId::index(id);
        }
        Ok(true)
    }

    /// Replaces the model file registered under a staged file's namespace
    /// with it, as [`Self::update_model_file`] with `validate: false` would
    /// (the same errors), without sending the AST again. The stage id is
    /// consumed. Returns the file's handle, or `undefined` if the stage id is
    /// unknown, and the caller falls back to [`Self::update_model_file`].
    #[wasm_bindgen(js_name = updateStagedModelFile)]
    pub fn update_staged_model_file(&mut self, stage: u32) -> JsResult<Option<u32>> {
        let Some(file) = self.staged.files.remove(&stage) else {
            return Ok(None);
        };
        let (_, filtered) = self.staged.take_extras(stage);
        self.bump_epoch();
        run(|| {
            let namespace = file.namespace().to_string();
            // Shared, not copied: a stage from the commit, validate-and-
            // commit, DecoratorManager and filter paths may be held
            // elsewhere too.
            let updated = self.manager.update_shared_model_file(file, false)?;
            self.manager.adopt(updated);
            let handle = self.file_handle(&namespace)?;
            if filtered {
                self.manager
                    .read_in_filtered_form(ModelFileId::from_index(handle));
            }
            Ok(Some(handle))
        })
    }

    /// TS `BaseModelManager.updateModelFile`'s validation and replacement of
    /// a staged model file in one call: [`Self::model_file_validate_staged`]
    /// then [`Self::update_staged_model_file`]. Returns the file's handle, or
    /// `undefined` if the stage id is unknown. A validation error leaves the
    /// file staged and the manager unchanged.
    #[wasm_bindgen(js_name = validateAndUpdateStagedModelFile)]
    pub fn validate_and_update_staged_model_file(&mut self, stage: u32) -> JsResult<Option<u32>> {
        let Some(file) = self.staged.files.get(&stage) else {
            return Ok(None);
        };
        run(|| Ok(self.manager.validate_detached_model_file(file)?))?;
        self.update_staged_model_file(stage)
    }

    /// [`Self::validate_ast_value`] over a staged model file's AST. Returns
    /// `true` once checked, `false` if the stage id is unknown; throws what
    /// [`Self::validate_ast_value`] throws. The file stays staged.
    #[wasm_bindgen(js_name = validateAstStaged)]
    pub fn validate_ast_staged(&mut self, stage: u32) -> JsResult<bool> {
        let Some(file) = self.staged.files.get(&stage).cloned() else {
            return Ok(false);
        };
        self.bump_epoch();
        run(|| {
            let ast = self.staged.read_ast(stage, &file);
            self.manager.validate_ast_value(&ast)?;
            Ok(true)
        })
    }

    /// `model_file_view_snapshot` of a staged model file's AST, or
    /// `undefined` if the stage id is unknown or the snapshot cannot be read.
    #[wasm_bindgen(js_name = stagedModelFileViewSnapshot)]
    pub fn staged_model_file_view_snapshot(
        &self,
        stage: u32,
        namespace: Option<String>,
    ) -> Option<String> {
        crate::properties::model_file_view_snapshot_of(
            self.staged.files.get(&stage)?.ast(),
            namespace,
        )
    }

    /// `model_file_view_snapshot` of a loaded model file's AST, or
    /// `undefined` if the handle is unknown or the snapshot cannot be read.
    #[wasm_bindgen(js_name = modelFileViewSnapshotOf)]
    pub fn model_file_view_snapshot_of(
        &self,
        model_file: u32,
        namespace: Option<String>,
    ) -> Option<String> {
        let file = self.manager.file(ModelFileId::from_index(model_file))?;
        crate::properties::model_file_view_snapshot_of(file.ast(), namespace)
    }

    /// TS `BaseModelManager.addModelFile`'s validation and registration of a
    /// staged model file in one call: [`Self::model_file_validate_staged`]
    /// then [`Self::commit_staged_model_file`]. Returns the file's handle, or
    /// `undefined` if the stage id is unknown. A validation error leaves the
    /// file staged and the manager unchanged; the stage is consumed once
    /// validation passes.
    #[wasm_bindgen(js_name = validateAndCommitStagedModelFile)]
    pub fn validate_and_commit_staged_model_file(
        &mut self,
        stage: u32,
        metamodel: Option<bool>,
    ) -> JsResult<Option<u32>> {
        let Some(file) = self.staged.files.remove(&stage) else {
            return Ok(None);
        };
        // The proof is not used: the file is validated here. Whether it is
        // read in TS's filtered form stays with the stage until it is
        // registered or gone.
        self.staged.proofs.remove(&stage);
        // With `metamodel`, `validateAst`'s check runs first, over the
        // staged AST ([`Self::validate_ast_staged`]'s check): its error is
        // thrown as that binding throws it, and the file stays staged.
        if metamodel == Some(true) {
            self.bump_epoch();
            let checked = self
                .manager
                .validate_ast_value(&self.staged.read_ast(stage, &file));
            if let Err(err) = checked {
                self.staged.files.insert(stage, file);
                // Marked (`metamodelCheck`), so the caller throws it as
                // `validateAst` does, not re-wrapped.
                return Err(throw_with(err.into(), None, true));
            }
        }
        // Validated and registered in one step, shared
        // ([`ModelManager::validate_and_add_shared_model_file`]). A
        // validation error hands the file back, still staged under the same
        // id; the epoch moves once validation passes.
        match self.manager.validate_and_add_shared_model_file(file) {
            Ok(id) => {
                self.bump_epoch();
                if self.staged.filtered.remove(&stage) {
                    self.manager.read_in_filtered_form(id);
                }
                Ok(Some(ModelFileId::index(id)))
            }
            Err((err, Some(file))) => {
                self.staged.files.insert(stage, file);
                Err(throw(err.into(), None))
            }
            Err((err, None)) => {
                self.staged.filtered.remove(&stage);
                self.bump_epoch();
                Err(throw(err.into(), None))
            }
        }
    }

    /// [`Self::model_file_validate_detached`] for a staged model file.
    /// Returns `true` once validated, `false` if the stage id is unknown;
    /// throws the first problem found. The file stays staged.
    #[wasm_bindgen(js_name = modelFileValidateStaged)]
    pub fn model_file_validate_staged(&self, stage: u32) -> JsResult<bool> {
        let Some(file) = self.staged.files.get(&stage) else {
            return Ok(false);
        };
        run(|| {
            self.manager.validate_detached_model_file(file)?;
            Ok(true)
        })
    }

    /// Drops a staged model file that will never be registered here.
    /// An unknown stage id is ignored.
    #[wasm_bindgen(js_name = dropStagedModelFile)]
    pub fn drop_staged_model_file(&mut self, stage: u32) {
        self.staged.files.remove(&stage);
        self.staged.forget(stage);
    }

    /// TS `BaseModelManager.resolveType(context, type)`
    /// ([`ModelManager::resolve_type`]).
    #[wasm_bindgen(js_name = resolveType)]
    pub fn resolve_type(&self, context: &str, type_name: &str) -> JsResult<String> {
        run(|| Ok(self.manager.resolve_type(context, type_name)?))
    }

    /// TS `BaseModelManager.derivesFrom(fqt1, fqt2)`.
    #[wasm_bindgen(js_name = derivesFrom)]
    pub fn derives_from(&self, fqt1: &str, fqt2: &str) -> JsResult<bool> {
        run(|| Ok(self.manager.derives_from(fqt1, fqt2)?))
    }

    /// TS `BaseModelManager.isAssignableTo(fqn, baseFqn)`: `false` when
    /// `fqn` is not found, and `derivesFrom`'s errors thrown.
    #[wasm_bindgen(js_name = isAssignableTo)]
    pub fn is_assignable_to(&self, fqn: &str, base_fqn: &str) -> JsResult<bool> {
        run(|| Ok(self.manager.is_type_assignable_to(fqn, base_fqn)?))
    }

    /// TS `BaseModelManager.getNamespaces()`: every registered model file's
    /// namespace, the system models included, in load order, as
    /// `Object.keys(this.modelFiles)` gives them.
    #[wasm_bindgen(js_name = getNamespaces)]
    pub fn get_namespaces(&self) -> Vec<String> {
        self.manager
            .model_files()
            .map(|file| file.namespace().to_string())
            .collect()
    }

    /// TS `BaseModelManager.updateModelFile`: rebuilds the model file for
    /// `ast`'s namespace from its JSON AST, replacing the one registered
    /// there. `validate` is TS's `!disableValidation`. Returns the
    /// namespace's model file handle.
    #[wasm_bindgen(js_name = updateModelFile)]
    pub fn update_model_file(
        &mut self,
        ast: &str,
        definitions: Option<String>,
        file_name: Option<String>,
        validate: bool,
    ) -> JsResult<u32> {
        self.bump_epoch();
        run(|| {
            let model_file = model_file_from_text(ast, definitions, file_name)?;
            let namespace = model_file.namespace().to_string();
            let updated = self.manager.update_model_file(model_file, validate)?;
            self.manager.adopt(updated);
            self.file_handle(&namespace)
        })
    }

    /// TS `BaseModelManager.deleteModelFile(namespace)`.
    #[wasm_bindgen(js_name = deleteModelFile)]
    pub fn delete_model_file(&mut self, namespace: &str) -> JsResult<()> {
        self.bump_epoch();
        run(|| {
            let deleted = self.manager.delete_model_file(namespace)?;
            self.manager.adopt(deleted);
            Ok(())
        })
    }

    // -----------------------------------------------------------------------
    // ModelFile (src/introspect/modelfile.ts), by the file's handle
    // (`modelFileId`).
    // -----------------------------------------------------------------------

    /// TS: `ModelFile.getImports`: the fully-qualified names this file
    /// imports, the built-in system import included for a non-system file.
    #[wasm_bindgen(js_name = modelFileGetImports)]
    pub fn model_file_get_imports(&self, model_file: u32) -> JsResult<Array> {
        run(|| {
            Ok(self
                .require_file(model_file)?
                .imported_type_names()
                .iter()
                .map(|n| JsValue::from_str(n))
                .collect())
        })
    }

    /// TS: `ModelFile.isLocalType`.
    #[wasm_bindgen(js_name = modelFileIsLocalType)]
    pub fn model_file_is_local_type(&self, model_file: u32, type_name: &str) -> JsResult<bool> {
        run(|| Ok(self.require_file(model_file)?.is_local_type(type_name)))
    }

    /// TS: `ModelFile.getType(type)`, by name
    /// ([`ModelManager::model_file_type_name`]): a primitive's own name, the
    /// fully-qualified name of the declaration it resolves to (the view
    /// maps it to its own view), or `undefined` for TS `null`.
    #[wasm_bindgen(js_name = modelFileGetTypeName)]
    pub fn model_file_get_type_name(
        &self,
        model_file: u32,
        type_name: &str,
    ) -> JsResult<Option<String>> {
        run(|| {
            self.require_file(model_file)?;
            Ok(self
                .manager
                .model_file_type_name(ModelFileId::from_index(model_file), type_name)?)
        })
    }

    /// TS: `ModelFile.getFullyQualifiedTypeName(type)`, `undefined` for TS
    /// `null`.
    #[wasm_bindgen(js_name = modelFileGetFullyQualifiedTypeName)]
    pub fn model_file_get_fully_qualified_type_name(
        &self,
        model_file: u32,
        type_name: &str,
    ) -> JsResult<Option<String>> {
        run(|| {
            Ok(self
                .require_file(model_file)?
                .fully_qualified_type_name(type_name))
        })
    }

    /// TS: `ModelFile.resolveType(context, type, fileLocation)`
    /// ([`ModelManager::model_file_resolve_type`]). `view` is the JS
    /// `ModelFile` (`this`), which the undeclared-type error names.
    #[wasm_bindgen(js_name = modelFileResolveType)]
    pub fn model_file_resolve_type(
        &self,
        model_file: u32,
        context: &str,
        type_name: &str,
        file_location: JsValue,
        view: JsValue,
    ) -> JsResult<()> {
        let body = || -> Result<()> {
            self.require_file(model_file)?;
            let location = to_json(&file_location)?;
            Ok(self.manager.model_file_resolve_type(
                ModelFileId::from_index(model_file),
                context,
                type_name,
                location,
            )?)
        };
        run_naming(|| view.clone(), body)
    }

    /// TS: `BaseModelManager.getType(qualifiedName)`, by name
    /// ([`ModelManager::type_declaration_name`]), or its
    /// `TypeNotFoundException`. The view maps the name to its own view.
    #[wasm_bindgen(js_name = getTypeName)]
    pub fn get_type_name(&self, qualified_name: &str) -> JsResult<String> {
        run(|| Ok(self.manager.type_declaration_name(qualified_name)?))
    }

    /// TS: `BaseModelManager.validateModelFiles()` in one call
    /// ([`ModelManager::validate_models_naming_file`]). `model_files` is the
    /// view's `this.modelFiles`: the first problem is thrown naming the JS
    /// `ModelFile` it was found in, as that file's `validate()` does.
    #[wasm_bindgen(js_name = validateModelFiles)]
    pub fn validate_model_files(&self, model_files: &JsValue) -> JsResult<()> {
        self.manager
            .validate_models_naming_file()
            .map_err(|(namespace, err)| {
                throw_naming_file(err.into(), model_files, Some(&namespace))
            })
    }

    /// TS: `BaseModelManager._throwAlreadyExists(modelFile)`: the plain
    /// `Error` naming `namespace`, the incoming `file_name` and the file
    /// registered under `namespace`; returns only when nothing is.
    #[wasm_bindgen(js_name = throwAlreadyExists)]
    pub fn throw_already_exists(&self, namespace: &str, file_name: Option<String>) -> JsResult<()> {
        run(|| {
            Ok(self
                .manager
                .check_namespace_available(namespace, file_name.as_deref())?)
        })
    }

    /// TS: `BaseModelManager.updateExternalModels` after the download. `sources`
    /// is JSON text, `[{ast, definitions, fileName}]`: each is added or replaces
    /// its namespace unvalidated, then every file is validated; any failure
    /// leaves the handle as it was. `model_files` are the view's files once
    /// applied, for naming the JS `ModelFile` a failure is in.
    #[wasm_bindgen(js_name = updateExternalModels)]
    pub fn update_external_models(&mut self, sources: &str, model_files: &JsValue) -> JsResult<()> {
        self.bump_epoch();
        let parsed = run(|| -> Result<Vec<ModelFileSource>> {
            let value = parse_json(sources)?;
            let text = |source: &Value, key: &str| {
                source.get(key).and_then(Value::as_str).map(str::to_string)
            };
            Ok(value
                .as_array()
                .map(|list| {
                    list.iter()
                        .map(|source| ModelFileSource {
                            ast: source.get("ast").cloned().unwrap_or(Value::Null),
                            definitions: text(source, "definitions"),
                            file_name: text(source, "fileName"),
                        })
                        .collect()
                })
                .unwrap_or_default())
        })?;
        self.manager
            .update_external_models_naming_file(parsed)
            .map(|_| ())
            .map_err(|(namespace, err)| {
                throw_naming_file(err.into(), model_files, namespace.as_deref())
            })
    }

    /// [`Self::update_external_models`] for downloaded files the view staged:
    /// `stages` are their stage ids, in order. Returns `false`, changing
    /// nothing, if any stage id is unknown; otherwise consumes the stages
    /// and throws what [`Self::update_external_models`] throws.
    #[wasm_bindgen(js_name = updateExternalModelsStaged)]
    pub fn update_external_models_staged(
        &mut self,
        stages: Vec<u32>,
        model_files: &JsValue,
    ) -> JsResult<bool> {
        if !stages
            .iter()
            .all(|stage| self.staged.files.contains_key(stage))
        {
            return Ok(false);
        }
        // A stage id given twice throws, as `commitStagedModelFiles` does,
        // before any stage is consumed.
        if let Some(stage) = stages.iter().enumerate().find_map(|(i, stage)| {
            stages
                .get(..i)
                .is_some_and(|earlier| earlier.contains(stage))
                .then_some(*stage)
        }) {
            return Err(throw(
                ContractError::pre_port(
                    ErrorKind::InvalidArgument,
                    format!("the stage {stage} given twice"),
                    None,
                )
                .into(),
                None,
            ));
        }
        let files: Vec<_> = stages
            .iter()
            .filter_map(|stage| {
                self.staged.forget(*stage);
                self.staged.files.remove(stage)
            })
            .collect();
        self.bump_epoch();
        self.manager
            .update_external_model_files_naming_file(files)
            .map(|_| true)
            .map_err(|(namespace, err)| {
                throw_naming_file(err.into(), model_files, namespace.as_deref())
            })
    }

    /// TS: `ModelFile.validate()` for a model file this manager holds under
    /// its namespace (`ModelManager::validate_model_file`).
    #[wasm_bindgen(js_name = modelFileValidate)]
    pub fn model_file_validate(&self, model_file: u32) -> JsResult<()> {
        run(|| {
            let file = self.require_file(model_file)?;
            Ok(self.manager.validate_model_file(file)?)
        })
    }

    /// TS: `ModelFile.validate()` for a `ModelFile` that need not be the one
    /// this manager holds under its namespace
    /// (`ModelManager::validate_detached_model_file`). `ast` is JSON text;
    /// `definitions`/`file_name` are the constructor's optional arguments.
    #[wasm_bindgen(js_name = modelFileValidateDetached)]
    pub fn model_file_validate_detached(
        &self,
        ast: &str,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> JsResult<()> {
        run(|| {
            let file = model_file_from_text(ast, definitions, file_name)?;
            Ok(self.manager.validate_detached_model_file(&file)?)
        })
    }

    /// TS `ModelFile.filter(predicate, modelManager)` for a file this manager
    /// holds, for a result manager whose handle is `target`, never this one.
    /// `predicate` gets each candidate's fully-qualified name, imported
    /// declarations included; a throw propagates. `undefined` when nothing
    /// is kept (TS `null`), and otherwise JSON text:
    ///
    /// - `{"stage": <id>}` when the file is kept unchanged: it is staged in
    ///   `target`, shared, with this manager's
    ///   [`concerto_core::model_manager::ValidityProof`], so `target`
    ///   registers it from the stage and validates it only where the proof
    ///   does not hold;
    /// - `{"staged": [<id>, ...header], "ast": <ast>}` otherwise: the
    ///   filtered file's AST, with the filtered file staged in `target` as
    ///   [`Self::stage_model_file_bytes`] would stage that AST, and its header
    ///   in the same flat layout.
    #[wasm_bindgen(js_name = modelFileFilterStaged)]
    pub fn model_file_filter_staged(
        &self,
        model_file: u32,
        predicate: Function,
        target: &mut ModelManagerHandle,
    ) -> JsResult<Option<String>> {
        run(|| self.filter_to_json(model_file, &predicate, Some(target)))
    }

    /// [`Self::model_file_filter_staged`] with no target, for a result
    /// manager whose handle is this one (a handle cannot be borrowed twice):
    /// `undefined` when nothing is kept, and otherwise `{"ast": <ast>}`, the
    /// filtered file's AST, or the file's own when it is kept unchanged.
    #[wasm_bindgen(js_name = modelFileFilterAst)]
    pub fn model_file_filter_ast(
        &self,
        model_file: u32,
        predicate: Function,
    ) -> JsResult<Option<String>> {
        run(|| self.filter_to_json(model_file, &predicate, None))
    }

    /// A new handle over the same models ([`ModelManager::fork`]): the same
    /// options, model files (shared), handles and warmed caches, validated
    /// as before. Later changes to either never reach the other. The staging
    /// slot and the extract memo are not carried over, and the fork's epoch
    /// restarts at 0: every epoch stamp is per handle, so the fork's own
    /// caches start empty and nothing compares epochs across handles.
    pub fn fork(&self) -> ModelManagerHandle {
        ModelManagerHandle {
            manager: self.manager.fork(),
            epoch: 0,
            staged: StagedModelFiles::default(),
            dcs_memo: RefCell::new(None),
        }
    }
}

impl ModelManagerHandle {
    /// The handle of the model file registered under `namespace`, or the
    /// `TypeNotFound` error naming it: read back after a binding
    /// registers a file.
    pub(crate) fn file_handle(&self, namespace: &str) -> Result<u32> {
        self.manager
            .model_file_id(namespace)
            .map(ModelFileId::index)
            .ok_or_else(|| CoreError::type_not_found(namespace.to_string()).into())
    }

    /// The filter bindings' one body ([`Self::model_file_filter_staged`],
    /// [`Self::model_file_filter_ast`]): a file kept unchanged is staged in
    /// `target` when one is given, and returned as its own AST otherwise.
    fn filter_to_json(
        &self,
        model_file: u32,
        predicate: &Function,
        target: Option<&mut ModelManagerHandle>,
    ) -> Result<Option<String>> {
        use concerto_core::introspect::model_file::FilterOutcome;
        let id = ModelFileId::from_index(model_file);
        match self.filter_with_predicate(id, predicate)? {
            FilterOutcome::Empty => Ok(None),
            FilterOutcome::Unchanged => {
                let file = self
                    .manager
                    .shared_file(id)
                    .ok_or_else(|| unknown(Node::ModelFile(id)))?;
                // TS 5.0.0's filtered file has the default super types its
                // declarations' views were given (R2A-4). Returned as an AST,
                // it is written in; staged, the file is shared and read in
                // that form once registered, built on first read.
                let Some(target) = target else {
                    let ast = file.filtered_ast();
                    return snapshot(&json!({ "ast": ast.as_ref().unwrap_or(file.ast()) }))
                        .map(Some);
                };
                let proof = self.manager.validity_proof(file.namespace());
                let stage = target.staged.insert_shared(Arc::clone(file));
                if let Some(proof) = proof {
                    target.staged.proofs.insert(stage, proof);
                }
                target.staged.filtered.insert(stage);
                Ok(Some(format!("{{\"stage\":{stage}}}")))
            }
            FilterOutcome::Filtered(filtered) => {
                let Some(target) = target else {
                    return snapshot(&json!({ "ast": filtered.ast() })).map(Some);
                };
                // Staged in `target` as built, with its header, so the view
                // TS builds over the AST takes the stage instead of loading
                // the AST again (the prestaged, trusted path of a
                // DecoratorManager result). Validated when it is added.
                let ast = snapshot(filtered.ast())?;
                let header =
                    staged_header_from_parts(filtered.namespace(), filtered.ast().get("imports"));
                let flat = flat_staged_text(0, header.as_ref()).map_err(internal)?;
                drop(header);
                let stage = target.staged.insert(*filtered);
                // `flat` was written for stage 0 (`[0` then `]` or `,...]`).
                Ok(Some(format!(
                    "{{\"staged\":[{stage}{},\"ast\":{ast}}}",
                    &flat[2..]
                )))
            }
        }
    }

    /// [`ModelManager::filter_model_file`] for the file `id` names, with a
    /// JS predicate over each candidate's fully-qualified name. The first
    /// throw is kept and returned once the filter is done; the predicate is
    /// not called again after it.
    fn filter_with_predicate(
        &self,
        id: ModelFileId,
        predicate: &Function,
    ) -> Result<concerto_core::introspect::model_file::FilterOutcome> {
        let js_err: RefCell<Option<Error>> = RefCell::new(None);
        let outcome = self.manager.filter_model_file(id, |fqn| {
            if js_err.borrow().is_some() {
                return false;
            }
            match predicate.call1(&JsValue::NULL, &JsValue::from_str(fqn)) {
                Ok(v) => v.is_truthy(),
                Err(e) => {
                    *js_err.borrow_mut() = Some(Error::Js(e));
                    false
                }
            }
        });
        match js_err.into_inner() {
            Some(err) => Err(err),
            None => Ok(outcome?),
        }
    }

    /// A model file, by its handle; the same [`unknown`] `TypeNotFound` every
    /// other by-handle lookup here throws for one that names nothing.
    pub(crate) fn require_file(&self, model_file: u32) -> Result<&concerto_core::ModelFile> {
        let id = ModelFileId::from_index(model_file);
        self.manager
            .file(id)
            .ok_or_else(|| unknown(Node::ModelFile(id)))
    }
}
