//! Arena answers to the model queries the TS views make (BC-52).
//!
//! Split out of `lib.rs`; the crate root glob-imports it.

use super::*;

// ---------------------------------------------------------------------------
// Arena answers (BC-52)
// ---------------------------------------------------------------------------
//
// `ModelUtil.isAssignableTo`, `isEnum`, `isMap`, `isScalar` and
// `isValidMapKeyScalar`, `ScalarDeclaration.validate`, `Decorator.validate`
// and `ClassDeclaration.getAssignableClassDeclarations`/`getDirectSubclasses`
// are methods of the manager's handle, taking the model file, declaration or
// property handle the view looked up, and answered by the arena and its
// caches ([`ResolutionContext`] for [`ModelManager`]) rather than by JS
// callbacks. `isAssignableTo` still reads the property's own type
// (`getFullyQualifiedTypeName`) in TS, since the serializer hands it
// stand-ins for relationship map values.

impl ModelManagerHandle {
    /// TS `ModelUtil.isEnum(field)`'s `field.getParent().getModelFile()
    /// .getType(field.getType())`, by the model file's handle and the
    /// field's type name (`None`: a nullish type, as an enum value has).
    /// A map key or value type crosses the same way as a property.
    pub(crate) fn field_type(
        &self,
        model_file: u32,
        type_name: Option<&str>,
    ) -> Result<Option<Node>> {
        Ok(ResolutionContext::get_type(
            &self.manager,
            &Node::ModelFile(ModelFileId::from_index(model_file)),
            type_name,
        )?)
    }
}

#[wasm_bindgen]
impl ModelManagerHandle {
    /// TS: `ModelUtil.isAssignableTo(modelFile, typeName, property)`, for the
    /// model file by handle, and `property`'s fully qualified type name,
    /// which the view reads with `property.getFullyQualifiedTypeName()` (the
    /// property may be a relationship map value's stand-in): `typeName`'s
    /// declaration in the model file, or one of its super types, must be
    /// that type ([`mu::is_assignable_to_type`]).
    #[wasm_bindgen(js_name = modelUtilIsAssignableTo)]
    pub fn model_util_is_assignable_to(
        &self,
        model_file: u32,
        type_name: &str,
        property_type: &str,
    ) -> JsResult<bool> {
        run(|| {
            Ok(mu::is_assignable_to_type(
                &self.manager,
                &Node::ModelFile(ModelFileId::from_index(model_file)),
                type_name,
                property_type,
            )?)
        })
    }

    /// TS: `ModelUtil.isEnum(field)`: whether the field's type is an enum;
    /// `undefined` when the type is not found. `model_file` is the handle of
    /// the model file of the field's parent, `type_name` the field's type.
    #[wasm_bindgen(js_name = modelUtilIsEnum)]
    pub fn model_util_is_enum(
        &self,
        model_file: u32,
        type_name: Option<String>,
    ) -> JsResult<JsValue> {
        run(|| {
            let found = match self.field_type(model_file, type_name.as_deref())? {
                Some(declaration) => Some(ResolutionContext::is_enum(&self.manager, &declaration)?),
                None => None,
            };
            Ok(js_opt_bool(found))
        })
    }

    /// TS: `ModelUtil.isMap(field)`, as [`Self::model_util_is_enum`];
    /// `undefined` when the type is not found or is a primitive.
    #[wasm_bindgen(js_name = modelUtilIsMap)]
    pub fn model_util_is_map(
        &self,
        model_file: u32,
        type_name: Option<String>,
    ) -> JsResult<JsValue> {
        run(|| {
            let found = match self.field_type(model_file, type_name.as_deref())? {
                Some(declaration) => {
                    ResolutionContext::is_map_declaration(&self.manager, &declaration)?
                }
                None => None,
            };
            Ok(js_opt_bool(found))
        })
    }

    /// TS: `ModelUtil.isScalar(field)`, as [`Self::model_util_is_enum`];
    /// `undefined` when the type is not found or is a primitive.
    #[wasm_bindgen(js_name = modelUtilIsScalar)]
    pub fn model_util_is_scalar(
        &self,
        model_file: u32,
        type_name: Option<String>,
    ) -> JsResult<JsValue> {
        run(|| {
            let found = match self.field_type(model_file, type_name.as_deref())? {
                Some(declaration) => {
                    ResolutionContext::is_scalar_declaration(&self.manager, &declaration)?
                }
                None => None,
            };
            Ok(js_opt_bool(found))
        })
    }

    /// TS: `ModelUtil.isValidMapKeyScalar(decl)` for a declaration by
    /// handle: whether it is a String or DateTime scalar.
    #[wasm_bindgen(js_name = modelUtilIsValidMapKeyScalar)]
    pub fn model_util_is_valid_map_key_scalar(&self, declaration: u32) -> JsResult<JsValue> {
        run(|| {
            let node = Node::Declaration(DeclId::from_index(declaration));
            Ok(js_opt_bool(mu::is_valid_map_key_scalar(
                &self.manager,
                Some(&node),
            )?))
        })
    }

    /// TS: `ScalarDeclaration.validate`, after `super.validate()`: no two
    /// declarations of the scalar's model file share a fully qualified name.
    #[wasm_bindgen(js_name = scalarDeclarationValidate)]
    pub fn scalar_declaration_validate(&self, declaration: u32) -> JsResult<()> {
        run(|| {
            Ok(ScalarDeclaration::validate(
                &self.manager,
                &Node::Declaration(DeclId::from_index(declaration)),
            )?)
        })
    }

    /// TS: `ClassDeclaration.getAssignableClassDeclarations`, as the fully
    /// qualified names of the declaration and of every declaration that
    /// (transitively) extends it, in TS's order
    /// ([`ModelManager::assignable_ids`], over the cached subclass map). A
    /// cyclic chain below it is the BC-11 `IllegalModelException`.
    #[wasm_bindgen(js_name = classDeclarationGetAssignableClassDeclarations)]
    pub fn class_declaration_get_assignable_class_declarations(
        &self,
        declaration: u32,
    ) -> JsResult<Vec<String>> {
        run(|| {
            self.manager
                .assignable_ids(DeclId::from_index(declaration))?
                .into_iter()
                .map(|id| Ok(self.manager.decl_fqn(id)?.to_string()))
                .collect()
        })
    }

    /// TS: `ClassDeclaration.getDirectSubclasses`, as the fully qualified
    /// names of the declarations that directly extend the declaration, in
    /// load order ([`ModelManager::direct_subclasses_of`]).
    #[wasm_bindgen(js_name = classDeclarationGetDirectSubclasses)]
    pub fn class_declaration_get_direct_subclasses(
        &self,
        declaration: u32,
    ) -> JsResult<Vec<String>> {
        run(|| {
            self.manager
                .direct_subclasses_of(DeclId::from_index(declaration))?
                .iter()
                .map(|id| Ok(self.manager.decl_fqn(*id)?.to_string()))
                .collect()
        })
    }

    /// TS: `Decorator.validate`. `view` is the processed Decorator; `model_file`
    /// and `model_file_id` are its file and handle, where types resolve;
    /// `context` is the parent's optional `getFullyQualifiedName?.()`, nullish
    /// for a model file's own decorator; `options` is
    /// `getDecoratorValidation()`. Every exception is built by calling back
    /// into `view.handleError`, so its construction, decoration and logging
    /// cannot drift from TS's (BC-14; DV-016). As TS's outer `catch`
    /// re-reports every thrown value through `missingDecorator`, a contract
    /// error is first turned into the JS exception it would be.
    #[wasm_bindgen(js_name = decoratorValidate)]
    pub fn decorator_validate(
        &self,
        view: JsValue,
        model_file: JsValue,
        model_file_id: u32,
        context: JsValue,
        options: JsValue,
    ) -> JsResult<()> {
        let body = || -> Result<()> {
            let missing = level_option(&options, "missingDecorator")?;
            let invalid = level_option(&options, "invalidDecorator")?;
            if missing.is_none() && invalid.is_none() {
                return Ok(());
            }
            let context_name = if nullish(&context) {
                None
            } else {
                Some(js_string(&context)?)
            };
            let check = DecoratorCheck {
                manager: &self.manager,
                view: &view,
                model_file: &model_file,
                file: ModelFileId::from_index(model_file_id),
                invalid: &invalid,
            };
            match check.try_validate(context_name.as_deref()) {
                Ok(()) => Ok(()),
                Err(Error::Js(caught)) => handle_error(&view, &missing, &caught),
                Err(err @ (Error::Contract(_) | Error::Instance(..) | Error::Unsupported(_))) => {
                    let caught = throw(err, Some(&model_file));
                    handle_error(&view, &missing, &caught)
                }
            }
        };
        run_naming(|| model_file.clone(), body)
    }
}
