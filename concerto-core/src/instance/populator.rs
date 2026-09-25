//! `JSONPopulator` (src/serializer/jsonpopulator.ts): populates an
//! [`Instance`] from a JSON object graph, with its per-field checks,
//! coercions and messages (task P3-01b, accordproject/concerto-rust#124).
//!
//! PORTING.md section 5 row 6 (option B): the TS visitor shell stays (the
//! white-box tests spy on `visitX`), and every check and coercion it runs
//! is this module's. Each function here is one TS method, in the same
//! order, so that the first error thrown is the same one (2.4). The TS
//! `jsonStack`/`resourceStack` pushes and pops become arguments and return
//! values; `parameters.path` is [`Populator::path`].

use indexmap::IndexMap;

use super::dayjs::{Dayjs, UtcOffset};
use super::deserialize::DeserializeOptions;
use super::factory::{self, InstanceEnv};
use super::model::{self, Field, FieldType, TypeRef};
use super::value::{Instance, JsValue};
use crate::error::{ContractError, DetailCode, ErrorKind, Result, ValidationDetail};
use crate::introspect::Declaration;
use crate::model_manager::ModelManager;
use crate::{ConcertoError, model_util};

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
    /// `parameters.path`, a `TypedStack` that starts as `['$']`.
    pub path: Vec<String>,
}

fn validation(code: &'static str, params: Vec<(&'static str, String)>) -> ConcertoError {
    ContractError::new(ErrorKind::Validation, code, params).into()
}

fn plain_error(code: &'static str, params: Vec<(&'static str, String)>) -> ConcertoError {
    ContractError::new(ErrorKind::Error, code, params).into()
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
pub(crate) fn read_properties_error(value: &JsValue, property: &str) -> ConcertoError {
    ContractError::new(
        ErrorKind::JsTypeError,
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

/// `Object.keys(value)`: V8's `TypeError` for `undefined` and `null`.
pub(crate) fn object_keys(value: &JsValue) -> Result<Vec<String>> {
    Ok(match value {
        JsValue::Undefined | JsValue::Null => {
            return Err(ContractError::new(
                ErrorKind::JsTypeError,
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
            let mut keys: Vec<String> = indices.into_iter().map(|(_, k)| k.clone()).collect();
            keys.extend(
                map.keys()
                    .filter(|k| !keys.contains(k))
                    .cloned()
                    .collect::<Vec<_>>(),
            );
            keys
        }
        JsValue::String(s) => (0..s.encode_utf16().count())
            .map(|i| i.to_string())
            .collect(),
        JsValue::Array(items) => (0..items.len()).map(|i| i.to_string()).collect(),
        JsValue::Instance(instance) => {
            let mut keys = vec!["$modelManager".to_string(), "$classDeclaration".to_string()];
            for key in instance.props.keys() {
                keys.push(key.clone());
                if key == "$timestamp"
                    && instance.kind == super::value::InstanceKind::ValidatedResource
                {
                    keys.push("$validator".to_string());
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
fn get_assignable_properties(
    resource_data: &JsValue,
    declaration: &TypeRef,
) -> Result<Vec<String>> {
    let properties = object_keys(resource_data)?;
    let private: Vec<&str> = properties
        .iter()
        .filter(|p| model_util::is_private_system_property(p))
        .map(String::as_str)
        .collect();
    if !private.is_empty() {
        return Err(validation(
            "jsonpopulator-getassignableproperties-reservedproperties",
            vec![
                ("fqn", declaration.fqn()),
                ("properties", private.join(", ")),
            ],
        ));
    }
    if properties.iter().any(|p| p == "$timestamp")
        && !(declaration.is_transaction() || declaration.is_event())
    {
        return Err(validation(
            "jsonpopulator-getassignableproperties-timestamp",
            vec![("fqn", declaration.fqn())],
        ));
    }
    let mut assignable = Vec::new();
    for property in properties {
        if model_util::is_system_property(&property) {
            continue;
        }
        if get_property(resource_data, &property)?.is_nullish() {
            continue;
        }
        assignable.push(property);
    }
    Ok(assignable)
}

/// TS `validateProperties(properties, classDeclaration)`.
fn validate_properties(properties: &[String], class_declaration: &TypeRef) -> Result<()> {
    let expected: Vec<String> = class_declaration
        .properties("classDeclaration.getProperties")?
        .iter()
        .map(|(_, p)| crate::Named::name(p).to_string())
        .collect();
    let invalid: Vec<&str> = properties
        .iter()
        .filter(|p| !expected.contains(p))
        .map(String::as_str)
        .collect();
    if !invalid.is_empty() {
        return Err(validation(
            "jsonpopulator-validateproperties-unexpectedproperties",
            vec![
                ("fqn", class_declaration.fqn()),
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
            path: vec!["$".to_string()],
        }
    }

    fn path_text(&self) -> String {
        self.path.concat()
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
                ConcertoError::from(ContractError::pre_port(
                    ErrorKind::Error,
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
        validate_properties(&properties, class_declaration)?;
        if options.reject_required_null {
            self.reject_required_null(json, class_declaration)?;
        }
        for property in properties {
            let value = get_property(json, &property)?;
            if value != JsValue::Null {
                self.path.push(format!(".{property}"));
                let (owner_fqn, class_property) = class_declaration
                    .property(&property)?
                    .expect("validateProperties found every property");
                let field = model::field(self.mm, &owner_fqn, class_property)?;
                let populated = self.visit_property(&field, &value)?;
                resource.set(&property, populated);
                self.path.pop();
            }
        }
        Ok(resource)
    }

    /// `rejectUnknownKeys` (accordproject/concerto#1273): every key that is
    /// not a system property and that the declaration does not declare,
    /// whatever its value (`null` included), in one error with one
    /// `UNKNOWN_PROPERTY` detail per key.
    fn reject_unknown_keys(&self, json: &JsValue, class_declaration: &TypeRef) -> Result<()> {
        let expected: Vec<String> = class_declaration
            .properties("classDeclaration.getProperties")?
            .iter()
            .map(|(_, p)| crate::Named::name(p).to_string())
            .collect();
        let unknown: Vec<String> = object_keys(json)?
            .into_iter()
            .filter(|p| !model_util::is_system_property(p) && !expected.contains(p))
            .collect();
        if unknown.is_empty() {
            return Ok(());
        }
        let path = self.path_text();
        let mut error = ContractError::new(
            ErrorKind::Validation,
            "jsonpopulator-rejectunknownkeys-unknownproperties",
            vec![
                ("fqn", class_declaration.fqn()),
                ("properties", unknown.join(", ")),
            ],
        );
        error.details = unknown
            .iter()
            .map(|property| ValidationDetail {
                path: format!("{path}.{property}"),
                code: DetailCode::UnknownProperty,
                expected: None,
                actual: None,
            })
            .collect();
        Err(error.into())
    }

    /// `rejectRequiredNull` (accordproject/concerto#1273): the first
    /// declared, required property (in the document's key order) whose value
    /// is `null` fails at once with its path and declared type, and a
    /// `TYPE_VIOLATION` detail.
    fn reject_required_null(&self, json: &JsValue, class_declaration: &TypeRef) -> Result<()> {
        for key in object_keys(json)? {
            if model_util::is_system_property(&key) || get_property(json, &key)? != JsValue::Null {
                continue;
            }
            let Some((_, property)) = class_declaration.property(&key)? else {
                continue;
            };
            if property.is_optional() {
                continue;
            }
            let path = format!("{}.{key}", self.path_text());
            let mut type_name = crate::introspect::Typed::type_name(&property)
                .unwrap_or_default()
                .to_string();
            if property.is_array() {
                type_name.push_str("[]");
            }
            let mut error = ContractError::new(
                ErrorKind::Validation,
                "jsonpopulator-rejectrequirednull-requirednull",
                vec![("path", path.clone()), ("type", type_name.clone())],
            );
            error.details = vec![ValidationDetail {
                path,
                code: DetailCode::TypeViolation,
                expected: Some(type_name),
                actual: Some("null".to_string()),
            }];
            return Err(error.into());
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
                    .model_file_fully_qualified_type_name(&namespace, type_name)?,
            };
            model::get_type(self.mm, &name).ok()
        })();
        if let Some(declaration) = declaration
            && declaration.is_class_declaration()
        {
            // `newConcept(ns, name, decl.getIdentifierFieldName())`: the
            // field's name as the identifier. DV-011
            let id = match declaration.identifier_field_name()? {
                Some(name) => JsValue::String(name),
                None => JsValue::Null,
            };
            let sub_resource = factory::new_resource(
                self.mm,
                &declaration.namespace(),
                declaration.name(),
                id,
                false,
                self.env,
            )?;
            return self.accept_declaration(&declaration, value, Some(sub_resource));
        }
        Ok(value.clone())
    }

    /// TS: JSONPopulator.visitField, for a field already unboxed by
    /// `getScalarField()` where its type is a scalar.
    fn visit_field(&mut self, field: &Field, json: &JsValue) -> Result<JsValue> {
        if field.is_array() {
            let JsValue::Array(items) = json else {
                return Err(validation(
                    "jsonpopulator-visitfield-notarray",
                    vec![("path", self.path_text()), ("type", field.type_name())],
                ));
            };
            let mut result = Vec::with_capacity(items.len());
            for (n, item) in items.iter().enumerate() {
                self.path.push(format!("[{n}]"));
                result.push(self.convert_item(field, item)?);
                self.path.pop();
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
        let mut type_name = get_property(json_item, "$class")?;
        if !type_name.is_truthy() {
            type_name = JsValue::String(field.fully_qualified_type_name());
        }
        let Some(type_name) = type_name.as_str() else {
            return Err(ContractError::pre_port(
                ErrorKind::Error,
                format!(
                    "a $class that is not a string: {}",
                    type_name.to_js_string()
                ),
                None,
            )
            .into());
        };
        let declaration = model::get_type(self.mm, type_name)?;
        let sub_resource = if declaration.is_map_declaration() {
            None
        } else if declaration.is_identified()? {
            let id_field = declaration
                .identifier_field_name()?
                .expect("an identified declaration names its identifying field");
            Some(factory::new_resource(
                self.mm,
                &declaration.namespace(),
                declaration.name(),
                get_property(json_item, &id_field)?,
                false,
                self.env,
            )?)
        } else {
            Some(factory::new_resource(
                self.mm,
                &declaration.namespace(),
                declaration.name(),
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
        convert_primitive(&field.type_name(), json, self.options, &self.path_text())
    }

    /// TS: JSONPopulator.visitRelationshipDeclaration.
    fn visit_relationship_declaration(
        &mut self,
        relationship: &Field,
        json: &JsValue,
    ) -> Result<JsValue> {
        let type_fqn = relationship.fully_qualified_type_name();
        let mut default_namespace = model_util::get_namespace(Some(&type_fqn))?.to_string();
        if default_namespace.is_empty() {
            default_namespace =
                model_util::get_namespace(Some(&relationship.owner_fqn))?.to_string();
        }
        let default_type = model_util::get_short_name(&type_fqn).to_string();

        if relationship.is_array() {
            let JsValue::Array(items) = json else {
                return Err(validation(
                    "jsonpopulator-visitfield-notarray",
                    vec![
                        ("path", self.path_text()),
                        ("type", relationship.type_name()),
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
        let Some(class_name) = class_name.as_str() else {
            return Err(ContractError::pre_port(
                ErrorKind::Error,
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
            Some(field) => get_property(item, &field)?,
            None => get_property(item, "null")?,
        };
        let sub_resource = factory::new_resource(
            self.mm,
            &class_declaration.namespace(),
            class_declaration.name(),
            id,
            false,
            self.env,
        )?;
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

/// `json.match(/^((?:(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2}(?:\.\d+)?))(Z|[+-]\d{2}:\d{2}))$/)`.
fn strict_qualified_date_time(s: &str) -> bool {
    let re = regress::Regex::new(
        r"^((?:(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2}(?:\.\d+)?))(Z|[+-]\d{2}:\d{2}))$",
    )
    .expect("static pattern");
    re.find(s).is_some()
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
