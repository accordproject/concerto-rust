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

use super::factory::{self, InstanceEnv};
use crate::deserialize::DeserializeOptions;
use crate::value::{Instance, JsObject, JsValue};
use concerto_core::error::{ContractError, ErrorKind, Result};
use concerto_core::instance::dayjs::{Dayjs, UtcOffset};
use concerto_core::instance::from_json::{
    required_null_error, strict_qualified_date_time, unknown_keys_error,
};
use concerto_core::instance::model::{self, Field, FieldType, RelationshipSlot, TypeRef};
use concerto_core::instance::plan;
use concerto_core::introspect::Declaration;
use concerto_core::model_manager::ModelManager;
use concerto_core::{Error, model_util};

/// The `JSONPopulator` constructor's options.
#[derive(Debug, Clone, PartialEq)]
pub struct PopulatorOptions {
    /// `acceptResourcesForRelationships`.
    pub accept_resources_for_relationships: bool,
    /// `utcOffset || 0`: the offset `DateTime` values get unless
    /// `strictQualifiedDateTimes` is `true`.
    pub utc_offset: JsValue,
    /// `strictQualifiedDateTimes === true`. Since P5-24 (BC-07, R1) every
    /// `DateTime` string must have the strict format either way; the flag
    /// only decides whether `utc_offset` is applied.
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
                    // P5-24 (BC-07, R1): only the strict format, whatever
                    // `strictQualifiedDateTimes` says; the flag now decides
                    // only whether `utcOffset` applies, as it did before.
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
                        parsed.utc_offset_set(&utc_offset_input(&options.utc_offset))
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
            // P5-51 (BC-10, R1; DV-012): an integral, finite number.
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
    Ok(get_assignable_entries(resource_data, declaration)?
        .into_iter()
        .map(|(property, _)| property)
        .collect())
}

/// [`object_keys_ref`] of a plain object, with each key's value (P5-16):
/// the same keys in the same order, read in one pass over the map rather
/// than one lookup per key.
fn object_entries_ref(map: &crate::value::JsObject) -> Vec<(Cow<'_, str>, &JsValue)> {
    // Integer-like keys come first, in ascending order.
    let mut indices: Vec<(u32, &String, &JsValue)> = map
        .iter()
        .filter_map(|(k, v)| {
            k.parse::<u32>()
                .ok()
                .filter(|i| *i != u32::MAX && k == &i.to_string())
                .map(|i| (i, k, v))
        })
        .collect();
    if indices.is_empty() {
        return map
            .iter()
            .map(|(k, v)| (Cow::Borrowed(k.as_str()), v))
            .collect();
    }
    indices.sort_by_key(|(i, _, _)| *i);
    let mut entries: Vec<(Cow<'_, str>, &JsValue)> = indices
        .into_iter()
        .map(|(_, k, v)| (Cow::Borrowed(k.as_str()), v))
        .collect();
    let leading = entries.len();
    let rest: Vec<_> = map
        .iter()
        .filter(|(k, _)| {
            !entries[..leading]
                .iter()
                .any(|(seen, _)| seen == k.as_str())
        })
        .map(|(k, v)| (Cow::Borrowed(k.as_str()), v))
        .collect();
    entries.extend(rest);
    entries
}

/// [`get_assignable_properties`], with each property's value
/// (`resourceData[property]`), which the caller reads again in TS: the
/// same value, since nothing changes the document in between (P5-16).
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
    // Each property with whether the declaration has it (P5-16: looked up
    // once by the caller, which reuses the lookup).
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

    /// `parameters.path.push('.' + property)`: [`Self::push_path`] without
    /// the formatting machinery, for the per-property push (P5-16).
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
        let options = self.options.deserialize;
        if options.reject_unknown_keys {
            self.reject_unknown_keys(json, class_declaration)?;
        }
        // `classDeclaration.getProperties()`, read once for
        // `validateProperties` and each `getProperty` below (the same
        // answer every time: the model does not change mid-walk).
        let class_properties = class_declaration.properties("classDeclaration.getProperties")?;
        // P5-80 (#424) prototype: the lookups and field types from the
        // plan, when there is one.
        if let Some(class_plan) = plan::class_plan(self.mm, class_declaration.id) {
            return self.visit_class_planned(
                class_declaration,
                json,
                resource,
                entries,
                &class_plan,
            );
        }
        // `validateProperties`, then each `getProperty` below: one lookup
        // per property serves both (P5-16).
        let declared: Vec<_> = entries
            .iter()
            .map(|(p, _)| class_properties.find(p))
            .collect();
        validate_properties(
            entries
                .iter()
                .zip(&declared)
                .map(|((p, _), found)| (&**p, found.is_some())),
            class_declaration,
        )?;
        if options.reject_required_null {
            self.reject_required_null(json, class_declaration)?;
        }
        for ((property, value), found) in entries.iter().zip(declared) {
            if **value != JsValue::Null {
                self.push_path_property(property);
                let (owner_fqn, class_property) =
                    found.expect("validateProperties found every property");
                let field = model::field(self.mm, owner_fqn, class_property)?;
                let populated = self.visit_property(&field, value)?;
                resource.set(property, populated);
                self.pop_path();
            }
        }
        // P5-24 (BC-45, R1): a non-strict `DateTime` default the document
        // did not replace is applied, so it throws.
        factory::check_populated_date_time_defaults(class_declaration, &resource)?;
        Ok(resource)
    }

    /// P5-80 (#424) prototype: the rest of [`Self::visit_class_declaration`]
    /// over the declaration's plan.
    fn visit_class_planned(
        &mut self,
        class_declaration: &TypeRef,
        json: &JsValue,
        mut resource: Instance,
        entries: Vec<(Cow<'_, str>, Cow<'_, JsValue>)>,
        class_plan: &plan::ClassPlan,
    ) -> Result<Instance> {
        let declared: Vec<Option<usize>> =
            entries.iter().map(|(p, _)| class_plan.find(p)).collect();
        validate_properties(
            entries
                .iter()
                .zip(&declared)
                .map(|((p, _), found)| (&**p, found.is_some())),
            class_declaration,
        )?;
        if self.options.deserialize.reject_required_null {
            self.reject_required_null(json, class_declaration)?;
        }
        for ((property, value), found) in entries.iter().zip(declared) {
            if **value != JsValue::Null {
                self.push_path_property(property);
                let index = found.expect("validateProperties found every property");
                let field = match class_plan.field(self.mm, index) {
                    Some(field) => field,
                    None => {
                        let (owner_fqn, class_property) = class_plan.property(self.mm, index);
                        model::field(self.mm, owner_fqn, class_property)?
                    }
                };
                let populated = self.visit_property(&field, value)?;
                resource.set(property, populated);
                self.pop_path();
            }
        }
        factory::check_populated_date_time_defaults(class_declaration, &resource)?;
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
        // P5-58 (BC-05, R1; DV-007): a relationship-typed value is read as
        // a relationship property is, not as an embedded concept. Its
        // target type is resolved at the first value, as TS resolves it.
        let is_relationship = model::is_relationship_map(map_declaration);
        let mut relationship: Option<(String, String, String)> = None;
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
            if is_relationship {
                if relationship.is_none() {
                    let target = model::map_relationship_target(map_declaration)?
                        .expect("is_relationship_map was checked");
                    let slot = model::map_relationship_slot(map_declaration, &target);
                    let (default_namespace, default_type) = relationship_defaults(&slot)?;
                    relationship = Some((target, default_namespace, default_type));
                }
                let (target, default_namespace, default_type) =
                    relationship.as_ref().expect("just set");
                let slot = model::map_relationship_slot(map_declaration, target);
                value =
                    self.convert_relationship(&slot, default_namespace, default_type, &value)?;
            } else if !model_util::is_primitive_type(&value_type) {
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
    /// A relationship-typed map value goes through here too (P5-58, BC-05).
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
pub fn populator_options(options: &JsObject) -> PopulatorOptions {
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

    /// P5-16: `object_entries_ref` gives `object_keys_ref`'s keys, in its
    /// order (integer-like keys first, ascending), each with its value.
    #[test]
    fn object_entries_ref_matches_object_keys_ref() {
        for keys in [
            vec!["b", "a", "c"],
            vec!["b", "10", "a", "2", "01", "4294967295", "0"],
            vec![],
        ] {
            let mut map = crate::value::JsObject::default();
            for (i, key) in keys.iter().enumerate() {
                map.insert((*key).to_string(), JsValue::Number(i as f64));
            }
            let entries = object_entries_ref(&map);
            let object = JsValue::Object(map.clone());
            let expected = object_keys_ref(&object).unwrap_or_default();
            let got: Vec<&str> = entries.iter().map(|(k, _)| &**k).collect();
            let want: Vec<&str> = expected.iter().map(|k| &**k).collect();
            assert_eq!(got, want);
            for (key, value) in &entries {
                assert_eq!(Some(*value), map.get(&**key));
            }
        }
    }

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

    /// P5-24 (BC-07, R1): `strictQualifiedDateTimes: false` no longer
    /// opens a lenient path. An embedded NUL (DV-009,
    /// accordproject/concerto-rust#169), a date-only string or an
    /// impossible date is rejected with or without the flag, with the same
    /// `ValidationException` as strict mode; a strict string is accepted
    /// either way, with `utcOffset` applied only when the flag is not set.
    #[test]
    fn datetime_strings_are_strict_either_way() {
        let non_strict = PopulatorOptions {
            accept_resources_for_relationships: false,
            utc_offset: JsValue::Number(60.0),
            strict_qualified_date_times: false,
            deserialize: DeserializeOptions::default(),
        };
        let strict = PopulatorOptions {
            strict_qualified_date_times: true,
            ..non_strict.clone()
        };
        for s in [
            "1970-01-01T00:00:00.000+00:00\u{0}",
            "2020-01-01",
            "2016-10-20T05:34:03.519",
            "2024-02-30T00:00:00Z",
            "2024-01-02T24:00:00Z",
        ] {
            for options in [&non_strict, &strict] {
                let result =
                    convert_primitive("DateTime", &JsValue::String(s.into()), options, "$.t");
                let err = result.expect_err(s);
                assert_eq!(err.kind().ts_class(), "ValidationException", "{s:?}: {err}");
            }
        }
        let s = JsValue::String("2021-01-01T00:00:00Z".into());
        let Ok(JsValue::DateTime(d)) = convert_primitive("DateTime", &s, &non_strict, "$.t") else {
            panic!("non-strict should accept a strict string");
        };
        assert_eq!(d.utc_offset(), 60.0);
        let Ok(JsValue::DateTime(d)) = convert_primitive("DateTime", &s, &strict, "$.t") else {
            panic!("strict should accept a strict string");
        };
        assert!(d.is_utc());
    }

    /// P5-51 (BC-10, R1; DV-012): `±Infinity` is not an Integer or a
    /// Long, whatever the options; the same `ValidationException` as a
    /// fractional number. `NaN` was already rejected.
    #[test]
    fn non_finite_integers_and_longs_are_rejected() {
        let options = PopulatorOptions {
            accept_resources_for_relationships: false,
            utc_offset: JsValue::Number(0.0),
            strict_qualified_date_times: false,
            deserialize: DeserializeOptions::default(),
        };
        for type_name in ["Integer", "Long"] {
            for n in [f64::INFINITY, f64::NEG_INFINITY, f64::NAN, 1.5] {
                let err = convert_primitive(type_name, &JsValue::Number(n), &options, "$.i")
                    .expect_err(&format!("{type_name} {n}"));
                assert_eq!(err.kind().ts_class(), "ValidationException", "{n}: {err}");
                assert_eq!(
                    err.to_string(),
                    format!("Expected value at path `$.i` to be of type `{type_name}`")
                );
            }
            for n in [0.0, -3.0, 9_007_199_254_740_993.0, 1e300] {
                assert_eq!(
                    convert_primitive(type_name, &JsValue::Number(n), &options, "$.i").ok(),
                    Some(JsValue::Number(n)),
                    "{type_name} {n}"
                );
            }
        }
        // A Double keeps them: BC-10 is about Integer and Long only.
        assert_eq!(
            convert_primitive("Double", &JsValue::Number(f64::INFINITY), &options, "$.d").ok(),
            Some(JsValue::Number(f64::INFINITY))
        );
    }
}
