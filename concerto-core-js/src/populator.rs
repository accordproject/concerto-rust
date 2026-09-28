//! `JSONPopulator` (src/serializer/jsonpopulator.ts): populates an
//! [`Instance`] from a JSON object graph, with its per-field checks,
//! coercions and messages (task P3-01b, accordproject/concerto-rust#124).
//!
//! PORTING.md section 5 row 6 (option B): the TS visitor shell stays (the
//! white-box tests spy on `visitX`), and every check and coercion it runs
//! is this module's. Each function here is one TS method, in the same
//! order, so that the first error thrown is the same one (2.4). The TS
//! `jsonStack`/`resourceStack` pushes and pops become arguments and return
//! values; `parameters.path` is `Populator::path`.

use std::borrow::Cow;
use std::fmt::Write as _;

use indexmap::IndexMap;

use super::factory::{self, InstanceEnv};
use crate::deserialize::DeserializeOptions;
use crate::value::{Instance, JsValue};
use concerto_core::error::{ContractError, ErrorKind, Result};
use concerto_core::instance::dayjs::{Dayjs, UtcOffset};
use concerto_core::instance::from_json::{
    required_null_error, strict_qualified_date_time, unknown_keys_error,
};
use concerto_core::instance::model::{self, Field, FieldType, TypeRef};
use concerto_core::introspect::Declaration;
use concerto_core::model_manager::{ClassProperties, ModelManager};
use concerto_core::{Error, model_util};

/// The `JSONPopulator` constructor's options.
#[derive(Debug, Clone, PartialEq)]
pub struct PopulatorOptions {
    /// `acceptResourcesForRelationships`.
    pub accept_resources_for_relationships: bool,
    /// `utcOffset || 0`: the offset non-strict `DateTime` values get.
    pub utc_offset: JsValue,
    /// `strictQualifiedDateTimes`.
    pub strict_qualified_date_times: bool,
    /// `rejectUnknownKeys` and `rejectRequiredNull` (accordproject/concerto#1273).
    pub deserialize: DeserializeOptions,
}

/// The visitor's state: its options and `parameters`.
pub(crate) struct Populator<'a> {
    pub mm: &'a ModelManager,
    pub env: &'a mut dyn InstanceEnv,
    pub options: &'a PopulatorOptions,
    /// `parameters.path`, a `TypedStack` that starts as `['$']`, kept
    /// joined (P5-13): what `path.stack.join('')` reads, with
    /// [`Self::path_marks`] recording where each pushed segment starts.
    path: String,
    path_marks: Vec<usize>,
}

fn validation(code: &'static str, params: Vec<(&'static str, String)>) -> Error {
    ContractError::new(ErrorKind::Validation, code, params).into()
}

fn plain_error(code: &'static str, params: Vec<(&'static str, String)>) -> Error {
    ContractError::new(ErrorKind::InvalidArgument, code, params).into()
}

/// TS: `JSONPopulator.convertToObject`'s primitive-type switch alone (task
/// P4-10, accordproject/concerto-rust#69): the part that needs no
/// declaration lookup, so the TS visitor shell can call it per field
/// directly (via the concerto-wasm binding), keeping its own recursion and
/// `jsonStack`/`resourceStack` handling -- and so the tests that spy on
/// `visitX` still see the same calls, in the same order, that they always
/// did.
pub fn convert_primitive(
    type_name: &str,
    json: &JsValue,
    options: &PopulatorOptions,
    path: &str,
) -> Result<JsValue> {
    let wrong_type = || {
        validation(
            "jsonpopulator-converttoobject-wrongtype",
            vec![("path", path.to_string()), ("type", type_name.to_string())],
        )
    };
    Ok(match type_name {
        "DateTime" => {
            let result = match json {
                JsValue::DateTime(d) => d.clone(),
                JsValue::String(s) => {
                    if !options.strict_qualified_date_times {
                        Dayjs::utc_parse(s).utc_offset_set(&utc_offset_input(&options.utc_offset))
                    } else if strict_qualified_date_time(s) {
                        Dayjs::utc_parse(s)
                    } else {
                        return Err(validation(
                            "jsonpopulator-converttoobject-datetimeformat",
                            vec![("path", path.to_string()), ("type", type_name.to_string())],
                        ));
                    }
                }
                _ => return Err(wrong_type()),
            };
            if !result.is_valid() {
                return Err(wrong_type());
            }
            JsValue::DateTime(result)
        }
        "Integer" | "Long" => match json {
            // `Math.trunc(num) !== num` (Infinity passes, NaN does not). DV-012
            JsValue::Number(n) if n.trunc() == *n => json.clone(),
            _ => return Err(wrong_type()),
        },
        "Double" => match json {
            JsValue::Number(_) => json.clone(),
            _ => return Err(wrong_type()),
        },
        "Boolean" => match json {
            JsValue::Bool(_) => json.clone(),
            _ => return Err(wrong_type()),
        },
        "String" => match json {
            JsValue::String(_) => json.clone(),
            _ => return Err(wrong_type()),
        },
        // Everything else should be an enumerated value.
        _ => json.clone(),
    })
}

/// TS `ResourceValidator.checkItem`'s primitive `switch(field.getType())`
/// (task P4-10, accordproject/concerto-rust#69, resourcevalidator.ts:397):
/// whether `value` (already coerced by [`convert_primitive`], as a real
/// Resource's field value always is by the time it reaches `checkItem`) is
/// valid for the declared primitive type `type_name`. `checkItem` reports
/// `dataType === 'undefined' || dataType === 'symbol'` before this switch
/// (a check the wire codec cannot cross, so the TS shell still makes it);
/// every other TS branch is `typeof`/`isFinite` on the value alone, with no
/// declaration lookup, so it is safe to call from the TS shell per field.
/// A type name the TS switch has no `case` for is valid (`invalid` stays
/// `false`).
pub fn primitive_field_valid(type_name: &str, value: &JsValue) -> bool {
    match type_name {
        "String" => matches!(value, JsValue::String(_)),
        "Long" | "Integer" | "Double" => matches!(value, JsValue::Number(n) if n.is_finite()),
        "Boolean" => matches!(value, JsValue::Bool(_)),
        "DateTime" => matches!(value, JsValue::DateTime(_)),
        // TS: `let invalid = false;` and no `default:` arm, so any other
        // type name (an enum, or a primitive the switch does not list) is
        // valid here.
        _ => true,
    }
}

/// V8's `TypeError: Cannot read properties of <value> (reading '<property>')`.
pub(crate) fn read_properties_error(value: &JsValue, property: &str) -> Error {
    ContractError::new(
        ErrorKind::MalformedInput,
        "engine-typeerror-readproperties",
        vec![
            ("value", value.to_js_string()),
            ("property", property.to_string()),
        ],
    )
    .into()
}

/// `value[key]`, for a value a JSON document can hold. Reading a property
/// of `undefined` or `null` is V8's `TypeError`.
pub(crate) fn get_property(value: &JsValue, key: &str) -> Result<JsValue> {
    Ok(match value {
        JsValue::Undefined | JsValue::Null => return Err(read_properties_error(value, key)),
        JsValue::Object(map) => map.get(key).cloned().unwrap_or(JsValue::Undefined),
        JsValue::Instance(instance) => instance.get(key).clone(),
        JsValue::String(s) => match key.parse::<usize>() {
            // An index reads one UTF-16 unit (a lone surrogate half becomes
            // U+FFFD here, which no recorded document reaches).
            Ok(i) if key == i.to_string() => {
                s.encode_utf16().nth(i).map_or(JsValue::Undefined, |u| {
                    JsValue::String(String::from_utf16_lossy(&[u]))
                })
            }
            _ if key == "length" => JsValue::Number(s.encode_utf16().count() as f64),
            _ => JsValue::Undefined,
        },
        JsValue::Array(items) => match key.parse::<usize>() {
            Ok(i) if key == i.to_string() => items.get(i).cloned().unwrap_or(JsValue::Undefined),
            _ if key == "length" => JsValue::Number(items.len() as f64),
            _ => JsValue::Undefined,
        },
        _ => JsValue::Undefined,
    })
}

/// [`get_property`], borrowing an object's or an instance's own property
/// value rather than cloning it (P5-13).
pub(crate) fn get_property_ref<'v>(value: &'v JsValue, key: &str) -> Result<Cow<'v, JsValue>> {
    match value {
        JsValue::Object(map) => Ok(map
            .get(key)
            .map_or(Cow::Owned(JsValue::Undefined), Cow::Borrowed)),
        JsValue::Instance(instance) => Ok(Cow::Borrowed(instance.get(key))),
        _ => get_property(value, key).map(Cow::Owned),
    }
}

/// `Object.keys(value)`: V8's `TypeError` for `undefined` and `null`.
pub(crate) fn object_keys(value: &JsValue) -> Result<Vec<String>> {
    Ok(object_keys_ref(value)?
        .into_iter()
        .map(Cow::into_owned)
        .collect())
}

/// [`object_keys`], borrowing each key that the value itself holds (P5-13).
pub(crate) fn object_keys_ref(value: &JsValue) -> Result<Vec<Cow<'_, str>>> {
    Ok(match value {
        JsValue::Undefined | JsValue::Null => {
            return Err(ContractError::new(
                ErrorKind::MalformedInput,
                "engine-typeerror-convertnulltoobject",
                Vec::new(),
            )
            .into());
        }
        JsValue::Object(map) => {
            // Integer-like keys come first, in ascending order.
            let mut indices: Vec<(u32, &String)> = map
                .keys()
                .filter_map(|k| {
                    k.parse::<u32>()
                        .ok()
                        .filter(|i| *i != u32::MAX && k == &i.to_string())
                        .map(|i| (i, k))
                })
                .collect();
            indices.sort_by_key(|(i, _)| *i);
            let mut keys: Vec<Cow<'_, str>> = indices
                .into_iter()
                .map(|(_, k)| Cow::Borrowed(k.as_str()))
                .collect();
            let leading = keys.len();
            keys.extend(
                map.keys()
                    .filter(|k| !keys[..leading].iter().any(|seen| seen == k.as_str()))
                    .map(|k| Cow::Borrowed(k.as_str()))
                    .collect::<Vec<_>>(),
            );
            keys
        }
        JsValue::String(s) => (0..s.encode_utf16().count())
            .map(|i| Cow::Owned(i.to_string()))
            .collect(),
        JsValue::Array(items) => (0..items.len())
            .map(|i| Cow::Owned(i.to_string()))
            .collect(),
        JsValue::Instance(instance) => {
            let mut keys = vec![
                Cow::Borrowed("$modelManager"),
                Cow::Borrowed("$classDeclaration"),
            ];
            for key in instance.props.keys() {
                keys.push(Cow::Borrowed(key.as_str()));
                if key == "$timestamp"
                    && instance.kind == crate::value::InstanceKind::ValidatedResource
                {
                    keys.push(Cow::Borrowed("$validator"));
                }
            }
            keys
        }
        _ => Vec::new(),
    })
}

/// TS `getAssignableProperties(resourceData, classDeclaration)`: the keys
/// that have a value and are not system properties, after the reserved
/// property and `$timestamp` checks.
fn get_assignable_properties<'j>(
    resource_data: &'j JsValue,
    declaration: &TypeRef,
) -> Result<Vec<Cow<'j, str>>> {
    let properties = object_keys_ref(resource_data)?;
    let private: Vec<&str> = properties
        .iter()
        .filter(|p| model_util::is_private_system_property(p))
        .map(|p| &**p)
        .collect();
    if !private.is_empty() {
        return Err(validation(
            "jsonpopulator-getassignableproperties-reservedproperties",
            vec![
                ("fqn", declaration.fqn().to_string()),
                ("properties", private.join(", ")),
            ],
        ));
    }
    if properties.iter().any(|p| p == "$timestamp")
        && !(declaration.is_transaction() || declaration.is_event())
    {
        return Err(validation(
            "jsonpopulator-getassignableproperties-timestamp",
            vec![("fqn", declaration.fqn().to_string())],
        ));
    }
    let mut assignable = Vec::with_capacity(properties.len());
    for property in properties {
        if model_util::is_system_property(&property) {
            continue;
        }
        if get_property_ref(resource_data, &property)?.is_nullish() {
            continue;
        }
        assignable.push(property);
    }
    Ok(assignable)
}

/// TS `validateProperties(properties, classDeclaration)`, against the
/// declaration's `getProperties()`.
fn validate_properties(
    properties: &[Cow<'_, str>],
    class_declaration: &TypeRef,
    expected: &ClassProperties,
) -> Result<()> {
    let invalid: Vec<&str> = properties
        .iter()
        .filter(|p| !expected.contains(p))
        .map(|p| &**p)
        .collect();
    if !invalid.is_empty() {
        return Err(validation(
            "jsonpopulator-validateproperties-unexpectedproperties",
            vec![
                ("fqn", class_declaration.fqn().to_string()),
                ("properties", invalid.join(", ")),
            ],
        ));
    }
    Ok(())
}

impl<'a> Populator<'a> {
    pub fn new(
        mm: &'a ModelManager,
        env: &'a mut dyn InstanceEnv,
        options: &'a PopulatorOptions,
    ) -> Self {
        Self {
            mm,
            env,
            options,
            path: "$".to_string(),
            path_marks: Vec::new(),
        }
    }

    /// `parameters.path.stack.join('')`.
    fn path_text(&self) -> &str {
        &self.path
    }

    /// `parameters.path.push(segment)`.
    fn push_path(&mut self, segment: std::fmt::Arguments<'_>) {
        self.path_marks.push(self.path.len());
        // Writing to a `String` cannot fail.
        let _ = self.path.write_fmt(segment);
    }

    /// `parameters.path.pop()`.
    fn pop_path(&mut self) {
        if let Some(mark) = self.path_marks.pop() {
            self.path.truncate(mark);
        }
    }

    /// TS `declaration.accept(this, parameters)` for a declaration: `visit`
    /// dispatches a class declaration (or an enum, which is one) to
    /// `visitClassDeclaration` and a map to `visitMapDeclaration`. The
    /// resource popped by `visitClassDeclaration` is `resource`.
    fn accept_declaration(
        &mut self,
        declaration: &TypeRef,
        json: &JsValue,
        resource: Option<Instance>,
    ) -> Result<JsValue> {
        if declaration.is_class_declaration() {
            let resource = resource.ok_or_else(|| {
                // `parameters.resourceStack.pop()` on an empty stack: not
                // reached, since every caller pushes a resource first.
                Error::from(ContractError::pre_port(
                    ErrorKind::InvalidArgument,
                    "Stack is empty!".to_string(),
                    None,
                ))
            })?;
            return Ok(JsValue::Instance(Box::new(self.visit_class_declaration(
                declaration,
                json,
                resource,
            )?)));
        }
        if declaration.is_map_declaration() {
            return self.visit_map_declaration(declaration, json);
        }
        // `throw new Error('Unrecognised ' + JSON.stringify(thing))`: a
        // scalar declaration.
        Err(model::unrecognised())
    }

    /// TS: JSONPopulator.visitClassDeclaration.
    pub fn visit_class_declaration(
        &mut self,
        class_declaration: &TypeRef,
        json: &JsValue,
        mut resource: Instance,
    ) -> Result<Instance> {
        let properties = get_assignable_properties(json, class_declaration)?;
        let options = self.options.deserialize;
        if options.reject_unknown_keys {
            self.reject_unknown_keys(json, class_declaration)?;
        }
        // `classDeclaration.getProperties()`, read once for
        // `validateProperties` and each `getProperty` below (the same
        // answer every time: the model does not change mid-walk).
        let class_properties = class_declaration.properties("classDeclaration.getProperties")?;
        validate_properties(&properties, class_declaration, &class_properties)?;
        if options.reject_required_null {
            self.reject_required_null(json, class_declaration)?;
        }
        for property in &properties {
            let value = get_property_ref(json, property)?;
            if *value != JsValue::Null {
                self.push_path(format_args!(".{property}"));
                let (owner_fqn, class_property) = class_properties
                    .find(property)
                    .expect("validateProperties found every property");
                let field = model::field(self.mm, owner_fqn, class_property)?;
                let populated = self.visit_property(&field, &value)?;
                resource.set(property, populated);
                self.pop_path();
            }
        }
        Ok(resource)
    }

    /// `rejectUnknownKeys` (accordproject/concerto#1273): every key that is
    /// not a system property and that the declaration does not declare,
    /// whatever its value (`null` included), in one error with one
    /// `UNKNOWN_PROPERTY` detail per key.
    fn reject_unknown_keys(&self, json: &JsValue, class_declaration: &TypeRef) -> Result<()> {
        let expected = class_declaration.properties("classDeclaration.getProperties")?;
        let unknown: Vec<String> = object_keys_ref(json)?
            .into_iter()
            .filter(|p| !model_util::is_system_property(p) && !expected.contains(p))
            .map(Cow::into_owned)
            .collect();
        if unknown.is_empty() {
            return Ok(());
        }
        Err(unknown_keys_error(
            class_declaration.fqn(),
            self.path_text(),
            &unknown,
        ))
    }

    /// `rejectRequiredNull` (accordproject/concerto#1273): the first
    /// declared, required property (in the document's key order) whose value
    /// is `null` fails at once with its path and declared type, and a
    /// `TYPE_VIOLATION` detail.
    fn reject_required_null(&self, json: &JsValue, class_declaration: &TypeRef) -> Result<()> {
        for key in object_keys_ref(json)? {
            if model_util::is_system_property(&key)
                || *get_property_ref(json, &key)? != JsValue::Null
            {
                continue;
            }
            let Some((_, property)) = class_declaration.property(&key)? else {
                continue;
            };
            if property.is_optional() {
                continue;
            }
            return Err(required_null_error(self.path_text(), &key, property));
        }
        Ok(())
    }

    /// TS `classProperty.accept(this, parameters)`, through `visit`: a
    /// relationship to `visitRelationshipDeclaration`, a scalar field to
    /// `visitField(thing.getScalarField())`, any other field to `visitField`.
    fn visit_property(&mut self, field: &Field, json: &JsValue) -> Result<JsValue> {
        match &field.field_type {
            FieldType::Relationship(_) => self.visit_relationship_declaration(field, json),
            // An enum value is not a `Field`: `visit` falls through to
            // `'Unrecognised ' + JSON.stringify(thing)`.
            FieldType::EnumValue => Err(model::unrecognised()),
            _ => self.visit_field(field, json),
        }
    }

    /// TS: JSONPopulator.visitMapDeclaration.
    fn visit_map_declaration(
        &mut self,
        map_declaration: &TypeRef,
        json: &JsValue,
    ) -> Result<JsValue> {
        // Throws if the map holds reserved properties.
        get_assignable_properties(json, map_declaration)?;
        let Declaration::Map(map) = map_declaration.decl else {
            unreachable!("visit_map_declaration is only reached for a map");
        };
        let key_type = map.key_type_name().to_string();
        let value_type = map.value_type_name().to_string();
        let mut result: Vec<(JsValue, JsValue)> = Vec::new();
        // `new Map(Object.entries(jsonObj))`
        for key in object_keys(json)? {
            let value = get_property(json, &key)?;
            let mut key = JsValue::String(key);
            let mut value = value;
            if key.as_str() == Some("$class") {
                map_set(&mut result, key, value);
                continue;
            }
            if !model_util::is_primitive_type(&key_type) {
                key = self.process_map_type(map_declaration, &key, &key_type)?;
            }
            if !model_util::is_primitive_type(&value_type) {
                value = self.process_map_type(map_declaration, &value, &value_type)?;
            }
            map_set(&mut result, key, value);
        }
        Ok(JsValue::Map(result))
    }

    /// TS: JSONPopulator.processMapType.
    fn process_map_type(
        &mut self,
        map_declaration: &TypeRef,
        value: &JsValue,
        type_name: &str,
    ) -> Result<JsValue> {
        let namespace = map_declaration.namespace();
        // `try { ... } catch (err) { decl = undefined; }`
        let declaration = (|| -> Option<TypeRef<'a>> {
            let class_name = match value {
                JsValue::Object(_) | JsValue::Instance(_) | JsValue::Array(_) | JsValue::Map(_) => {
                    get_property(value, "$class")
                        .ok()
                        .filter(JsValue::is_truthy)
                }
                _ => None,
            };
            let name = match class_name {
                Some(JsValue::String(s)) => s,
                Some(_) => return None,
                None => self
                    .mm
                    .model_file_fully_qualified_type_name(namespace, type_name)?,
            };
            model::get_type(self.mm, &name).ok()
        })();
        if let Some(declaration) = declaration
            && declaration.is_class_declaration()
        {
            // `newConcept(ns, name, decl.getIdentifierFieldName())`: the
            // field's name as the identifier. DV-011
            let id = match declaration.identifier_field_name()? {
                Some(name) => JsValue::String(name.to_string()),
                None => JsValue::Null,
            };
            let sub_resource = factory::new_resource_of(&declaration, id, false, self.env)?;
            return self.accept_declaration(&declaration, value, Some(sub_resource));
        }
        // TS's `catch` leaves `value` exactly as parsed: an explicit but
        // unresolvable `$class` never becomes a `Resource` here. See
        // `visit_class_declaration`'s `is_map_value` parameter
        // (accordproject/concerto-rust#194) for how the validator turns
        // this same unresolvable `$class`, reached again later through
        // `ResourceValidator.checkMapType`, into the TS-faithful verdict.
        Ok(value.clone())
    }

    /// TS: JSONPopulator.visitField, for a field already unboxed by
    /// `getScalarField()` where its type is a scalar.
    fn visit_field(&mut self, field: &Field, json: &JsValue) -> Result<JsValue> {
        if field.is_array() {
            let JsValue::Array(items) = json else {
                return Err(validation(
                    "jsonpopulator-visitfield-notarray",
                    vec![
                        ("path", self.path_text().to_string()),
                        ("type", field.type_name().to_string()),
                    ],
                ));
            };
            let mut result = Vec::with_capacity(items.len());
            for (n, item) in items.iter().enumerate() {
                self.push_path(format_args!("[{n}]"));
                result.push(self.convert_item(field, item)?);
                self.pop_path();
            }
            Ok(JsValue::Array(result))
        } else {
            self.convert_item(field, json)
        }
    }

    /// TS: JSONPopulator.convertItem.
    fn convert_item(&mut self, field: &Field, json_item: &JsValue) -> Result<JsValue> {
        if field.is_primitive() || matches!(field.field_type, FieldType::Enum(_)) {
            return self.convert_to_object(field, json_item);
        }
        let class_value = get_property_ref(json_item, "$class")?;
        let type_name = if class_value.is_truthy() {
            // DV-015: see instance/serializer.rs from_json.
            let Some(type_name) = class_value.as_str() else {
                return Err(ContractError::pre_port(
                    ErrorKind::InvalidArgument,
                    format!(
                        "a $class that is not a string: {}",
                        class_value.to_js_string()
                    ),
                    None,
                )
                .into());
            };
            type_name
        } else {
            field.fully_qualified_type_name()
        };
        let declaration = model::get_type(self.mm, type_name)?;
        let sub_resource = if declaration.is_map_declaration() {
            None
        } else if let Some(id_field) = declaration.identifier_field_name()? {
            // `isIdentified()`, then `getIdentifierFieldName()`.
            Some(factory::new_resource_of(
                &declaration,
                get_property(json_item, id_field)?,
                false,
                self.env,
            )?)
        } else {
            Some(factory::new_resource_of(
                &declaration,
                JsValue::Undefined,
                false,
                self.env,
            )?)
        };
        self.accept_declaration(&declaration, json_item, sub_resource)
    }

    /// TS: JSONPopulator.convertToObject. The primitive-type switch itself
    /// has no dependency on `self.mm`/`self.env` (only on the field's type
    /// name, the options and the current path), so it is [`convert_primitive`],
    /// a free function the concerto-wasm binding (P4-10, jsonpopulator.ts)
    /// calls directly per field, without needing a live `Populator`.
    fn convert_to_object(&mut self, field: &Field, json: &JsValue) -> Result<JsValue> {
        convert_primitive(field.type_name(), json, self.options, self.path_text())
    }

    /// TS: JSONPopulator.visitRelationshipDeclaration.
    fn visit_relationship_declaration(
        &mut self,
        relationship: &Field,
        json: &JsValue,
    ) -> Result<JsValue> {
        let type_fqn = relationship.fully_qualified_type_name();
        let mut default_namespace = model_util::get_namespace(Some(type_fqn))?.to_string();
        if default_namespace.is_empty() {
            default_namespace =
                model_util::get_namespace(Some(relationship.owner_fqn))?.to_string();
        }
        let default_type = model_util::short_name(type_fqn).to_string();

        if relationship.is_array() {
            let JsValue::Array(items) = json else {
                return Err(validation(
                    "jsonpopulator-visitfield-notarray",
                    vec![
                        ("path", self.path_text().to_string()),
                        ("type", relationship.type_name().to_string()),
                    ],
                ));
            };
            let mut result = Vec::with_capacity(items.len());
            for item in items {
                if let JsValue::String(uri) = item {
                    result.push(JsValue::Instance(Box::new(factory::relationship_from_uri(
                        self.mm,
                        uri,
                        Some(&default_namespace),
                        Some(&default_type),
                    )?)));
                } else {
                    result.push(self.relationship_resource(relationship, json, item)?);
                }
            }
            Ok(JsValue::Array(result))
        } else {
            match json {
                JsValue::String(uri) => {
                    Ok(JsValue::Instance(Box::new(factory::relationship_from_uri(
                        self.mm,
                        uri,
                        Some(&default_namespace),
                        Some(&default_type),
                    )?)))
                }
                JsValue::Object(_)
                | JsValue::Array(_)
                | JsValue::Map(_)
                | JsValue::DateTime(_)
                | JsValue::Instance(_) => self.relationship_resource(relationship, json, json),
                _ => Err(plain_error(
                    "jsonpopulator-visitrelationshipdeclaration-notstringorobject",
                    vec![
                        ("value", json.to_js_string()),
                        ("relationship", relationship.relationship_to_string()),
                    ],
                )),
            }
        }
    }

    /// The object branch of `visitRelationshipDeclaration`, for the whole
    /// value `json` (which the "not a string" message prints) and the item
    /// being populated.
    fn relationship_resource(
        &mut self,
        relationship: &Field,
        json: &JsValue,
        item: &JsValue,
    ) -> Result<JsValue> {
        if !self.options.accept_resources_for_relationships {
            return Err(plain_error(
                "jsonpopulator-visitrelationshipdeclaration-notastring",
                vec![
                    ("value", json.to_js_string()),
                    ("relationship", relationship.relationship_to_string()),
                ],
            ));
        }
        let class_name = get_property(item, "$class")?;
        if !class_name.is_truthy() {
            return Err(plain_error(
                "jsonpopulator-visitrelationshipdeclaration-noclass",
                vec![
                    ("value", item.to_js_string()),
                    ("relationship", relationship.relationship_to_string()),
                ],
            ));
        }
        // DV-015: see instance/serializer.rs from_json.
        let Some(class_name) = class_name.as_str() else {
            return Err(ContractError::pre_port(
                ErrorKind::InvalidArgument,
                format!(
                    "a $class that is not a string: {}",
                    class_name.to_js_string()
                ),
                None,
            )
            .into());
        };
        let class_declaration = model::get_type(self.mm, class_name)?;
        let id = match class_declaration.identifier_field_name()? {
            Some(field) => get_property(item, field)?,
            None => get_property(item, "null")?,
        };
        let sub_resource = factory::new_resource_of(&class_declaration, id, false, self.env)?;
        self.accept_declaration(&class_declaration, item, Some(sub_resource))
    }
}

/// `map.set(key, value)`: a key seen before keeps its place.
fn map_set(entries: &mut Vec<(JsValue, JsValue)>, key: JsValue, value: JsValue) {
    match entries.iter_mut().find(|(k, _)| *k == key) {
        Some(entry) => entry.1 = value,
        None => entries.push((key, value)),
    }
}

/// What `utcOffset(this.utcOffset)` receives: a string as it is, anything
/// else through `Math.abs`'s `ToNumber`.
fn utc_offset_input(value: &JsValue) -> UtcOffset {
    match value {
        JsValue::String(s) => UtcOffset::String(s.clone()),
        JsValue::Number(n) => UtcOffset::Number(*n),
        JsValue::Bool(b) => UtcOffset::Number(f64::from(u8::from(*b))),
        JsValue::Null => UtcOffset::Number(0.0),
        _ => UtcOffset::Number(f64::NAN),
    }
}

/// The populator's options from the serializer's merged options. `pub`
/// (not `pub(crate)`) so the concerto-wasm binding (P4-10) can build a
/// `PopulatorOptions` for [`convert_primitive`] from the options object the
/// TS visitor shell already has.
pub fn populator_options(options: &IndexMap<String, JsValue>) -> PopulatorOptions {
    let get = |key: &str| options.get(key).cloned().unwrap_or(JsValue::Undefined);
    let utc_offset = get("utcOffset");
    PopulatorOptions {
        accept_resources_for_relationships: get("acceptResourcesForRelationships")
            == JsValue::Bool(true),
        utc_offset: if utc_offset.is_truthy() {
            utc_offset
        } else {
            JsValue::Number(0.0)
        },
        strict_qualified_date_times: get("strictQualifiedDateTimes") == JsValue::Bool(true),
        deserialize: DeserializeOptions::from_serializer_options(options),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ResourceValidator.checkItem`'s switch has no `default:` arm and
    /// starts from `invalid = false`, so a type name it does not list is
    /// valid (P4-10 review).
    #[test]
    fn primitive_field_valid_matches_the_ts_switch() {
        assert!(primitive_field_valid(
            "String",
            &JsValue::String("x".into())
        ));
        assert!(!primitive_field_valid("String", &JsValue::Number(1.0)));
        assert!(primitive_field_valid("Double", &JsValue::Number(1.5)));
        assert!(!primitive_field_valid("Double", &JsValue::Number(f64::NAN)));
        assert!(!primitive_field_valid(
            "Integer",
            &JsValue::Number(f64::INFINITY)
        ));
        assert!(primitive_field_valid("Boolean", &JsValue::Bool(false)));
        assert!(!primitive_field_valid(
            "DateTime",
            &JsValue::String("x".into())
        ));
        assert!(primitive_field_valid("Unknown", &JsValue::Number(1.0)));
        assert!(primitive_field_valid("Unknown", &JsValue::Null));
    }

    /// DV-009 / accordproject/concerto-rust#169 (P5-05 fuzz cluster T1c):
    /// non-strict `Serializer.fromJSON` accepts a `DateTime` string with an
    /// embedded NUL, as `dayjs.rs`'s `date_parse` now truncates there like
    /// V8 does; `strictQualifiedDateTimes` still rejects it, because
    /// `strict_qualified_date_time`'s anchored regex never matches a NUL —
    /// the fix does not widen what the strict path accepts.
    #[test]
    fn datetime_with_embedded_nul() {
        let s = "1970-01-01T00:00:00.000+00:00\u{0}";
        let non_strict = PopulatorOptions {
            accept_resources_for_relationships: false,
            utc_offset: JsValue::Number(0.0),
            strict_qualified_date_times: false,
            deserialize: DeserializeOptions::default(),
        };
        let result = convert_primitive("DateTime", &JsValue::String(s.into()), &non_strict, "$.t");
        assert!(result.is_ok(), "non-strict should accept: {result:?}");

        let strict = PopulatorOptions {
            strict_qualified_date_times: true,
            ..non_strict
        };
        let result = convert_primitive("DateTime", &JsValue::String(s.into()), &strict, "$.t");
        assert!(result.is_err(), "strict should still reject: {result:?}");
    }
}
