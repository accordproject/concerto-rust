//! The TS-parity members of the JS-compatibility seam (`js-compat`): TS
//! `BaseModelManager` methods the binding and the oracle harness replay.

use super::*;

js_compat_pub! {
    /// TS `ModelFileSource` (basemodelmanager.ts): a model file as a
    /// `FileLoader` returns it, before it becomes a [`ModelFile`] —
    /// [`ModelManager::update_external_models`]' input.
    #[derive(Debug, Clone)]
    pub struct ModelFileSource {
        /// The model's metamodel AST.
        pub ast: Value,
        /// Its CTO source text, when it has one.
        pub definitions: Option<String>,
        /// Its file name (a downloaded file's starts with `@`).
        pub file_name: Option<String>,
    }
}

impl ModelManager {
    js_compat_pub! {
        /// The [`DeclId`] of `fqn`'s direct super type, or `None` when it has
        /// none.
        ///
        /// TS: `ClassDeclaration.getSuperTypeDeclaration`, inherited unchanged by
        /// `EnumDeclaration`.
        pub fn get_super_type_declaration(&self, fqn: &str) -> Result<Option<DeclId>> {
            let Some(super_fqn) = self.super_type_name(fqn)? else {
                return Ok(None);
            };
            Ok(self.declaration_id(&super_fqn))
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.isAssignableTo(fqn, baseFqn)`
        /// (basemodelmanager.ts). This is a different method from
        /// [`ModelManager::is_assignable_to`] — TS itself gives `ModelManager`
        /// two unrelated `isAssignableTo`s, `ModelUtil`'s own static
        /// (`crate::model_util::is_assignable_to`) and this one: `fqn` must
        /// resolve to a *concrete* (non-abstract) type before
        /// [`ModelManager::derives_from`] is even asked — an abstract `fqn` is
        /// `false` even against itself — and a lookup failure is caught, not
        /// propagated.
        ///
        /// A scalar is abstract here, as TS 5.0.0's
        /// `ScalarDeclaration.isAbstract()` answers `true`, so a scalar `fqn`
        /// is `false` even against itself (P5-98). A map declaration answers
        /// as [`ModelManager::derives_from`] does, where TS 5.0.0 throws a
        /// `TypeError` (its `MapDeclaration` has no `isAbstract`): DV-022,
        /// maintainer-accepted.
        pub fn is_type_assignable_to(&self, fqn: &str, base_fqn: &str) -> bool {
            let Ok(id) = self.get_type_declaration(fqn) else {
                return false;
            };
            if self.declaration(id).is_some_and(|decl| {
                decl.is_scalar_declaration() || decl.as_class().is_some_and(|class| class.is_abstract())
            }) {
                return false;
            }
            self.derives_from(fqn, base_fqn).unwrap_or(false)
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getAssignableConcreteTypes(baseFqn)`
        /// (basemodelmanager.ts): every concrete (non-abstract) declaration
        /// assignable to `baseFqn`, `baseFqn` itself included when it is
        /// concrete; empty when `baseFqn` is not in the model (TS catches
        /// `getType`'s error and returns `[]`).
        pub fn get_assignable_concrete_types(&self, base_fqn: &str) -> Vec<DeclId> {
            let Ok(names) = self.assignable_type_names(base_fqn) else {
                return Vec::new();
            };
            names
                .into_iter()
                .filter_map(|fqn| self.declaration_id(&fqn))
                .filter(|id| {
                    !self
                        .declaration(*id)
                        .and_then(Declaration::as_class)
                        .is_some_and(|class| class.is_abstract())
                })
                .collect()
        }
    }

    /// TS `getFileNameFromIdentifier` (basemodelmanager.ts, module-private):
    /// the last non-empty `/`- or `\`-delimited segment of `file_identifier`
    /// once its trailing separators are stripped; `file_identifier` itself
    /// when that leaves nothing.
    pub(super) fn file_name_from_identifier(file_identifier: &str) -> String {
        let trimmed = file_identifier.trim_end_matches(['/', '\\']);
        trimmed
            .rsplit(['/', '\\'])
            .find(|segment| !segment.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| file_identifier.to_string())
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getModels(options)` (basemodelmanager.ts): every
        /// registered model file but the root and decorator models
        /// (`this.getModelFiles()`'s default excludes `EXCLUDE_NS`), as a
        /// `(name, content)` pair — `content` is `None` exactly where TS's
        /// `file.definitions` is `undefined`. `include_external_models` is TS's
        /// `options.includeExternalModels` (`true` by default there; the oracle
        /// harness always passes it explicitly).
        pub fn get_models(&self, include_external_models: bool) -> Vec<(String, Option<String>)> {
            self.model_files()
                .filter(|mf| !EXCLUDE_NS.contains(&mf.namespace()))
                .filter(|mf| include_external_models || !mf.is_external())
                .map(|mf| {
                    let name = match mf.file_name() {
                        None | Some("UNKNOWN") | Some("") => format!("{}.cto", mf.namespace()),
                        Some(identifier) => Self::file_name_from_identifier(identifier),
                    };
                    (name, mf.definitions().map(str::to_string))
                })
                .collect()
        }
    }

    /// TS `BaseModelManager.get<Kind>Declarations()` (basemodelmanager.ts,
    /// six near-identical methods, each `this.getModelFiles().reduce((prev,
    /// cur) => prev.concat(cur.get<Kind>Declarations()), [])`): every
    /// non-system, non-decorator model file's own declarations whose
    /// constructor name is `ctor` — matched exactly on `$class`, never by
    /// inheritance, the same way every other `{ctor, fqn}` summary in this
    /// port already does (P2-08 review) — concatenated in registration
    /// order.
    pub(super) fn declarations_by_ctor(&self, ctor: &str) -> Vec<DeclId> {
        self.model_files()
            .filter(|mf| !EXCLUDE_NS.contains(&mf.namespace()))
            .flat_map(|mf| {
                let file = self.model_file_id(mf.namespace());
                file.into_iter().flat_map(|f| self.declaration_ids(f))
            })
            .filter(|id| {
                self.declaration(*id).is_some_and(|d| match d {
                    Declaration::Class(class) => class.declaration_kind() == ctor,
                    Declaration::Enum(_) => ctor == "EnumDeclaration",
                    Declaration::Scalar(_) => ctor == "ScalarDeclaration",
                    Declaration::Map(_) => ctor == "MapDeclaration",
                })
            })
            .collect()
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getAssetDeclarations()`.
        pub fn get_asset_declarations(&self) -> Vec<DeclId> {
            self.declarations_by_ctor("AssetDeclaration")
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getTransactionDeclarations()`.
        pub fn get_transaction_declarations(&self) -> Vec<DeclId> {
            self.declarations_by_ctor("TransactionDeclaration")
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getEventDeclarations()`.
        pub fn get_event_declarations(&self) -> Vec<DeclId> {
            self.declarations_by_ctor("EventDeclaration")
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getParticipantDeclarations()`.
        pub fn get_participant_declarations(&self) -> Vec<DeclId> {
            self.declarations_by_ctor("ParticipantDeclaration")
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getConceptDeclarations()`.
        pub fn get_concept_declarations(&self) -> Vec<DeclId> {
            self.declarations_by_ctor("ConceptDeclaration")
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getEnumDeclarations()`.
        pub fn get_enum_declarations(&self) -> Vec<DeclId> {
            self.declarations_by_ctor("EnumDeclaration")
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.filter(predicate, options)` (basemodelmanager.ts):
        /// a scratch manager holding every registered model file's declarations
        /// for which `keep_fqn` is true, filtering each file's own imports the
        /// same way ([`crate::introspect::model_file::ModelFile::filter`]'s
        /// module doc); a file with nothing left is dropped. `predicate` is a
        /// `Declaration -> bool` in TS, keyed here by fully-qualified name
        /// instead, since that is all the oracle's own `predicate` encoding
        /// carries (`tests/oracle/ops.rs`). Every file the fresh result already
        /// holds from its constructor (the decorator and root models) is
        /// skipped, so the result keeps its own copy whole whatever `keep_fqn`
        /// says about its declarations (BC-53, P5-108,
        /// accordproject/concerto-rust#466: TS 5.0.0 skipped only the root
        /// model and threw re-adding the decorator model, so `filter(() =>
        /// true)` failed). The result always starts from a fresh `BaseModelManager`
        /// (TS: `new BaseModelManager({...this.options}, this.processFile)`),
        /// never the receiver's own kind. `disable_validation` is TS's
        /// `options?.disableValidation`; unless set, the filtered files are
        /// validated once, together (TS: `modelManager.addModelFiles(...)`).
        pub fn filter_by_fqn(
            &self,
            keep_fqn: impl Fn(&str) -> bool,
            disable_validation: bool,
        ) -> Result<Self> {
            self.filter_declarations(|fqn, _| keep_fqn(fqn), disable_validation)
        }
    }

    js_compat_pub! {
        /// The Rust half of TS `BaseModelManager.updateExternalModels(options,
        /// fileDownloader)` (basemodelmanager.ts; ledger: HYBRID, the download
        /// stays in JS). `external_models` is what
        /// `downloader.downloadExternalDependencies(...)` resolved to, in order:
        /// each is built as `new ModelFile(this, ast, definitions, fileName)`,
        /// then registered without validation — `updateModelFile(mf, name,
        /// true)` when its namespace is already registered (by `self` or an
        /// earlier download in the same batch), `addModelFile(mf, null, name,
        /// true)` otherwise — and finally every registered model file is
        /// validated (`validateModelFiles`). The model files are returned in the
        /// same order, as TS's `externalModelFiles`.
        ///
        /// Any error leaves `self` exactly as it was, as TS's `catch` restores
        /// `this.modelFiles` before rethrowing.
        pub fn update_external_models(
            &mut self,
            external_models: impl IntoIterator<Item = ModelFileSource>,
        ) -> Result<Vec<Arc<ModelFile>>> {
            self.update_external_models_naming_file(external_models)
                .map_err(|(_, err)| err)
        }
    }

    js_compat_pub! {
        /// [`ModelManager::update_external_models`], with the namespace of
        /// the model file whose validation failed, when that is the failure
        /// (P5-11, accordproject/concerto-rust#287): TS's final
        /// `validateModelFiles()` throws that file's own `validate()` error,
        /// which names the file.
        pub fn update_external_models_naming_file(
            &mut self,
            external_models: impl IntoIterator<Item = ModelFileSource>,
        ) -> std::result::Result<Vec<Arc<ModelFile>>, (Option<String>, Error)> {
            self.update_external_model_files(external_models.into_iter().map(|source| {
                ModelFile::from_json_with_definitions(
                    &source.ast,
                    source.definitions,
                    source.file_name,
                )
                .map(Arc::new)
            }))
        }
    }

    js_compat_pub! {
        /// [`ModelManager::update_external_models_naming_file`] for model
        /// files already built (P5-100, accordproject/concerto-rust#454): the
        /// files the TS view staged when it built each downloaded
        /// `ModelFile`, so their ASTs are not sent and parsed again.
        pub fn update_external_model_files_naming_file(
            &mut self,
            external_model_files: impl IntoIterator<Item = Arc<ModelFile>>,
        ) -> std::result::Result<Vec<Arc<ModelFile>>, (Option<String>, Error)> {
            self.update_external_model_files(external_model_files.into_iter().map(Ok))
        }
    }

    /// The core of [`ModelManager::update_external_models_naming_file`] and
    /// [`ModelManager::update_external_model_files_naming_file`]: each model
    /// file in turn (an error building one fails the update there), then the
    /// validation.
    pub(super) fn update_external_model_files(
        &mut self,
        external_model_files: impl IntoIterator<Item = Result<Arc<ModelFile>>>,
    ) -> std::result::Result<Vec<Arc<ModelFile>>, (Option<String>, Error)> {
        {
            // A-4: each downloaded file is built once and shared (`Arc`)
            // between the scratch manager and the list returned, and the
            // scratch is rebuilt only for a file that replaces one; a new
            // namespace is appended to the scratch this call already owns,
            // which is the same arena `with_model_file_registered` would
            // build, without copying it once per file.
            let mut updated: Option<Self> = None;
            let mut registered = Vec::new();
            for mf in external_model_files {
                let mf = mf.map_err(|err| (None, err))?;
                let replaces = updated
                    .as_ref()
                    .unwrap_or(self)
                    .model_file(mf.namespace())
                    .is_some();
                match updated.as_mut() {
                    Some(scratch)
                        if !replaces && scratch.namespaces.len() == scratch.files.len() =>
                    {
                        // `addModelFile`'s already-exists check cannot fire here.
                        scratch
                            .insert_shared(Arc::clone(&mf))
                            .map_err(|err| (None, err))?;
                    }
                    _ => {
                        // `addModelFile`'s already-exists check cannot fire
                        // here; `updateModelFile` without validation is the
                        // same scratch registration.
                        let next = updated
                            .as_ref()
                            .unwrap_or(self)
                            .with_model_file_registered(Arc::clone(&mf))
                            .map_err(|err| (None, err))?;
                        updated = Some(next);
                    }
                }
                registered.push(mf);
            }
            updated
                .as_ref()
                .unwrap_or(self)
                .validate_models_naming_file()
                .map_err(|(namespace, err)| (Some(namespace), err))?;
            if let Some(updated) = updated {
                self.adopt(updated);
            }
            Ok(registered)
        }
    }
}
