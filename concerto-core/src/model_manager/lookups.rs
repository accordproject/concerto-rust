//! Lookups by name and by handle: model files, declarations, properties,
//! type-name resolution and the models' AST.

use super::{
    AstOptions, ClassDeclaration, ClassKind, ClassLike, ContractError, DeclId, Declaration,
    EXCLUDE_NS, EnumDeclaration, Error, ErrorKind, FileSlot, ModelFile, ModelFileId, ModelManager,
    Node, PropId, Property, ResolutionContext, Result, Value, already_exists, get_namespace,
    is_primitive_type, metamodel_util, namespace_of, qualify, short_name, unknown,
};
#[cfg(feature = "js-compat")]
use crate::introspect::model_file::TypeTarget;

impl ModelManager {
    /// The loaded model file for a namespace, if there is one.
    pub fn model_file(&self, namespace: &str) -> Option<&ModelFile> {
        self.model_file_id(namespace).and_then(|id| self.file(id))
    }

    /// Every loaded model file, including the built-in decorator and root
    /// models, in the order they were loaded.
    pub fn model_files(&self) -> impl Iterator<Item = &ModelFile> {
        self.files.iter().map(|slot| &*slot.model_file)
    }

    /// TS: `BaseModelManager.getModelFileByFileName(fileName)`:
    /// `this.getModelFiles().filter(mf => mf.getName() === fileName)[0]`, so
    /// the first non-system model file whose `getName()` equals `file_name`,
    /// or `None`, also for a system file's name.
    pub fn model_file_by_file_name(&self, file_name: &str) -> Option<&ModelFile> {
        self.model_file_by_optional_file_name(Some(file_name))
    }

    js_compat_pub! {
        /// [`Self::model_file_by_file_name`] for a `fileName` that may be JS
        /// `undefined`: `None` finds the first non-system model file loaded
        /// without a file name, as TS's `mf.getName() === fileName` does.
        pub fn model_file_by_optional_file_name(
            &self,
            file_name: Option<&str>,
        ) -> Option<&ModelFile> {
            self.user_model_files()
                .find(|mf| mf.file_name() == file_name)
        }
    }

    /// The handle of the loaded model file for a namespace, if there is one.
    pub fn model_file_id(&self, namespace: &str) -> Option<ModelFileId> {
        self.namespaces.get(namespace).copied()
    }

    /// The handle of a declaration, by its fully-qualified name. The lookup is
    /// exact, as [`ModelManager::get_declaration`]'s is.
    pub fn declaration_id(&self, fqn: &str) -> Option<DeclId> {
        self.local_type(self.model_file_id(namespace_of(fqn))?, short_name(fqn))
    }

    /// The model file a handle names.
    pub fn file(&self, id: ModelFileId) -> Option<&ModelFile> {
        self.files.get(id.slot()).map(|slot| &*slot.model_file)
    }

    /// The declaration a handle names.
    pub fn declaration(&self, id: DeclId) -> Option<&Declaration> {
        let slot = self.declarations.get(id.slot())?;
        self.file(slot.model_file)?.declarations().get(slot.index)
    }

    js_compat_pub! {
        /// The property a handle names, an enum's values included.
        pub fn property_by_id(&self, id: PropId) -> Option<&Property> {
            let slot = self.properties.get(id.slot())?;
            ClassLike::from_declaration(self.declaration(slot.declaration)?)?
                .own_properties()
                .get(slot.index)
        }
    }

    /// The handles of a model file's declarations, in the order they appear
    /// in the file. None for a handle the manager never handed out.
    #[cfg(feature = "js-compat")]
    pub fn declaration_ids(&self, file: ModelFileId) -> impl Iterator<Item = DeclId> + use<> {
        self.files
            .get(file.slot())
            .map_or(0..0, |slot| slot.declarations.clone())
            .map(DeclId)
    }

    /// Every class-like or enum declaration of the model files whose
    /// namespace is not in `EXCLUDE_NS`, in file order, then declaration
    /// order.
    ///
    /// TS: `Introspector.getClassDeclarations`: `getAllDeclarations()` less
    /// maps and scalars, over `modelManager.getModelFiles()`.
    #[cfg(feature = "js-compat")]
    pub fn class_declarations(&self) -> impl Iterator<Item = DeclId> + '_ {
        self.all_class_like().map(|(id, _, _)| id)
    }

    js_compat_pub! {
        /// The handles of a class declaration's own properties, in the order they
        /// are declared. None for any other declaration, or for a handle the
        /// manager never handed out.
        pub fn property_ids(&self, declaration: DeclId) -> impl Iterator<Item = PropId> + use<> {
            self.declarations
                .get(declaration.slot())
                .map_or(0..0, |slot| slot.properties.clone())
                .map(PropId)
        }
    }

    js_compat_pub! {
        /// The model file a declaration belongs to.
        ///
        /// TS: Declaration.getModelFile (src/introspect/declaration.ts)
        pub fn model_file_of(&self, declaration: DeclId) -> Option<ModelFileId> {
            self.declarations
                .get(declaration.slot())
                .map(|slot| slot.model_file)
        }
    }

    js_compat_pub! {
        /// The declaration a property belongs to.
        ///
        /// TS: Property.getParent (src/introspect/property.ts)
        pub fn parent_of(&self, property: PropId) -> Option<DeclId> {
            self.properties
                .get(property.slot())
                .map(|slot| slot.declaration)
        }
    }

    js_compat_pub! {
        /// A property's own `defaultValue`, read straight off its raw AST.
        ///
        /// TS: `Field.getDefaultValue` reads `this.ast.defaultValue` whatever
        /// the property's kind, a JSON `null` counting as absent. The raw
        /// AST is read because the generated `mm::DateTimeProperty` has no
        /// `defaultValue` field. `None` for an unknown handle.
        pub fn property_default_value(&self, id: PropId) -> Option<&Value> {
            let slot = self.properties.get(id.slot())?;
            self.declaration_ast(slot.declaration)?
                .get("properties")?
                .get(slot.index)?
                .get("defaultValue")
                .filter(|v| !v.is_null())
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getType(qualifiedName)`: the model file of the
        /// name's namespace, then its `getType(qualifiedName)`, each failure
        /// its own `TypeNotFoundException`.
        pub fn get_type_declaration(&self, qualified_name: &str) -> Result<DeclId> {
            let namespace = get_namespace(Some(qualified_name))?;
            let Some(file) = self.model_file_id(namespace) else {
                return Err(ContractError::type_not_found(
                    "modelmanager-gettype-noregisteredns",
                    vec![("type", qualified_name.to_string())],
                    qualified_name.to_string(),
                    None,
                )
                .into());
            };
            match ResolutionContext::get_type(self, &Node::ModelFile(file), Some(qualified_name))? {
                Some(Node::Declaration(id)) => Ok(id),
                _ => Err(ContractError::type_not_found(
                    "modelmanager-gettype-notypeinns",
                    vec![
                        ("type", short_name(qualified_name).to_string()),
                        ("namespace", namespace.to_string()),
                    ],
                    qualified_name.to_string(),
                    None,
                )
                .into()),
            }
        }
    }

    /// Every declaration of every loaded model file, the system models
    /// included, with its fully-qualified name: files in load order, then
    /// declarations in AST order.
    pub fn declarations(&self) -> impl Iterator<Item = (String, &Declaration)> {
        self.declarations_in(self.files.iter())
            .map(|(_, fqn, declaration)| (fqn.to_string(), declaration))
    }

    /// Every declaration of the files `files` yields, with its handle and
    /// its fully-qualified name borrowed from the arena: files in the
    /// order given, then declarations in AST order.
    pub(super) fn declarations_in<'a>(
        &'a self,
        files: impl Iterator<Item = &'a FileSlot> + 'a,
    ) -> impl Iterator<Item = (DeclId, &'a str, &'a Declaration)> + 'a {
        files.flat_map(move |file| {
            file.declarations.clone().map(DeclId).filter_map(move |id| {
                let slot = self.declarations.get(id.slot())?;
                let declaration = file.model_file.declarations().get(slot.index)?;
                Some((id, &*slot.fqn, declaration))
            })
        })
    }

    /// The arena slots of the loaded model files outside `EXCLUDE_NS`, in
    /// load order ([`ModelManager::user_model_files`]).
    pub(super) fn user_file_slots(&self) -> impl Iterator<Item = &FileSlot> {
        self.files
            .iter()
            .filter(|slot| !EXCLUDE_NS.contains(&slot.model_file.namespace()))
    }

    /// The class declarations of one kind, with their fully-qualified names,
    /// across the user models (the system models left out), in load order.
    /// The kind is matched exactly, not through inheritance.
    ///
    /// TS: `BaseModelManager.getAssetDeclarations`, `getConceptDeclarations`
    /// and the other `get<Kind>Declarations`.
    pub fn class_declarations_of_kind(
        &self,
        kind: ClassKind,
    ) -> impl Iterator<Item = (String, &ClassDeclaration)> {
        self.declarations_in(self.user_file_slots())
            .filter_map(move |(_, fqn, declaration)| {
                let class = declaration.as_class()?;
                (class.kind() == kind).then(|| (fqn.to_string(), class))
            })
    }

    /// The enum declarations of the user models (the system models left
    /// out), with their fully-qualified names, in load order.
    ///
    /// TS: `BaseModelManager.getEnumDeclarations`.
    pub fn enum_declarations(&self) -> impl Iterator<Item = (String, &EnumDeclaration)> {
        self.declarations_in(self.user_file_slots())
            .filter_map(|(_, fqn, declaration)| Some((fqn.to_string(), declaration.as_enum()?)))
    }

    /// The loaded model files outside `EXCLUDE_NS`, in load order: what TS's
    /// `getModelFiles()` returns.
    pub(super) fn user_model_files(&self) -> impl Iterator<Item = &ModelFile> {
        self.user_file_slots().map(|slot| &*slot.model_file)
    }

    /// Every loaded model's AST, in load order, in the metamodel's `Models`
    /// envelope. [`AstOptions`] chooses whether the system models are
    /// included and whether type names are resolved to their namespaces.
    ///
    /// TS: `BaseModelManager.getAst(resolve, includeConcertoNamespaces)`.
    pub fn ast(&self, options: AstOptions) -> Result<Value> {
        self.models_ast(options.resolve, options.include_system_models)
    }

    /// Looks up a declaration by its fully-qualified name.
    ///
    /// Namespace versions are mandatory in Concerto v4, so the lookup is
    /// exact: the name must be written with the versioned namespace it was
    /// declared in.
    pub fn get_declaration(&self, fqn: &str) -> Result<&Declaration> {
        self.declaration_id(fqn)
            .and_then(|id| self.declaration(id))
            .ok_or_else(|| Error::type_not_found(fqn.to_string()))
    }

    /// TS: ModelFile.getFullyQualifiedTypeName (src/introspect/modelfile.ts):
    /// a primitive's own name, an imported name's fully-qualified name, a
    /// local declaration's fully-qualified name, or `None` (JS `null`), for
    /// `type_name` as written in the model file of `namespace`. `None` too
    /// when no model file has that namespace.
    pub fn model_file_fully_qualified_type_name(
        &self,
        namespace: &str,
        type_name: &str,
    ) -> Option<String> {
        self.model_file(namespace)?
            .fully_qualified_type_name(type_name)
    }

    /// Resolves a type name, as written inside `in_namespace`, to its
    /// fully-qualified name, using the primitives, named imports and local
    /// declarations (short or qualified) the model file can see
    /// ([`ModelFile::fully_qualified_type_name`]).
    pub fn resolve_type_name(&self, in_namespace: &str, short: &str) -> Result<String> {
        self.resolve_type_name_at(in_namespace, short, None)
    }

    js_compat_pub! {
        /// [`ModelManager::resolve_type_name`], with the AST node's
        /// `location`, copied into the error raised when the namespace is not
        /// registered; `None` where the caller has no node in scope.
        pub fn resolve_type_name_at(
            &self,
            in_namespace: &str,
            short: &str,
            location: Option<crate::json::Value>,
        ) -> Result<String> {
            self.resolve_type_name_lazy(in_namespace, short, || location)
        }
    }

    /// [`ModelManager::resolve_type_name_at`], building the location only
    /// when the error that carries it is raised.
    pub(crate) fn resolve_type_name_lazy(
        &self,
        in_namespace: &str,
        short: &str,
        location: impl FnOnce() -> Option<crate::json::Value>,
    ) -> Result<String> {
        let mf = self.model_file(in_namespace).ok_or_else(|| {
            // TS: `BaseModelManager.getType`'s unregistered-namespace error.
            let fqn = qualify(in_namespace, short);
            ContractError::type_not_found(
                "modelmanager-gettype-noregisteredns",
                vec![("type", fqn.clone())],
                fqn,
                location(),
            )
        })?;

        mf.fully_qualified_type_name(short)
            .ok_or_else(|| Error::type_not_found(qualify(in_namespace, short)))
    }

    /// The handle `getType(fqn)` finds: the exact-name lookup when it
    /// succeeds (always `get_type_declaration`'s answer too), else
    /// [`ModelManager::get_type_declaration`] itself, for its errors.
    pub(super) fn type_declaration_impl(&self, fqn: &str) -> Result<DeclId> {
        match self.declaration_id(fqn) {
            Some(id) => Ok(id),
            None => self.get_type_declaration(fqn),
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager.getType(qualifiedName)`'s handle, found by
        /// the exact-name lookup first: the same handle and errors as
        /// [`ModelManager::get_type_declaration`].
        pub fn type_declaration(&self, fqn: &str) -> Result<DeclId> {
            self.type_declaration_impl(fqn)
        }
    }

    js_compat_pub! {
        /// TS `BaseModelManager._throwAlreadyExists(modelFile)`: the plain
        /// `Error` for a model file named `new_file_name` declaring
        /// `namespace`, which the model file registered under it already
        /// declares; `Ok` when nothing is registered under `namespace`.
        pub fn check_namespace_available(
            &self,
            namespace: &str,
            new_file_name: Option<&str>,
        ) -> Result<()> {
            match self.model_file(namespace) {
                Some(existing) => Err(already_exists(namespace, new_file_name, existing)),
                None => Ok(()),
            }
        }
    }

    /// TS `BaseModelManager.getType(qualifiedName)`, answered by name:
    /// the fully-qualified name of the declaration
    /// [`ModelManager::type_declaration`] finds, with its
    /// `TypeNotFoundException`s. The view maps the name to its own
    /// declaration view.
    #[cfg(feature = "js-compat")]
    pub fn type_declaration_name(&self, fqn: &str) -> Result<String> {
        let id = self.type_declaration_impl(fqn)?;
        self.declaration_fqn(id)
    }

    /// TS `ModelFile.getType(type)` of the model file `file`, by name: a
    /// primitive's own name (no dot), the fully-qualified name of the local
    /// or imported declaration it resolves to, or `None` (TS `null`).
    #[cfg(feature = "js-compat")]
    pub fn model_file_type_name(
        &self,
        file: ModelFileId,
        type_name: &str,
    ) -> Result<Option<String>> {
        match ResolutionContext::get_type(self, &Node::ModelFile(file), Some(type_name))? {
            Some(Node::Primitive(primitive)) => Ok(Some(primitive.to_string())),
            Some(Node::Declaration(id)) => self.declaration_fqn(id).map(Some),
            Some(_) | None => Ok(None),
        }
    }

    /// TS `ModelFile.resolveType(context, type, fileLocation)` of the model
    /// file `file`: a primitive passes; an imported name must resolve in its
    /// namespace's model file ([`ModelManager::resolve_type`]); any other
    /// name must be declared locally, or the `IllegalModelException`
    /// `modelfile-resolvetype-undecltype` naming this file is raised, at
    /// `location`.
    #[cfg(feature = "js-compat")]
    pub fn model_file_resolve_type(
        &self,
        file: ModelFileId,
        context: &str,
        type_name: &str,
        location: Option<crate::json::Value>,
    ) -> Result<()> {
        if is_primitive_type(type_name) {
            return Ok(());
        }
        let mf = self
            .file(file)
            .ok_or_else(|| unknown(Node::ModelFile(file)))?;
        match mf.type_target(type_name) {
            Some(TypeTarget::Imported { namespace, name }) => {
                return self
                    .resolve_type(context, &qualify(namespace, name))
                    .map(|_| ());
            }
            Some(TypeTarget::Primitive | TypeTarget::Local(_)) => return Ok(()),
            None => {}
        }
        let mut err = ContractError::new(
            ErrorKind::IllegalModel,
            "modelfile-resolvetype-undecltype",
            vec![
                ("type", type_name.to_string()),
                ("context", context.to_string()),
            ],
        );
        err.model_file = Some(mf.file_name().map(str::to_string));
        err.location = location;
        Err(err.into())
    }

    /// The handle of the declaration a model file's `getLocalType(type)`
    /// finds: `type` is prefixed with the file's namespace unless it already
    /// starts with it.
    ///
    /// TS: ModelFile.getLocalType (src/introspect/modelfile.ts)
    pub(super) fn local_type(&self, file: ModelFileId, type_name: &str) -> Option<DeclId> {
        let slot = self.files.get(file.slot())?;
        self.decl_id_at(slot, slot.model_file.local_type_index(type_name)?)
    }

    /// The handle of the declaration at `index` in `slot`'s model file.
    pub(super) fn decl_id_at(&self, slot: &FileSlot, index: usize) -> Option<DeclId> {
        let index = u32::try_from(index).ok()?;
        Some(DeclId(slot.declarations.start.checked_add(index)?))
    }

    /// The fully qualified name of a declaration: its model file's namespace,
    /// then its name.
    ///
    /// TS: Declaration.getFullyQualifiedName (src/introspect/declaration.ts)
    pub(super) fn declaration_fqn(&self, id: DeclId) -> Result<String> {
        self.decl_fqn(id).map(str::to_string)
    }

    js_compat_pub! {
        /// A declaration's fully-qualified name, borrowed from the arena,
        /// where it is built once when the file is registered.
        pub fn decl_fqn(&self, id: DeclId) -> Result<&str> {
            self.declarations
                .get(id.slot())
                .map(|slot| &*slot.fqn)
                .ok_or_else(|| unknown(Node::Declaration(id)))
        }
    }

    /// TS `BaseModelManager.resolveType(context, type)`: a primitive passes
    /// through; otherwise `type` must name a registered namespace and a type
    /// local to that namespace's file (an imported name is rejected).
    /// `context` only words the error message.
    pub fn resolve_type(&self, context: &str, type_name: &str) -> Result<String> {
        if is_primitive_type(type_name) {
            return Ok(type_name.to_string());
        }
        let namespace = get_namespace(Some(type_name))?;
        let Some(model_file) = self.model_file(namespace) else {
            return Err(ContractError::new(
                ErrorKind::IllegalModel,
                "modelmanager-resolvetype-nonsfortype",
                vec![
                    ("type", type_name.to_string()),
                    ("context", context.to_string()),
                ],
            )
            .into());
        };
        if model_file.is_local_type(type_name) {
            return Ok(type_name.to_string());
        }
        Err(ContractError::new(
            ErrorKind::IllegalModel,
            "modelmanager-resolvetype-notypeinnsforcontext",
            vec![
                ("context", context.to_string()),
                ("type", type_name.to_string()),
                ("namespace", model_file.namespace().to_string()),
            ],
        )
        .into())
    }

    /// The body of [`ModelManager::ast`]: [`ModelManager::model_asts`] in the
    /// metamodel's `Models` envelope.
    pub(crate) fn models_ast(
        &self,
        resolve: bool,
        include_concerto_namespaces: bool,
    ) -> Result<Value> {
        Ok(crate::json!({
            "$class": "concerto.metamodel@1.0.0.Models",
            "models": self.model_asts(resolve, include_concerto_namespaces)?,
        }))
    }

    /// The models of [`ModelManager::models_ast`]'s envelope, without it:
    /// what the decorator command sets walk.
    pub(crate) fn model_asts(
        &self,
        resolve: bool,
        include_concerto_namespaces: bool,
    ) -> Result<Vec<Value>> {
        // TS re-reads `getAst(false, true)` in every `resolveMetaModel`
        // call, but nothing changes in between, so one borrowed snapshot
        // serves every file.
        let prior_models = if resolve {
            Some(self.prior_models())
        } else {
            None
        };
        let mut models = Vec::new();
        for mf in self.model_files() {
            if !include_concerto_namespaces && EXCLUDE_NS.contains(&mf.namespace()) {
                continue;
            }
            models.push(match &prior_models {
                Some(prior_models) => metamodel_util::resolve_local_names(prior_models, mf.ast())?,
                None => mf.ast().clone(),
            });
        }
        Ok(models)
    }

    /// TS `BaseModelManager.resolveMetaModel(metaModel)`: `meta_model` with
    /// every type name resolved to its declaring namespace, against this
    /// manager's registered models (`MetaModelUtil.resolveLocalNames`,
    /// `metamodel_util`).
    pub fn resolve_meta_model(&self, meta_model: &Value) -> Result<Value> {
        metamodel_util::resolve_local_names(&self.prior_models(), meta_model)
    }

    /// The models `resolve_meta_model` resolves against (TS
    /// `this.getAst(false, true).models`), borrowed and indexed by
    /// namespace; the first model with a namespace wins, as TS's
    /// `Array.find` does.
    pub(super) fn prior_models(&self) -> metamodel_util::PriorModels<'_> {
        let mut prior_models = metamodel_util::PriorModels::default();
        for mf in self.model_files() {
            let ast = mf.ast();
            if let Some(namespace) = ast.get("namespace").and_then(Value::as_str) {
                prior_models.entry(namespace).or_insert(ast);
            }
        }
        prior_models
    }
}
