//! The `Property`, `Field` and `RelationshipDeclaration` bindings, and the
//! view snapshots.

use super::*;

// ---------------------------------------------------------------------------
// Property (src/introspect/property.ts)
// ---------------------------------------------------------------------------

/// The snapshot [`property_process`] returns for a processed property.
pub(crate) fn property_snapshot(processed: &property::ProcessedProperty) -> Value {
    let mut snapshot = serde_json::Map::new();
    snapshot.insert("name".to_string(), json!(processed.name));
    if processed.type_set {
        snapshot.insert("type".to_string(), json!(processed.property_type));
    }
    snapshot.insert("array".to_string(), json!(processed.array));
    snapshot.insert("optional".to_string(), json!(processed.optional));
    Value::Object(snapshot)
}

/// The snapshot [`field_process`] returns for a processed field.
pub(crate) fn field_snapshot(processed: &field::ProcessedField) -> Value {
    let validator = match &processed.validator {
        None => Value::Null,
        Some(field::FieldValidator::Number(v)) => {
            let mut snapshot = serde_json::to_value(v).unwrap_or(Value::Null);
            if let Value::Object(map) = &mut snapshot {
                map.insert("kind".to_string(), json!("NumberValidator"));
            }
            snapshot
        }
        Some(field::FieldValidator::String { .. }) => json!({ "kind": "StringValidator" }),
    };
    json!({
        "validator": validator,
        "defaultValue": processed.default_value,
    })
}

/// A property node, keeping only the keys `property::process` and
/// `field::process` read. A `null` value reads as absent, which both treat
/// the same way (`as_str`/`as_bool`/truthiness/`!Util.isNull`).
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LightProperty {
    #[serde(rename = "$class")]
    class: Option<Value>,
    name: Option<Value>,
    #[serde(rename = "type")]
    type_: Option<Value>,
    is_array: Option<Value>,
    is_optional: Option<Value>,
    validator: Option<Value>,
    length_validator: Option<Value>,
    default_value: Option<Value>,
    // Read by `model_file_view_snapshot` only, never part of
    // `into_value`.
    decorators: Option<Value>,
    size_validator: Option<Value>,
}

impl LightProperty {
    /// The property as the JSON object the per-property bindings would
    /// have read those keys from.
    pub(crate) fn into_value(self) -> Value {
        let mut map = serde_json::Map::new();
        for (key, value) in [
            ("$class", self.class),
            ("name", self.name),
            ("type", self.type_),
            ("isArray", self.is_array),
            ("isOptional", self.is_optional),
            ("validator", self.validator),
            ("lengthValidator", self.length_validator),
            ("defaultValue", self.default_value),
        ] {
            if let Some(value) = value {
                map.insert(key.to_string(), value);
            }
        }
        Value::Object(map)
    }
}

/// Appends one `{p, f}` entry of [`model_file_property_snapshots`] (or
/// `null`) to `out` as JSON text: the same JSON `property_snapshot` and
/// `field_snapshot` serialise to.
pub(crate) fn write_property_entry(out: &mut String, ast: &Value) -> Option<()> {
    let Ok(processed) = property::process::<Error>(ast) else {
        out.push_str("null");
        return Some(());
    };
    out.push_str("{\"p\":{\"name\":");
    out.push_str(&serde_json::to_string(&processed.name).ok()?);
    if processed.type_set {
        out.push_str(",\"type\":");
        out.push_str(&serde_json::to_string(&processed.property_type).ok()?);
    }
    out.push_str(if processed.array {
        ",\"array\":true"
    } else {
        ",\"array\":false"
    });
    out.push_str(if processed.optional {
        ",\"optional\":true}"
    } else {
        ",\"optional\":false}"
    });
    let property_type = if processed.type_set {
        processed.property_type.as_deref()
    } else {
        None
    };
    // A field error names the view's fully-qualified name, which only the
    // view knows: never produced here (the entry falls back instead).
    let no_fqn = || -> Result<String> { Err(Error::Js(JsValue::UNDEFINED)) };
    out.push_str(",\"f\":");
    match field::process(property_type, ast, &no_fqn) {
        Ok(field) => out.push_str(&serde_json::to_string(&field_snapshot(&field)).ok()?),
        Err(_) => out.push_str("null"),
    }
    out.push('}');
    Some(())
}

/// The view snapshot of a whole model file, from its JSON AST text, so that
/// building a `ModelFile`'s declaration and property views crosses once per
/// file: [`model_file_property_snapshots`] plus each declaration's own
/// construction decisions (`Declaration.process`'s
/// `isValidIdentifier`/`getFullyQualifiedName`, and
/// [`class_declaration_process`]'s). `namespace` is the file's namespace.
///
/// Returns JSON text, an array aligned with `ast.declarations`, of
/// `{"d": d, "p": p}`:
/// - `d` is `{"name", "fqn", "cd", "defaulted"}` for a declaration with a
///   valid string `name` and a non-empty `namespace`, else `null`. `cd` is
///   the `classDeclarationProcess` snapshot, or `null` where it cannot be
///   decided here exactly (a `superType` or `identified` that is not a plain
///   object with a string `name`, or a super-type-less `Concept`, which
///   depends on `isSystemModelFile()`). `defaulted` marks `cd` computed with
///   the default super type `fromAst` gives an asset, participant,
///   transaction or event that names none.
/// - `p` is the [`model_file_property_snapshots`] entry array, or `null`.
///
/// Never throws: whatever would raise an error gets `null`, and the view
/// calls the per-element binding, so every error comes from the same call.
/// `undefined` for text this reading cannot read.
#[wasm_bindgen(js_name = modelFileViewSnapshot)]
pub fn model_file_view_snapshot(ast: &str, namespace: Option<String>) -> Option<String> {
    let model: ViewModel = serde_json::from_str(ast).ok()?;
    view_snapshot(model, namespace, ast.len() / 3)
}

/// [`model_file_view_snapshot`] of an AST the engine already holds, read in
/// place.
pub(crate) fn model_file_view_snapshot_of(
    ast: &Value,
    namespace: Option<String>,
) -> Option<String> {
    use serde::Deserialize;
    let model = ViewModel::deserialize(ast).ok()?;
    view_snapshot(model, namespace, 4096)
}

/// The body of [`model_file_view_snapshot`], for a model already read.
pub(crate) fn view_snapshot(
    model: ViewModel,
    namespace: Option<String>,
    capacity: usize,
) -> Option<String> {
    let declarations = model.declarations?;
    let namespace = namespace.unwrap_or_default();
    let mut out = String::with_capacity(capacity);
    out.push('[');
    for (i, declaration) in declarations.into_iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str("{\"d\":");
        match declaration_view_entry(&declaration, &namespace) {
            Some(entry) => out.push_str(&entry.to_string()),
            None => out.push_str("null"),
        }
        // The declaration's own decorators, and its scalar or map
        // decisions, each only when it has one.
        write_optional(
            &mut out,
            "dec",
            decorators_view_snapshot(declaration.decorators.as_ref()),
        );
        write_optional(&mut out, "s", scalar_view_snapshot(&declaration));
        write_optional(&mut out, "m", map_view_snapshot(&declaration));
        out.push_str(",\"p\":");
        match declaration.properties {
            None => out.push_str("null"),
            Some(properties) => {
                out.push('[');
                for (j, property) in properties.into_iter().enumerate() {
                    if j > 0 {
                        out.push(',');
                    }
                    write_view_property_entry(&mut out, property)?;
                }
                out.push(']');
            }
        }
        out.push('}');
    }
    out.push(']');
    Some(out)
}

/// Appends `,"key":value` to `out` when `value` is `Some`.
pub(crate) fn write_optional(out: &mut String, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        out.push_str(",\"");
        out.push_str(key);
        out.push_str("\":");
        out.push_str(&value.to_string());
    }
}

/// One property's entry of [`model_file_view_snapshot`]: the
/// [`write_property_entry`] `{p, f}` entry (or `null`), with the property's
/// lazily built parts where they can be decided exactly as their bindings
/// decide them: `dec`, its decorators ([`decorators_view_snapshot`]); `sz`,
/// its `collectionSizeValidatorNew` snapshot `{minSize, maxSize}`; `sv`, its
/// `stringValidatorNew` snapshot `{minLength, maxLength}`.
pub(crate) fn write_view_property_entry(
    out: &mut String,
    mut property: LightProperty,
) -> Option<()> {
    let decorators = property.decorators.take();
    let size_validator = property.size_validator.take();
    let ast = property.into_value();
    let start = out.len();
    write_property_entry(out, &ast)?;
    if !out[start..].starts_with('{') {
        return Some(());
    }
    // The entry's closing brace, reopened for the extra keys.
    out.pop();
    write_optional(out, "dec", decorators_view_snapshot(decorators.as_ref()));
    let name = ast.get("name").and_then(Value::as_str);
    if let Some(name) = name {
        write_optional(
            out,
            "sz",
            size_validator_view_snapshot(name, size_validator.as_ref()),
        );
        // `Field.process`'s `StringValidator` arm (field::process): a
        // `String` property with a truthy `validator` or `lengthValidator`.
        let string_typed = property::process::<Error>(&ast)
            .is_ok_and(|p| p.type_set && p.property_type.as_deref() == Some("String"));
        if string_typed
            && (json_truthy(ast.get("validator")) || json_truthy(ast.get("lengthValidator")))
        {
            write_optional(out, "sv", string_validator_view_snapshot(name, &ast));
        }
    }
    out.push('}');
    Some(())
}

/// The element a validator built from JSON is attached to, for
/// [`model_file_view_snapshot`]: its `getName()` and `ast.defaultValue`.
/// With no fully qualified name, any error a constructor would report fails
/// and the entry is left out.
pub(crate) struct JsonElement<'a> {
    name: &'a str,
    default_value: Option<&'a Value>,
}

impl FullyQualified for JsonElement<'_> {
    type Error = Error;

    fn fully_qualified_name(&self) -> Result<String> {
        Err(Error::Js(JsValue::UNDEFINED))
    }
}

impl ValidatedElement for JsonElement<'_> {
    fn default_value(&self) -> Result<Option<Value>> {
        Ok(self.default_value.cloned())
    }

    fn name(&self) -> Result<String> {
        Ok(self.name.to_string())
    }
}

/// `collectionSizeValidatorNew`'s snapshot for a property's truthy
/// `sizeValidator`, or `None` when there is none or its constructor would
/// throw. The bounds are compared as numbers: BC-19's shape check rejects a
/// bound that is not one.
pub(crate) fn size_validator_view_snapshot(name: &str, ast: Option<&Value>) -> Option<Value> {
    let ast = ast.filter(|v| json_truthy(Some(v)))?;
    let typed =
        validators::size_validator_from_ast(Some(ast)).unwrap_or(mm::CollectionSizeValidator {
            _class: Default::default(),
            min_size: None,
            max_size: None,
        });
    let element = JsonElement {
        name,
        default_value: None,
    };
    let built = CollectionSizeValidator::new(&element, &typed, None).ok()?;
    Some(json!({ "minSize": built.min_size(), "maxSize": built.max_size() }))
}

/// `stringValidatorNew`'s snapshot for an element's `validator` and
/// `lengthValidator` (a field, or a String scalar), or `None` when its
/// constructor would throw. The length bounds are compared as numbers, as
/// in [`size_validator_view_snapshot`].
pub(crate) fn string_validator_view_snapshot(name: &str, ast: &Value) -> Option<Value> {
    let validator = ast.get("validator").filter(|v| !v.is_null());
    let length_validator = ast.get("lengthValidator").filter(|v| !v.is_null());
    let regex_ast = validators::regex_validator_from_ast(validator);
    let length_ast = validators::length_validator_from_ast(length_validator);
    let element = JsonElement {
        name,
        default_value: ast.get("defaultValue"),
    };
    let built =
        StringValidator::new(&element, regex_ast.as_ref(), length_ast.as_ref(), None).ok()?;
    Some(json!({ "minLength": built.min_length(), "maxLength": built.max_length() }))
}

/// The `decoratorProcess` results for an AST `decorators` value, as `[{"n":
/// name, "a": arguments}]`, or `None` when there are none or they cannot be
/// decided exactly here (not an array, a node that is not an object or has a
/// non-string `name`, a number JSON cannot carry).
pub(crate) fn decorators_view_snapshot(decorators: Option<&Value>) -> Option<Value> {
    let Some(Value::Array(nodes)) = decorators else {
        return None;
    };
    let mut out = Vec::with_capacity(nodes.len());
    for node in nodes {
        let Value::Object(map) = node else {
            return None;
        };
        if map.get("name").is_some_and(|n| !n.is_string()) {
            return None;
        }
        let decorator = Decorator::from_ast(node);
        let mut arguments = Vec::with_capacity(decorator.arguments().len());
        for argument in decorator.arguments() {
            arguments.push(match argument {
                DecoratorArgument::String(s) => json!(s),
                DecoratorArgument::Number(n) => {
                    Value::Number(serde_json::Number::from_f64(*n).filter(|_| n.is_finite())?)
                }
                DecoratorArgument::Boolean(b) => json!(b),
                DecoratorArgument::TypeReference(t) => match t.array {
                    Some(array) => json!({ "type": "Identifier", "name": t.name, "array": array }),
                    None => json!({ "type": "Identifier", "name": t.name }),
                },
                // `DecoratorArgument` is `#[non_exhaustive]`: a kind the
                // fast path does not know falls back to the view.
                _ => return None,
            });
        }
        let mut entry = serde_json::Map::new();
        if let Some(name) = decorator.js_name() {
            entry.insert("n".to_string(), json!(name));
        }
        entry.insert("a".to_string(), Value::Array(arguments));
        out.push(Value::Object(entry));
    }
    Some(Value::Array(out))
}

/// The metamodel classes `ModelFile.fromAst` builds a `ScalarDeclaration`
/// from.
pub(crate) const SCALAR_CLASSES: [&str; 6] = [
    "concerto.metamodel@1.0.0.BooleanScalar",
    "concerto.metamodel@1.0.0.IntegerScalar",
    "concerto.metamodel@1.0.0.LongScalar",
    "concerto.metamodel@1.0.0.DoubleScalar",
    "concerto.metamodel@1.0.0.StringScalar",
    "concerto.metamodel@1.0.0.DateTimeScalar",
];

/// A scalar declaration's `scalarDeclarationProcess` snapshot `{type,
/// validator, defaultValue}`, where a `StringValidator` also carries its
/// `stringValidatorNew` snapshot (`minLength`, `maxLength`), or `None` when
/// the declaration is not a scalar or processing it would throw.
pub(crate) fn scalar_view_snapshot(declaration: &ViewDeclaration) -> Option<Value> {
    let class = declaration.class.as_ref().and_then(Value::as_str)?;
    if !SCALAR_CLASSES.contains(&class) {
        return None;
    }
    let Some(Value::String(name)) = &declaration.name else {
        return None;
    };
    let mut ast = serde_json::Map::new();
    ast.insert("$class".to_string(), json!(class));
    ast.insert("name".to_string(), json!(name));
    for (key, value) in [
        ("validator", &declaration.validator),
        ("lengthValidator", &declaration.length_validator),
        ("defaultValue", &declaration.default_value),
    ] {
        if let Some(value) = value {
            ast.insert(key.to_string(), value.clone());
        }
    }
    let ast = Value::Object(ast);
    let no_fqn = || -> Result<String> { Err(Error::Js(JsValue::UNDEFINED)) };
    let processed = ScalarDeclaration::process(&ast, None, &no_fqn).ok()?;
    let validator = match &processed.validator {
        None => Value::Null,
        Some(ScalarValidator::Number(v)) => {
            let mut snapshot = serde_json::to_value(v).ok()?;
            let Value::Object(map) = &mut snapshot else {
                return None;
            };
            map.insert("kind".to_string(), json!("NumberValidator"));
            snapshot
        }
        Some(ScalarValidator::String(_)) => {
            let mut snapshot = string_validator_view_snapshot(name, &ast)?;
            let Value::Object(map) = &mut snapshot else {
                return None;
            };
            map.insert("kind".to_string(), json!("StringValidator"));
            snapshot
        }
    };
    Some(json!({
        "type": processed.scalar_type,
        "validator": validator,
        "defaultValue": processed.default_value,
    }))
}

/// A map declaration's `mapDeclarationProcess` decision, with its key and
/// value types' `mapKeyTypeProcess`/`mapValueTypeProcess` types and
/// decorators, as `{"k": {"t", "dec"?}, "v": {"t", "dec"?}}`, or `None` when
/// the declaration is not a map or any of those would throw (or cannot be
/// decided here exactly as the bindings decide it).
pub(crate) fn map_view_snapshot(declaration: &ViewDeclaration) -> Option<Value> {
    if declaration.class.as_ref().and_then(Value::as_str)
        != Some("concerto.metamodel@1.0.0.MapDeclaration")
    {
        return None;
    }
    // `mapDeclarationProcess`: `this.ast.name` is interpolated into its
    // errors; a map with a string name only.
    if !matches!(declaration.name, Some(Value::String(_))) {
        return None;
    }
    let key = declaration.key.as_ref().filter(|v| json_truthy(Some(v)))?;
    let value = declaration
        .value
        .as_ref()
        .filter(|v| json_truthy(Some(v)))?;
    if !matches!(key, Value::Object(_)) || !matches!(value, Value::Object(_)) {
        return None;
    }
    if !mu::is_valid_map_key(Some(key)).ok()? || !mu::is_valid_map_value(Some(value)).ok()? {
        return None;
    }
    fn class(node: &Value) -> Option<&str> {
        node.get("$class").and_then(Value::as_str).map(short_class)
    }
    let key_type = match class(key)? {
        "DateTimeMapKeyType" => "DateTime".to_string(),
        "StringMapKeyType" => "String".to_string(),
        "ObjectMapKeyType" => key.get("type")?.get("name")?.as_str()?.to_string(),
        _ => return None,
    };
    let value_type = match class(value)? {
        "ObjectMapValueType" | "RelationshipMapValueType" => {
            let Some(Value::Object(ty)) = value.get("type") else {
                return None;
            };
            if ty.get("$class").and_then(Value::as_str)
                != Some("concerto.metamodel@1.0.0.TypeIdentifier")
            {
                return None;
            }
            ty.get("name")?.as_str()?.to_string()
        }
        "BooleanMapValueType" => "Boolean".to_string(),
        "DateTimeMapValueType" => "DateTime".to_string(),
        "StringMapValueType" => "String".to_string(),
        "IntegerMapValueType" => "Integer".to_string(),
        "LongMapValueType" => "Long".to_string(),
        "DoubleMapValueType" => "Double".to_string(),
        _ => return None,
    };
    let side = |node: &Value, type_name: String| -> Option<Value> {
        let mut entry = serde_json::Map::new();
        entry.insert("t".to_string(), json!(type_name));
        // A present `decorators` must be one the snapshot can decide.
        match node.get("decorators") {
            None | Some(Value::Null) => {}
            decorators => {
                entry.insert("dec".to_string(), decorators_view_snapshot(decorators)?);
            }
        }
        Some(Value::Object(entry))
    };
    Some(json!({ "k": side(key, key_type)?, "v": side(value, value_type)? }))
}

/// A model AST, as far as [`model_file_view_snapshot`] reads it.
#[derive(serde::Deserialize)]
pub(crate) struct ViewModel {
    declarations: Option<Vec<ViewDeclaration>>,
}

/// A declaration, as far as [`model_file_view_snapshot`] reads it. A `null`
/// value reads as absent; both are falsy to the TS tests this mirrors.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ViewDeclaration {
    #[serde(rename = "$class")]
    class: Option<Value>,
    name: Option<Value>,
    super_type: Option<Value>,
    identified: Option<Value>,
    properties: Option<Vec<LightProperty>>,
    // The declaration's decorators, a scalar's validators and default
    // value, and a map's key and value types.
    decorators: Option<Value>,
    validator: Option<Value>,
    length_validator: Option<Value>,
    default_value: Option<Value>,
    key: Option<Value>,
    value: Option<Value>,
}

/// JavaScript truthiness of a JSON value (`None` is `undefined`).
pub(crate) fn json_truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(Value::Array(_)) | Some(Value::Object(_)) => true,
    }
}

/// The metamodel declaration classes `ModelFile.fromAst` gives a default
/// super type when their AST names none, with that super type's name.
pub(crate) const DEFAULT_SUPER_TYPES: [(&str, &str); 4] = [
    ("concerto.metamodel@1.0.0.AssetDeclaration", "Asset"),
    (
        "concerto.metamodel@1.0.0.TransactionDeclaration",
        "Transaction",
    ),
    ("concerto.metamodel@1.0.0.EventDeclaration", "Event"),
    (
        "concerto.metamodel@1.0.0.ParticipantDeclaration",
        "Participant",
    ),
];

/// One declaration's `d` entry of [`model_file_view_snapshot`], or `None`.
pub(crate) fn declaration_view_entry(
    declaration: &ViewDeclaration,
    namespace: &str,
) -> Option<Value> {
    // `Declaration.process`: `isValidIdentifier(this.ast.name)`, then
    // `this.fqn` from a truthy namespace.
    let Some(Value::String(name)) = &declaration.name else {
        return None;
    };
    if namespace.is_empty() || !mu::is_valid_identifier(name) {
        return None;
    }
    let fqn = mu::qualify(namespace, name);

    // `ModelFile.fromAst`'s default super type for four declaration kinds.
    let class = declaration.class.as_ref().and_then(Value::as_str);
    let defaulted_to = if json_truthy(declaration.super_type.as_ref()) {
        None
    } else {
        DEFAULT_SUPER_TYPES
            .iter()
            .find(|(c, _)| Some(*c) == class)
            .map(|(_, t)| *t)
    };
    let cd = class_declaration_view_decision(declaration, name, &fqn, defaulted_to);
    Some(json!({
        "name": name,
        "fqn": fqn,
        "cd": cd,
        "defaulted": defaulted_to.is_some(),
    }))
}

/// The [`class_declaration_process`] snapshot for a declaration read from
/// JSON, or `Value::Null` when it cannot be decided here exactly as the
/// binding decides it from the view.
pub(crate) fn class_declaration_view_decision(
    declaration: &ViewDeclaration,
    name: &str,
    fqn: &str,
    defaulted_to: Option<&str>,
) -> Value {
    // `this.ast.superType`: truthy, then its raw `.name`.
    let super_type: Option<String> = if let Some(t) = defaulted_to {
        Some(t.to_string())
    } else if json_truthy(declaration.super_type.as_ref()) {
        match declaration.super_type.as_ref() {
            Some(Value::Object(node)) => match node.get("name") {
                Some(Value::String(s)) => Some(s.clone()),
                _ => return Value::Null,
            },
            _ => return Value::Null,
        }
    } else {
        None
    };
    // A super-type-less `Concept`: the decision reads
    // `this.modelFile.isSystemModelFile()`, which only the view knows.
    if super_type.is_none() && name == "Concept" {
        return Value::Null;
    }
    // `this.ast.identified`: truthy, then its `$class` (strict equality) and,
    // for `IdentifiedBy`, its raw `.name`.
    let (identified_class, identified_name) = if json_truthy(declaration.identified.as_ref()) {
        match declaration.identified.as_ref() {
            Some(Value::Object(node)) => {
                let class = node
                    .get("$class")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                if class == "concerto.metamodel@1.0.0.IdentifiedBy" {
                    match node.get("name") {
                        Some(Value::String(s)) => (Some(class), Some(s.clone())),
                        _ => return Value::Null,
                    }
                } else {
                    (Some(class), None)
                }
            }
            _ => return Value::Null,
        }
    } else {
        (None, None)
    };
    let decision = concerto_core::ClassDeclaration::process_decision(
        super_type.as_deref(),
        false,
        name,
        identified_class.as_deref(),
        identified_name.as_deref(),
        fqn,
    );
    json!({
        "superType": decision.super_type,
        "idField": decision.id_field,
        "addIdentifierField": decision.add_identifier_field,
        "addTimestampField": decision.add_timestamp_field,
    })
}

/// TS: Property.process, after `super.process()`. Returns the snapshot
/// `{name, type, array, optional}`, `type` omitted for an `EnumProperty`
/// (TS never assigns `this.type` there). The view builds
/// `this.sizeValidator` itself.
#[wasm_bindgen(js_name = propertyProcess)]
pub fn property_process(view: JsValue) -> JsResult<JsValue> {
    let body = || -> Result<JsValue> {
        let ast = to_json(&get(&view, "ast")?)?.unwrap_or(Value::Null);
        let processed = property::process::<Error>(&ast)?;
        Ok(to_js(&property_snapshot(&processed)))
    };
    run_naming(
        || call(&view, "getModelFile", &[], "this.getModelFile").unwrap_or(JsValue::UNDEFINED),
        body,
    )
}

/// TS: Property.validate, after `super.validate()` (which the view calls
/// first). `classDecl` is TS's argument. Type resolution and the
/// map-declaration check call back the JS collaborators TS calls
/// (`modelFile.resolveType`, `modelFile.getType`), whose errors propagate.
#[wasm_bindgen(js_name = propertyValidate)]
pub fn property_validate(property: JsValue, class_decl: JsValue) -> JsResult<()> {
    let model_file = call(&class_decl, "getModelFile", &[], "classDecl.getModelFile")
        .unwrap_or(JsValue::UNDEFINED);
    let body = || -> Result<()> {
        let property_type = get(&property, "type")?;
        // TS: `if(this.type)`, JS truthiness.
        if property_type.is_truthy() {
            let fqn = js_string(&call(
                &property,
                "getFullyQualifiedName",
                &[],
                "this.getFullyQualifiedName",
            )?)?;
            let message = JsValue::from_str(&format!("property {fqn}"));
            call(
                &model_file,
                "resolveType",
                &[message, property_type.clone()],
                "modelFile.resolveType",
            )?;
        }

        let size_validator = get(&property, "sizeValidator")?;
        let array = get(&property, "array")?.is_truthy();
        if !nullish(&size_validator) && !array {
            let mut is_map_type = false;
            // TS: `if(this.type && !this.isPrimitive())`.
            if property_type.is_truthy() {
                let is_primitive =
                    call(&property, "isPrimitive", &[], "this.isPrimitive")?.is_truthy();
                if !is_primitive
                    && let Ok(resolved) = call(
                        &model_file,
                        "getType",
                        std::slice::from_ref(&property_type),
                        "modelFile.getType",
                    )
                    && let Some(v) = call_optional(&resolved, "isMapDeclaration")?
                {
                    is_map_type = v.is_truthy();
                }
            }
            if !is_map_type {
                let fqn = js_string(&call(
                    &property,
                    "getFullyQualifiedName",
                    &[],
                    "this.getFullyQualifiedName",
                )?)?;
                let ast = get(&property, "ast")?;
                let location = to_json(&get(&ast, "location")?)?;
                let mut err = ContractError::new(
                    ErrorKind::IllegalModel,
                    "property-validate-sizevalidator",
                    vec![("fqn", fqn)],
                );
                err.location = location;
                err.model_file = Some(None);
                return Err(err.into());
            }
        }
        Ok(())
    };
    run_naming(|| model_file.clone(), body)
}

// ---------------------------------------------------------------------------
// Field (src/introspect/field.ts)
// ---------------------------------------------------------------------------

/// TS: Field.process, after Property's. Returns the snapshot `{validator,
/// defaultValue}`, `validator` chosen as `scalarDeclarationProcess` chooses
/// it ([`ScalarValidator`]).
#[wasm_bindgen(js_name = fieldProcess)]
pub fn field_process(view: JsValue) -> JsResult<JsValue> {
    let body = || -> Result<JsValue> {
        let ast = to_json(&get(&view, "ast")?)?.unwrap_or(Value::Null);
        let property_type = get(&view, "type")?;
        let property_type = if nullish(&property_type) {
            None
        } else {
            Some(js_string(&property_type)?)
        };
        let fqn = || {
            js_string(&call(
                &view,
                "getFullyQualifiedName",
                &[],
                "this.getFullyQualifiedName",
            )?)
        };
        let processed = field::process(property_type.as_deref(), &ast, &fqn)?;
        Ok(to_js(&field_snapshot(&processed)))
    };
    run_naming(
        || call(&view, "getModelFile", &[], "this.getModelFile").unwrap_or(JsValue::UNDEFINED),
        body,
    )
}

/// TS: `Field.getScalarField`, after the view's `this.scalarField` cache
/// check. `isTypeScalar()`'s collaborator calls go back to the JS objects,
/// as in `propertyValidate`; the scalar-to-property `$class` mapping is
/// `field::scalar_to_field_ast`. Returns the synthetic field's AST, from
/// which the view builds the `Field` and sets `array`, as TS does.
#[wasm_bindgen(js_name = fieldGetScalarField)]
pub fn field_get_scalar_field(view: JsValue) -> JsResult<JsValue> {
    run(|| {
        // `isTypeScalar()`.
        let is_primitive = call(&view, "isPrimitive", &[], "this.isPrimitive")?.is_truthy();
        let resolved_type = if is_primitive {
            None
        } else {
            let parent = call(&view, "getParent", &[], "this.getParent")?;
            let model_file = call(&parent, "getModelFile", &[], "parent.getModelFile")?;
            let fqn = js_string(&call(
                &view,
                "getFullyQualifiedName",
                &[],
                "this.getFullyQualifiedName",
            )?)?;
            let property_type = call(&view, "getType", &[], "this.getType")?;
            call(
                &model_file,
                "resolveType",
                &[
                    JsValue::from_str(&format!("property {fqn}")),
                    property_type.clone(),
                ],
                "modelFile.resolveType",
            )?;
            Some(call(
                &model_file,
                "getType",
                &[property_type],
                "modelFile.getType",
            )?)
        };
        // `type.isScalarDeclaration?.()`: `undefined` (no such method) reads
        // as falsy, same as `call_optional`'s `None`; `resolved_type` is
        // `None` when `isPrimitive()` was true, which is also not scalar.
        let not_scalar = |view: &JsValue| -> Result<Error> {
            let name = js_string(&get(view, "name")?)?;
            Ok(plain_error(
                "field-getscalarfield-notscalar",
                vec![("name", name)],
            ))
        };
        let Some(resolved) = resolved_type else {
            return Err(not_scalar(&view)?);
        };
        let is_type_scalar =
            call_optional(&resolved, "isScalarDeclaration")?.is_some_and(|v| v.is_truthy());
        if !is_type_scalar {
            return Err(not_scalar(&view)?);
        }
        let scalar_ast = to_json(&get(&resolved, "ast")?)?.unwrap_or(Value::Null);
        let field_name = to_json(&get(&get(&view, "ast")?, "name")?)?.unwrap_or(Value::Null);
        let field_ast = field::scalar_to_field_ast::<Error>(&scalar_ast, field_name)?;
        Ok(to_js(&field_ast))
    })
}

/// TS: `Field.toString`: `name`, `array` and `optional` read off `this`;
/// `getFullyQualifiedTypeName()` called through the view (a scalar field's
/// is the scalar's own FQN).
#[wasm_bindgen(js_name = fieldToString)]
pub fn field_to_string(view: JsValue) -> JsResult<String> {
    run(|| {
        let name = js_string(&get(&view, "name")?)?;
        let fully_qualified_type_name = js_string(&call(
            &view,
            "getFullyQualifiedTypeName",
            &[],
            "this.getFullyQualifiedTypeName",
        )?)?;
        let array = get(&view, "array")?.is_truthy();
        let optional = get(&view, "optional")?.is_truthy();
        Ok(field::to_string(
            &name,
            &fully_qualified_type_name,
            array,
            optional,
        ))
    })
}

// ---------------------------------------------------------------------------
// RelationshipDeclaration (src/introspect/relationshipdeclaration.ts)
// ---------------------------------------------------------------------------

/// TS: RelationshipDeclaration.validate, after Property's (which the view
/// calls first). It calls back the JS collaborators TS calls, in the same
/// order and with the same try/catch shape (the own model file's lookup is
/// unguarded, as in TS).
#[wasm_bindgen(js_name = relationshipDeclarationValidate)]
pub fn relationship_declaration_validate(view: JsValue, class_decl: JsValue) -> JsResult<()> {
    let model_file = call(&class_decl, "getModelFile", &[], "classDecl.getModelFile")
        .unwrap_or(JsValue::UNDEFINED);
    let body = || -> Result<()> {
        let ast = get(&view, "ast")?;
        let location = to_json(&get(&ast, "location")?)?;
        let name = js_string(&call(&view, "getName", &[], "this.getName")?)?;
        // TS calls `this.getType()`, so a stubbed `getType()` takes effect.
        let property_type = call(&view, "getType", &[], "this.getType")?;

        // TS: `if(!this.getType())`, JS truthiness.
        if !property_type.is_truthy() {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "relationshipdeclaration-validate-notype",
                vec![],
            );
            err.location = location;
            err.model_file = Some(None);
            return Err(err.into());
        }

        let type_str = js_string(&property_type)?;
        if mu::is_primitive_type(&type_str) {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "relationshipdeclaration-validate-primitivetype",
                vec![("name", name), ("type", type_str)],
            );
            err.location = location;
            err.model_file = Some(None);
            return Err(err.into());
        }

        let parent = call(&view, "getParent", &[], "this.getParent")?;
        let namespace = js_string(&call(&parent, "getNamespace", &[], "parent.getNamespace")?)?;
        let fqtn = js_string(&call(
            &view,
            "getFullyQualifiedTypeName",
            &[],
            "this.getFullyQualifiedTypeName",
        )?)?;
        let type_namespace = mu::get_namespace(Some(&fqtn))?.to_string();
        let parent_model_file = call(&parent, "getModelFile", &[], "parent.getModelFile")?;

        let mut class_declaration: Option<JsValue> = None;
        if namespace == type_namespace {
            // TS does not guard this lookup: any error it raises propagates.
            let resolved = call(
                &parent_model_file,
                "getType",
                std::slice::from_ref(&property_type),
                "modelFile.getType",
            )?;
            if !nullish(&resolved) {
                class_declaration = Some(resolved);
            }
        } else if let Ok(model_manager) = call(
            &parent_model_file,
            "getModelManager",
            &[],
            "modelFile.getModelManager",
        ) && let Ok(resolved) = call(
            &model_manager,
            "getType",
            &[JsValue::from_str(&fqtn)],
            "modelManager.getType",
        ) && !nullish(&resolved)
        {
            class_declaration = Some(resolved);
        }

        let Some(class_declaration) = class_declaration else {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "relationshipdeclaration-validate-missingtype",
                vec![("name", name), ("type", fqtn)],
            );
            err.location = location;
            err.model_file = Some(None);
            return Err(err.into());
        };

        let is_identified = call(
            &class_declaration,
            "isIdentified",
            &[],
            "classDeclaration.isIdentified",
        )?
        .is_truthy();
        if !is_identified {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "relationshipdeclaration-validate-notidentified",
                vec![("name", name), ("type", fqtn)],
            );
            err.location = location;
            err.model_file = Some(None);
            return Err(err.into());
        }
        Ok(())
    };
    run_naming(|| model_file.clone(), body)
}
