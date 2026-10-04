//! The `ScalarDeclaration`, `ClassDeclaration` family and `MapDeclaration`
//! bindings.

use super::*;

// ---------------------------------------------------------------------------
// ScalarDeclaration (src/introspect/scalardeclaration.ts)
// ---------------------------------------------------------------------------

/// TS: ScalarDeclaration.process, after `super.process()`. Returns the
/// snapshot `{type, validator, defaultValue}`, where `validator` is `null`,
/// `{kind: "NumberValidator", lowerBound, upperBound}`, or
/// `{kind: "StringValidator"}` (the view builds the `StringValidator`).
#[wasm_bindgen(js_name = scalarDeclarationProcess)]
pub fn scalar_declaration_process(declaration: JsValue) -> JsResult<JsValue> {
    let body = || -> Result<JsValue> {
        let ast = to_json(&get(&declaration, "ast")?)?.unwrap_or(Value::Null);
        let fqn = || {
            js_string(&call(
                &declaration,
                "getFullyQualifiedName",
                &[],
                "this.getFullyQualifiedName",
            )?)
        };
        let processed = ScalarDeclaration::process(&ast, None, &fqn)?;
        let validator = match &processed.validator {
            None => Value::Null,
            Some(ScalarValidator::Number(v)) => {
                let mut snapshot = serde_json::to_value(v).unwrap_or(Value::Null);
                if let Value::Object(map) = &mut snapshot {
                    map.insert("kind".to_string(), json!("NumberValidator"));
                }
                snapshot
            }
            Some(ScalarValidator::String(_)) => json!({ "kind": "StringValidator" }),
        };
        Ok(to_js(&json!({
            "type": processed.scalar_type,
            "validator": validator,
            "defaultValue": processed.default_value,
        })))
    };
    run_naming(
        || get(&declaration, "modelFile").unwrap_or(JsValue::UNDEFINED),
        body,
    )
}

/// TS: ScalarDeclaration.toString
#[wasm_bindgen(js_name = scalarDeclarationToString)]
pub fn scalar_declaration_to_string(declaration: JsValue) -> JsResult<String> {
    run(|| {
        let fqn = js_string(&call(
            &declaration,
            "getFullyQualifiedName",
            &[],
            "this.getFullyQualifiedName",
        )?)?;
        Ok(ScalarDeclaration::to_string(&fqn))
    })
}

// ---------------------------------------------------------------------------
// ClassDeclaration family (src/introspect/classdeclaration.ts and its
// subclasses, enumdeclaration.ts included)
//
// These bindings are handed the JS declaration views: each ports the
// member's own algorithm and makes each collaborator call TS makes as a
// call back onto the JS object, in TS's order (`get`/`call`), since the
// view may be over stubbed collaborators. A pure decision at the end of a
// member (`classDeclarationProcess`'s superType/idField choice, the
// kind-compatibility and identifier-redeclare checks) is a plain function
// of `concerto_core::ClassDeclaration`.

/// The metamodel `$class`'s short name: the text after the last `.`.
pub(crate) fn short_class(ast_class: &str) -> &str {
    ast_class.rsplit('.').next().unwrap_or(ast_class)
}

/// TS: `ClassDeclaration.process`'s superType/idField decision, as
/// `{superType, idField, addIdentifierField, addTimestampField}`.
/// `superType` is the AST's `superType.name` as is (`undefined` stays
/// `undefined`; only `name: null` is no super type), else `null` for the
/// system `Concept` and `'Concept'` otherwise. `addTimestampField` is set
/// for the system `Transaction` and `Event`.
#[wasm_bindgen(js_name = classDeclarationProcess)]
pub fn class_declaration_process(declaration: JsValue) -> JsResult<JsValue> {
    let body = || -> Result<JsValue> {
        let ast = get(&declaration, "ast")?;

        // TS: `if (this.ast.superType) { this.superType = this.ast.superType.name; } else if
        // (!(isSystemModelFile && name === 'Concept')) { this.superType = 'Concept'; }`. The
        // outer test is JS truthiness of the node, and `.name` is stored as is, uncoerced:
        // its truthiness (`_resolveSuperType`), strict non-null (`getProperty`/
        // `getProperties`) and `ToString` (the error messages) can all differ, so the raw
        // `JsValue` goes to the snapshot.
        let super_type_ast = get(&ast, "superType")?;
        let raw_super_type = if super_type_ast.is_truthy() {
            Some(get(&super_type_ast, "name")?)
        } else {
            None
        };
        // `process_decision` only needs to know whether the AST named a
        // super type; `raw_super_type` is what reaches the snapshot.
        let explicit_super_type = raw_super_type.as_ref().map(|_| String::new());
        let is_system_model_file = if explicit_super_type.is_none() {
            let model_file = call(&declaration, "getModelFile", &[], "this.getModelFile")?;
            call(
                &model_file,
                "isSystemModelFile",
                &[],
                "this.modelFile.isSystemModelFile",
            )?
            .is_truthy()
        } else {
            // TS never evaluates `isSystemModelFile()` on this branch.
            false
        };
        // TS: `name === 'Concept'`, a strict equality, so a non-string name
        // never matches and is never coerced.
        let name = get(&declaration, "name")?.as_string().unwrap_or_default();

        // TS: `if (this.ast.identified) { ... }`, JS truthiness.
        let identified = get(&ast, "identified")?;
        let (identified_class, identified_name, raw_identified_name) = if identified.is_truthy() {
            // TS: `this.ast.identified.$class === '...IdentifiedBy'` (strict)
            // and `this.idField = this.ast.identified.name` (uncoerced).
            let class_value = get(&identified, "$class")?;
            let identified_class = class_value.as_string().unwrap_or_default();
            // `raw_identified_name` is `this.ast.identified.name` as TS
            // leaves it, so a falsy value skips `idField`'s later checks
            // rather than naming a property `"undefined"`.
            let raw_identified_name = if identified_class == "concerto.metamodel@1.0.0.IdentifiedBy"
            {
                Some(get(&identified, "name")?)
            } else {
                None
            };
            let identified_name = raw_identified_name.as_ref().map(|_| String::new());
            (Some(identified_class), identified_name, raw_identified_name)
        } else {
            (None, None, None)
        };

        let fqn = receiver(&get(&declaration, "fqn")?, "this.fqn", "toString")?;

        let decision = concerto_core::ClassDeclaration::process_decision(
            explicit_super_type.as_deref(),
            is_system_model_file,
            &name,
            identified_class.as_deref(),
            identified_name.as_deref(),
            &fqn,
        );

        // The AST's own `.name` value when it named a super type, else
        // `process_decision`'s decision.
        let super_type_js = match raw_super_type {
            Some(v) => v,
            None => decision
                .super_type
                .as_deref()
                .map_or(JsValue::NULL, JsValue::from_str),
        };
        // The AST's own `.identified.name` value for an `IdentifiedBy`, else
        // `process_decision`'s decision (`$identifier`, or `null`).
        let id_field_js = match raw_identified_name {
            Some(v) => v,
            None => decision
                .id_field
                .as_deref()
                .map_or(JsValue::NULL, JsValue::from_str),
        };

        let result = Object::new();
        set(&result, "superType", &super_type_js);
        set(&result, "idField", &id_field_js);
        set(
            &result,
            "addIdentifierField",
            &JsValue::from_bool(decision.add_identifier_field),
        );
        set(
            &result,
            "addTimestampField",
            &JsValue::from_bool(decision.add_timestamp_field),
        );
        Ok(result.into())
    };
    run_naming(
        || get(&declaration, "modelFile").unwrap_or(JsValue::UNDEFINED),
        body,
    )
}

/// TS: the super-type identifier redeclaration check in
/// `ClassDeclaration.validate`, under `superType.isIdentified()` (the
/// caller's check): `true` when the super type's identifier cannot be
/// redeclared.
#[wasm_bindgen(js_name = classDeclarationIdentifierRedeclareConflict)]
pub fn class_declaration_identifier_redeclare_conflict(
    child_is_system_identified: bool,
    super_is_system_identified: bool,
    super_is_explicitly_identified: bool,
) -> bool {
    concerto_core::ClassDeclaration::identifier_redeclare_conflict(
        child_is_system_identified,
        super_is_system_identified,
        super_is_explicitly_identified,
    )
}

/// `target[name] = value`, the write-back `_resolveSuperType` makes onto its
/// `this.superTypeDeclaration` field.
pub(crate) fn set_property(target: &JsValue, name: &str, value: &JsValue) -> Result<()> {
    Reflect::set(target, &JsValue::from_str(name), value).map_err(Error::Js)?;
    Ok(())
}

/// An `IllegalModelException` as `new IllegalModelException(message,
/// modelFile, location)` builds it: `model_file: Some(None)` makes [`throw`]
/// attach the caller's JS model file. `code` is `"pre-port"`: `message` is
/// TS's own string concatenation.
pub(crate) fn illegal_model_error(message: String, location: Option<Value>) -> Error {
    ContractError {
        kind: ErrorKind::IllegalModel,
        code: "pre-port",
        params: vec![("message", message)],
        location,
        model_file: Some(None),
        validator: None,
        details: Vec::new(),
    }
    .into()
}

/// BC-11: the `IllegalModelException` for a cyclic inheritance chain met by
/// a walk over the JS declaration views, as the engine's own walk raises it.
/// `cycle` is the loop from the declaration met again round to the one whose
/// super type it is; `repeated` is that declaration.
pub(crate) fn circular_inheritance_error(cycle: &[JsValue], repeated: &JsValue) -> Result<Error> {
    let fqn = |declaration: &JsValue| -> Result<String> { js_string(&get(declaration, "fqn")?) };
    let mut names = cycle.iter().map(fqn).collect::<Result<Vec<_>>>()?;
    let name = fqn(repeated)?;
    names.push(name.clone());
    let mut err = ContractError::new(
        ErrorKind::IllegalModel,
        "classdeclaration-circularinheritance",
        vec![("type", name), ("cycle", names.join(" -> "))],
    );
    err.model_file = Some(None);
    Ok(err.into())
}

/// A running step of a super type walk, removed from [`caches::SUPER_WALKS`] when
/// dropped.
pub(crate) struct SuperWalk;

impl SuperWalk {
    /// Records `declaration` as walked by the binding `properties` names, or
    /// returns the BC-11 error when that binding is already walking it
    /// further out (a cyclic chain).
    pub(crate) fn enter(properties: &'static str, declaration: &JsValue) -> Result<Self> {
        let cycle = caches::SUPER_WALKS.with(|walks| {
            let walks = walks.borrow();
            walks
                .iter()
                .position(|(kind, seen)| *kind == properties && Object::is(seen, declaration))
                .map(|start| {
                    walks
                        .iter()
                        .skip(start)
                        .filter(|(kind, _)| *kind == properties)
                        .map(|(_, seen)| seen.clone())
                        .collect::<Vec<_>>()
                })
        });
        if let Some(cycle) = cycle {
            return Err(circular_inheritance_error(&cycle, declaration)?);
        }
        caches::SUPER_WALKS
            .with(|walks| walks.borrow_mut().push((properties, declaration.clone())));
        Ok(SuperWalk)
    }
}

impl Drop for SuperWalk {
    fn drop(&mut self) {
        caches::SUPER_WALKS.with(|walks| {
            walks.borrow_mut().pop();
        });
    }
}

/// `declaration.ast.location`, as JSON (`None` when nullish).
pub(crate) fn ast_location(declaration: &JsValue) -> Result<Option<Value>> {
    to_json(&get(&get(declaration, "ast")?, "location")?)
}

/// TS: the `classDecl = ...` resolution of `_resolveSuperType`,
/// `getProperty` and `getProperties`:
/// `this.getModelFile().isImportedType(name) ?
/// this.modelFile.getModelManager().getType(this.getModelFile().resolveImport(name))
/// : this.getModelFile().getType(name)`. The result may be nullish.
pub(crate) fn resolve_named_type(declaration: &JsValue, type_name: &JsValue) -> Result<JsValue> {
    let model_file = call(declaration, "getModelFile", &[], "this.getModelFile")?;
    let is_imported = call(
        &model_file,
        "isImportedType",
        std::slice::from_ref(type_name),
        "this.getModelFile().isImportedType",
    )?
    .is_truthy();
    if is_imported {
        let fqn_super = call(
            &model_file,
            "resolveImport",
            std::slice::from_ref(type_name),
            "this.getModelFile().resolveImport",
        )?;
        let own_model_file = get(declaration, "modelFile")?;
        let manager = call(
            &own_model_file,
            "getModelManager",
            &[],
            "this.modelFile.getModelManager",
        )?;
        call(
            &manager,
            "getType",
            &[fqn_super],
            "this.modelFile.getModelManager().getType",
        )
    } else {
        call(
            &model_file,
            "getType",
            std::slice::from_ref(type_name),
            "this.getModelFile().getType",
        )
    }
}

/// The system declaration kinds a user model may reuse the name of when
/// `dangerouslyAllowReservedSystemTypeNamesInUserModels` is set: TS
/// `Declaration.isReservedSystemTypeImport`'s `isConcept() || isAsset() ||
/// isTransaction() || isParticipant() || isEvent()` (declaration.ts), in
/// that order.
pub(crate) const RESERVED_SYSTEM_TYPE_KINDS: [&str; 5] = [
    "isConcept",
    "isAsset",
    "isTransaction",
    "isParticipant",
    "isEvent",
];

/// TS: `Declaration.validate`'s own check, after `super.validate()`: a
/// declaration may not take the name of a type its model file imports,
/// unless `dangerouslyAllowReservedSystemTypeNamesInUserModels` is set and
/// `this.isReservedSystemTypeImport(modelFile, name)` holds (concerto-core's
/// `check_import_clash`, over the view's collaborators). Throws TS's
/// `IllegalModelException`; a collaborator's error propagates.
#[wasm_bindgen(js_name = declarationValidate)]
pub fn declaration_validate(declaration: JsValue) -> JsResult<()> {
    let body = || -> Result<()> {
        let model_file = call(&declaration, "getModelFile", &[], "this.getModelFile")?;
        let name = call(&declaration, "getName", &[], "this.getName")?;
        let imported = call(
            &model_file,
            "isImportedType",
            std::slice::from_ref(&name),
            "modelFile.isImportedType",
        )?;
        if !imported.is_truthy() {
            return Ok(());
        }
        // `Boolean(modelFile.getModelManager()?.options?.dangerously…)`.
        let manager = call(
            &model_file,
            "getModelManager",
            &[],
            "modelFile.getModelManager",
        )?;
        let allow = !nullish(&manager) && {
            let options = get(&manager, "options")?;
            !nullish(&options)
                && get(
                    &options,
                    "dangerouslyAllowReservedSystemTypeNamesInUserModels",
                )?
                .is_truthy()
        };
        if allow {
            let name = call(&declaration, "getName", &[], "this.getName")?;
            let reserved = call(
                &declaration,
                "isReservedSystemTypeImport",
                &[model_file, name],
                "this.isReservedSystemTypeImport",
            )?;
            if reserved.is_truthy() {
                return Ok(());
            }
        }
        let name = js_string(&call(&declaration, "getName", &[], "this.getName")?)?;
        Err(illegal_model_error(
            format!("Type '{name}' clashes with an imported type with the same name."),
            ast_location(&declaration)?,
        ))
    };
    run_naming(
        || get(&declaration, "modelFile").unwrap_or(JsValue::UNDEFINED),
        body,
    )
}

/// TS: `Declaration.isReservedSystemTypeImport(modelFile, typeName)`:
/// `typeName` resolves, through `modelFile.getType`, to a system model
/// file's declaration of a reserved kind ([`RESERVED_SYSTEM_TYPE_KINDS`]).
#[wasm_bindgen(js_name = declarationIsReservedSystemTypeImport)]
pub fn declaration_is_reserved_system_type_import(
    model_file: JsValue,
    type_name: JsValue,
) -> JsResult<bool> {
    run(|| {
        let imported = call(&model_file, "getType", &[type_name], "modelFile.getType")?;
        if !imported.is_truthy() || imported.is_string() {
            return Ok(false);
        }
        let imported_file = call(&imported, "getModelFile", &[], "importedType.getModelFile")?;
        if !imported_file.is_truthy()
            || !call(
                &imported_file,
                "isSystemModelFile",
                &[],
                "importedModelFile.isSystemModelFile",
            )?
            .is_truthy()
        {
            return Ok(false);
        }
        for kind in RESERVED_SYSTEM_TYPE_KINDS {
            if call(&imported, kind, &[], kind)?.is_truthy() {
                return Ok(true);
            }
        }
        Ok(false)
    })
}

/// TS: `ClassDeclaration._resolveSuperType`: resolves `this.superType`
/// ([`resolve_named_type`]), throws TS's `IllegalModelException` when it is
/// not found or the kinds are incompatible
/// ([`concerto_core::ClassDeclaration::kinds_compatible`]), and caches the
/// result in `this.superTypeDeclaration`.
#[wasm_bindgen(js_name = classDeclarationResolveSuperType)]
pub fn class_declaration_resolve_super_type(declaration: JsValue) -> JsResult<JsValue> {
    let body = || -> Result<JsValue> {
        let super_type = get(&declaration, "superType")?;
        if !super_type.is_truthy() {
            return Ok(JsValue::NULL);
        }
        set_property(&declaration, "superTypeDeclaration", &JsValue::NULL)?;

        let class_decl = resolve_named_type(&declaration, &super_type)?;
        if nullish(&class_decl) {
            let super_type_name = js_string(&super_type)?;
            return Err(illegal_model_error(
                format!("Could not find super type {super_type_name}"),
                ast_location(&declaration)?,
            ));
        }

        let child_kind = js_string(&call(
            &declaration,
            "declarationKind",
            &[],
            "this.declarationKind",
        )?)?;
        let super_kind = js_string(&call(
            &class_decl,
            "declarationKind",
            &[],
            "classDecl.declarationKind",
        )?)?;
        if !concerto_core::ClassDeclaration::kinds_compatible(&child_kind, &super_kind) {
            let child_name = js_string(&call(&declaration, "getName", &[], "this.getName")?)?;
            let super_name = js_string(&call(&class_decl, "getName", &[], "classDecl.getName")?)?;
            return Err(illegal_model_error(
                format!("{child_kind} ({child_name}) cannot extend {super_kind} ({super_name})"),
                ast_location(&declaration)?,
            ));
        }

        set_property(&declaration, "superTypeDeclaration", &class_decl)?;
        Ok(class_decl)
    };
    run_naming(
        || get(&declaration, "modelFile").unwrap_or(JsValue::UNDEFINED),
        body,
    )
}

/// TS: `ClassDeclaration.getSuperTypeDeclaration`: field reads, else
/// `this._resolveSuperType()`.
#[wasm_bindgen(js_name = classDeclarationGetSuperTypeDeclaration)]
pub fn class_declaration_get_super_type_declaration(declaration: JsValue) -> JsResult<JsValue> {
    run(|| {
        if !get(&declaration, "superType")?.is_truthy() {
            return Ok(JsValue::NULL);
        }
        let cached = get(&declaration, "superTypeDeclaration")?;
        if cached.is_truthy() {
            return Ok(cached);
        }
        call(
            &declaration,
            "_resolveSuperType",
            &[],
            "this._resolveSuperType",
        )
    })
}

/// TS: `ClassDeclaration.getSuperType`: `this.getSuperTypeDeclaration()`,
/// then `getFullyQualifiedName()` on the result if there is one.
#[wasm_bindgen(js_name = classDeclarationGetSuperType)]
pub fn class_declaration_get_super_type(declaration: JsValue) -> JsResult<JsValue> {
    run(|| {
        let super_type_decl = call(
            &declaration,
            "getSuperTypeDeclaration",
            &[],
            "this.getSuperTypeDeclaration",
        )?;
        if !super_type_decl.is_truthy() {
            return Ok(JsValue::NULL);
        }
        call(
            &super_type_decl,
            "getFullyQualifiedName",
            &[],
            "superTypeDeclaration.getFullyQualifiedName",
        )
    })
}

/// TS: `ClassDeclaration.getAllSuperTypeDeclarations`: repeats `type =
/// type.getSuperTypeDeclaration()`, collecting every non-null result. A
/// declaration met again is the BC-11 `IllegalModelException`.
#[wasm_bindgen(js_name = classDeclarationGetAllSuperTypeDeclarations)]
pub fn class_declaration_get_all_super_type_declarations(declaration: JsValue) -> JsResult<Array> {
    run(|| {
        let results = Array::new();
        let mut chain = vec![declaration.clone()];
        let mut current = declaration;
        loop {
            let next = call(
                &current,
                "getSuperTypeDeclaration",
                &[],
                "type.getSuperTypeDeclaration",
            )?;
            if !next.is_truthy() {
                break;
            }
            if let Some(start) = chain.iter().position(|seen| Object::is(seen, &next)) {
                return Err(circular_inheritance_error(
                    chain.get(start..).unwrap_or_default(),
                    &next,
                )?);
            }
            chain.push(next.clone());
            results.push(&next);
            current = next;
        }
        Ok(results)
    })
}

/// TS: `ClassDeclaration.getIdentifierFieldName`: `this.idField` if set,
/// else the super type's answer, through `getLocalType` or the model
/// manager; a `null` resolution reaches TS's unguarded call (a host
/// `TypeError`). A declaration met again is BC-11's error ([`SuperWalk`]).
#[wasm_bindgen(js_name = classDeclarationGetIdentifierFieldName)]
pub fn class_declaration_get_identifier_field_name(declaration: JsValue) -> JsResult<JsValue> {
    run(|| {
        let _walk = SuperWalk::enter("getIdentifierFieldName", &declaration)?;
        let id_field = get(&declaration, "idField")?;
        if id_field.is_truthy() {
            return Ok(id_field);
        }
        let super_type = call(&declaration, "getSuperType", &[], "this.getSuperType")?;
        if !super_type.is_truthy() {
            return Ok(JsValue::NULL);
        }
        let model_file = call(&declaration, "getModelFile", &[], "this.getModelFile")?;
        let mut class_decl = call(
            &model_file,
            "getLocalType",
            std::slice::from_ref(&super_type),
            "this.getModelFile().getLocalType",
        )?;
        if !class_decl.is_truthy() {
            let own_model_file = get(&declaration, "modelFile")?;
            let manager = call(
                &own_model_file,
                "getModelManager",
                &[],
                "this.modelFile.getModelManager",
            )?;
            class_decl = call(
                &manager,
                "getType",
                &[super_type],
                "this.modelFile.getModelManager().getType",
            )?;
        }
        call(
            &class_decl,
            "getIdentifierFieldName",
            &[],
            "classDecl.getIdentifierFieldName",
        )
    })
}

/// TS `ClassDeclaration.getIdentifierFieldName` with its super type walk in
/// one binding, calling `_resolveSuperType`, `getLocalType`,
/// `getModelManager` and `getType` as TS does (BC-50: replaced methods are
/// not called; BC-11 for a cycle). Returns `[answer, cacheable, ...chain]`:
/// every declaration read, and whether the walk ended without a call, so
/// the answer depends only on the chain's fields.
#[wasm_bindgen(js_name = classDeclarationGetIdentifierFieldNameWalk)]
pub fn class_declaration_get_identifier_field_name_walk(declaration: JsValue) -> JsResult<Array> {
    run(|| {
        let mut cacheable = true;
        let mut chain: Vec<JsValue> = vec![declaration.clone()];
        let mut current = declaration;
        let answer = loop {
            let id_field = get(&current, "idField")?;
            if id_field.is_truthy() {
                break id_field;
            }

            // `const superType = this.getSuperType();`, through
            // `this.getSuperTypeDeclaration()`.
            let super_type_decl = if !get(&current, "superType")?.is_truthy() {
                JsValue::NULL
            } else {
                let cached = get(&current, "superTypeDeclaration")?;
                if cached.is_truthy() {
                    cached
                } else {
                    call(&current, "_resolveSuperType", &[], "this._resolveSuperType")?
                }
            };
            let super_type = if !super_type_decl.is_truthy() {
                JsValue::NULL
            } else {
                // `superTypeDeclaration.getFullyQualifiedName()`
                get(&super_type_decl, "fqn")?
            };
            if !super_type.is_truthy() {
                break JsValue::NULL;
            }

            // `this.getModelFile()`
            let model_file = get(&current, "modelFile")?;
            let mut class_decl = call(
                &model_file,
                "getLocalType",
                std::slice::from_ref(&super_type),
                "this.getModelFile().getLocalType",
            )?;
            if !class_decl.is_truthy() {
                let manager = call(
                    &model_file,
                    "getModelManager",
                    &[],
                    "this.modelFile.getModelManager",
                )?;
                class_decl = call(
                    &manager,
                    "getType",
                    &[super_type],
                    "this.modelFile.getModelManager().getType",
                )?;
            }

            // `return classDecl.getIdentifierFieldName();`: a nullish
            // `classDecl` raises TS's `TypeError` through `call`; a
            // declaration met again is BC-11's error.
            if let Some(start) = chain.iter().position(|d| Object::is(d, &class_decl)) {
                return Err(circular_inheritance_error(
                    chain.get(start..).unwrap_or_default(),
                    &class_decl,
                )?);
            }
            if !nullish(&class_decl) {
                chain.push(class_decl.clone());
                current = class_decl;
                continue;
            }
            cacheable = false;
            break call(
                &class_decl,
                "getIdentifierFieldName",
                &[],
                "classDecl.getIdentifierFieldName",
            )?;
        };

        let result = Array::new();
        result.push(&answer);
        result.push(&JsValue::from_bool(cacheable));
        for declaration in &chain {
            result.push(declaration);
        }
        Ok(result)
    })
}

/// TS: `ClassDeclaration.getProperty`: the own property, else the super
/// type's answer ([`resolve_named_type`]), a `null` resolution reaching TS's
/// unguarded call. The guard is `this.superType !== null`, not truthiness,
/// so a falsy super type name is still resolved.
#[wasm_bindgen(js_name = classDeclarationGetProperty)]
pub fn class_declaration_get_property(declaration: JsValue, name: JsValue) -> JsResult<JsValue> {
    run(|| {
        let _walk = SuperWalk::enter("getProperty", &declaration)?;
        let own = call(
            &declaration,
            "getOwnProperty",
            std::slice::from_ref(&name),
            "this.getOwnProperty",
        )?;
        if !nullish(&own) {
            return Ok(own);
        }
        // TS tests `this.superType !== null`, not truthiness.
        let super_type = get(&declaration, "superType")?;
        if super_type.is_null() {
            return Ok(JsValue::NULL);
        }
        let class_decl = resolve_named_type(&declaration, &super_type)?;
        call(&class_decl, "getProperty", &[name], "classDecl.getProperty")
    })
}

/// TS: `ClassDeclaration.getProperties`: the own properties, then the super
/// type's answer ([`resolve_named_type`]), the resolution guarded by
/// `_resolveSuperType`'s "Could not find super type" error. The same
/// `this.superType !== null` guard as `getProperty`.
#[wasm_bindgen(js_name = classDeclarationGetProperties)]
pub fn class_declaration_get_properties(declaration: JsValue) -> JsResult<Array> {
    let body = || -> Result<Array> {
        let _walk = SuperWalk::enter("getProperties", &declaration)?;
        let own = call(
            &declaration,
            "getOwnProperties",
            &[],
            "this.getOwnProperties",
        )?;
        let result = Array::new();
        for property in Array::from(&own).iter() {
            result.push(&property);
        }
        // TS tests `this.superType !== null`, not truthiness, so a falsy
        // name is resolved, fails, and throws "Could not find super type".
        let super_type = get(&declaration, "superType")?;
        if super_type.is_null() {
            return Ok(result);
        }
        let class_decl = resolve_named_type(&declaration, &super_type)?;
        if nullish(&class_decl) {
            let super_type_name = js_string(&super_type)?;
            return Err(illegal_model_error(
                format!("Could not find super type {super_type_name}"),
                ast_location(&declaration)?,
            ));
        }
        let inherited = call(&class_decl, "getProperties", &[], "classDecl.getProperties")?;
        for property in Array::from(&inherited).iter() {
            result.push(&property);
        }
        Ok(result)
    };
    run_naming(
        || get(&declaration, "modelFile").unwrap_or(JsValue::UNDEFINED),
        body,
    )
}

/// TS: `ClassDeclaration.getNestedProperty`: walks a dotted property path
/// one name at a time, resolving each step's class through
/// `getFullyQualifiedTypeName` and `modelManager.getType`, and stopping with
/// the same `IllegalModelException`/plain `Error` TS raises for a missing
/// property or a primitive/enum step that isn't the path's last element.
#[wasm_bindgen(js_name = classDeclarationGetNestedProperty)]
pub fn class_declaration_get_nested_property(
    declaration: JsValue,
    property_path: JsValue,
) -> JsResult<JsValue> {
    let body = || -> Result<JsValue> {
        let path = js_string(&property_path)?;
        let names: Vec<&str> = path.split('.').collect();
        let mut class_declaration = declaration.clone();
        let mut result = JsValue::UNDEFINED;

        for (n, name) in names.iter().enumerate() {
            let property = call(
                &class_declaration,
                "getProperty",
                &[JsValue::from_str(name)],
                "classDeclaration.getProperty",
            )?;
            if nullish(&property) {
                let fqn = js_string(&call(
                    &class_declaration,
                    "getFullyQualifiedName",
                    &[],
                    "classDeclaration.getFullyQualifiedName",
                )?)?;
                return Err(ContractError {
                    kind: ErrorKind::IllegalModel,
                    code: "classdeclaration-getnestedproperty-doesnotexist",
                    params: vec![("propertyName", (*name).to_string()), ("fqn", fqn)],
                    location: ast_location(&declaration)?,
                    model_file: Some(None),
                    validator: None,
                    details: Vec::new(),
                }
                .into());
            }
            result = property.clone();

            if n < names.len() - 1 {
                let is_primitive =
                    call(&property, "isPrimitive", &[], "result.isPrimitive")?.is_truthy();
                let is_enum = call(&property, "isTypeEnum", &[], "result.isTypeEnum")?.is_truthy();
                if is_primitive || is_enum {
                    return Err(plain_error(
                        "classdeclaration-getnestedproperty-primitiveorenum",
                        vec![
                            ("propertyName", (*name).to_string()),
                            ("propertyPath", path.clone()),
                        ],
                    ));
                }
                let type_fqn = call(
                    &property,
                    "getFullyQualifiedTypeName",
                    &[],
                    "result.getFullyQualifiedTypeName",
                )?;
                let own_model_file = get(&declaration, "modelFile")?;
                let manager = call(
                    &own_model_file,
                    "getModelManager",
                    &[],
                    "this.modelFile.getModelManager",
                )?;
                class_declaration = call(
                    &manager,
                    "getType",
                    &[type_fqn],
                    "this.modelFile.getModelManager().getType",
                )?;
            }
        }

        Ok(result)
    };
    run_naming(
        || get(&declaration, "modelFile").unwrap_or(JsValue::UNDEFINED),
        body,
    )
}

// ---------------------------------------------------------------------------
// MapDeclaration, MapKeyType, MapValueType (src/introspect/mapdeclaration.ts,
// mapkeytype.ts, mapvaluetype.ts)
// ---------------------------------------------------------------------------

/// TS: MapDeclaration.process, after `super.process()`: checks the AST's
/// `key`/`value` shape ([`mu::is_valid_map_key`],
/// [`mu::is_valid_map_value`]). The view builds the
/// `MapKeyType`/`MapValueType` child views itself.
#[wasm_bindgen(js_name = mapDeclarationProcess)]
pub fn map_declaration_process(view: JsValue) -> JsResult<()> {
    let body = || -> Result<()> {
        let ast = get(&view, "ast")?;
        // TS interpolates the raw `this.ast.name` into every message here,
        // so `undefined` and `null` read as such, not as an empty name.
        let name = js_string(&opt_get(&ast, "name")?)?;
        // TS: `if (!this.ast.key || !this.ast.value)`, JS truthiness: a
        // falsy `key`/`value` fails this check, not the later key/value
        // checks.
        let key_raw = get(&ast, "key")?;
        let value_raw = get(&ast, "value")?;
        let key = to_json(&key_raw)?;
        let value = to_json(&value_raw)?;
        let location = to_json(&get(&ast, "location")?)?;

        if !key_raw.is_truthy() || !value_raw.is_truthy() {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "mapdeclaration-process-missingkeyvalue",
                vec![("name", name)],
            );
            err.location = location;
            err.model_file = Some(None);
            return Err(err.into());
        }
        if !mu::is_valid_map_key(key.as_ref())? {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "mapdeclaration-process-invalidkey",
                vec![("name", name)],
            );
            err.location = location;
            err.model_file = Some(None);
            return Err(err.into());
        }
        if !mu::is_valid_map_value(value.as_ref())? {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "mapdeclaration-process-invalidvalue",
                vec![("name", name)],
            );
            err.location = location;
            err.model_file = Some(None);
            return Err(err.into());
        }
        Ok(())
    };
    run_naming(
        || get(&view, "modelFile").unwrap_or(JsValue::UNDEFINED),
        body,
    )
}

/// TS: MapKeyType.processType. The `$class` switch has no default arm;
/// `mapDeclarationProcess` already restricts the AST to its three kinds.
#[wasm_bindgen(js_name = mapKeyTypeProcess)]
pub fn map_key_type_process(view: JsValue) -> JsResult<JsValue> {
    run(|| {
        let ast = get(&view, "ast")?;
        let class = get(&ast, "$class")?;
        let class = if nullish(&class) {
            String::new()
        } else {
            js_string(&class)?
        };
        let type_name = match short_class(&class) {
            "DateTimeMapKeyType" => "DateTime".to_string(),
            "StringMapKeyType" => "String".to_string(),
            "ObjectMapKeyType" => {
                let ast_type = get(&ast, "type")?;
                js_string(&get(&ast_type, "name")?)?
            }
            _ => String::new(),
        };
        Ok(JsValue::from_str(&type_name))
    })
}

/// TS: MapKeyType.validate, over the JS model file's `getType(...)`; the
/// scalar-kind check is [`js_is_valid_map_key_scalar`] over that JS
/// declaration.
#[wasm_bindgen(js_name = mapKeyTypeValidate)]
pub fn map_key_type_validate(view: JsValue) -> JsResult<()> {
    run(|| {
        let type_name = js_string(&get(&view, "type")?)?;
        if mu::is_primitive_type(&type_name) {
            return Ok(());
        }
        let model_file = get(&view, "modelFile")?;
        let ast = get(&view, "ast")?;
        let ast_type = get(&ast, "type")?;
        let type_name_ast = get(&ast_type, "name")?;
        let decl = call(
            &model_file,
            "getType",
            &[type_name_ast],
            "modelFile.getType",
        )?;
        // `modelFile.getType` answers `null` for a type not found, which
        // `isValidMapKeyScalar` optional-chains off, so it becomes `None`.
        let decl_opt = if nullish(&decl) { None } else { Some(&decl) };
        let valid = js_is_valid_map_key_scalar(decl_opt)?;
        if valid != Some(true) {
            let parent = get(&view, "parent")?;
            let parent_name = js_string(&get(&parent, "name")?)?;
            return Err(ContractError::new(
                ErrorKind::IllegalModel,
                "mapkeytype-validate-invalidscalar",
                vec![("type", type_name), ("name", parent_name)],
            )
            .into());
        }
        Ok(())
    })
}

/// TS: MapValueType.processType, with the inline checks of the
/// `ObjectMapValueType`/`RelationshipMapValueType` arm.
#[wasm_bindgen(js_name = mapValueTypeProcess)]
pub fn map_value_type_process(view: JsValue) -> JsResult<JsValue> {
    run(|| {
        let ast = get(&view, "ast")?;
        let parent = get(&view, "parent")?;
        let parent_name = js_string(&get(&parent, "name")?)?;
        let class = get(&ast, "$class")?;
        let class = if nullish(&class) {
            String::new()
        } else {
            js_string(&class)?
        };
        let type_name = match short_class(&class) {
            "ObjectMapValueType" | "RelationshipMapValueType" => {
                // TS: `!('type' in ast)`: `ast` is an object here, so this
                // is key presence, true for a present `type: null` too
                // (`get` gives `undefined` only for a missing key).
                let ast_type = get(&ast, "type")?;
                if ast_type.is_undefined() {
                    return Err(ContractError::new(
                        ErrorKind::IllegalModel,
                        "mapvaluetype-process-missingtype",
                        vec![("name", parent_name)],
                    )
                    .into());
                }
                // TS: `!('$class' in ast.type) || !('name' in ast.type)`:
                // `ast.type` may be a primitive, for which `in` throws a
                // `TypeError`.
                if !ast_type.is_object() && !ast_type.is_function() {
                    return Err(type_error(
                        "engine-typeerror-inoperator",
                        vec![
                            ("key", "$class".to_string()),
                            ("value", js_string(&ast_type)?),
                        ],
                    ));
                }
                let type_class = get(&ast_type, "$class")?;
                let type_name_field = get(&ast_type, "name")?;
                // Key presence, as TS's `in`: a present `null` passes on to
                // the `$class !== 'TypeIdentifier'` check.
                if type_class.is_undefined() || type_name_field.is_undefined() {
                    return Err(ContractError::new(
                        ErrorKind::IllegalModel,
                        "mapvaluetype-process-malformedtype",
                        vec![("name", parent_name)],
                    )
                    .into());
                }
                if js_string(&type_class)? != "concerto.metamodel@1.0.0.TypeIdentifier" {
                    return Err(ContractError::new(
                        ErrorKind::IllegalModel,
                        "mapvaluetype-process-invalidtypeclass",
                        vec![("name", parent_name)],
                    )
                    .into());
                }
                js_string(&type_name_field)?
            }
            "BooleanMapValueType" => "Boolean".to_string(),
            "DateTimeMapValueType" => "DateTime".to_string(),
            "StringMapValueType" => "String".to_string(),
            "IntegerMapValueType" => "Integer".to_string(),
            "LongMapValueType" => "Long".to_string(),
            "DoubleMapValueType" => "Double".to_string(),
            _ => String::new(),
        };
        Ok(JsValue::from_str(&type_name))
    })
}

/// TS: MapValueType.validate, over the JS model file's `getType(...)` and
/// the declaration's `isMapDeclaration?.()` ([`js_declaration_is`]).
#[wasm_bindgen(js_name = mapValueTypeValidate)]
pub fn map_value_type_validate(view: JsValue) -> JsResult<()> {
    run(|| {
        let type_name = js_string(&get(&view, "type")?)?;
        if mu::is_primitive_type(&type_name) {
            return Ok(());
        }
        let model_file = get(&view, "modelFile")?;
        let ast = get(&view, "ast")?;
        let ast_type = get(&ast, "type")?;
        let type_name_ast = get(&ast_type, "name")?;
        let decl = call(
            &model_file,
            "getType",
            &[type_name_ast],
            "modelFile.getType",
        )?;
        if js_declaration_is(&decl, "isMapDeclaration")?.unwrap_or(false) {
            return Err(ContractError::new(
                ErrorKind::IllegalModel,
                "mapvaluetype-validate-mapnotsupported",
                vec![("type", type_name)],
            )
            .into());
        }
        Ok(())
    })
}
