//! The collaborator traits a ported member calls through
//! ([`ResolutionContext`], [`ValidatedElement`]), and the manager's
//! implementation over its arena.

use super::*;

js_compat_pub! {
    /// The collaborator calls a ported member makes. Where TS calls another
    /// model object (a model file, the manager, a parent declaration), the
    /// port calls this trait, so a member does not depend on where the graph
    /// lives. Each method mirrors the TS method it replaces, in snake case,
    /// with the same failure. [`ModelManager`] implements it over its arena,
    /// with [`Node`] as the handle; tests implement it over fakes.
    pub trait ResolutionContext {
        /// A handle to a model element: a model file, a declaration or a property.
        type Node;
        /// What a collaborator call can raise.
        type Error: From<ContractError>;

        /// TS: ModelFile.getType (src/introspect/modelfile.ts). `type_name` is
        /// `None` when TS passes `null` or `undefined`; the result is `None` when
        /// TS returns a nullish value.
        fn get_type(
            &self,
            model_file: &Self::Node,
            type_name: Option<&str>,
        ) -> std::result::Result<Option<Self::Node>, Self::Error>;

        /// TS: ClassDeclaration.getAllSuperTypeDeclarations (src/introspect/classdeclaration.ts)
        #[cfg(feature = "js-compat")]
        fn get_all_super_type_declarations(
            &self,
            declaration: &Self::Node,
        ) -> std::result::Result<Vec<Self::Node>, Self::Error>;

        /// TS: Declaration.getFullyQualifiedName (src/introspect/declaration.ts)
        #[cfg(feature = "js-compat")]
        fn get_fully_qualified_name(
            &self,
            declaration: &Self::Node,
        ) -> std::result::Result<String, Self::Error>;

        /// TS: Property.getFullyQualifiedTypeName (src/introspect/property.ts)
        fn get_fully_qualified_type_name(
            &self,
            property: &Self::Node,
        ) -> std::result::Result<String, Self::Error>;

        /// TS: Property.getParent (src/introspect/property.ts)
        fn get_parent(&self, property: &Self::Node) -> std::result::Result<Self::Node, Self::Error>;

        /// TS: Declaration.getModelFile (src/introspect/declaration.ts)
        fn get_model_file(
            &self,
            declaration: &Self::Node,
        ) -> std::result::Result<Self::Node, Self::Error>;

        /// TS: Property.getType (src/introspect/property.ts). `None` is a nullish
        /// type, as an enum value has.
        fn get_type_name(
            &self,
            property: &Self::Node,
        ) -> std::result::Result<Option<String>, Self::Error>;

        /// TS: Declaration.isEnum (src/introspect/declaration.ts)
        fn is_enum(&self, declaration: &Self::Node) -> std::result::Result<bool, Self::Error>;

        /// TS: `declaration.isMapDeclaration?.()`; `None` when the method is
        /// missing.
        #[cfg(feature = "js-compat")]
        fn is_map_declaration(
            &self,
            declaration: &Self::Node,
        ) -> std::result::Result<Option<bool>, Self::Error>;

        /// TS: `declaration.isScalarDeclaration?.()`; `None` when the method is
        /// missing.
        #[cfg(feature = "js-compat")]
        fn is_scalar_declaration(
            &self,
            declaration: &Self::Node,
        ) -> std::result::Result<Option<bool>, Self::Error>;

        /// TS: `declaration.ast.$class`; `None` when it is not a string.
        #[cfg(feature = "js-compat")]
        fn get_ast_class(
            &self,
            declaration: &Self::Node,
        ) -> std::result::Result<Option<String>, Self::Error>;

        /// TS: ModelFile.getAllDeclarations (src/introspect/modelfile.ts)
        #[cfg(feature = "js-compat")]
        fn get_all_declarations(
            &self,
            model_file: &Self::Node,
        ) -> std::result::Result<Vec<Self::Node>, Self::Error>;
    }
}

js_compat_pub! {
    /// The field or scalar declaration a validator is attached to, as a validator
    /// reads it (TS: `Validator.field`, typed `Property | ScalarDeclaration`).
    ///
    /// Its own trait, not [`ResolutionContext`] methods on a node: TS builds
    /// a validator while it constructs the element (`ScalarDeclaration.process`
    /// runs in the constructor), before the element has a handle. Its
    /// [`FullyQualified`] name is read only when an error is reported.
    pub trait ValidatedElement: FullyQualified {
        /// TS: `this.field?.ast?.defaultValue`; `None` is `undefined`.
        fn default_value(&self) -> std::result::Result<Option<crate::json::Value>, Self::Error>;

        /// TS: `field.getName()`. `StringValidator` and `CollectionSizeValidator`
        /// (unlike `NumberValidator`) pass this as the identifier of every error
        /// their constructor reports, and `StringValidator` passes it again as
        /// the identifier for the `defaultValue` check it runs at load time.
        fn name(&self) -> std::result::Result<String, Self::Error>;
    }
}

impl ModelManager {
    /// The AST node an element was built from, as TS keeps it in `ast`;
    /// `None` for a primitive type name, whose `ast` is `undefined`.
    #[cfg(feature = "js-compat")]
    pub(super) fn node_ast(&self, node: Node) -> Result<Option<&Value>> {
        let found = match node {
            Node::ModelFile(id) => self.file(id).map(ModelFile::ast),
            Node::Declaration(id) => self.declaration_ast(id),
            Node::Property(id) => self.properties.get(id.slot()).and_then(|slot| {
                self.declaration_ast(slot.declaration)?
                    .get("properties")?
                    .get(slot.index)
            }),
            Node::Primitive(_) => return Ok(None),
        };
        found.map(Some).ok_or_else(|| unknown(node))
    }

    /// The AST node of a declaration, within its model file's AST.
    pub(super) fn declaration_ast(&self, id: DeclId) -> Option<&Value> {
        let slot = self.declarations.get(id.slot())?;
        self.file(slot.model_file)?
            .ast()
            .get("declarations")?
            .get(slot.index)
    }
}

/// The manager answers collaborator calls from its own graph. A node kind
/// whose TS object lacks the method answers V8's "is not a function"
/// `TypeError`; a handle this manager never handed out is an error. Every
/// chain has the implicit `Concept` super type.
impl ResolutionContext for ModelManager {
    type Node = Node;
    type Error = Error;

    fn get_type(&self, model_file: &Node, type_name: Option<&str>) -> Result<Option<Node>> {
        let Node::ModelFile(file) = *model_file else {
            return Err(not_a_function("modelFile.getType"));
        };
        let mf = self.file(file).ok_or_else(|| unknown(*model_file))?;
        // A nullish type is not a primitive, not imported, and fails the
        // `type &&` of `isLocalType`.
        let Some(type_name) = type_name else {
            return Ok(None);
        };
        if let Some(&primitive) = PRIMITIVE_TYPES.iter().find(|&&p| p == type_name) {
            return Ok(Some(Node::Primitive(primitive)));
        }
        if let Some(fqn) = mf.find_import(type_name) {
            // `getModelManager().getModelFile(getNamespace(fqn))`, then that
            // file's `getLocalType(fqn)`.
            return Ok(self
                .model_file_id(namespace_of(&fqn))
                .and_then(|other| self.local_type(other, &fqn))
                .map(Node::Declaration));
        }
        Ok(self.local_type(file, type_name).map(Node::Declaration))
    }

    #[cfg(feature = "js-compat")]
    fn get_all_super_type_declarations(&self, declaration: &Node) -> Result<Vec<Node>> {
        let not_a_function = || not_a_function("typeDeclaration.getAllSuperTypeDeclarations");
        let Node::Declaration(id) = *declaration else {
            return Err(not_a_function());
        };
        match self.declaration(id).ok_or_else(|| unknown(*declaration))? {
            // The cached chain, resolved by name as `getType` does; it
            // starts with the type itself.
            Declaration::Class(_) | Declaration::Enum(_) => {
                Ok(self.class_info(self.decl_fqn(id)?)?.chain[1..]
                    .iter()
                    .map(|id| Node::Declaration(*id))
                    .collect())
            }
            Declaration::Scalar(_) | Declaration::Map(_) => Err(not_a_function()),
        }
    }

    #[cfg(feature = "js-compat")]
    fn get_fully_qualified_name(&self, declaration: &Node) -> Result<String> {
        match *declaration {
            Node::Declaration(id) => self.declaration_fqn(id),
            // TS: Property.getFullyQualifiedName (src/introspect/property.ts)
            Node::Property(id) => {
                let (Some(parent), Some(property)) = (self.parent_of(id), self.property_by_id(id))
                else {
                    return Err(unknown(*declaration));
                };
                Ok(format!(
                    "{}.{}",
                    self.declaration_fqn(parent)?,
                    property.name()
                ))
            }
            Node::ModelFile(_) | Node::Primitive(_) => {
                Err(not_a_function("type.getFullyQualifiedName"))
            }
        }
    }

    fn get_fully_qualified_type_name(&self, property: &Node) -> Result<String> {
        let Node::Property(id) = *property else {
            return Err(not_a_function("property.getFullyQualifiedTypeName"));
        };
        let (Some(field), Some(file)) = (
            self.property_by_id(id),
            self.parent_of(id)
                .and_then(|parent| self.model_file_of(parent)),
        ) else {
            return Err(unknown(*property));
        };
        let type_name = field.type_name();
        if let Some(type_name) = type_name
            && is_primitive_type(type_name)
        {
            return Ok(type_name.to_string());
        }
        let mf = self.file(file).ok_or_else(|| unknown(*property))?;
        // TS: ModelFile.getFullyQualifiedTypeName (src/introspect/modelfile.ts)
        let resolved = match type_name {
            None => None,
            Some(type_name) => match mf.find_import(type_name) {
                Some(fqn) => Some(fqn),
                None => self
                    .local_type(file, type_name)
                    .map(|local| self.declaration_fqn(local))
                    .transpose()?,
            },
        };
        // TS: `Property.getFullyQualifiedTypeName` throws a plain `Error`
        // (`property-getfullyqualifiedtypename-notfound`) when
        // `ModelFile.getFullyQualifiedTypeName` returns `null`; an enum
        // value's `null` type renders as `null`.
        resolved.ok_or_else(|| {
            let field = self.property_by_id(id).expect("checked above");
            ContractError::new(
                ErrorKind::InvalidArgument,
                "property-getfullyqualifiedtypename-notfound",
                vec![
                    ("name", field.name().to_string()),
                    ("type", type_name.unwrap_or("null").to_string()),
                ],
            )
            .into()
        })
    }

    fn get_parent(&self, property: &Node) -> Result<Node> {
        let Node::Property(id) = *property else {
            return Err(not_a_function("field.getParent"));
        };
        self.parent_of(id)
            .map(Node::Declaration)
            .ok_or_else(|| unknown(*property))
    }

    fn get_model_file(&self, declaration: &Node) -> Result<Node> {
        let Node::Declaration(id) = *declaration else {
            return Err(not_a_function("getModelFile"));
        };
        self.model_file_of(id)
            .map(Node::ModelFile)
            .ok_or_else(|| unknown(*declaration))
    }

    fn get_type_name(&self, property: &Node) -> Result<Option<String>> {
        let Node::Property(id) = *property else {
            return Err(not_a_function("field.getType"));
        };
        let field = self.property_by_id(id).ok_or_else(|| unknown(*property))?;
        Ok(field.type_name().map(str::to_string))
    }

    fn is_enum(&self, declaration: &Node) -> Result<bool> {
        let Node::Declaration(id) = *declaration else {
            return Err(not_a_function("typeDeclaration.isEnum"));
        };
        let found = self.declaration(id).ok_or_else(|| unknown(*declaration))?;
        Ok(found.is_enum_declaration())
    }

    #[cfg(feature = "js-compat")]
    fn is_map_declaration(&self, declaration: &Node) -> Result<Option<bool>> {
        match *declaration {
            Node::Declaration(id) => Ok(Some(
                self.declaration(id)
                    .ok_or_else(|| unknown(*declaration))?
                    .is_map_declaration(),
            )),
            // No such method on a model file, a property or a string.
            Node::ModelFile(_) | Node::Property(_) | Node::Primitive(_) => Ok(None),
        }
    }

    #[cfg(feature = "js-compat")]
    fn is_scalar_declaration(&self, declaration: &Node) -> Result<Option<bool>> {
        match *declaration {
            Node::Declaration(id) => Ok(Some(
                self.declaration(id)
                    .ok_or_else(|| unknown(*declaration))?
                    .is_scalar_declaration(),
            )),
            // No such method on a model file, a property or a string.
            Node::ModelFile(_) | Node::Property(_) | Node::Primitive(_) => Ok(None),
        }
    }

    #[cfg(feature = "js-compat")]
    fn get_ast_class(&self, declaration: &Node) -> Result<Option<String>> {
        let Some(ast) = self.node_ast(*declaration)? else {
            return Err(ContractError::new(
                ErrorKind::MalformedInput,
                "engine-typeerror-readproperties",
                vec![
                    ("value", "undefined".to_string()),
                    ("property", "$class".to_string()),
                ],
            )
            .into());
        };
        Ok(ast
            .get("$class")
            .and_then(Value::as_str)
            .map(str::to_string))
    }

    #[cfg(feature = "js-compat")]
    fn get_all_declarations(&self, model_file: &Node) -> Result<Vec<Node>> {
        let Node::ModelFile(id) = *model_file else {
            return Err(not_a_function("this.getModelFile().getAllDeclarations"));
        };
        if self.file(id).is_none() {
            return Err(unknown(*model_file));
        }
        Ok(self.declaration_ids(id).map(Node::Declaration).collect())
    }
}
