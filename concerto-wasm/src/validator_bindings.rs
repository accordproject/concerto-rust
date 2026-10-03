//! The `NumberValidator`, `StringValidator` and `CollectionSizeValidator` bindings.
//!
//! Split out of `lib.rs` (P5-104, review M7); the crate root glob-imports it.

use super::*;

// ---------------------------------------------------------------------------
// NumberValidator (src/introspect/numbervalidator.ts)
// ---------------------------------------------------------------------------

/// The element a JS validator is attached to, read through the validator the
/// way `Validator` does: `this.field` and `this.getFieldOrScalarDeclaration()`.
/// This is the collaborator-call path of the `needs_fallback` constructor row
/// (the tests build it over `sinon.createStubInstance(Field)`).
pub(crate) struct JsElement<'a> {
    validator: &'a JsValue,
}

impl ValidatedElement for JsElement<'_> {
    fn default_value(&self) -> Result<Option<Value>> {
        // `this.field?.ast?.defaultValue`
        let field = get(self.validator, "field")?;
        if nullish(&field) {
            return Ok(None);
        }
        let ast = get(&field, "ast")?;
        if nullish(&ast) {
            return Ok(None);
        }
        to_json(&get(&ast, "defaultValue")?)
    }

    fn name(&self) -> Result<String> {
        // `this.field.getName()`
        let field = get(self.validator, "field")?;
        js_string(&call(&field, "getName", &[], "this.field.getName")?)
    }
}

impl FullyQualified for JsElement<'_> {
    type Error = Error;

    fn fully_qualified_name(&self) -> Result<String> {
        let element = call(
            self.validator,
            "getFieldOrScalarDeclaration",
            &[],
            "this.getFieldOrScalarDeclaration",
        )?;
        js_string(&call(
            &element,
            "getFullyQualifiedName",
            &[],
            "this.getFieldOrScalarDeclaration(...).getFullyQualifiedName",
        )?)
    }
}

/// The validator's snapshot, read back from the view's fields.
pub(crate) fn number_validator(
    view: &JsValue,
    lower: &str,
    upper: &str,
) -> Result<NumberValidator> {
    let read = |name: &str| -> Result<Value> {
        let value = if name.starts_with("get") {
            call(view, name, &[], name)?
        } else {
            get(view, name)?
        };
        Ok(to_json(&value)?.unwrap_or(Value::Null))
    };
    let snapshot = json!({ "lowerBound": read(lower)?, "upperBound": read(upper)? });
    serde_json::from_value(snapshot).map_err(internal)
}

/// The `{lowerBound, upperBound}` snapshot as a JS object.
pub(crate) fn number_snapshot(validator: &NumberValidator) -> JsValue {
    serde_json::to_value(validator).map_or(JsValue::NULL, |v| to_js(&v))
}

/// TS: NumberValidator constructor, after `super(field, ast)`. `view` is the
/// object under construction; the result is its `{lowerBound, upperBound}`.
/// `ast.lower`/`ast.upper` are read only when they are own properties.
#[wasm_bindgen(js_name = numberValidatorNew)]
pub fn number_validator_new(view: JsValue, ast: JsValue) -> JsResult<JsValue> {
    run(|| {
        let own = |key: &str| -> Result<Option<Value>> {
            // `Object.prototype.hasOwnProperty.call(ast, key)`
            if !ast.is_object()
                || !Object::has_own(ast.unchecked_ref::<Object>(), &JsValue::from_str(key))
            {
                return Ok(None);
            }
            Ok(Some(to_json(&get(&ast, key)?)?.unwrap_or(Value::Null)))
        };
        let mut node = serde_json::Map::new();
        for key in ["lower", "upper"] {
            if let Some(value) = own(key)? {
                node.insert(key.to_string(), value);
            }
        }
        let validator =
            NumberValidator::new(&JsElement { validator: &view }, &Value::Object(node))?;
        Ok(number_snapshot(&validator))
    })
}

/// TS: NumberValidator.validate. `null` is always accepted; any other value
/// is compared as JS `<`/`>` would, through `Number(value)` (ResourceValidator
/// only passes finite numbers). A non-null identifier prints as `String(id)`.
#[wasm_bindgen(js_name = numberValidatorValidate)]
pub fn number_validator_validate(
    view: JsValue,
    identifier: JsValue,
    value: JsValue,
) -> JsResult<()> {
    run(|| {
        let validator = number_validator(&view, "lowerBound", "upperBound")?;
        let identifier = if identifier.is_null() {
            None
        } else {
            Some(js_string(&identifier)?)
        };
        let value = if value.is_null() {
            None
        } else {
            // `+value`: JS ToNumber.
            Some(value.unchecked_into_f64())
        };
        validator.validate(
            &JsElement { validator: &view },
            identifier.as_deref(),
            value,
        )
    })
}

/// TS: NumberValidator.toString
#[wasm_bindgen(js_name = numberValidatorToString)]
pub fn number_validator_to_string(view: JsValue) -> JsResult<String> {
    run(|| Ok(number_validator(&view, "lowerBound", "upperBound")?.to_string()))
}

/// TS: NumberValidator.compatibleWith. `number_validator_class` is the
/// `NumberValidator` class, for the `other instanceof NumberValidator` check;
/// the bounds are read through the getters, as TS does.
#[wasm_bindgen(js_name = numberValidatorCompatibleWith)]
pub fn number_validator_compatible_with(
    view: JsValue,
    other: JsValue,
    number_validator_class: Function,
) -> JsResult<bool> {
    run(|| {
        // `other instanceof NumberValidator`
        let prototype = get(&number_validator_class, "prototype")?;
        if !other.is_object() || !prototype.unchecked_ref::<Object>().is_prototype_of(&other) {
            return Ok(false);
        }
        let this = number_validator(&view, "getLowerBound", "getUpperBound")?;
        let other = number_validator(&other, "getLowerBound", "getUpperBound")?;
        Ok(this.compatible_with(Some(&Validator::Number(other))))
    })
}

// ---------------------------------------------------------------------------
// StringValidator (src/introspect/stringvalidator.ts) and
// CollectionSizeValidator (src/introspect/collectionsizevalidator.ts) (P4-04)
// ---------------------------------------------------------------------------
//
// Neither Rust type derives `Serialize`/`Deserialize` (`StringValidator` owns
// a compiled `regress::Regex`, which does not), so unlike `NumberValidator`
// there is no snapshot to deserialise a validator back from on every
// `validate`/`compatibleWith` call. Instead each call rebuilds the validator
// from the view's own cached AST (`view.validator`, the regex AST `super()`
// stored; length/size bounds read back from the snapshot the constructor
// cached), the same inputs the constructor itself validated, so the rebuild
// is deterministic and never observably re-runs a check that could now fail
// differently.

/// Tags a plain JS AST object (as the unit tests and the TS views hand it
/// across, with no `$class`) with the metamodel type it is, so it
/// deserialises into the typed AST the core validators take.
pub(crate) fn tag(mut json: Value, class: &str) -> Value {
    if let Value::Object(map) = &mut json {
        map.entry("$class".to_string())
            .or_insert_with(|| json!(class));
    }
    json
}

/// `{pattern, flags}`, or `None` for a nullish value. Built through
/// `validators::regex_validator_from_ast`, which reads `pattern`/`flags`
/// completely untyped — a plain `ToString`-style coercion, matching `new
/// RegExp(validator.pattern, validator.flags)` — rather than `serde`'s
/// strict decode this used to run directly: TS's own call site for this
/// constructor is `Property.process`'s `new StringValidator(this,
/// this.ast.validator, this.ast.lengthValidator)` (property.ts/field.ts),
/// reading `this.ast.validator` with no type check at all, so a
/// fuzz-mutated, wrongly-typed `pattern`/`flags` (a bool, a number, an
/// array) must coerce here too, not fail the whole property's `process()`
/// (accordproject/concerto-rust#217: this binding is *TS's* call site for
/// the same constructor `validators::regex_validator_from_ast`'s own doc
/// comment already fixed the `Property::try_from` side of, so it needs the
/// identical fix).
pub(crate) fn string_regex_ast(value: &JsValue) -> Result<Option<mm::StringRegexValidator>> {
    if nullish(value) {
        return Ok(None);
    }
    let json = to_json(value)?.unwrap_or(Value::Null);
    Ok(validators::regex_validator_from_ast(Some(&json)))
}

/// `{minLength, maxLength}`, or `None` for a nullish value. Built through
/// `validators::length_validator_from_ast`, for the same reason and in the
/// same way as [`string_regex_ast`] — TS's own call site for
/// `new StringValidator(..., this.ast.lengthValidator)`
/// (accordproject/concerto-rust#217).
pub(crate) fn string_length_ast(value: &JsValue) -> Result<Option<mm::StringLengthValidator>> {
    if nullish(value) {
        return Ok(None);
    }
    let json = to_json(value)?.unwrap_or(Value::Null);
    Ok(validators::length_validator_from_ast(Some(&json)))
}

/// `{minSize, maxSize}`. Built through `validators::size_validator_from_ast`,
/// for the same reason as [`string_regex_ast`] — TS's own call site for
/// `new CollectionSizeValidator(this, this.ast.sizeValidator)`
/// (property.ts/field.ts), reading `minSize`/`maxSize` with no type check
/// (accordproject/concerto-rust#217). A nullish `value` (this binding's own
/// caller, like TS's constructor call site, only ever passes one when
/// `this.ast.sizeValidator` is itself present) falls back to the same
/// "$class only" node `size_validator_from_ast`'s own null-filter maps to
/// `None` for, so this preserves this function's pre-existing contract of
/// never itself returning `None`: an absent `minSize`/`maxSize` decodes as
/// `None` either way, so the unwrap below only ever supplies the
/// `$class`/bounds-absent shape.
pub(crate) fn collection_size_ast(value: &JsValue) -> Result<mm::CollectionSizeValidator> {
    let json = to_json(value)?.unwrap_or(Value::Null);
    Ok(
        validators::size_validator_from_ast(Some(&json)).unwrap_or(mm::CollectionSizeValidator {
            _class: Default::default(),
            min_size: None,
            max_size: None,
        }),
    )
}

/// TS: StringValidator constructor, after `super(field, validator)`. `view`
/// is the object under construction; the result is its `{minLength,
/// maxLength}` snapshot. The view builds its own cached `RegExp` from
/// `validator` afterwards: `StringValidator.getRegex` stays TS (public API
/// returns a live `RegExp`), and the pluggable `options.regExp` hook is never
/// reached here (the view only calls this binding when no hook is
/// configured, PORTING.md section 3).
#[wasm_bindgen(js_name = stringValidatorNew)]
pub fn string_validator_new(
    view: JsValue,
    validator: JsValue,
    length_validator: JsValue,
) -> JsResult<JsValue> {
    run(|| {
        let regex_ast = string_regex_ast(&validator)?;
        let length_ast = string_length_ast(&length_validator)?;
        // The raw `lengthValidator` AST itself, not just its typed
        // `{minLength, maxLength}` snapshot (`length_ast` above): a
        // fuzz-mutated `minLength`/`maxLength` needs JS's own untyped `>`
        // comparison, not one against a value already coerced to `f64`
        // (accordproject/concerto-rust#219). `nullish` mirrors
        // `string_length_ast`'s own guard: no AST, no raw value to compare.
        let raw_length_ast = if nullish(&length_validator) {
            None
        } else {
            to_json(&length_validator)?
        };
        let built = StringValidator::new(
            &JsElement { validator: &view },
            regex_ast.as_ref(),
            length_ast.as_ref(),
            raw_length_ast.as_ref(),
        )?;
        Ok(to_js(&json!({
            "minLength": built.min_length(),
            "maxLength": built.max_length(),
        })))
    })
}

/// Rebuilds the validator's snapshot from the view: the regex AST `super()`
/// cached at `view.validator`, and the length bounds the constructor cached
/// at `view.minLength`/`view.maxLength` (`None` for both, the only state a
/// successful construction can have left, means no length AST was ever
/// given).
pub(crate) fn string_validator(view: &JsValue) -> Result<StringValidator> {
    let regex_ast = string_regex_ast(&get(view, "validator")?)?;
    let min_length = get(view, "minLength")?;
    let max_length = get(view, "maxLength")?;
    let length_ast = if nullish(&min_length) && nullish(&max_length) {
        None
    } else {
        let json = tag(
            json!({
                "minLength": to_json(&min_length)?,
                "maxLength": to_json(&max_length)?,
            }),
            "concerto.metamodel@1.0.0.StringLengthValidator",
        );
        Some(serde_json::from_value::<mm::StringLengthValidator>(json).map_err(internal)?)
    };
    StringValidator::new(
        &JsElement { validator: view },
        regex_ast.as_ref(),
        length_ast.as_ref(),
        // No raw AST to re-compare here: this rebuilds the validator from a
        // view whose construction already succeeded, so `minLength`/
        // `maxLength` are already the real, in-order numbers a prior,
        // successful `stringValidatorNew` call normalised.
        None,
    )
}

/// TS: StringValidator.validate. `null` is always accepted.
#[wasm_bindgen(js_name = stringValidatorValidate)]
pub fn string_validator_validate(
    view: JsValue,
    identifier: JsValue,
    value: JsValue,
) -> JsResult<()> {
    run(|| {
        let validator = string_validator(&view)?;
        let identifier = if identifier.is_null() {
            None
        } else {
            Some(js_string(&identifier)?)
        };
        let value = if value.is_null() {
            None
        } else {
            Some(js_string(&value)?)
        };
        validator.validate(
            &JsElement { validator: &view },
            identifier.as_deref(),
            value.as_deref(),
        )
    })
}

/// TS: StringValidator.compatibleWith. `string_validator_class` is the
/// `StringValidator` class, for the `other instanceof StringValidator` check.
#[wasm_bindgen(js_name = stringValidatorCompatibleWith)]
pub fn string_validator_compatible_with(
    view: JsValue,
    other: JsValue,
    string_validator_class: Function,
) -> JsResult<bool> {
    run(|| {
        // `other instanceof StringValidator`
        let prototype = get(&string_validator_class, "prototype")?;
        if !other.is_object() || !prototype.unchecked_ref::<Object>().is_prototype_of(&other) {
            return Ok(false);
        }
        let this = string_validator(&view)?;
        let other = string_validator(&other)?;
        Ok(this.compatible_with(Some(&Validator::String(other))))
    })
}

/// TS: CollectionSizeValidator constructor, after `super(field, validator)`.
/// `view` is the object under construction; the result is its `{minSize,
/// maxSize}` snapshot.
#[wasm_bindgen(js_name = collectionSizeValidatorNew)]
pub fn collection_size_validator_new(view: JsValue, ast: JsValue) -> JsResult<JsValue> {
    run(|| {
        let typed = collection_size_ast(&ast)?;
        // The raw `sizeValidator` AST itself, not just its typed `{minSize,
        // maxSize}` snapshot (`typed` above): a fuzz-mutated `minSize`/
        // `maxSize` needs JS's own untyped `>` comparison, not one against a
        // value already coerced to `f64` (accordproject/concerto-rust#219).
        let raw_ast = to_json(&ast)?;
        let built = CollectionSizeValidator::new(
            &JsElement { validator: &view },
            &typed,
            raw_ast.as_ref(),
        )?;
        Ok(to_js(&json!({
            "minSize": built.min_size(),
            "maxSize": built.max_size(),
        })))
    })
}

/// Rebuilds the validator from `view.validator`, the AST `super()` cached.
pub(crate) fn collection_size_validator(view: &JsValue) -> Result<CollectionSizeValidator> {
    let ast = collection_size_ast(&get(view, "validator")?)?;
    // No raw AST to re-compare here: this rebuilds the validator from a view
    // whose construction already succeeded, so `minSize`/`maxSize` are
    // already the real, in-order numbers a prior, successful
    // `collectionSizeValidatorNew` call normalised.
    CollectionSizeValidator::new(&JsElement { validator: view }, &ast, None)
}

/// TS: CollectionSizeValidator.validate. `value` is compared as JS `<`/`>`
/// would, through `Number(value)`.
#[wasm_bindgen(js_name = collectionSizeValidatorValidate)]
pub fn collection_size_validator_validate(
    view: JsValue,
    identifier: JsValue,
    value: JsValue,
) -> JsResult<()> {
    run(|| {
        let validator = collection_size_validator(&view)?;
        let identifier = if identifier.is_null() {
            None
        } else {
            Some(js_string(&identifier)?)
        };
        // `+value`: JS ToNumber.
        let value = value.unchecked_into_f64();
        validator.validate(
            &JsElement { validator: &view },
            identifier.as_deref(),
            value,
        )
    })
}

/// TS: CollectionSizeValidator.compatibleWith.
/// `collection_size_validator_class` is the `CollectionSizeValidator` class,
/// for the `other instanceof CollectionSizeValidator` check.
#[wasm_bindgen(js_name = collectionSizeValidatorCompatibleWith)]
pub fn collection_size_validator_compatible_with(
    view: JsValue,
    other: JsValue,
    collection_size_validator_class: Function,
) -> JsResult<bool> {
    run(|| {
        // `other instanceof CollectionSizeValidator`
        let prototype = get(&collection_size_validator_class, "prototype")?;
        if !other.is_object() || !prototype.unchecked_ref::<Object>().is_prototype_of(&other) {
            return Ok(false);
        }
        let this = collection_size_validator(&view)?;
        let other = collection_size_validator(&other)?;
        Ok(this.compatible_with(Some(&Validator::CollectionSize(other))))
    })
}
