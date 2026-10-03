//! The handle API: [`ModelManagerHandle`], one object per `ModelManager` (P4-01).
//!
//! Split out of `lib.rs` (P5-104, review M7); the crate root glob-imports it.

use super::*;

// ---------------------------------------------------------------------------
// The handle API: one object per ModelManager (P4-01)
// ---------------------------------------------------------------------------

/// A handle that names nothing in this manager, as the arena reports one
/// (`model_manager.rs`, `unknown`): a `TypeNotFound` naming the node.
pub(crate) fn unknown(node: Node) -> Error {
    CoreError::type_not_found(format!("{node:?}")).into()
}

/// A snapshot as the JSON text that crosses the boundary: one string per
/// element, which the view parses once (spike REPORT §3: JSON text beats
/// serde-wasm-bindgen and per-field getters for trees).
pub(crate) fn snapshot(value: &Value) -> Result<String> {
    serde_json::to_string(value).map_err(|e| Error::Js(js_sys::Error::new(&e.to_string()).into()))
}

/// A `ModelManager`, exported to JS as one object (spike "Input to P1-04",
/// point 1). The model files, declarations and properties it holds are
/// addressed by the arena's dense `u32` handles, which cross the boundary as
/// plain numbers and keep naming the same element for the life of the
/// manager (PORTING.md 1.4). An element's state crosses as a JSON snapshot,
/// which a view caches until [`ModelManagerHandle::epoch`] changes
/// (PORTING.md 1.5).
///
/// wasm-bindgen registers a `FinalizationRegistry`, so a view need not call
/// `free()`; after `free()`, every call throws.
#[wasm_bindgen]
pub struct ModelManagerHandle {
    pub(crate) manager: ModelManager,
    /// The epoch ([`Self::epoch`], P5-06), under one rule (P5-101, D-7,
    /// accordproject/concerto-rust#455; A-3, #448): **the epoch moves iff
    /// `manager` may have changed** — a model file added, replaced or
    /// removed, an option set, or a check that may leave a model file
    /// registered (`validateAstValue`'s metamodel copy) — and never goes
    /// back. The staging slot (`staged`) and the extract memo (`dcs_memo`)
    /// never move it: staging, dropping a stage and every read leave the
    /// manager as it was, so a view's cache of what it read from this
    /// handle stays current across them. Being a `&mut self` binding is not
    /// the test (the `stage*` bindings, `dropStagedModelFile` and
    /// `dcsDecorateModels` take `&mut self` and leave the epoch alone); a
    /// binding that changes the manager calls [`Self::bump_epoch`], and so
    /// does [`Self::model_file_filter`] for its `target`. The TS views key
    /// their caches on the epoch only (engine/views.ts); P5-103 removed the
    /// unused `generation()` export.
    pub(crate) epoch: u64,
    /// Model files loaded by [`Self::stage_model_file_bytes`] and not yet
    /// committed or dropped (lazy views: P5-06a, P5-10a).
    pub(crate) staged: StagedModelFiles,
    /// The per-epoch extract result memo (P5-56, T2, F-A2,
    /// [`DcsExtractMemo`]): dropped whenever the epoch moves
    /// ([`Self::bump_epoch`]). A `RefCell`,
    /// so the extract bindings keep taking `&self` and never move the epoch.
    pub(crate) dcs_memo: std::cell::RefCell<Option<DcsExtractMemo>>,
}

impl ModelManagerHandle {
    /// Moves the epoch on ([`Self::epoch`]): the manager has, or may have,
    /// changed. Also drops the extract result memo (P5-56), which is only
    /// ever valid for the epoch it was built at.
    pub(crate) fn bump_epoch(&mut self) {
        self.epoch += 1;
        *self.dcs_memo.get_mut() = None;
    }
}

#[wasm_bindgen]
impl ModelManagerHandle {
    /// A fresh manager with the `concerto@1.0.0` system model loaded.
    #[wasm_bindgen(constructor)]
    pub fn new() -> std::result::Result<ModelManagerHandle, JsValue> {
        run(|| {
            Ok(Self {
                manager: ModelManager::new()?,
                epoch: 0,
                staged: StagedModelFiles::default(),
                dcs_memo: std::cell::RefCell::new(None),
            })
        })
    }

    /// TS `ModelManager`'s `dangerouslyAllowReservedSystemTypeNamesInUserModels`
    /// option (`ModelManager::set_dangerously_allow_reserved_system_type_names_in_user_models`).
    /// Additive: [`Self::new`] takes no options and leaves this `false`, so
    /// every existing caller is unaffected. A view must call this before
    /// [`Self::add_model_with_definitions`] validates a model that relies on
    /// it — otherwise that call's `ModelManager::validate_detached_model_file`
    /// check (P4-08a, accordproject/concerto-rust#173) runs with the option
    /// off, unlike native `add_model(s)`, and rejects a system type name the
    /// caller meant to allow.
    #[wasm_bindgen(js_name = setDangerouslyAllowReservedSystemTypeNamesInUserModels)]
    pub fn set_dangerously_allow_reserved_system_type_names_in_user_models(&mut self, allow: bool) {
        self.bump_epoch();
        self.manager
            .set_dangerously_allow_reserved_system_type_names_in_user_models(allow);
    }

    /// Loads a model from its JSON AST, passed as JSON text (the view calls
    /// `JSON.stringify(ast)`: spike REPORT §3), and returns the handle of its
    /// model file. Malformed JSON throws a JS `SyntaxError`.
    #[wasm_bindgen(js_name = addModel)]
    pub fn add_model(
        &mut self,
        ast: &str,
        file_name: Option<String>,
    ) -> std::result::Result<u32, JsValue> {
        self.bump_epoch();
        run(|| {
            let model_file = model_file_from_text(ast, None, file_name)?;
            let namespace = model_file.namespace().to_string();
            self.manager.add_model_file(model_file)?;
            self.manager
                .model_file_id(&namespace)
                .map(ModelFileId::index)
                .ok_or_else(|| CoreError::type_not_found(namespace.to_string()).into())
        })
    }

    /// TS `ModelManagerOptions.decoratorValidation`
    /// (`ModelManager::set_decorator_validation`). Additive, on the same
    /// pattern as [`Self::set_dangerously_allow_reserved_system_type_names_in_user_models`]:
    /// [`Self::new`] leaves this at its `Default` (both fields `None`, i.e.
    /// TS's `DEFAULT_DECORATOR_VALIDATION`, the check disabled), so every
    /// existing caller is unaffected until it calls this.
    ///
    /// `options` is a plain JS object shaped like the TS constructor option,
    /// `{missingDecorator?, invalidDecorator?}`; either or both keys may be
    /// omitted. As in TS, only a truthy (non-empty string) value enables the
    /// check for that field — `level_option` reproduces the same
    /// `validationOptions.missingDecorator || ...` truthiness TS uses when it
    /// reads these fields elsewhere.
    #[wasm_bindgen(js_name = setDecoratorValidation)]
    pub fn set_decorator_validation(
        &mut self,
        options: &JsValue,
    ) -> std::result::Result<(), JsValue> {
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

    /// [`Self::validate_ast`] over the AST alone (P5-13,
    /// accordproject/concerto-rust#297): the JSON AST text is checked as it
    /// is ([`concerto_core::ModelManager::validate_ast_value`]), without
    /// first building a model file, which the check never reads and whose
    /// own constructor would reject some malformed ASTs with an
    /// `IllegalModelException` where TS's `validateAst` throws a
    /// `MetamodelException`. The TS caller already holds the `ModelFile`.
    /// Additive; malformed JSON throws a JS `SyntaxError`.
    #[wasm_bindgen(js_name = validateAstValue)]
    pub fn validate_ast_value(&mut self, ast: &str) -> std::result::Result<(), JsValue> {
        self.bump_epoch();
        run(|| {
            let value: Value = serde_json::from_str(ast)
                .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
            Ok(self.manager.validate_ast_value(&value)?)
        })
    }

    /// The handle's own mutation counter (P5-06): it moves iff the manager
    /// may have changed (the rule on the field, P5-101 D-7), and never goes
    /// back, so anything a view read from the handle is still current while
    /// the epoch is unchanged. Additive; a JS number (exact up to 2^53).
    /// No caller in concerto-core: the smoke checks (`scripts/checks.mjs`)
    /// read it to show which bindings leave the manager unchanged (P5-103
    /// kept it for them).
    pub fn epoch(&self) -> f64 {
        // Precision loss only past 2^53 mutations.
        #[allow(clippy::cast_precision_loss)]
        let epoch = self.epoch as f64;
        epoch
    }

    /// The handle of a declaration, by its exact fully-qualified name;
    /// `undefined` if none. P5-106 (BC-52): the views pass it to the arena
    /// answers of the retired JsContext bindings.
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

    /// TS: `BaseModelManager.getModelFileByFileName(fileName)` —
    /// `this.getModelFiles().filter(mf => mf.getName() === fileName)[0]`.
    /// The namespace of the first loaded, non-system model file
    /// (registration order; the built-in decorator and root models are
    /// excluded, as `getModelFiles()`'s default argument excludes them)
    /// whose `getName()` equals `file_name`; `undefined` if none does,
    /// including when `file_name` names one of those system files
    /// (P2-11b-U4). The caller looks the namespace up in its own
    /// `this.modelFiles`, the way `getModelFile(namespace)` already does.
    /// An omitted or `undefined` `file_name` matches, as TS
    /// `getName() === undefined` does, the first such file loaded with no
    /// file name (`addCTOModel(text)` with no `fileName`), rather than
    /// failing the argument conversion (accordproject/concerto-rust#262).
    #[wasm_bindgen(js_name = modelManagerGetModelFileByFileName)]
    pub fn model_manager_get_model_file_by_file_name(
        &self,
        file_name: Option<String>,
    ) -> Option<String> {
        self.manager
            .model_file_by_optional_file_name(file_name.as_deref())
            .map(|mf| mf.namespace().to_string())
    }

    /// A model file's snapshot, as JSON text:
    /// `{namespace, version, fileName, ast}`. `fileName` is `null` when the
    /// file has none; `ast` is the AST as it was loaded (OD-3).
    #[wasm_bindgen(js_name = modelFileSnapshot)]
    pub fn model_file_snapshot(&self, model_file: u32) -> std::result::Result<String, JsValue> {
        run(|| {
            let id = ModelFileId::from_index(model_file);
            let file = self
                .manager
                .file(id)
                .ok_or_else(|| unknown(Node::ModelFile(id)))?;
            snapshot(&json!({
                "namespace": file.namespace(),
                "version": file.version(),
                "fileName": file.file_name(),
                "ast": file.ast(),
            }))
        })
    }

    /// [`Self::serializer_from_json`] with the result in the compact shape
    /// (P5-16, accordproject/concerto-rust#310): the same resource, its
    /// top level written as [`CompactInstanceOut`] instead of a `"typed"`
    /// wire value, which the view parses and materialises in about half
    /// the time. Additive: [`Self::serializer_from_json`] is unchanged.
    #[wasm_bindgen(js_name = serializerFromJsonCompact)]
    pub fn serializer_from_json_compact(
        &self,
        json_text: &str,
        options_text: &str,
        env: JsValue,
    ) -> std::result::Result<String, JsValue> {
        run(|| {
            let resource = self.build_from_json(WireDoc::Text(json_text), options_text, env)?;
            serde_json::to_string(&CompactInstanceOut(&resource))
                .map_err(|e| Error::Js(js_sys::Error::new(&e.to_string()).into()))
        })
    }

    /// `Serializer.toJSON`'s fast path (P4-10; module doc above): the
    /// counterpart of [`Self::serializer_from_json`]. `wire_text` is the
    /// resource's `"typed"` wire encoding (module doc), `options_text` its
    /// merged options or `"null"`.
    #[wasm_bindgen(js_name = serializerToJson)]
    pub fn serializer_to_json(
        &self,
        wire_text: &str,
        options_text: &str,
    ) -> std::result::Result<String, JsValue> {
        run(|| {
            // P5-101 (D-3): one pass each way ([`parse_wire`], [`WireOut`])
            // and the serializer reused while the options text is unchanged
            // ([`with_serializer_options`]), as for `serializerFromJsonCompact`.
            self.to_json_text(WireDoc::Text(wire_text), options_text)
        })
    }

    /// P5-101 (E-7, F-8; accordproject/concerto-rust#455):
    /// [`Self::serializer_to_json`] with the resource's wire value in the
    /// compact binary layout (`bytes`, [`WireDoc::Bytes`]), as the TS
    /// binary writer (src/engine/wire.ts) writes it straight from the live
    /// object: the same result and the same errors as its JSON text gives.
    /// Additive.
    #[wasm_bindgen(js_name = serializerToJsonBytes)]
    pub fn serializer_to_json_bytes(
        &self,
        bytes: &[u8],
        options_text: &str,
    ) -> std::result::Result<String, JsValue> {
        run(|| self.to_json_text(WireDoc::Bytes(bytes), options_text))
    }

    /// P5-101 (E-7, F-8; accordproject/concerto-rust#455):
    /// [`Self::serializer_from_json_compact`] with the document's wire value
    /// in the compact binary layout (`bytes`, [`WireDoc::Bytes`]): the same
    /// result, and the same errors (diagnostics included), as its JSON text
    /// gives. Additive.
    #[wasm_bindgen(js_name = serializerFromJsonCompactBytes)]
    pub fn serializer_from_json_compact_bytes(
        &self,
        bytes: &[u8],
        options_text: &str,
        env: JsValue,
    ) -> std::result::Result<String, JsValue> {
        run(|| {
            let resource = self.build_from_json(WireDoc::Bytes(bytes), options_text, env)?;
            serde_json::to_string(&CompactInstanceOut(&resource))
                .map_err(|e| Error::Js(js_sys::Error::new(&e.to_string()).into()))
        })
    }

    /// Loads a model from its JSON AST, as [`Self::add_model`] does, but
    /// also keeps `definitions` (the CTO source text, when the caller has
    /// it) exactly as [`ModelManager::add_model_with_definitions`] does —
    /// so a mirrored model file's `getModels()` content matches the TS
    /// `ModelFile.getDefinitions()` it was loaded from (P4-08). Additive:
    /// [`Self::add_model`] is unchanged and still passes `definitions: None`.
    ///
    /// `validate` mirrors TS `BaseModelManager.addModelFile`'s
    /// `!disableValidation`: when true, the new file is checked with
    /// [`ModelManager::validate_detached_model_file`] — against the manager
    /// as it stands, before the file is registered — exactly as the oracle
    /// harness's own `addModelFile`/`addModel` recipe step does
    /// (`tests/oracle/recipe.rs`), which is also how the reference decides
    /// these fixtures. Before this (P4-08a, accordproject/concerto-rust#173),
    /// this binding never validated at all — regardless of `validate` — so a
    /// model with, say, a missing identifier field, a declared type clashing
    /// with an import, or an undeclared referenced type was silently
    /// registered instead of rejected. The unconditional duplicate-namespace
    /// check (`ModelManager::add_model_with_definitions`'s own) still fires
    /// first regardless of `validate`, as TS's does.
    #[wasm_bindgen(js_name = addModelWithDefinitions)]
    pub fn add_model_with_definitions(
        &mut self,
        ast: &str,
        definitions: Option<String>,
        file_name: Option<String>,
        validate: bool,
    ) -> std::result::Result<u32, JsValue> {
        self.bump_epoch();
        run(|| {
            // P5-06c: the file is built once, through the typed AST path,
            // for both the check and the add; building it is the first
            // thing that can fail either way, so the errors are unchanged.
            let model_file = model_file_from_text(ast, definitions, file_name)?;
            let namespace = model_file.namespace().to_string();
            if validate && self.manager.model_file(&namespace).is_none() {
                // P5-48: validated and registered in one step, without a
                // scratch copy of the manager (the same checks and errors
                // as `validate_detached_model_file` then `add_model_file`).
                return self
                    .manager
                    .validate_and_add_model_file(model_file)
                    .map(ModelFileId::index)
                    .map_err(|(err, _)| err.into());
            }
            self.manager.add_model_file(model_file)?;
            self.manager
                .model_file_id(&namespace)
                .map(ModelFileId::index)
                .ok_or_else(|| CoreError::type_not_found(namespace).into())
        })
    }

    /// P5-101 (D-4, D-10; accordproject/concerto-rust#455): the one staging
    /// binding, which the TS side stages every model file through: loads
    /// the AST from `ast`, its JSON text as UTF-8 bytes (a `TextEncoder`'s
    /// output, P5-76) or, with [`STAGE_COMPACT`], its compact binary layout
    /// (P5-92), with BC-19's shape check folded into the load when
    /// [`STAGE_CHECKED`] is set (P5-69), and stages it, as the bindings it
    /// stood for did (`stageModelFileCheckedUtf8`,
    /// `stageModelFileWithHeaderUtf8`, `stageModelFileCheckedCompactFlat`
    /// and `stageModelFileWithHeaderCompactFlat`, which P5-103 removed with
    /// the other staging bindings). Returns the stage and its header in the
    /// flat layout ([`FlatStaged`]). Bytes that are not UTF-8, or not in the
    /// compact layout, throw a `TypeError`; malformed JSON a `SyntaxError`.
    /// Does not change the manager or its epoch.
    #[wasm_bindgen(js_name = stageModelFileBytes)]
    pub fn stage_model_file_bytes(
        &mut self,
        ast: &[u8],
        definitions: Option<String>,
        file_name: Option<String>,
        flags: u32,
    ) -> std::result::Result<String, JsValue> {
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
            self.stage_loaded_flat(
                loaded
                    .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))??,
            )
        })
    }

    /// P5-94: stages a model file [`Self::stage_model_file_bytes`] has just
    /// loaded, with the AST's own `imports` node, and returns the stage and
    /// its header in the flat layout ([`flat_staged_text`]).
    pub(crate) fn stage_loaded_flat(
        &mut self,
        loaded: (ModelFile, Option<Value>),
    ) -> Result<String> {
        let (file, imports) = loaded;
        let header = staged_header_from_parts(file.namespace(), imports.as_ref());
        let text = flat_staged_text(self.staged.next_id(), header.as_ref())
            .map_err(|e| Error::Js(js_sys::Error::new(&e.to_string()).into()))?;
        self.staged.insert(file);
        Ok(text)
    }

    /// P5-06a: registers a staged model file, as
    /// [`Self::add_model_with_definitions`] with `validate: false` would
    /// register the AST it was staged from (the same duplicate-namespace
    /// check, the same errors), without sending or parsing the AST again.
    /// The stage id is consumed. Returns the file's handle, or `undefined`
    /// if the stage id is unknown (evicted, or already consumed); the caller
    /// then falls back to [`Self::add_model_with_definitions`].
    #[wasm_bindgen(js_name = commitStagedModelFile)]
    pub fn commit_staged_model_file(
        &mut self,
        stage: u32,
    ) -> std::result::Result<Option<u32>, JsValue> {
        let Some(file) = self.staged.files.remove(&stage) else {
            return Ok(None);
        };
        // P5-97: a file staged shared from a manager that had validated it
        // carries that manager's proof, so a later `validateModelFiles`
        // need not validate it again where the proof holds.
        let proof = self.staged.proofs.remove(&stage);
        self.bump_epoch();
        run(|| {
            let id = self.manager.add_shared_model_file_with_proof(file, proof)?;
            Ok(Some(ModelFileId::index(id)))
        })
    }

    /// P5-101 (D-10, M5; accordproject/concerto-rust#455):
    /// [`Self::commit_staged_model_file`] for several staged files, in
    /// order, in one call: the batch `addModelFiles` and the
    /// DecoratorManager results (`adoptStagedModels`, which decorateModels
    /// and every extract use) register each of their files from its stage
    /// this way, where they used to cross once per file. On success each
    /// entry of `stages` is overwritten with its file's handle, in place,
    /// and `true` is returned: the ids cross back in the caller's own
    /// buffer, which TS reuses, rather than in a new `Uint32Array` per call
    /// (one per call cost about 1 ms of garbage collection on an extract of
    /// 47 files, more than the per-file crossings it saves). `false`, having
    /// changed nothing, when any stage id is unknown (evicted, or already
    /// consumed): the caller then registers each file as before. A
    /// registration error is thrown as [`Self::commit_staged_model_file`]
    /// throws it, with the files before it registered and the stages after
    /// it left staged, as the same commits one by one would leave them.
    /// Additive.
    #[wasm_bindgen(js_name = commitStagedModelFiles)]
    pub fn commit_staged_model_files(
        &mut self,
        stages: &mut [u32],
    ) -> std::result::Result<bool, JsValue> {
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
                // A stage id given twice: the second is already consumed,
                // as a second `commit_staged_model_file` would find it.
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
            let proof = self.staged.proofs.remove(&stage);
            let id = run(|| Ok(self.manager.add_shared_model_file_with_proof(file, proof)?))?;
            *slot = ModelFileId::index(id);
        }
        Ok(true)
    }

    /// P5-100 (E-6, accordproject/concerto-rust#454): replaces the model
    /// file registered under a staged file's namespace with it, as
    /// [`Self::update_model_file`] with `validate: false` would replace it
    /// with the AST it was staged from (the same errors), without sending or
    /// parsing the AST again. The stage id is consumed. Returns the file's
    /// handle, or `undefined` if the stage id is unknown (evicted, or already
    /// consumed); the caller then falls back to [`Self::update_model_file`].
    /// Additive.
    #[wasm_bindgen(js_name = updateStagedModelFile)]
    pub fn update_staged_model_file(
        &mut self,
        stage: u32,
    ) -> std::result::Result<Option<u32>, JsValue> {
        let Some(file) = self.staged.files.remove(&stage) else {
            return Ok(None);
        };
        self.staged.proofs.remove(&stage);
        self.bump_epoch();
        run(|| {
            let model_file = std::sync::Arc::unwrap_or_clone(file);
            let namespace = model_file.namespace().to_string();
            let updated = self.manager.update_model_file(model_file, false)?;
            self.manager.adopt(updated);
            self.manager
                .model_file_id(&namespace)
                .map(ModelFileId::index)
                .map(Some)
                .ok_or_else(|| CoreError::type_not_found(namespace).into())
        })
    }

    /// P5-100 (E-6): [`Self::validate_ast_value`] over a staged model file's
    /// AST, without sending it again. Returns `true` once checked, `false`
    /// if the stage id is unknown (the caller then sends the AST, as
    /// before); throws what [`Self::validate_ast_value`] throws. The file
    /// stays staged. Additive.
    #[wasm_bindgen(js_name = validateAstStaged)]
    pub fn validate_ast_staged(&mut self, stage: u32) -> std::result::Result<bool, JsValue> {
        let Some(file) = self.staged.files.get(&stage).cloned() else {
            return Ok(false);
        };
        self.bump_epoch();
        run(|| {
            self.manager.validate_ast_value(file.ast())?;
            Ok(true)
        })
    }

    /// P5-100 (E-6): [`model_file_view_snapshot`] of a staged model file's
    /// AST, or `undefined` if the stage id is unknown or the snapshot cannot
    /// be read. Changes nothing. Additive.
    #[wasm_bindgen(js_name = stagedModelFileViewSnapshot)]
    pub fn staged_model_file_view_snapshot(
        &self,
        stage: u32,
        namespace: Option<String>,
    ) -> Option<String> {
        model_file_view_snapshot_of(self.staged.files.get(&stage)?.ast(), namespace)
    }

    /// P5-100 (E-6): [`model_file_view_snapshot`] of a loaded model file's
    /// AST, or `undefined` if the handle is unknown or the snapshot cannot
    /// be read. Changes nothing. Additive.
    #[wasm_bindgen(js_name = modelFileViewSnapshotOf)]
    pub fn model_file_view_snapshot_of(
        &self,
        model_file: u32,
        namespace: Option<String>,
    ) -> Option<String> {
        let file = self.manager.file(ModelFileId::from_index(model_file))?;
        model_file_view_snapshot_of(file.ast(), namespace)
    }

    /// P5-34 (I-5, accordproject/concerto-rust#344): TS
    /// `BaseModelManager.addModelFile`'s validation and registration of a
    /// staged model file in one call: [`Self::model_file_validate_staged`]
    /// then [`Self::commit_staged_model_file`]. Returns the file's handle, or
    /// `undefined` if the stage id is unknown (evicted, or already consumed);
    /// the caller then validates and registers the file as before. A
    /// validation error is thrown as [`Self::model_file_validate_staged`]
    /// throws it and leaves the file staged and the manager unchanged; the
    /// stage is consumed only once validation passes, and a registration
    /// error is then thrown as [`Self::commit_staged_model_file`] throws it.
    /// Additive.
    #[wasm_bindgen(js_name = validateAndCommitStagedModelFile)]
    pub fn validate_and_commit_staged_model_file(
        &mut self,
        stage: u32,
        metamodel: Option<bool>,
    ) -> std::result::Result<Option<u32>, JsValue> {
        let Some(file) = self.staged.files.remove(&stage) else {
            return Ok(None);
        };
        self.staged.proofs.remove(&stage);
        // P5-101 (D-9, accordproject/concerto-rust#455): with `metamodel`,
        // `BaseModelManager.validateAst`'s check runs first, over the staged
        // file's AST ([`Self::validate_ast_staged`]'s check, in the same
        // call): its error is thrown as that binding throws it, the epoch
        // moved as it moves it, and the file stays staged. Additive: an
        // omitted `metamodel` is `false`.
        if metamodel == Some(true) {
            self.bump_epoch();
            if let Err(err) = self.manager.validate_ast_value(file.ast()) {
                self.staged.files.insert(stage, file);
                // Marked (`metamodelCheck`, not enumerable, as the error
                // factory's own internal flags are), so the caller throws it
                // as `validateAst` throws it, not as `ModelFile.validate()`
                // re-wraps a validation error.
                let thrown = throw(err.into(), None);
                if let Some(target) = thrown.dyn_ref::<Object>() {
                    let descriptor = Object::new();
                    set(&descriptor, "value", &JsValue::TRUE);
                    set(&descriptor, "configurable", &JsValue::TRUE);
                    let _ = Reflect::define_property(
                        target,
                        &JsValue::from_str("metamodelCheck"),
                        &descriptor,
                    );
                }
                return Err(thrown);
            }
        }
        // P5-48 (accordproject/concerto-rust#369): validated and registered
        // in one step, without a scratch copy of the manager and of the
        // file. A validation error hands the file back, and it stays staged
        // under the same id, as before; the epoch moves once validation has
        // passed, as `commit_staged_model_file` moves it.
        // P5-101 (D-9): registered shared, as `commit_staged_model_file`
        // registers it ([`ModelManager::validate_and_add_shared_model_file`]),
        // where a file staged shared (a DecoratorManager result) used to be
        // deep-copied here.
        match self.manager.validate_and_add_shared_model_file(file) {
            Ok(id) => {
                self.bump_epoch();
                Ok(Some(ModelFileId::index(id)))
            }
            Err((err, Some(file))) => {
                self.staged.files.insert(stage, file);
                run(|| Err(err.into()))
            }
            Err((err, None)) => {
                self.bump_epoch();
                run(|| Err(err.into()))
            }
        }
    }

    /// P5-06a: [`Self::model_file_validate_detached`] for a staged model
    /// file, without sending the AST again. Returns `true` once validated;
    /// `false` if the stage id is unknown, and the caller then falls back to
    /// [`Self::model_file_validate_detached`]. Throws the first problem
    /// found, as that binding does. The staged file stays staged.
    #[wasm_bindgen(js_name = modelFileValidateStaged)]
    pub fn model_file_validate_staged(&self, stage: u32) -> std::result::Result<bool, JsValue> {
        let Some(file) = self.staged.files.get(&stage) else {
            return Ok(false);
        };
        run(|| {
            self.manager.validate_detached_model_file(file)?;
            Ok(true)
        })
    }

    /// P5-06a: drops a staged model file that will never be registered
    /// here. An unknown stage id is ignored.
    #[wasm_bindgen(js_name = dropStagedModelFile)]
    pub fn drop_staged_model_file(&mut self, stage: u32) {
        self.staged.files.remove(&stage);
        self.staged.proofs.remove(&stage);
    }

    /// TS `BaseModelManager.resolveType(context, type)` (P4-08): delegates
    /// to [`ModelManager::resolve_type`], which the manager mirrors from
    /// every model the view has mirrored in with [`Self::add_model`]/
    /// [`Self::add_model_with_definitions`].
    #[wasm_bindgen(js_name = resolveType)]
    pub fn resolve_type(
        &self,
        context: &str,
        type_name: &str,
    ) -> std::result::Result<String, JsValue> {
        run(|| Ok(self.manager.resolve_type(context, type_name)?))
    }

    /// TS `BaseModelManager.derivesFrom(fqt1, fqt2)` (P4-08).
    #[wasm_bindgen(js_name = derivesFrom)]
    pub fn derives_from(&self, fqt1: &str, fqt2: &str) -> std::result::Result<bool, JsValue> {
        run(|| Ok(self.manager.derives_from(fqt1, fqt2)?))
    }

    /// TS `BaseModelManager.isAssignableTo(fqn, baseFqn)` (P4-08).
    #[wasm_bindgen(js_name = isAssignableTo)]
    pub fn is_assignable_to_type(&self, fqn: &str, base_fqn: &str) -> bool {
        self.manager.is_type_assignable_to(fqn, base_fqn)
    }

    /// TS `BaseModelManager.getNamespaces()` (P4-08): every registered
    /// model file's namespace, the system models included, in load order --
    /// matching `Object.keys(this.modelFiles)`, since TS inserts the
    /// decorator and root models into `this.modelFiles` in its constructor
    /// exactly as [`ModelManager::new`] mirrors them here.
    #[wasm_bindgen(js_name = getNamespaces)]
    pub fn get_namespaces(&self) -> Vec<String> {
        self.manager
            .model_files()
            .map(|file| file.namespace().to_string())
            .collect()
    }

    /// Mirrors TS `BaseModelManager.updateModelFile` (P4-08): rebuilds the
    /// model file for `ast`'s namespace from its JSON AST (as
    /// [`Self::add_model_with_definitions`] does), replacing whatever was
    /// registered there. `validate` is TS's `!disableValidation`. Returns
    /// the (possibly unchanged) handle of that namespace's model file.
    #[wasm_bindgen(js_name = updateModelFile)]
    pub fn update_model_file(
        &mut self,
        ast: &str,
        definitions: Option<String>,
        file_name: Option<String>,
        validate: bool,
    ) -> std::result::Result<u32, JsValue> {
        self.bump_epoch();
        run(|| {
            let model_file = model_file_from_text(ast, definitions, file_name)?;
            let namespace = model_file.namespace().to_string();
            let updated = self.manager.update_model_file(model_file, validate)?;
            self.manager.adopt(updated);
            self.manager
                .model_file_id(&namespace)
                .map(ModelFileId::index)
                .ok_or_else(|| CoreError::type_not_found(namespace).into())
        })
    }

    /// Mirrors TS `BaseModelManager.deleteModelFile(namespace)` (P4-08).
    #[wasm_bindgen(js_name = deleteModelFile)]
    pub fn delete_model_file(&mut self, namespace: &str) -> std::result::Result<(), JsValue> {
        self.bump_epoch();
        run(|| {
            let deleted = self.manager.delete_model_file(namespace)?;
            self.manager.adopt(deleted);
            Ok(())
        })
    }

    // -----------------------------------------------------------------------
    // ModelFile (src/introspect/modelfile.ts) — P4-08c
    //
    // A model file is not its own handle type: it already has one, the same
    // `ModelFileId` P1-04's arena gives every loaded file (`modelFileId`,
    // `modelFileSnapshot`, above). Bound below, keyed by the same `u32`
    // handle: the `ModelFile` members `modelFileSnapshot`'s plain
    // `{namespace, version, fileName, ast}` does not already answer,
    // `getImports` (the resolved fully-qualified names, built-in import
    // included), `isLocalType`, `filter` and `validate`.
    // -----------------------------------------------------------------------

    /// TS: `ModelFile.getImports` — the fully-qualified names this file
    /// imports (the built-in system import included for a non-system file),
    /// as `ModelFile::get_imports` already resolves them.
    #[wasm_bindgen(js_name = modelFileGetImports)]
    pub fn model_file_get_imports(&self, model_file: u32) -> std::result::Result<Array, JsValue> {
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
    pub fn model_file_is_local_type(
        &self,
        model_file: u32,
        type_name: &str,
    ) -> std::result::Result<bool, JsValue> {
        run(|| Ok(self.require_file(model_file)?.is_local_type(type_name)))
    }

    /// TS: `ModelFile.getType(type)` (P5-11, accordproject/concerto-rust#287),
    /// answered by name ([`ModelManager::model_file_type_name`]): a
    /// primitive's own name, the fully-qualified name of the declaration the
    /// type resolves to, or `undefined` for TS `null`. The view maps a
    /// fully-qualified name (the only answer with a dot) to its own
    /// declaration view. Additive.
    #[wasm_bindgen(js_name = modelFileGetTypeName)]
    pub fn model_file_get_type_name(
        &self,
        model_file: u32,
        type_name: &str,
    ) -> std::result::Result<Option<String>, JsValue> {
        run(|| {
            self.require_file(model_file)?;
            Ok(self
                .manager
                .model_file_type_name(ModelFileId::from_index(model_file), type_name)?)
        })
    }

    /// TS: `ModelFile.getFullyQualifiedTypeName(type)` (P5-11,
    /// accordproject/concerto-rust#287): `ModelFile::fully_qualified_type_name`,
    /// `undefined` for TS `null`. Additive.
    #[wasm_bindgen(js_name = modelFileGetFullyQualifiedTypeName)]
    pub fn model_file_get_fully_qualified_type_name(
        &self,
        model_file: u32,
        type_name: &str,
    ) -> std::result::Result<Option<String>, JsValue> {
        run(|| {
            Ok(self
                .require_file(model_file)?
                .fully_qualified_type_name(type_name))
        })
    }

    /// TS: `ModelFile.resolveType(context, type, fileLocation)` (P5-11,
    /// accordproject/concerto-rust#287): [`ModelManager::model_file_resolve_type`].
    /// `file_location` is TS's optional `fileLocation`, and `view` the JS
    /// `ModelFile` (`this`), which the undeclared-type
    /// `IllegalModelException` names, as TS's does. Additive.
    #[wasm_bindgen(js_name = modelFileResolveType)]
    pub fn model_file_resolve_type(
        &self,
        model_file: u32,
        context: &str,
        type_name: &str,
        file_location: JsValue,
        view: JsValue,
    ) -> std::result::Result<(), JsValue> {
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
        body().map_err(|e| throw(e, Some(&view)))
    }

    /// TS: `BaseModelManager.getType(qualifiedName)` (P5-11,
    /// accordproject/concerto-rust#287), answered by name
    /// ([`ModelManager::type_declaration_name`]): the fully-qualified name
    /// of the declaration found, or the `TypeNotFoundException` TS throws.
    /// The view maps the name to its own declaration view. Additive.
    #[wasm_bindgen(js_name = getTypeName)]
    pub fn get_type_name(&self, qualified_name: &str) -> std::result::Result<String, JsValue> {
        run(|| Ok(self.manager.type_declaration_name(qualified_name)?))
    }

    /// TS: `BaseModelManager.validateModelFiles()` (P5-11,
    /// accordproject/concerto-rust#287): every model file validated in one
    /// call ([`ModelManager::validate_models_naming_file`]). `model_files`
    /// is the view's `this.modelFiles`: the first problem found is thrown
    /// naming the JS `ModelFile` it was found in, as that file's own
    /// `validate()` does. Additive.
    #[wasm_bindgen(js_name = validateModelFiles)]
    pub fn validate_model_files(&self, model_files: &JsValue) -> std::result::Result<(), JsValue> {
        self.manager
            .validate_models_naming_file()
            .map_err(|(namespace, err)| throw_naming_file(err.into(), model_files, &namespace))
    }

    /// TS: `BaseModelManager._throwAlreadyExists(modelFile)` (P5-11,
    /// accordproject/concerto-rust#287): throws the plain `Error` naming
    /// `namespace`, the incoming file's name (`file_name`) and the name of
    /// the model file already registered under `namespace`
    /// ([`ModelManager::check_namespace_available`]). Returns normally only
    /// when nothing is registered under `namespace`. Additive.
    #[wasm_bindgen(js_name = throwAlreadyExists)]
    pub fn throw_already_exists(
        &self,
        namespace: &str,
        file_name: Option<String>,
    ) -> std::result::Result<(), JsValue> {
        run(|| {
            Ok(self
                .manager
                .check_namespace_available(namespace, file_name.as_deref())?)
        })
    }

    /// TS: the apply, validate and rollback part of
    /// `BaseModelManager.updateExternalModels(options, fileDownloader)`
    /// (P5-11, accordproject/concerto-rust#287;
    /// [`ModelManager::update_external_models_naming_file`]); the download
    /// stays in JS. `sources` is JSON text: the downloaded files, in order,
    /// each `{ast, definitions, fileName}`. Each is added, or replaces the
    /// file under its namespace, without validation; then every model file
    /// is validated, and any failure leaves this handle as it was.
    /// `model_files` is the view's model files as they would be once
    /// applied (namespace to JS `ModelFile`): a validation failure is thrown
    /// naming the JS `ModelFile` it was found in. Additive.
    #[wasm_bindgen(js_name = updateExternalModels)]
    pub fn update_external_models(
        &mut self,
        sources: &str,
        model_files: &JsValue,
    ) -> std::result::Result<(), JsValue> {
        self.bump_epoch();
        let parsed = (|| -> Result<Vec<ModelFileSource>> {
            let value: Value = serde_json::from_str(sources)
                .map_err(|e| Error::Js(js_sys::SyntaxError::new(&e.to_string()).into()))?;
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
        })()
        .map_err(|e| throw(e, None))?;
        self.manager
            .update_external_models_naming_file(parsed)
            .map(|_| ())
            .map_err(|(namespace, err)| match namespace {
                Some(namespace) => throw_naming_file(err.into(), model_files, &namespace),
                None => throw(err.into(), None),
            })
    }

    /// P5-100 (E-6, accordproject/concerto-rust#454):
    /// [`Self::update_external_models`] for downloaded files the view staged
    /// when it built their `ModelFile`s: `stages` are their stage ids, in
    /// order, and no AST is sent or parsed again
    /// ([`ModelManager::update_external_model_files_naming_file`]). Returns
    /// `false`, having changed nothing, if any stage id is unknown (the
    /// caller then sends the ASTs, as before); otherwise consumes the
    /// stages, and throws what [`Self::update_external_models`] throws,
    /// leaving this handle as it was. Additive.
    #[wasm_bindgen(js_name = updateExternalModelsStaged)]
    pub fn update_external_models_staged(
        &mut self,
        stages: Vec<u32>,
        model_files: &JsValue,
    ) -> std::result::Result<bool, JsValue> {
        if !stages
            .iter()
            .all(|stage| self.staged.files.contains_key(stage))
        {
            return Ok(false);
        }
        let files: Vec<_> = stages
            .iter()
            .filter_map(|stage| {
                self.staged.proofs.remove(stage);
                self.staged.files.remove(stage)
            })
            .collect();
        self.bump_epoch();
        self.manager
            .update_external_model_files_naming_file(files)
            .map(|_| true)
            .map_err(|(namespace, err)| match namespace {
                Some(namespace) => throw_naming_file(err.into(), model_files, &namespace),
                None => throw(err.into(), None),
            })
    }

    /// TS: `ModelFile.validate()`, for a model file this manager already
    /// holds under its own namespace — the common case for a view whose
    /// `getModelManager()` is this handle (`ModelManager::validate_model_file`).
    /// Throws the first problem found.
    #[wasm_bindgen(js_name = modelFileValidate)]
    pub fn model_file_validate(&self, model_file: u32) -> std::result::Result<(), JsValue> {
        run(|| {
            let file = self.require_file(model_file)?;
            Ok(self.manager.validate_model_file(file)?)
        })
    }

    /// TS: `ModelFile.validate()` for a `ModelFile` that need not be the one
    /// this manager holds under its namespace — `new ModelFile(modelManager,
    /// ast, …)` followed directly by `validate()`, or the
    /// validate-before-register path a caller like `BaseModelManager.addModelFile`
    /// takes (`ModelManager::validate_detached_model_file`). `ast` is the
    /// model's JSON AST as JSON text (`JSON.stringify(ast)`, `addModel`'s own
    /// convention); `definitions`/`file_name` mirror the `ModelFile`
    /// constructor's own optional arguments.
    #[wasm_bindgen(js_name = modelFileValidateDetached)]
    pub fn model_file_validate_detached(
        &self,
        ast: &str,
        definitions: Option<String>,
        file_name: Option<String>,
    ) -> std::result::Result<(), JsValue> {
        run(|| {
            let file = model_file_from_text(ast, definitions, file_name)?;
            Ok(self.manager.validate_detached_model_file(&file)?)
        })
    }

    /// TS: `ModelFile.filter(predicate, modelManager)`, for a model file this
    /// manager (the source) already holds; `target` is the `modelManager`
    /// argument — ordinarily a *different*, otherwise-empty manager, since
    /// `filter`'s own caller (`BaseModelManager.filter`) always builds a
    /// fresh one before filtering into it; TS never adds the result back to
    /// the file's own manager, and neither must this (a caller that means to
    /// keep it in `self` passes `self` as `target` too, but the common case
    /// is another handle). `predicate` is called with each candidate
    /// declaration's fully-qualified name — including a declaration of
    /// *another* file this one imports from, which `filter`'s own import
    /// pruning reaches (module doc on [`concerto_core::ModelFile::filter`])
    /// — the same `keep_fqn` convention `ModelManager::filter`
    /// (`BaseModelManager.filter`) already uses, so the view's own
    /// `Declaration -> bool` predicate is expected to look its argument back
    /// up by fully-qualified name, as the TS `ModelFile.filter` does. A predicate that throws propagates unchanged.
    ///
    /// The filtered model file, if any declaration survived, is added to
    /// `target` exactly as `addModel` would (so its declarations get the
    /// arena's ordinary handles there) and its handle in `target` is
    /// returned; `None` (JS `undefined`) if every declaration was filtered
    /// out, matching TS's `null`.
    #[wasm_bindgen(js_name = modelFileFilter)]
    pub fn model_file_filter(
        &self,
        model_file: u32,
        predicate: Function,
        target: &mut ModelManagerHandle,
    ) -> std::result::Result<Option<u32>, JsValue> {
        target.bump_epoch();
        run(|| {
            let file = self.require_file(model_file)?;
            // `ModelFile::filter`'s predicate carries no namespace of its
            // own (its doc comment): it is called both on `file`'s own
            // declarations *and*, while pruning `file`'s imports, on
            // declarations belonging to a *different* model file
            // (`source_manager.model_file(ns).get_local_type(...)`). Keying
            // the fully-qualified name off `file`'s namespace alone would
            // ask the JS predicate about the wrong FQN for every cross-file
            // (import) declaration, exactly the failure
            // `ModelManager::filter`'s own doc comment warns about. So the
            // real namespace for every declaration reachable from this
            // filter call is looked up by identity up front, across every
            // file `self.manager` holds.
            let fqn_by_decl: std::collections::HashMap<
                *const concerto_core::introspect::Declaration,
                String,
            > = self
                .manager
                .model_files()
                .flat_map(|mf| {
                    let namespace = mf.namespace();
                    mf.declarations().iter().map(move |decl| {
                        (
                            decl as *const concerto_core::introspect::Declaration,
                            mu::qualify(namespace, decl.name()),
                        )
                    })
                })
                .collect();
            let file_namespace = file.namespace().to_string();
            let js_err: RefCell<Option<Error>> = RefCell::new(None);
            let filtered = file.filter(
                |decl| {
                    if js_err.borrow().is_some() {
                        return false;
                    }
                    let fqn = fqn_by_decl
                        .get(&(decl as *const concerto_core::introspect::Declaration))
                        .cloned()
                        .unwrap_or_else(|| mu::qualify(&file_namespace, decl.name()));
                    match predicate.call1(&JsValue::NULL, &JsValue::from_str(&fqn)) {
                        Ok(v) => v.is_truthy(),
                        Err(e) => {
                            *js_err.borrow_mut() = Some(Error::Js(e));
                            false
                        }
                    }
                },
                &self.manager,
            )?;
            if let Some(err) = js_err.into_inner() {
                return Err(err);
            }
            let Some(filtered) = filtered else {
                return Ok(None);
            };
            let ast = filtered.ast().clone();
            let ns = filtered.namespace().to_string();
            let new_file_name = filtered.file_name().map(str::to_string);
            target
                .manager
                .add_model_with_definitions(&ast, None, new_file_name)?;
            target
                .manager
                .model_file_id(&ns)
                .map(ModelFileId::index)
                .map(Some)
                .ok_or_else(|| CoreError::type_not_found(ns.clone()).into())
        })
    }
}

#[wasm_bindgen]
impl ModelManagerHandle {
    /// P5-97 (accordproject/concerto-rust#448): TS `ModelFile.filter`, as
    /// [`Self::model_file_filter`], for `BaseModelManager.filter`'s result
    /// manager, whose handle is `target`: the predicate is called the same
    /// way, on the same declarations, in the same order, and throws the same
    /// errors. Returns `undefined` when no declaration is kept (TS `null`),
    /// and otherwise JSON text:
    ///
    /// - `{"stage": <id>}` when every declaration is kept and every import
    ///   is unchanged ([`concerto_core::introspect::model_file::FilterOutcome::Unchanged`]):
    ///   the file itself is staged in `target`, shared, not copied, with
    ///   this manager's [`concerto_core::model_manager::ValidityProof`] for
    ///   it, so that `target` registers the same file from its stage
    ///   ([`Self::commit_staged_model_file`]) and validates it only if the
    ///   proof does not hold there. Nothing crosses but the stage id: the
    ///   view builds the filtered ModelFile from the source's own AST.
    /// - `{"ast": <ast>}` otherwise: the filtered file's AST, which
    ///   [`Self::model_file_filter`] added to its target and the view read
    ///   back with [`Self::model_file_snapshot`]. Nothing is added to
    ///   `target`.
    ///
    /// Additive: `modelFileFilter` is unchanged.
    #[wasm_bindgen(js_name = modelFileFilterStaged)]
    pub fn model_file_filter_staged(
        &self,
        model_file: u32,
        predicate: Function,
        target: &mut ModelManagerHandle,
    ) -> std::result::Result<Option<String>, JsValue> {
        run(|| {
            let id = ModelFileId::from_index(model_file);
            self.require_file(model_file)?;
            let file = self
                .manager
                .shared_model_files()
                .nth(id.index() as usize)
                .ok_or_else(|| unknown(Node::ModelFile(id)))?;
            // The fully-qualified name of every declaration the predicate can
            // be handed, by identity, as `model_file_filter` builds it.
            let fqn_by_decl: std::collections::HashMap<
                *const concerto_core::introspect::Declaration,
                String,
            > = self
                .manager
                .model_files()
                .flat_map(|mf| {
                    let namespace = mf.namespace();
                    mf.declarations().iter().map(move |decl| {
                        (
                            decl as *const concerto_core::introspect::Declaration,
                            mu::qualify(namespace, decl.name()),
                        )
                    })
                })
                .collect();
            let file_namespace = file.namespace().to_string();
            let js_err: RefCell<Option<Error>> = RefCell::new(None);
            let outcome = file.filter_outcome(
                |decl| {
                    if js_err.borrow().is_some() {
                        return false;
                    }
                    let fqn = fqn_by_decl
                        .get(&(decl as *const concerto_core::introspect::Declaration))
                        .cloned()
                        .unwrap_or_else(|| mu::qualify(&file_namespace, decl.name()));
                    match predicate.call1(&JsValue::NULL, &JsValue::from_str(&fqn)) {
                        Ok(v) => v.is_truthy(),
                        Err(e) => {
                            *js_err.borrow_mut() = Some(Error::Js(e));
                            false
                        }
                    }
                },
                &self.manager,
            )?;
            if let Some(err) = js_err.into_inner() {
                return Err(err);
            }
            use concerto_core::introspect::model_file::FilterOutcome;
            match outcome {
                FilterOutcome::Empty => Ok(None),
                FilterOutcome::Unchanged => {
                    let proof = self.manager.validity_proof(&file_namespace);
                    let stage = target.staged.insert_shared(std::sync::Arc::clone(file));
                    if let Some(proof) = proof {
                        target.staged.proofs.insert(stage, proof);
                    }
                    Ok(Some(format!("{{\"stage\":{stage}}}")))
                }
                FilterOutcome::Filtered(filtered) => {
                    snapshot(&json!({ "ast": filtered.ast() })).map(Some)
                }
            }
        })
    }

    /// P5-97 (accordproject/concerto-rust#448): a new handle over the same
    /// models ([`ModelManager::fork`]): the same options, the same model
    /// files (shared, never copied), the same model file, declaration and
    /// property handles, and this handle's warmed caches. Nothing is
    /// validated again. Later changes to either handle never reach the
    /// other. The staging slot and the extract memo are not carried over.
    /// Additive.
    pub fn fork(&self) -> ModelManagerHandle {
        ModelManagerHandle {
            manager: self.manager.fork(),
            epoch: 0,
            staged: StagedModelFiles::default(),
            dcs_memo: std::cell::RefCell::new(None),
        }
    }
}

impl ModelManagerHandle {
    /// A model file, by its handle; the same [`unknown`] `TypeNotFound` every
    /// other by-handle lookup here throws for one that names nothing.
    pub(crate) fn require_file(&self, model_file: u32) -> Result<&concerto_core::ModelFile> {
        let id = ModelFileId::from_index(model_file);
        self.manager
            .file(id)
            .ok_or_else(|| unknown(Node::ModelFile(id)))
    }
}
