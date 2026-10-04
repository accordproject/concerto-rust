//! The TS-parity members of the JS-compatibility seam (`js-compat`): TS
//! `BaseModelManager` methods the binding and the oracle harness replay.
//!
//! The members only the oracle harness and this crate's tests call (the
//! `get<Kind>Declarations`, `getModels`, `getAssignableConcreteTypes`,
//! `getSuperTypeDeclaration` and `filter_by_fqn`) exist only with the
//! `js-compat` feature: no binding calls them, and the native API has its
//! own forms.

use super::*;
#[cfg(feature = "js-compat")]
use crate::introspect::DeclarationKind;

/// TS `ModelFileSource` (basemodelmanager.ts): a model file as a
/// `FileLoader` returns it, before it becomes a [`ModelFile`] —
/// [`ModelManager::update_external_models`]' input.
#[cfg(feature = "js-compat")]
#[derive(Debug, Clone)]
pub struct ModelFileSource {
    /// The model's metamodel AST.
    pub ast: Value,
    /// Its CTO source text, when it has one.
    pub definitions: Option<String>,
    /// Its file name (a downloaded file's starts with `@`).
    pub file_name: Option<String>,
}

impl ModelManager {
    /// The [`DeclId`] of `fqn`'s direct super type, or `None` when it has
    /// none.
    ///
    /// TS: `ClassDeclaration.getSuperTypeDeclaration`, inherited unchanged by
    /// `EnumDeclaration`.
    #[cfg(feature = "js-compat")]
    pub fn get_super_type_declaration(&self, fqn: &str) -> Result<Option<DeclId>> {
        let Some(super_fqn) = self.super_type_name(fqn)? else {
            return Ok(None);
        };
        Ok(self.declaration_id(&super_fqn))
    }

    /// TS `BaseModelManager.isAssignableTo(fqn, baseFqn)`, not
    /// [`ModelManager::is_assignable_to`]: `fqn` must resolve to a concrete
    /// type before [`ModelManager::derives_from`] is asked (an abstract
    /// `fqn`, a scalar included, is `false` even against itself), and a
    /// lookup failure is caught. A map declaration answers as
    /// [`ModelManager::derives_from`] does, where TS 5.0.0 throws a
    /// `TypeError` (DV-022).
    #[cfg(feature = "js-compat")]
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

    /// TS `BaseModelManager.getAssignableConcreteTypes(baseFqn)`
    /// (basemodelmanager.ts): every concrete (non-abstract) declaration
    /// assignable to `baseFqn`, `baseFqn` itself included when it is
    /// concrete; empty when `baseFqn` is not in the model (TS catches
    /// `getType`'s error and returns `[]`).
    #[cfg(feature = "js-compat")]
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

    /// TS `getFileNameFromIdentifier` (basemodelmanager.ts, module-private):
    /// the last non-empty `/`- or `\`-delimited segment of `file_identifier`
    /// once its trailing separators are stripped; `file_identifier` itself
    /// when that leaves nothing.
    #[cfg(feature = "js-compat")]
    pub(super) fn file_name_from_identifier(file_identifier: &str) -> String {
        let trimmed = file_identifier.trim_end_matches(['/', '\\']);
        trimmed
            .rsplit(['/', '\\'])
            .find(|segment| !segment.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| file_identifier.to_string())
    }

    /// TS `BaseModelManager.getModels(options)`: every registered model file
    /// but `EXCLUDE_NS`'s, as a `(name, content)` pair, `content` `None`
    /// where TS's `file.definitions` is `undefined`.
    #[cfg(feature = "js-compat")]
    pub fn get_models(&self, include_external_models: bool) -> Vec<(String, Option<String>)> {
        self.user_model_files()
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

    /// TS `BaseModelManager.get<Kind>Declarations()`: the declarations of
    /// every non-system model file whose `$class` is `ctor` exactly, in
    /// registration order.
    #[cfg(feature = "js-compat")]
    pub(super) fn declarations_by_ctor(&self, ctor: &str) -> Vec<DeclId> {
        self.declarations_in(self.user_file_slots())
            .filter(|(_, _, d)| match d {
                Declaration::Class(class) => class.declaration_kind() == ctor,
                Declaration::Enum(_) => ctor == "EnumDeclaration",
                Declaration::Scalar(_) => ctor == "ScalarDeclaration",
                Declaration::Map(_) => ctor == "MapDeclaration",
            })
            .map(|(id, _, _)| id)
            .collect()
    }

    /// TS `BaseModelManager.getAssetDeclarations()`.
    #[cfg(feature = "js-compat")]
    pub fn get_asset_declarations(&self) -> Vec<DeclId> {
        self.declarations_by_ctor("AssetDeclaration")
    }

    /// TS `BaseModelManager.getTransactionDeclarations()`.
    #[cfg(feature = "js-compat")]
    pub fn get_transaction_declarations(&self) -> Vec<DeclId> {
        self.declarations_by_ctor("TransactionDeclaration")
    }

    /// TS `BaseModelManager.getEventDeclarations()`.
    #[cfg(feature = "js-compat")]
    pub fn get_event_declarations(&self) -> Vec<DeclId> {
        self.declarations_by_ctor("EventDeclaration")
    }

    /// TS `BaseModelManager.getParticipantDeclarations()`.
    #[cfg(feature = "js-compat")]
    pub fn get_participant_declarations(&self) -> Vec<DeclId> {
        self.declarations_by_ctor("ParticipantDeclaration")
    }

    /// TS `BaseModelManager.getConceptDeclarations()`.
    #[cfg(feature = "js-compat")]
    pub fn get_concept_declarations(&self) -> Vec<DeclId> {
        self.declarations_by_ctor("ConceptDeclaration")
    }

    /// TS `BaseModelManager.getEnumDeclarations()`.
    #[cfg(feature = "js-compat")]
    pub fn get_enum_declarations(&self) -> Vec<DeclId> {
        self.declarations_by_ctor("EnumDeclaration")
    }

    /// TS `BaseModelManager.filter(predicate, options)`, with the predicate
    /// keyed by fully-qualified name (as the oracle encodes it): a new
    /// manager holding every declaration `keep_fqn` keeps, each file's imports
    /// filtered the same way; a file left empty is dropped. The decorator and
    /// root models the fresh result holds from its constructor are kept whole
    /// (BC-53). Unless `disable_validation`, the files are validated together.
    #[cfg(feature = "js-compat")]
    pub fn filter_by_fqn(
        &self,
        keep_fqn: impl Fn(&str) -> bool,
        disable_validation: bool,
    ) -> Result<Self> {
        self.filter_declarations(|fqn, _| keep_fqn(fqn), disable_validation)
    }

    /// The engine half of TS `BaseModelManager.updateExternalModels`, after
    /// the download: each external model is built as `new ModelFile(this,
    /// ast, definitions, fileName)` and registered without validation (an
    /// update when its namespace is already registered, by `self` or an
    /// earlier download, an add otherwise), then every file is validated.
    /// The model files are returned in order, as TS's `externalModelFiles`.
    /// Any error leaves `self` as it was.
    #[cfg(feature = "js-compat")]
    pub fn update_external_models(
        &mut self,
        external_models: impl IntoIterator<Item = ModelFileSource>,
    ) -> Result<Vec<Arc<ModelFile>>> {
        self.update_external_models_naming_file(external_models)
            .map_err(|(_, err)| err)
    }

    /// [`ModelManager::update_external_models`], with the namespace of
    /// the model file whose validation failed, when that is the failure:
    /// TS's final `validateModelFiles()` throws that file's own
    /// `validate()` error, which names the file.
    #[cfg(feature = "js-compat")]
    pub fn update_external_models_naming_file(
        &mut self,
        external_models: impl IntoIterator<Item = ModelFileSource>,
    ) -> std::result::Result<Vec<Arc<ModelFile>>, (Option<String>, Error)> {
        self.update_external_model_files(external_models.into_iter().map(|source| {
            ModelFile::from_json_with_definitions(&source.ast, source.definitions, source.file_name)
                .map(Arc::new)
        }))
    }

    /// [`ModelManager::update_external_models_naming_file`] for files the
    /// view already staged, so their ASTs are not sent again.
    #[cfg(feature = "js-compat")]
    pub fn update_external_model_files_naming_file(
        &mut self,
        external_model_files: impl IntoIterator<Item = Arc<ModelFile>>,
    ) -> std::result::Result<Vec<Arc<ModelFile>>, (Option<String>, Error)> {
        self.update_external_model_files(external_model_files.into_iter().map(Ok))
    }

    /// The core of [`ModelManager::update_external_models_naming_file`] and
    /// [`ModelManager::update_external_model_files_naming_file`]: each model
    /// file in turn (an error building one fails the update there), then the
    /// validation.
    #[cfg(feature = "js-compat")]
    pub(super) fn update_external_model_files(
        &mut self,
        external_model_files: impl IntoIterator<Item = Result<Arc<ModelFile>>>,
    ) -> std::result::Result<Vec<Arc<ModelFile>>, (Option<String>, Error)> {
        {
            // Each downloaded file is built once and shared between the
            // scratch manager and the list returned; the scratch is rebuilt
            // only for a file that replaces one.
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
                        // `addModelFile`'s already-exists check cannot fire;
                        // `updateModelFile` is the same scratch registration.
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
