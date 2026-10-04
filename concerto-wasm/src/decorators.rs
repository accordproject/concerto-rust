//! The `Decorator` and `Decorated` bindings.
//!
//! Split out of `lib.rs`; the crate root glob-imports it.

use super::*;

// ---------------------------------------------------------------------------
// Decorator, Decorated (src/introspect/decorator.ts, decorated.ts)
// ---------------------------------------------------------------------------

/// TS: Decorator.process. Builds `{name, arguments}` from the raw AST node
/// ([`Decorator::from_ast`]).
///
/// DV-018: a `null` or `undefined` node, where TS's `this.ast.name` throws a
/// `TypeError`, is an `IllegalModelException` instead
/// ([`decorator::not_an_object`]). `view`, when given, is the `Decorator`
/// being processed; its `getParent().getModelFile()` is the model file the
/// exception names, as TS's `Decorator.handleError` passes it.
#[wasm_bindgen(js_name = decoratorProcess)]
pub fn decorator_process(ast: JsValue, view: JsValue) -> JsResult<JsValue> {
    if ast.is_null() || ast.is_undefined() {
        let mut err = decorator::not_an_object(if ast.is_null() { "null" } else { "undefined" });
        err.model_file = Some(None);
        let model_file = if view.is_undefined() || view.is_null() {
            None
        } else {
            call(&view, "getParent", &[], "this.getParent")
                .and_then(|parent| {
                    call(
                        &parent,
                        "getModelFile",
                        &[],
                        "this.getParent().getModelFile",
                    )
                })
                .ok()
        };
        return Err(throw(err.into(), model_file.as_ref()));
    }
    run(|| {
        let ast_json = to_json(&ast)?.unwrap_or(Value::Null);
        let decorator = Decorator::from_ast(&ast_json);
        let arguments = Array::new();
        for arg in decorator.arguments() {
            arguments.push(&argument_to_js(arg));
        }
        let out = Object::new();
        // TS: `this.name = ast.name`, uncoerced, so a node with no `name`
        // leaves `this.name` `undefined`, not the empty string. That shows in
        // `Decorated.validate`'s duplicate scan ("Duplicate decorator
        // undefined"), so this reads `js_name`, not `name()`.
        let name_js = decorator
            .js_name()
            .map_or(JsValue::UNDEFINED, JsValue::from_str);
        set(&out, "name", &name_js);
        set(&out, "arguments", &arguments);
        Ok(out.into())
    })
}

/// One decoded [`DecoratorArgument`], as TS's `Decorator.process` would have
/// pushed it onto `this.arguments`. Built as a JS value directly, not
/// through [`to_js`]'s JSON round trip, which cannot represent `undefined`
/// (TS: `{ type: 'Identifier', name: ..., array: thing.isArray }` — the
/// object literal always creates the `array` *property*, even when
/// `thing.isArray` is `undefined`, which is a different, observable state
/// from the property being absent).
pub(crate) fn argument_to_js(arg: &DecoratorArgument) -> JsValue {
    match arg {
        DecoratorArgument::String(s) => JsValue::from_str(s),
        DecoratorArgument::Number(n) => JsValue::from_f64(*n),
        DecoratorArgument::Boolean(b) => JsValue::from_bool(*b),
        DecoratorArgument::TypeReference(t) => {
            let out = Object::new();
            set(&out, "type", &JsValue::from_str("Identifier"));
            set(&out, "name", &JsValue::from_str(&t.name));
            let array = t.array.map_or(JsValue::UNDEFINED, JsValue::from_bool);
            set(&out, "array", &array);
            out.into()
        }
        // `DecoratorArgument` is `#[non_exhaustive]`; no other kind exists
        // today.
        _ => JsValue::UNDEFINED,
    }
}

/// TS: the duplicate-decorator loop in `Decorated.validate`
/// (src/introspect/decorated.ts) — `names` is `this.decorators.map(d =>
/// d.getName())`. Returns the first name that repeats, in original order, or
/// `null`; the view throws the `IllegalModelException` itself (a plain
/// string message, no engine error payload needed).
#[wasm_bindgen(js_name = decoratedFindDuplicateName)]
pub fn decorated_find_duplicate_name(names: JsValue) -> JsResult<JsValue> {
    run(|| {
        // Decorator names come from user models: a seeded set
        // (PORTING.md 3.7).
        let mut seen = concerto_core::hash::SeededHashSet::default();
        for name in Array::from(&names).iter() {
            let name = js_string(&name)?;
            if !seen.insert(name.clone()) {
                return Ok(JsValue::from_str(&name));
            }
        }
        Ok(JsValue::NULL)
    })
}

/// `value?.name`: `undefined` for a nullish `value`, as an optional-chain
/// property read is (unlike [`get`], which raises V8's error for one).
pub(crate) fn opt_get(value: &JsValue, name: &str) -> Result<JsValue> {
    if nullish(value) {
        return Ok(JsValue::UNDEFINED);
    }
    get(value, name)
}

/// JS `typeof value`, for the shapes a decorator argument or a decorator
/// validation option's value can be.
pub(crate) fn js_typeof(value: &JsValue) -> &'static str {
    if value.as_f64().is_some() {
        "number"
    } else if value.as_string().is_some() {
        "string"
    } else if value.as_bool().is_some() {
        "boolean"
    } else if value.is_undefined() {
        "undefined"
    } else {
        "object"
    }
}

/// `JSON.stringify(value)`.
pub(crate) fn json_stringify(value: &JsValue) -> Result<String> {
    JSON::stringify(value)
        .map(|s| s.as_string().unwrap_or_default())
        .map_err(Error::Js)
}

/// One property of a decorator's own type declaration, as
/// `Decorator.validate` reads it: `p.getName()`, `p.isOptional()`,
/// `p.getType()`, from the arena (BC-52). `id` is kept for the
/// [`mu::is_assignable_to`] call.
pub(crate) struct PropertyView {
    id: PropId,
    name: String,
    optional: bool,
    type_name: Option<String>,
}

/// An option level (`'error'`, `'warn'`, ...) of a `DecoratorValidationOptions`
/// value, `None` when it is falsy: TS's `if (validationOptions.missingDecorator
/// || ...)` and `level === 'error'` checks, together.
pub(crate) fn level_option(options: &JsValue, key: &str) -> Result<Option<String>> {
    let value = get(options, key)?;
    if !value.is_truthy() {
        return Ok(None);
    }
    Ok(Some(js_string(&value)?))
}

/// `level === undefined` when TS's `validationOptions.<x>` is falsy, else the
/// level string, as a `JsValue` for a `handleError` call.
pub(crate) fn level_js(level: &Option<String>) -> JsValue {
    level
        .as_deref()
        .map_or(JsValue::UNDEFINED, JsValue::from_str)
}

/// TS: `this.handleError(level, err)`, called back on `view` so its own
/// method builds the exact `IllegalModelException` (message, model file,
/// location) and logs through `Logger.dispatch`, exactly as every other
/// call site of `handleError` does. `err` is a message string for one of
/// this function's own checks, or (from the outer catch,
/// [`ModelManagerHandle::decorator_validate`]) whatever the try-equivalent
/// threw — TS passes `handleError` either shape.
pub(crate) fn handle_error(view: &JsValue, level: &Option<String>, err: &JsValue) -> Result<()> {
    call(
        view,
        "handleError",
        &[level_js(level), err.clone()],
        "this.handleError",
    )?;
    Ok(())
}

/// TS: `this.handleError(validationOptions.invalidDecorator, err)` with a
/// message this function built itself.
pub(crate) fn report_invalid(
    view: &JsValue,
    invalid: &Option<String>,
    message: String,
) -> Result<()> {
    handle_error(view, invalid, &JsValue::from_str(&message))
}

/// TS `Decorator.validate`'s reads of the model, answered by the arena
/// (BC-52): `mf` is the model file the decorator's parent belongs to, by
/// handle. The decorator itself (`name`, `arguments`, `ast.location`) and
/// its `handleError` are the JS view's.
pub(crate) struct DecoratorCheck<'a> {
    pub(crate) manager: &'a ModelManager,
    pub(crate) view: &'a JsValue,
    pub(crate) model_file: &'a JsValue,
    pub(crate) file: ModelFileId,
    pub(crate) invalid: &'a Option<String>,
}

impl DecoratorCheck<'_> {
    /// The body of TS `Decorator.validate`'s `try` block.
    pub(crate) fn try_validate(&self, context: Option<&str>) -> Result<()> {
        let view = self.view;
        let name = js_string(&get(view, "name")?)?;
        // TS: `mf.resolveType(decoratedName, this.getName(), this.ast.location);
        // const decoratorDecl = mf.getType(this.getName());`: `getType`
        // returning nothing is treated as `resolveType` failing, as the
        // native `Decorator::validate` does (`resolve_own_name`).
        let Some(decorator_decl) =
            ResolutionContext::get_type(self.manager, &Node::ModelFile(self.file), Some(&name))?
        else {
            let raw = format!(
                "Undeclared type \"{}\" in \"{}\".",
                name,
                context.unwrap_or("undefined"),
            );
            // `ModelFile.resolveType`'s own `IllegalModelException(message, mf,
            // location)`, built by the shim, so that `handleError` rethrows it
            // as it is (BC-14).
            let location = to_json(&opt_get(&get(view, "ast")?, "location")?)?;
            let err = illegal_model_error(raw, location);
            return Err(Error::Js(throw(err, Some(self.model_file))));
        };

        let properties = self.properties_of(decorator_decl)?;
        let (required, optional): (Vec<&PropertyView>, Vec<&PropertyView>) =
            properties.iter().partition(|p| !p.optional);
        let ordered: Vec<&PropertyView> = required
            .iter()
            .copied()
            .chain(optional.iter().copied())
            .collect();

        let arguments = Array::from(&get(view, "arguments")?);
        let arg_count = arguments.length() as usize;

        if arg_count < required.len() {
            let names = required
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>()
                .join(",");
            report_invalid(
                view,
                self.invalid,
                format!(
                    "Decorator {name} has too few arguments. Required properties are: [{names}]"
                ),
            )?;
        }

        for n in 0..arg_count {
            let arg = arguments.get(n as u32);
            let Some(property) = ordered.get(n) else {
                let names = ordered
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",");
                report_invalid(
                    view,
                    self.invalid,
                    format!("Decorator {name} has too many arguments. Properties are: [{names}]"),
                )?;
                continue;
            };
            self.check_argument(&name, property, &arg)?;
        }
        Ok(())
    }

    /// TS: `decoratorDecl.getProperties()`, each read as a [`PropertyView`].
    /// Only a class-like or enum declaration has the method: anything else
    /// `mf.getType` returns (a scalar or map declaration, a primitive type's
    /// name) is V8's "is not a function" `TypeError`, as TS raises.
    pub(crate) fn properties_of(&self, decorator_decl: Node) -> Result<Vec<PropertyView>> {
        let not_a_function = || {
            type_error(
                "engine-typeerror-notafunction",
                vec![("expression", "decoratorDecl.getProperties".to_string())],
            )
        };
        let Node::Declaration(id) = decorator_decl else {
            return Err(not_a_function());
        };
        let declaration = self
            .manager
            .declaration(id)
            .ok_or_else(|| unknown(decorator_decl))?;
        if declaration.is_scalar_declaration() || declaration.is_map_declaration() {
            return Err(not_a_function());
        }
        let properties = self.manager.class_properties_of(id)?;
        properties
            .ids()
            .map(|id| {
                let property = self
                    .manager
                    .property_by_id(id)
                    .ok_or_else(|| unknown(Node::Property(id)))?;
                Ok(PropertyView {
                    id,
                    name: property.name().to_string(),
                    optional: property.is_optional(),
                    type_name: property.type_name().map(str::to_string),
                })
            })
            .collect()
    }

    /// TS: one iteration of the `switch (property.getType())` in
    /// `Decorator.validate`.
    pub(crate) fn check_argument(
        &self,
        name: &str,
        property: &PropertyView,
        arg: &JsValue,
    ) -> Result<()> {
        let expected = match property.type_name.as_deref() {
            Some("Integer") | Some("Double") | Some("Long") => {
                (arg.as_f64().is_none()).then_some("number")
            }
            Some("String") => (arg.as_string().is_none()).then_some("string"),
            Some("Boolean") => (arg.as_bool().is_none()).then_some("boolean"),
            _ => return self.check_type_reference_argument(name, property, arg),
        };
        if let Some(expected) = expected {
            return report_invalid(
                self.view,
                self.invalid,
                format!(
                    "Decorator {name} has invalid decorator argument. Expected {expected}. Found {}, with value {}",
                    js_typeof(arg),
                    json_stringify(arg)?,
                ),
            );
        }
        Ok(())
    }

    /// TS: the `default:` arm — the argument must be a type reference,
    /// resolvable, and assignable to the property's declared type.
    pub(crate) fn check_type_reference_argument(
        &self,
        name: &str,
        property: &PropertyView,
        arg: &JsValue,
    ) -> Result<()> {
        let (view, invalid) = (self.view, self.invalid);
        // TS: `typeof arg !== 'object' || arg?.type !== 'Identifier'`.
        let is_type_reference = js_typeof(arg) == "object"
            && opt_get(arg, "type")?.as_string().as_deref() == Some("Identifier");
        if !is_type_reference {
            report_invalid(
                view,
                invalid,
                format!(
                    "Decorator {name} has invalid decorator argument. Expected object. Found {}, with value {}",
                    js_typeof(arg),
                    json_stringify(arg)?,
                ),
            )?;
        }
        // TS: `handleError` above only throws when the decorator validation
        // option is `'error'` (a `?` propagation here, matching TS's `throw`),
        // so under `'warn'` control falls through to here with no
        // `return`/`else` guarding it in the TS `default:` arm, even though
        // `arg` may still not be a type reference. `typeReference.name` is a
        // direct (non-optional) property read of `arg`, which is exactly what
        // `get` already reproduces: V8's own `TypeError` for a nullish `arg`,
        // `undefined` for a non-object `arg`.
        let type_name = js_string(&get(arg, "name")?)?;
        // TS: `mf.getType(typeReference.name)` — non-throwing.
        let manager = self.manager;
        let Some(type_decl) =
            ResolutionContext::get_type(manager, &Node::ModelFile(self.file), Some(&type_name))?
        else {
            return report_invalid(
                view,
                invalid,
                format!(
                    "Decorator {name} references a type {type_name} which has not been defined/imported."
                ),
            );
        };
        let type_model_file = ResolutionContext::get_model_file(manager, &type_decl)?;
        let type_fqn = ResolutionContext::get_fully_qualified_name(manager, &type_decl)?;
        let property_node = Node::Property(property.id);
        if !mu::is_assignable_to(manager, &type_model_file, &type_fqn, &property_node)? {
            let property_fqn =
                ResolutionContext::get_fully_qualified_type_name(manager, &property_node)?;
            report_invalid(
                view,
                invalid,
                format!(
                    "Decorator {name} references a type {type_name} which cannot be assigned to the declared type {property_fqn}"
                ),
            )?;
        }
        Ok(())
    }
}
