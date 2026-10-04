//! `JSONPopulator` (src/serializer/jsonpopulator.ts): populates an
//! [`Instance`] from a JSON object graph, with its per-field checks,
//! coercions and messages.
//!
//! PORTING.md section 5 row 6 (option B): the TS visitor shell stays (the
//! white-box tests spy on `visitX`), and every check and coercion it runs
//! is this module's. Each function here is one TS method, in the same
//! order, so that the first error thrown is the same one (2.4). The TS
//! `jsonStack`/`resourceStack` pushes and pops become arguments and return
//! values; `parameters.path` is `Populator::path`.

use std::borrow::Cow;
use std::fmt::Write as _;

use super::factory::{self, InstanceEnv};
use crate::value::{Instance, JsObject, JsValue};
use concerto_core::error::{ContractError, ErrorKind, Result};
use concerto_core::instance::dayjs::{Dayjs, UtcOffset};
use concerto_core::instance::from_json::{
    FromJsonOptions, js_key_order, required_null_error, strict_qualified_date_time,
    unknown_keys_error,
};
use concerto_core::instance::model::{self, Field, FieldType, RelationshipSlot, TypeRef};
use concerto_core::instance::plan::{self, ClassPlan};
use concerto_core::introspect::Declaration;
use concerto_core::model_manager::ModelManager;
use concerto_core::{Error, model_util};

/// The visitor's state: its options and `parameters`.
pub(crate) struct Populator<'a> {
    pub mm: &'a ModelManager,
    pub env: &'a mut dyn InstanceEnv,
    pub options: &'a FromJsonOptions,
    /// `parameters.path`, a `TypedStack` that starts as `['$']`, kept
    /// joined: what `path.stack.join('')` reads, with
    /// [`Self::path_marks`] recording where each pushed segment
    /// starts.
    path: String,
    path_marks: Vec<usize>,
}

fn validation(code: &'static str, params: Vec<(&'static str, String)>) -> Error {
    ContractError::new(ErrorKind::Validation, code, params).into()
}

fn plain_error(code: &'static str, params: Vec<(&'static str, String)>) -> Error {
    ContractError::new(ErrorKind::InvalidArgument, code, params).into()
}

/// TS: `JSONPopulator.convertToObject`'s primitive-type switch alone: the
/// part that needs no declaration lookup, so the TS visitor shell can call
/// it per field directly (via the concerto-wasm binding), keeping its own
/// recursion and `jsonStack`/`resourceStack` handling -- and so the tests
/// that spy on `visitX` still see the same calls, in the same order, that
/// they always did.
pub fn convert_primitive(
    type_name: &str,
    json: &JsValue,
    options: &FromJsonOptions,
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
                    // BC-07: only the strict format, whatever
                    // `strictQualifiedDateTimes` says; the flag decides only
                    // whether `utcOffset` applies.
                    if !strict_qualified_date_time(s) {
                        return Err(validation(
                            "jsonpopulator-converttoobject-datetimeformat",
                            vec![("path", path.to_string()), ("type", type_name.to_string())],
                        ));
                    }
                    let parsed = Dayjs::utc_parse(s);
                    if options.strict_qualified_date_times {
                        parsed
                    } else {
                        parsed.utc_offset_set(&options.utc_offset)
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
            // BC-10, DV-012: an integral, finite number.
            // `Math.trunc(num) !== num` alone passed `±Infinity`.
            JsValue::Number(n) if n.is_finite() && n.trunc() == *n => json.clone(),
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

/// TS `ResourceValidator.checkItem`'s primitive `switch(field.getType())`:
/// whether the coerced `value` is valid for primitive `type_name`. The
/// `undefined`/`symbol` check before the switch stays in TS; each branch
/// tests the value alone, so the TS shell may call this per field. A type
/// name with no `case` is valid.
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
/// value rather than cloning it.
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

/// [`object_keys`], borrowing each key that the value itself holds.
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
        JsValue::Object(map) => js_key_order(map.keys(), |k| k.as_str())
            .into_iter()
            .map(|k| Cow::Borrowed(k.as_str()))
            .collect(),
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
    Ok(get_assignable_entries(resource_data, declaration)?
        .into_iter()
        .map(|(property, _)| property)
        .collect())
}

/// [`object_keys_ref`] of a plain object, with each key's value: the
/// same keys in the same order, read in one pass over the map rather
/// than one lookup per key.
fn object_entries_ref(map: &crate::value::JsObject) -> Vec<(Cow<'_, str>, &JsValue)> {
    js_key_order(map.iter(), |(k, _)| k.as_str())
        .into_iter()
        .map(|(k, v)| (Cow::Borrowed(k.as_str()), v))
        .collect()
}

/// [`get_assignable_properties`], with each property's value
/// (`resourceData[property]`), which the caller reads again in TS: the
/// same value, since nothing changes the document in between.
fn get_assignable_entries<'j>(
    resource_data: &'j JsValue,
    declaration: &TypeRef,
) -> Result<Vec<(Cow<'j, str>, Cow<'j, JsValue>)>> {
    let entries: Vec<(Cow<'j, str>, Option<&'j JsValue>)> = match resource_data {
        JsValue::Object(map) => object_entries_ref(map)
            .into_iter()
            .map(|(k, v)| (k, Some(v)))
            .collect(),
        _ => object_keys_ref(resource_data)?
            .into_iter()
            .map(|k| (k, None))
            .collect(),
    };
    let properties: Vec<&Cow<'j, str>> = entries.iter().map(|(k, _)| k).collect();
    let private: Vec<&str> = properties
        .iter()
        .filter(|p| model_util::is_private_system_property(p))
        .map(|p| &***p)
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
    if properties.iter().any(|p| *p == "$timestamp")
        && !(declaration.is_transaction() || declaration.is_event())
    {
        return Err(validation(
            "jsonpopulator-getassignableproperties-timestamp",
            vec![("fqn", declaration.fqn().to_string())],
        ));
    }
    let mut assignable = Vec::with_capacity(entries.len());
    for (property, value) in entries {
        if model_util::is_system_property(&property) {
            continue;
        }
        let value = match value {
            Some(value) => Cow::Borrowed(value),
            None => get_property_ref(resource_data, &property)?,
        };
        if value.is_nullish() {
            continue;
        }
        assignable.push((property, value));
    }
    Ok(assignable)
}

/// TS `validateProperties(properties, classDeclaration)`, against the
/// declaration's `getProperties()`.
fn validate_properties<'p>(
    properties: impl Iterator<Item = (&'p str, bool)>,
    class_declaration: &TypeRef,
) -> Result<()> {
    // Each property with whether the declaration has it (looked up once
    // by the caller, which reuses the lookup).
    let invalid: Vec<&str> = properties
        .filter(|(_, declared)| !declared)
        .map(|(p, _)| p)
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
        options: &'a FromJsonOptions,
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

    /// `parameters.path.push('.' + property)`: [`Self::push_path`] without
    /// the formatting machinery, for the per-property push.
    fn push_path_property(&mut self, property: &str) {
        self.path_marks.push(self.path.len());
        self.path.push('.');
        self.path.push_str(property);
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
        // `visit`'s `Unrecognised` fallthrough: a scalar declaration (BC-08).
        Err(model::unrecognised(declaration.fqn()))
    }

    /// TS: JSONPopulator.visitClassDeclaration.
    pub fn visit_class_declaration(
        &mut self,
        class_declaration: &TypeRef,
        json: &JsValue,
        mut resource: Instance,
    ) -> Result<Instance> {
        let entries = get_assignable_entries(json, class_declaration)?;
        let options = self.options;
        // `classDeclaration.getProperties()`, and each `getProperty` below
        // and in the `reject_*` options, from the validation plan: the same
        // answer every time, and the chain's error first, as
        // `getProperties()` raises it.
        let class_plan = plan::class_plan(self.mm, class_declaration.id)?;
        if options.reject_unknown_keys {
            self.reject_unknown_keys(json, class_declaration, &class_plan)?;
        }
        // `validateProperties`, then each `getProperty` below: one lookup
        // per property serves both.
        let declared: Vec<Option<usize>> =
            entries.iter().map(|(p, _)| class_plan.find(p)).collect();
        validate_properties(
            entries
                .iter()
                .zip(&declared)
                .map(|((p, _), found)| (&**p, found.is_some())),
            class_declaration,
        )?;
        if options.reject_required_null {
            self.reject_required_null(json, &class_plan)?;
        }
        for ((property, value), found) in entries.iter().zip(declared) {
            if **value != JsValue::Null {
                self.push_path_property(property);
                let index = found.expect("validateProperties found every property");
                let field = class_plan.field(self.mm, index)?;
                let populated = self.visit_property(&field, value)?;
                resource.set(property, populated);
                self.pop_path();
            }
        }
        // BC-45: a non-strict `DateTime` default the document did not
        // replace is applied, so it throws.
        factory::check_populated_date_time_defaults(class_declaration, &resource)?;
        Ok(resource)
    }

    /// `rejectUnknownKeys` (accordproject/concerto#1273): every key that is
    /// not a system property and that the declaration does not declare,
    /// whatever its value (`null` included), in one error with one
    /// `UNKNOWN_PROPERTY` detail per key.
    fn reject_unknown_keys(
        &self,
        json: &JsValue,
        class_declaration: &TypeRef,
        class_plan: &ClassPlan,
    ) -> Result<()> {
        let unknown: Vec<String> = object_keys_ref(json)?
            .into_iter()
            .filter(|p| !model_util::is_system_property(p) && !class_plan.contains(p))
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
    fn reject_required_null(&self, json: &JsValue, class_plan: &ClassPlan) -> Result<()> {
        for key in object_keys_ref(json)? {
            if model_util::is_system_property(&key)
                || *get_property_ref(json, &key)? != JsValue::Null
            {
                continue;
            }
            let Some(index) = class_plan.find(&key) else {
                continue;
            };
            let (_, property) = class_plan.property(self.mm, index);
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
            // An enum value is not a `Field`: `visit` falls through to its
            // `Unrecognised` error (BC-08).
            FieldType::EnumValue => Err(model::unrecognised_field(field)),
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
        // BC-05, DV-007: a relationship-typed value is read as a
        // relationship property is, not as an embedded concept. Its target
        // type, and its slot, are resolved once; an error resolving them is
        // raised at the first value, as TS resolves it there.
        let relationship = model::is_relationship_map(map_declaration).then(|| {
            let target = model::map_relationship_target(map_declaration)?
                .expect("is_relationship_map was checked");
            let (default_namespace, default_type) =
                relationship_defaults(&model::map_relationship_slot(map_declaration, &target))?;
            Ok::<_, Error>((target, default_namespace, default_type))
        });
        let slot = match &relationship {
            Some(Ok((target, _, _))) => Some(model::map_relationship_slot(map_declaration, target)),
            _ => None,
        };
        // `processMapType`'s declaration for a key or value with no `$class`
        // of its own: the same for every entry, so resolved once.
        let mut key_declaration = None;
        let mut value_declaration = None;
        let mut result: Vec<(JsValue, JsValue)> = Vec::new();
        // `new Map(Object.entries(jsonObj))`. The keys are an object's, so
        // each is new, and each `map.set` appends: a key's
        // `processMapType` hands a string back as it is, or a new
        // `Resource`, which a JS `Map` keys by identity.
        for key in object_keys(json)? {
            let value = get_property(json, &key)?;
            let key = JsValue::String(key);
            if key.as_str() == Some("$class") {
                result.push((key, value));
                continue;
            }
            let key = if model_util::is_primitive_type(&key_type) {
                key
            } else {
                self.process_map_type(map_declaration, &key, &key_type, &mut key_declaration)?
            };
            let value = match &relationship {
                Some(resolved) => {
                    let (_, default_namespace, default_type) =
                        resolved.as_ref().map_err(Clone::clone)?;
                    let slot = slot.as_ref().expect("resolved with the target");
                    self.convert_relationship(slot, default_namespace, default_type, &value)?
                }
                None if model_util::is_primitive_type(&value_type) => value,
                None => self.process_map_type(
                    map_declaration,
                    &value,
                    &value_type,
                    &mut value_declaration,
                )?,
            };
            result.push((key, value));
        }
        Ok(JsValue::Map(result))
    }

    /// TS: JSONPopulator.processMapType. `declaration` holds the
    /// declaration `type_name` names in the map's model file, resolved at
    /// the first entry that needs it.
    fn process_map_type(
        &mut self,
        map_declaration: &TypeRef,
        value: &JsValue,
        type_name: &str,
        declaration: &mut Option<Option<TypeRef<'a>>>,
    ) -> Result<JsValue> {
        let namespace = map_declaration.namespace();
        let mm = self.mm;
        // `try { ... } catch (err) { decl = undefined; }`
        let class_name = match value {
            JsValue::Object(_) | JsValue::Instance(_) | JsValue::Array(_) | JsValue::Map(_) => {
                get_property(value, "$class")
                    .ok()
                    .filter(JsValue::is_truthy)
            }
            _ => None,
        };
        let found = match class_name {
            Some(JsValue::String(s)) => model::get_type(mm, &s).ok(),
            Some(_) => None,
            None => *declaration.get_or_insert_with(|| {
                mm.model_file_fully_qualified_type_name(namespace, type_name)
                    .and_then(|name| model::get_type(mm, &name).ok())
            }),
        };
        if let Some(declaration) = found
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
        // `visit_class_declaration`'s `is_map_value` parameter for how
        // the validator turns this same unresolvable `$class`, reached
        // again later through `ResourceValidator.checkMapType`, into the
        // TS-faithful verdict.
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

    /// TS: JSONPopulator.convertToObject. The primitive-type switch itself has
    /// no dependency on `self.mm`/`self.env` (only on the field's type name,
    /// the options and the current path), so it is [`convert_primitive`], a
    /// free function the concerto-wasm binding (jsonpopulator.ts) calls
    /// directly per field, without needing a live `Populator`.
    fn convert_to_object(&mut self, field: &Field, json: &JsValue) -> Result<JsValue> {
        convert_primitive(field.type_name(), json, self.options, self.path_text())
    }

    /// TS: JSONPopulator.visitRelationshipDeclaration.
    fn visit_relationship_declaration(
        &mut self,
        relationship: &Field,
        json: &JsValue,
    ) -> Result<JsValue> {
        let slot = relationship
            .relationship_slot()
            .expect("visit_relationship_declaration is only reached for a relationship");
        let (default_namespace, default_type) = relationship_defaults(&slot)?;

        if slot.is_array {
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
                    result.push(self.relationship_resource(&slot, json, item)?);
                }
            }
            Ok(JsValue::Array(result))
        } else {
            self.convert_relationship(&slot, &default_namespace, &default_type, json)
        }
    }

    /// One relationship value, `visitRelationshipDeclaration`'s non-array
    /// branch: a URI string becomes a `Relationship`, and an object an
    /// embedded resource when `acceptResourcesForRelationships` allows it.
    /// A relationship-typed map value goes through here too (BC-05).
    fn convert_relationship(
        &mut self,
        slot: &RelationshipSlot,
        default_namespace: &str,
        default_type: &str,
        json: &JsValue,
    ) -> Result<JsValue> {
        match json {
            JsValue::String(uri) => {
                Ok(JsValue::Instance(Box::new(factory::relationship_from_uri(
                    self.mm,
                    uri,
                    Some(default_namespace),
                    Some(default_type),
                )?)))
            }
            JsValue::Object(_)
            | JsValue::Array(_)
            | JsValue::Map(_)
            | JsValue::DateTime(_)
            | JsValue::Instance(_) => self.relationship_resource(slot, json, json),
            _ => Err(plain_error(
                "jsonpopulator-visitrelationshipdeclaration-notstringorobject",
                vec![
                    ("value", json.to_js_string()),
                    ("relationship", slot.relationship_to_string()),
                ],
            )),
        }
    }

    /// The object branch of `visitRelationshipDeclaration`, for the whole
    /// value `json` (which the "not a string" message prints) and the item
    /// being populated.
    fn relationship_resource(
        &mut self,
        slot: &RelationshipSlot,
        json: &JsValue,
        item: &JsValue,
    ) -> Result<JsValue> {
        if !self.options.accept_resources_for_relationships {
            return Err(plain_error(
                "jsonpopulator-visitrelationshipdeclaration-notastring",
                vec![
                    ("value", json.to_js_string()),
                    ("relationship", slot.relationship_to_string()),
                ],
            ));
        }
        let class_name = get_property(item, "$class")?;
        if !class_name.is_truthy() {
            return Err(plain_error(
                "jsonpopulator-visitrelationshipdeclaration-noclass",
                vec![
                    ("value", item.to_js_string()),
                    ("relationship", slot.relationship_to_string()),
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

/// `visitRelationshipDeclaration`'s `defaultNamespace` and `defaultType`:
/// the target type's namespace (else the owner's) and short name, which a
/// URI without them takes.
fn relationship_defaults(slot: &RelationshipSlot) -> Result<(String, String)> {
    let type_fqn = slot.target_fqn;
    let mut default_namespace = model_util::get_namespace(Some(type_fqn))?.to_string();
    if default_namespace.is_empty() {
        default_namespace = model_util::get_namespace(Some(slot.owner_fqn))?.to_string();
    }
    Ok((
        default_namespace,
        model_util::short_name(type_fqn).to_string(),
    ))
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

/// What `Serializer.fromJSON` reads from its merged options, as core's
/// [`FromJsonOptions`]. `pub` so concerto-wasm can read them for
/// [`convert_primitive`] from the TS shell's options object. The
/// validator's options are not read: a `ValidatedResource` uses its own.
pub fn from_json_options(options: &JsObject) -> FromJsonOptions {
    let get = |key: &str| options.get(key).unwrap_or(&JsValue::Undefined);
    let truthy = |key: &str| get(key).is_truthy();
    // `utcOffset || 0`, as `utcOffset(this.utcOffset)` reads it.
    let utc_offset = get("utcOffset");
    FromJsonOptions {
        validate: truthy("validate"),
        utc_offset: if utc_offset.is_truthy() {
            utc_offset_input(utc_offset)
        } else {
            UtcOffset::Number(0.0)
        },
        strict_qualified_date_times: *get("strictQualifiedDateTimes") == JsValue::Bool(true),
        accept_resources_for_relationships: *get("acceptResourcesForRelationships")
            == JsValue::Bool(true),
        reject_unknown_keys: truthy(crate::deserialize::REJECT_UNKNOWN_KEYS),
        reject_required_null: truthy(crate::deserialize::REJECT_REQUIRED_NULL),
        ..FromJsonOptions::default()
    }
}

#[cfg(test)]
mod tests;
