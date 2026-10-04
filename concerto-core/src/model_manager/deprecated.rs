//! The deprecated wrappers kept for the pre-D11 names
//! (docs/public-api.md section 5.2).

use super::{ModelFileId, ModelManager, Property, Result, Value};

impl ModelManager {
    /// Loads a model from its JSON AST. Loading two models with the same
    /// namespace is an error.
    ///
    /// Deprecated: TS `addModel` takes CTO text, so the name is kept free for
    /// the CTO follow-up (docs/public-api.md section 5.2).
    #[deprecated(since = "0.1.0", note = "use `add_model_ast`")]
    pub fn add_model(
        &mut self,
        value: &serde_json::Value,
        file_name: Option<String>,
    ) -> Result<()> {
        self.load_model(value, file_name)
    }

    /// Loads a batch of models irrespective of import order between them,
    /// then validates the whole manager once; on any failure the batch has
    /// no effect ([`ModelManager::add_model_asts`]).
    #[deprecated(since = "0.1.0", note = "use `add_model_asts`")]
    pub fn add_models<'a>(
        &mut self,
        models: impl IntoIterator<Item = (&'a serde_json::Value, Option<String>)>,
    ) -> Result<Vec<ModelFileId>> {
        self.load_models(models)
    }

    /// The name of the field that gives `fqn` its identity, its own (`identified
    /// by field`, or `$identifier` when system-identified) or its nearest super
    /// type's; `None` if nothing up the chain declares one. TS
    /// `isIdentified`/`isSystemIdentified` compare this result. Contrast
    /// [`ClassDeclaration::identifier_field_name`], which reads only `fqn`.
    ///
    /// TS: ClassDeclaration.getIdentifierFieldName (src/introspect/classdeclaration.ts)
    #[deprecated(since = "0.1.0", note = "use `identifier_field`")]
    pub fn identifier_field_name(&self, fqn: &str) -> Result<Option<String>> {
        Ok(self.identifier_field(fqn)?.map(str::to_string))
    }

    /// The property with a given name, own or inherited, or `None` if it does
    /// not exist, alongside its declaring type's fully-qualified name (see
    /// [`ModelManager::get_all_properties`]).
    ///
    /// TS: `ClassDeclaration.getProperty`, inherited unchanged by `EnumDeclaration`.
    #[deprecated(since = "0.1.0", note = "use `property`")]
    pub fn get_property(&self, fqn: &str, name: &str) -> Result<Option<(String, Property)>> {
        Ok(self
            .property(fqn, name)?
            .map(|(owner, property)| (owner, property.clone())))
    }

    /// The properties declared directly on `fqn`, not those it inherits.
    ///
    /// TS: `ClassDeclaration.getOwnProperties`, inherited unchanged by `EnumDeclaration`.
    #[deprecated(since = "0.1.0", note = "use `own_properties`")]
    pub fn get_own_properties(&self, fqn: &str) -> Result<Vec<Property>> {
        Ok(self.own_properties(fqn)?.to_vec())
    }

    /// A nested property, following a dotted path (`a.b.c`) through the
    /// declared types of each element but the last.
    ///
    /// TS: `ClassDeclaration.getNestedProperty` (src/introspect/classdeclaration.ts),
    /// inherited unchanged by `EnumDeclaration`.
    #[deprecated(since = "0.1.0", note = "use `property_path`")]
    pub fn get_nested_property(
        &self,
        fqn: &str,
        property_path: &str,
    ) -> Result<(String, Property)> {
        self.property_path(fqn, property_path)
            .map(|(owner, property)| (owner, property.clone()))
    }

    /// The FQN of `fqn`'s direct super type, or `None` when it has none (only
    /// the system model's own `Concept`).
    ///
    /// TS: `ClassDeclaration.getSuperType` (src/introspect/classdeclaration.ts),
    /// inherited unchanged by `EnumDeclaration`.
    #[deprecated(since = "0.1.0", note = "use `super_type`")]
    pub fn get_super_type(&self, fqn: &str) -> Result<Option<String>> {
        self.super_type_name(fqn)
    }

    /// Every super type of `fqn`, from its direct super type up to the root.
    /// A cyclic chain is BC-11's `IllegalModelException` naming the cycle (TS
    /// 5.0.0 runs out of memory, DV-013).
    ///
    /// TS: `ClassDeclaration.getAllSuperTypeDeclarations`
    #[deprecated(since = "0.1.0", note = "use `super_types`")]
    pub fn get_all_super_type_names(&self, fqn: &str) -> Result<Vec<String>> {
        self.super_type_names(fqn)
    }

    /// `fqn` itself, plus every declaration that (transitively) extends it.
    ///
    /// TS: `ClassDeclaration.getAssignableClassDeclarations`, inherited
    /// unchanged by `EnumDeclaration`.
    #[deprecated(since = "0.1.0", note = "use `assignable_types`")]
    pub fn get_assignable_class_declarations(&self, fqn: &str) -> Result<Vec<String>> {
        self.assignable_type_names(fqn)
    }

    /// Just the declarations that directly extend `fqn`, excluding `fqn`
    /// itself.
    ///
    /// TS: `ClassDeclaration.getDirectSubclasses`, inherited unchanged by
    /// `EnumDeclaration`.
    #[deprecated(since = "0.1.0", note = "use `subclasses`")]
    pub fn get_direct_subclasses(&self, fqn: &str) -> Result<Vec<String>> {
        self.direct_subclass_names(fqn)
    }

    /// TS `BaseModelManager.getAst(resolve, includeConcertoNamespaces)`: every
    /// registered file's AST in [`ModelManager::model_files`] order, in the
    /// `Models` envelope, system namespaces only with
    /// `include_concerto_namespaces`. `resolve` runs
    /// [`ModelManager::resolve_meta_model`] first, the only possible failure.
    #[deprecated(since = "0.1.0", note = "use `ast`")]
    pub fn get_ast(&self, resolve: bool, include_concerto_namespaces: bool) -> Result<Value> {
        self.models_ast(resolve, include_concerto_namespaces)
    }
}
