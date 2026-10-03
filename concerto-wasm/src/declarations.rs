//! The `ScalarDeclaration`, `ClassDeclaration` family and `MapDeclaration` bindings.
//!
//! Split out of `lib.rs` (P5-104, review M7); the crate root glob-imports it.

use super::*;

// ---------------------------------------------------------------------------
// ScalarDeclaration (src/introspect/scalardeclaration.ts)
// ---------------------------------------------------------------------------

/// TS: ScalarDeclaration.process, after `super.process()`. Returns the
/// snapshot `{type, validator, defaultValue}`, where `validator` is `null`,
/// `{kind: "NumberValidator", lowerBound, upperBound}`, or
/// `{kind: "StringValidator"}` (the view builds the TS `StringValidator`
/// until P2-02 ports it).
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
// ClassDeclaration family (src/introspect/classdeclaration.ts,
// assetdeclaration.ts, conceptdeclaration.ts, participantdeclaration.ts,
// transactiondeclaration.ts, eventdeclaration.ts, enumdeclaration.ts) — P4-06
//
// The model graph these views meet is still TS (ModelFile/ModelManager are
// not Rust-backed until P4-08; PORTING.md 1.4), so every ported member that
// needs a collaborator (`getModelFile().getType(...)`, the model manager's
// `getType`) reaches it through the JS-callback context (PORTING.md 1.4,
// "until the arena owns the graph, every collaborator call goes through the
// JS-callback context"): the member's own algorithm is ported here, and each
// collaborator call it makes is a plain call back onto the JS object it was
// given, via the `get`/`call` helpers (not the generic `ResolutionContext`
// trait — the exact TS collaborator sequence, e.g. `_resolveSuperType`'s
// `isImportedType`/`resolveImport` branch, matters more here than a shape
// shared with the arena implementation). A member whose *only* pure content
// is a small decision at the end (`classDeclarationProcess`'s superType/
// idField choice, the kind-compatibility and identifier-redeclare checks) has
// that decision itself pulled into `concerto_core::ClassDeclaration` as a
// plain function, so the binding stays a collaborator-calling wrapper around
// real core logic rather than a reimplementation of it (the grain
// Declaration/Decorated, P4-05, used for `modelUtilIsValidIdentifier` and
// `decoratedFindDuplicateName`).
// ---------------------------------------------------------------------------

/// The metamodel `$class`'s short name: the text after the last `.`.
pub(crate) fn short_class(ast_class: &str) -> &str {
    ast_class.rsplit('.').next().unwrap_or(ast_class)
}

/// TS: `ClassDeclaration.process`, the superType/idField decision made
/// before the `ast.properties` loop (the loop itself builds `Field`/
/// `RelationshipDeclaration`/`EnumValueDeclaration` views; that construction
/// stays in TS, and since P4-07 those Property views delegate their own
/// `process`/`validate` to the `propertyProcess`/`propertyValidate`/
/// `fieldProcess`/`relationshipDeclarationValidate` bindings). Returns `{superType, idField,
/// addIdentifierField, addTimestampField}`:
/// - `superType`: `this.ast.superType.name` when the AST names one — including
///   the literal text `"undefined"` when a `superType` node is present but
///   carries no `name` at all (`this.ast.superType.name` reads as `undefined`
///   there, not `null`, and every downstream guard treats those two
///   differently: only an explicit `name: null` reads as "no super type",
///   review finding 2 on accordproject/concerto-rust#217); otherwise `null`
///   only for the system model's own `Concept` declaration, else the implicit
///   `'Concept'` (TS: the `this.modelFile.isSystemModelFile() && this.name
///   === 'Concept'` exemption).
/// - `idField`/`addIdentifierField`: mirrors the `this.ast.identified` match;
///   `addIdentifierField` tells the view to still call its own
///   `addIdentifierField()` (it pushes a real `Field` view).
/// - `addTimestampField`: `this.fqn` is the system `Transaction` or `Event`.
#[wasm_bindgen(js_name = classDeclarationProcess)]
pub fn class_declaration_process(declaration: JsValue) -> JsResult<JsValue> {
    let body = || -> Result<JsValue> {
        let ast = get(&declaration, "ast")?;

        // TS: `if (this.ast.superType) { this.superType = this.ast.superType.name; }
        // else if (!(isSystemModelFile && name === 'Concept')) { this.superType = 'Concept'; }`
        // Neither branch ever calls `.toString()`: the outer test is plain JS
        // truthiness of the whole `superType` node (not merely non-nullish —
        // a fuzzed AST can put `false`/`0`/`""` there too, all falsy), and
        // once truthy, whatever `.name` holds (string, number, boolean,
        // `null`, absent, object, array) is stored on `this.superType`
        // as-is, UNSTRINGIFIED and uncoerced. That raw JS value's own type
        // and truthiness are themselves observable later: `_resolveSuperType`
        // (`classDeclarationResolveSuperType` above) keys off its truthiness,
        // `getProperty`/`getProperties` (below) off strict non-null, and
        // every "Could not find super type" message off its `ToString` —
        // three different tests a fuzzer can pull apart (`undefined` is
        // falsy but not `null`; `ToString(undefined)` is `"undefined"`, not
        // `""`). A single Rust `String` cannot answer the first two at once
        // (accordproject/concerto-rust#219, P5-05 stage-2 T2c), so the raw
        // `JsValue` is threaded straight through to the snapshot below
        // instead of being coerced or blanked here the way `receiver` would.
        let super_type_ast = get(&ast, "superType")?;
        let raw_super_type = if super_type_ast.is_truthy() {
            Some(get(&super_type_ast, "name")?)
        } else {
            None
        };
        // `process_decision` only needs to know whether the AST named a
        // super type at all (`None` applies its own implicit-`Concept`
        // default, or leaves it unset for the system model's own `Concept`);
        // once it has named one, the placeholder's content is never read —
        // `raw_super_type` is what actually reaches the snapshot.
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
            // TS never evaluates `this.modelFile.isSystemModelFile()` on this
            // branch (short-circuited by `this.ast.superType`); no collaborator
            // call to make.
            false
        };
        // TS: `name === 'Concept'` — a strict equality, not a `.toString()`
        // call, so a non-string `this.name` (reachable through a fuzzed
        // `ast.name`, missing, `null`, a bool, an array, ...) simply can
        // never equal the literal `'Concept'`, and TS accepts every one of
        // those without throwing (accordproject/concerto-rust#217); the
        // empty string can't equal `'Concept'` either, so it stands in
        // without a `receiver`/`js_string` call that could itself throw
        // (or, for a value like `["Concept"]` whose `toString()` happens to
        // read `"Concept"`, wrongly coerce a non-match into a match that
        // real `===` never would).
        let name = get(&declaration, "name")?.as_string().unwrap_or_default();

        // TS: `if (this.ast.identified) { ... }` — again plain truthiness of
        // the whole node, not merely non-nullish.
        let identified = get(&ast, "identified")?;
        let (identified_class, identified_name, raw_identified_name) = if identified.is_truthy() {
            // TS: `this.ast.identified.$class === '...IdentifiedBy'` (strict
            // equality) and `this.idField = this.ast.identified.name` (plain
            // assignment) — neither coerces. A non-string `$class` can never
            // match the literal comparison, so the empty string (never a
            // real `$class`) stands in for it without a receiver check.
            let class_value = get(&identified, "$class")?;
            let identified_class = class_value.as_string().unwrap_or_default();
            // `raw_identified_name` is `this.ast.identified.name`, UNSTRINGIFIED
            // and uncoerced, exactly as TS's plain assignment leaves it — a
            // fuzzed AST can put a number, boolean, `null`, or leave it
            // absent (`undefined`), and every one of those is falsy in TS,
            // so `idField`'s later truthiness guard (in
            // `ClassDeclaration.validate`, still TS) skips its
            // `getProperty(this.idField)` check entirely rather than
            // looking up a property literally named `"undefined"`/`"null"`/
            // `"false"` the way stringifying here would produce
            // (accordproject/concerto-rust#219 review: "Match TS name
            // handling: keep undefined, not the string \"undefined\"").
            // `identified_name` (a `&str`, for `process_decision` below) is
            // only ever consulted on this same branch, and only to decide
            // `process_decision`'s own placeholder `id_field` — which
            // `id_field_js` below always overrides with the raw value once
            // this branch is taken — so it need not itself be coerced.
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

        // `raw_super_type` (the AST's own `.name` value, untouched) when the
        // AST named a super type at all; otherwise `process_decision`'s own
        // string decision (the implicit `'Concept'`, or `null` for the
        // system model's own `Concept`).
        let super_type_js = match raw_super_type {
            Some(v) => v,
            None => decision
                .super_type
                .as_deref()
                .map_or(JsValue::NULL, JsValue::from_str),
        };
        // `raw_identified_name` (the AST's own `.identified.name` value,
        // untouched) when the AST named an explicit `IdentifiedBy`;
        // otherwise `process_decision`'s own string decision (`$identifier`
        // for the system-identified case, or `null` for no identity at
        // all).
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
/// `ClassDeclaration.validate` (the block guarded by `superType.isIdentified()`,
/// which the caller checks before calling this): `true` when the super type's
/// existing identifier cannot be redeclared. Resolving `superType` itself
/// (`getModelFile().getType(this.superType)`) stays TS.
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
/// own `this.superTypeDeclaration` field (a real field, not a getter — TS
/// reads it directly as a cache in `getSuperTypeDeclaration`).
pub(crate) fn set_property(target: &JsValue, name: &str, value: &JsValue) -> Result<()> {
    Reflect::set(target, &JsValue::from_str(name), value).map_err(Error::Js)?;
    Ok(())
}

/// A real `IllegalModelException`, decorated with `model_file`/`location`
/// exactly as `new IllegalModelException(message, modelFile, location)`
/// would (`engine/errors.ts`'s `IllegalModel` factory applies the same
/// decoration to `message` unconditionally): `model_file: Some(None)` marks
/// the error as one TS passes a model file to, so [`throw`] attaches the
/// caller's own JS model file object to the payload. `code` stays
/// `"pre-port"` (`message` is TS's own un-templated string concatenation,
/// not a catalogue entry).
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

/// BC-11 (R1): the `IllegalModelException` for a cyclic inheritance chain
/// met by a walk over the JS declaration views, the same error the engine's
/// own walk raises (concerto-core `ModelManager::class_info_of`). `cycle` is
/// the loop from the declaration met again round to the one whose super type
/// it is; `repeated` is that declaration. TS 5.0.0 overflowed V8's stack or
/// ran out of memory instead (DV-013).
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

/// TS: the `classDecl = ...` resolution duplicated in
/// `ClassDeclaration._resolveSuperType`, `.getProperty` and `.getProperties`
/// (src/introspect/classdeclaration.ts): `this.getModelFile().isImportedType(name)`
/// ? `this.modelFile.getModelManager().getType(this.getModelFile().resolveImport(name))`
/// : `this.getModelFile().getType(name)`. `type_name` is the JS string being
/// resolved (`this.superType`, in every caller here); the result may be
/// nullish, exactly as `ModelFile.getType`/`ModelManager.getType` can answer.
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

/// TS: `Declaration.validate`'s own check (P5-11,
/// accordproject/concerto-rust#287), after `super.validate()` (the view's
/// `Decorated.validate`): a declaration may not take the name of a type its
/// model file imports (#648), unless the model manager's
/// `dangerouslyAllowReservedSystemTypeNamesInUserModels` option is set and
/// `this.isReservedSystemTypeImport(modelFile, name)` says the name
/// resolves to a reserved system type. The same rule as concerto-core's
/// `check_import_clash` (validation.rs), run over the view's collaborators,
/// since a direct call may be on a view of a stubbed model file. Throws
/// the `IllegalModelException` TS throws, naming `this.modelFile` and at
/// `this.ast.location`; a collaborator's own error propagates unchanged.
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

/// TS: `Declaration.isReservedSystemTypeImport(modelFile, typeName)`
/// (P5-11, accordproject/concerto-rust#287): whether `typeName` resolves,
/// through `modelFile.getType`, to a declaration of a system model file
/// that is one of the reserved kinds ([`RESERVED_SYSTEM_TYPE_KINDS`]). The
/// same rule as concerto-core's `is_reserved_system_type_import`
/// (validation.rs), run over the view's collaborators; their own errors
/// propagate unchanged.
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

/// TS: `ClassDeclaration._resolveSuperType`. Resolves `this.superType`
/// through [`resolve_named_type`], throws the same `IllegalModelException`
/// TS does when it cannot find the super type or the two kinds are
/// incompatible ([`concerto_core::ClassDeclaration::kinds_compatible`]), and
/// caches the result onto `this.superTypeDeclaration` before returning it —
/// the same field `getSuperTypeDeclaration` reads back as a cache.
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

/// TS: `ClassDeclaration.getSuperTypeDeclaration`: the branch is pure field
/// reads; the fallback calls back `this._resolveSuperType()` (a collaborator
/// call — that method resolves and validates the super type).
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

/// TS: `ClassDeclaration.getAllSuperTypeDeclarations`: repeats
/// `type = type.getSuperTypeDeclaration()` from `this`, collecting every
/// non-null result. A declaration met again is a cyclic inheritance chain,
/// the BC-11 `IllegalModelException` (R1; TS 5.0.0 looped until it ran out
/// of memory, DV-013).
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
/// otherwise the super type's own answer, found through `getLocalType` (or,
/// failing that, the model manager). A `null` super type resolution reaches
/// the same unguarded `classDecl.getIdentifierFieldName()` call TS makes
/// (and the same host `TypeError` `call` raises for it). A declaration met
/// again is a cyclic inheritance chain (BC-11, [`SuperWalk`]).
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

/// P5-19 (accordproject/concerto-rust#317): TS `ClassDeclaration
/// .getIdentifierFieldName`, including its super type walk, in one binding.
///
/// Each level of the walk does what
/// [`class_declaration_get_identifier_field_name`] does, but runs the
/// `ClassDeclaration` methods the TS body reaches here instead of crossing
/// back into JS: `this.getSuperType()` (through `getSuperTypeDeclaration()`,
/// the field reads of [`class_declaration_get_super_type_declaration`], and
/// `getFullyQualifiedName()`, a read of `fqn`), `this.getModelFile()` (a
/// read of `modelFile`) and, the walk itself, `classDecl
/// .getIdentifierFieldName()`. `_resolveSuperType`, `getLocalType`,
/// `getModelManager` and `getType` are still called, as TS calls them, so
/// every error is raised by the same collaborator as before.
///
/// P5-36 (BC-50, accordproject/concerto-rust#346): the walk always inlines.
/// A `ClassDeclaration` method replaced at runtime (on the object or its
/// prototype) is not called; replacing these methods is not supported. A
/// `ScalarDeclaration` or `MapDeclaration` reached as a super type gives the
/// same `null` its own `getIdentifierFieldName` does (no truthy `idField`
/// or `superType`).
///
/// A super type seen earlier in the walk is a cyclic inheritance chain: the
/// BC-11 `IllegalModelException` (R1; TS 5.0.0 recursed until V8's stack
/// overflowed, DV-013).
///
/// Returns `[answer, cacheable, ...chain]`: `chain` is every declaration the
/// walk read, from `declaration` on, and `cacheable` is false when the walk
/// ended in a call (a nullish super type resolution), so its answer depends
/// on more than the chain's fields (engine/views.ts keeps the answer only
/// when it is true).
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

            // `return classDecl.getIdentifierFieldName();` -- a nullish
            // `classDecl` raises the same TypeError through `call`. A
            // declaration met again is a cyclic inheritance chain (BC-11).
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

/// TS: `ClassDeclaration.getProperty`: the receiver's own property if it has
/// one, otherwise the super type's answer (through [`resolve_named_type`]).
/// A `null` super type resolution reaches the same unguarded
/// `classDecl.getProperty(name)` call TS makes.
///
/// The guard is `this.superType !== null` — strict, not TS truthiness — so a
/// fuzzer-produced `this.superType` that is merely falsy (`undefined`, `0`,
/// `false`, `""`, from a `superType` AST node whose `name` was itself falsy;
/// [`class_declaration_process`]'s own module doc) still reaches the same
/// resolution TS does, rather than being treated as "no super type" the way
/// `getSuperType`/`_resolveSuperType`'s own, separate, truthiness guard
/// would (accordproject/concerto-rust#219, P5-05 stage-2 T2c).
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
        // TS tests `this.superType !== null` (classdeclaration.ts), not
        // truthiness: an empty-string or `undefined` super type still goes
        // on to be resolved (#218).
        let super_type = get(&declaration, "superType")?;
        if super_type.is_null() {
            return Ok(JsValue::NULL);
        }
        let class_decl = resolve_named_type(&declaration, &super_type)?;
        call(&class_decl, "getProperty", &[name], "classDecl.getProperty")
    })
}

/// TS: `ClassDeclaration.getProperties`: the receiver's own properties, plus
/// (when it has a super type) the super type's own answer, found through
/// [`resolve_named_type`] — unlike `getProperty`, TS itself guards this
/// resolution with the same "Could not find super type" `IllegalModelException`
/// `_resolveSuperType` raises.
///
/// Same `this.superType !== null` guard as `getProperty` above (not
/// truthiness): accordproject/concerto-rust#219.
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
        // TS tests `this.superType !== null` (classdeclaration.ts), not
        // truthiness: an empty-string or `undefined` super type still goes
        // on to be resolved, fails to, and throws "Could not find super
        // type" (#218).
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
// mapkeytype.ts, mapvaluetype.ts) — P4-07
// ---------------------------------------------------------------------------

/// TS: MapDeclaration.process, after `super.process()`. Checks the AST's
/// `key`/`value` shape with the same checks `ModelUtil.isValidMapKey`/
/// `isValidMapValue` already run through Rust (P2-06), called here
/// natively ([`mu::is_valid_map_key`], [`mu::is_valid_map_value`]) rather
/// than through another JS round trip. The view still builds the
/// `MapKeyType`/`MapValueType` child views itself afterwards, the same way
/// `propertyProcess` still builds its own `CollectionSizeValidator`.
#[wasm_bindgen(js_name = mapDeclarationProcess)]
pub fn map_declaration_process(view: JsValue) -> JsResult<()> {
    let body = || -> Result<()> {
        let ast = get(&view, "ast")?;
        // TS interpolates the raw `this.ast.name` into a template literal
        // in every one of this function's own messages
        // (`MapDeclaration must contain Key & Value properties
        // ${this.ast.name}`, mapdeclaration.ts), which applies JS `ToString`
        // to whatever value is there — including `undefined` (a missing
        // `name` key stringifies to the literal text `"undefined"`, not an
        // empty string) and `null` (`"null"`), not only a real string
        // (accordproject/concerto-rust#219, P5-05 stage-2 T2c): collapsing
        // both of those to `String::new()` reported `"MapDeclaration must
        // contain Key & Value properties  "` (an empty name) where TS
        // reports `"... properties undefined "`/`"... properties null "`.
        let name = js_string(&opt_get(&ast, "name")?)?;
        // TS: `if (!this.ast.key || !this.ast.value)` — plain JS truthiness
        // of the whole node, not merely "not `undefined`": a fuzz-mutated
        // `key`/`value` of `false`, `0`, `null` or `""` is exactly as falsy
        // as a missing one, and must fail this same check, not reach
        // `is_valid_map_key`/`is_valid_map_value`'s own, differently-worded
        // rejection instead (accordproject/concerto-rust#219, P5-05
        // stage-2 T2c: `key: 0` reached `to_json`'s `is_undefined`-only gate
        // here, which passed it through as `Some(0)`, giving "must contain
        // valid MapKeyType" instead of TS's "must contain Key & Value
        // properties" — the same theme `MapDeclaration::from_json`'s native
        // Rust construction path already fixed, here again for this WASM
        // binding's own, separate check).
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

/// TS: MapKeyType.processType. Pure AST logic (module doc); the `$class`
/// switch has no default arm, which is unreachable here since
/// `mapDeclarationProcess`'s `mu::is_valid_map_key` check already restricts
/// the AST to one of these three kinds before a `MapKeyType` is ever built —
/// the empty-string fallback below is never actually observed.
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

/// TS: MapKeyType.validate. `this.modelFile.getType(...)` is a live TS
/// collaborator call (`ModelFile` is not yet Rust-backed, P2-08); the
/// scalar-kind check is [`js_is_valid_map_key_scalar`], over that JS
/// declaration (the arena's own is [`mu::is_valid_map_key_scalar`]).
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
        // `modelFile.getType` returns `null` when the type is not found (not
        // a thrown error), and TS's `isValidMapKeyScalar(decl)` optional-
        // chains off that (`decl?.isScalarDeclaration?.()`), so a nullish
        // `decl` here must become `None`, not `Some` of a JS null.
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

/// TS: MapValueType.processType. Pure AST logic (module doc), except the
/// `ObjectMapValueType`/`RelationshipMapValueType` arm's own shape checks,
/// which TS throws inline for.
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
                // TS: `!('type' in ast)`. `ast` is a genuine JS object here
                // (its own `$class` was just read as a real string above),
                // so the `in` operator itself cannot throw on this check; it
                // is exactly "does the key exist", true whenever `type` is
                // present at all — including an explicit `type: null`, which
                // is present, not missing. `get` returns real `undefined`
                // only for a key that is not there at all, so testing that
                // directly (not `nullish`, which also matches a present
                // `null`) is what keeps the two apart
                // (accordproject/concerto-rust#219 stage-2 T2c: `nullish`
                // here wrongly took the "missing type" branch for a present
                // `type: null`, which TS does not).
                let ast_type = get(&ast, "type")?;
                if ast_type.is_undefined() {
                    return Err(ContractError::new(
                        ErrorKind::IllegalModel,
                        "mapvaluetype-process-missingtype",
                        vec![("name", parent_name)],
                    )
                    .into());
                }
                // TS: `!('$class' in ast.type) || !('name' in ast.type)`.
                // Unlike the check above, `ast.type` is NOT guaranteed to be
                // an object here — a fuzzed AST can set it to `null`, a
                // boolean, a number or a string — and the ECMAScript `in`
                // operator throws a `TypeError` when its right-hand side is
                // not an object (an array or a plain object does not throw;
                // it just falls through to the "malformed type" rejection
                // below like any other object missing both keys).
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
                // TS: `!('$class' in ast.type) || !('name' in ast.type)` —
                // a key-presence check, not a nullish one
                // (accordproject/concerto-rust#219 stage-2 T2c: this used
                // `nullish`, which wrongly took the "malformed type" branch
                // below for a present `type.$class: null`/`type.name: null`,
                // when TS's `in` sees the key, skips this branch, and goes
                // on to the `$class !== 'TypeIdentifier'` check instead —
                // the same "missing key" vs "present but null" distinction
                // this function's own `ast_type.is_undefined()` check above
                // already gets right for the outer `type` key). `get`
                // returns real `undefined` only for a key that is not there
                // at all, exactly like the outer check.
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

/// TS: MapValueType.validate. `this.modelFile.getType(...)` is a live TS
/// collaborator call (`ModelFile` is not yet Rust-backed, P2-08), and so is
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
