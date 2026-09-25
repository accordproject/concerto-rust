//! `JSONGenerator` (src/serializer/jsongenerator.ts): converts an
//! [`Instance`] to a plain JSON object, with its checks and messages (task
//! P3-01b, accordproject/concerto-rust#124).
//!
//! As with [`super::populator`], the TS visitor shell stays (PORTING.md
//! section 5 row 6) and every check it runs is here, one function per TS
//! method, in the same order. `parameters.stack` pushes and pops become
//! arguments; `parameters.seenResources` and `dedupeResources` are
//! [`Generator`]'s sets.

use std::collections::HashSet;

use indexmap::IndexMap;

use super::dayjs::UtcOffset;
use super::model::{self, Field, FieldType, TypeRef};
use super::populator::read_properties_error;
use super::value::{Instance, InstanceKind, JsValue};
use crate::ConcertoError;
use crate::error::{ContractError, ErrorKind, Result};
use crate::introspect::Declaration;
use crate::model_manager::ModelManager;
use crate::model_util;

/// The `JSONGenerator` constructor's options.
#[derive(Debug, Clone, PartialEq)]
pub struct GeneratorOptions {
    pub convert_resources_to_relationships: bool,
    pub permit_resources_for_relationships: bool,
    pub deduplicate_resources: bool,
    pub convert_resources_to_id: bool,
    /// `utcOffset || 0`.
    pub utc_offset: JsValue,
}

fn plain_error(code: &'static str, params: Vec<(&'static str, String)>) -> ConcertoError {
    ContractError::new(ErrorKind::Error, code, params).into()
}

/// TS: `JSONGenerator.convertToJSON`'s body (task P4-10,
/// accordproject/concerto-rust#69): no dependency on the model manager, so
/// the TS visitor shell can call it per field directly.
pub fn convert_primitive(
    type_name: &str,
    obj: &JsValue,
    options: &GeneratorOptions,
) -> Result<JsValue> {
    if type_name == "DateTime" {
        let JsValue::DateTime(d) = obj else {
            return Err(method_error(obj, "obj.utc", "utc"));
        };
        let offset = match &options.utc_offset {
            JsValue::String(s) => UtcOffset::String(s.clone()),
            JsValue::Number(n) => UtcOffset::Number(*n),
            JsValue::Bool(b) => UtcOffset::Number(f64::from(u8::from(*b))),
            _ => UtcOffset::Number(f64::NAN),
        };
        let with_offset = d.to_utc().utc_offset_set(&offset);
        return Ok(JsValue::String(with_offset.format_json()));
    }
    Ok(obj.clone())
}

/// The visitor's state.
pub(crate) struct Generator<'a> {
    mm: &'a ModelManager,
    options: &'a GeneratorOptions,
    /// `parameters.seenResources`.
    seen_resources: HashSet<String>,
    /// `parameters.dedupeResources`.
    dedupe_resources: HashSet<String>,
}

/// `obj instanceof Resource` (a `ValidatedResource` is one; a
/// `Relationship` is not).
fn as_resource(value: &JsValue) -> Option<&Instance> {
    match value {
        JsValue::Instance(i) if i.kind != InstanceKind::Relationship => Some(i),
        _ => None,
    }
}

/// `for (let index in obj)`: the values a `for...in` over `obj` visits, in
/// order.
fn for_in_values(obj: &JsValue) -> Result<Vec<JsValue>> {
    Ok(match obj {
        JsValue::Array(items) => items.clone(),
        JsValue::String(s) => s
            .encode_utf16()
            .map(|u| JsValue::String(String::from_utf16_lossy(&[u])))
            .collect(),
        JsValue::Object(map) => map.values().cloned().collect(),
        JsValue::Undefined
        | JsValue::Null
        | JsValue::Bool(_)
        | JsValue::Number(_)
        | JsValue::Map(_) => Vec::new(),
        JsValue::DateTime(_) | JsValue::Instance(_) => {
            return Err(ContractError::pre_port(
                ErrorKind::Error,
                "for...in over an object with its own enumerable state".to_string(),
                None,
            )
            .into());
        }
    })
}

/// `obj.<method>(...)` on a value that is not an instance: V8's
/// `TypeError`, `Cannot read properties of null/undefined` or `<expression>
/// is not a function`.
fn method_error(obj: &JsValue, expression: &str, method: &str) -> ConcertoError {
    if obj.is_nullish() {
        read_properties_error(obj, method)
    } else {
        model::not_a_function(expression)
    }
}

impl<'a> Generator<'a> {
    pub fn new(mm: &'a ModelManager, options: &'a GeneratorOptions) -> Self {
        Self {
            mm,
            options,
            seen_resources: HashSet::new(),
            dedupe_resources: HashSet::new(),
        }
    }

    /// TS `declaration.accept(this, parameters)`: `visit` sends a class
    /// declaration (an enum is one) to `visitClassDeclaration` and a map to
    /// `visitMapDeclaration`.
    pub fn accept_declaration(&mut self, declaration: &TypeRef, obj: &JsValue) -> Result<JsValue> {
        if declaration.is_class_declaration() {
            return self.visit_class_declaration(declaration, obj);
        }
        if declaration.is_map_declaration() {
            return self.visit_map_declaration(declaration, obj);
        }
        // `throw new Error('Unrecognised ' + JSON.stringify(thing))`.
        Err(model::unrecognised())
    }

    /// TS: JSONGenerator.visitClassDeclaration.
    pub fn visit_class_declaration(
        &mut self,
        class_declaration: &TypeRef,
        obj: &JsValue,
    ) -> Result<JsValue> {
        let Some(resource) = as_resource(obj) else {
            return Err(plain_error(
                "jsongenerator-visitclassdeclaration-notaresource",
                vec![("obj", obj.to_js_string())],
            ));
        };
        let mut result: IndexMap<String, JsValue> = IndexMap::new();
        let mut id: Option<String> = None;
        // `obj.isIdentifiable()`: its own class declaration's `isIdentified()`.
        if self.options.deduplicate_resources
            && model::get_type(self.mm, &resource.class_fqn)?.is_identified()?
        {
            let uri = resource.to_uri()?;
            if self.dedupe_resources.contains(&uri) {
                return Ok(JsValue::String(uri));
            }
            self.dedupe_resources.insert(uri.clone());
            id = Some(uri);
        }
        result.insert(
            "$class".to_string(),
            JsValue::String(class_declaration.fqn()),
        );
        if self.options.deduplicate_resources
            && let Some(id) = id.filter(|id| !id.is_empty())
        {
            result.insert("$id".to_string(), JsValue::String(id));
        }
        for (owner_fqn, property) in
            class_declaration.properties("classDeclaration.getProperties")?
        {
            let name = crate::Named::name(&property).to_string();
            let value = resource.get(&name).clone();
            if value.is_nullish() {
                continue;
            }
            let field = model::field(self.mm, &owner_fqn, property)?;
            let converted = match &field.field_type {
                FieldType::Relationship(_) => {
                    self.visit_relationship_declaration(&field, &value)?
                }
                FieldType::EnumValue => return Err(model::unrecognised()),
                _ => self.visit_field(&field, &value)?,
            };
            result.insert(name, converted);
        }
        Ok(JsValue::Object(result))
    }

    /// TS: JSONGenerator.visitMapDeclaration.
    fn visit_map_declaration(
        &mut self,
        map_declaration: &TypeRef,
        obj: &JsValue,
    ) -> Result<JsValue> {
        let JsValue::Map(entries) = obj else {
            return Err(method_error(obj, "obj.forEach", "forEach"));
        };
        let Declaration::Map(map) = map_declaration.decl else {
            unreachable!("visit_map_declaration is only reached for a map");
        };
        let mut result: Vec<(String, JsValue)> = Vec::new();
        for (key, value) in entries {
            let key = key.to_js_string();
            // Don't serialize system properties, other than $class (which is
            // one too).
            if model_util::is_system_property(&key) {
                continue;
            }
            let mut value = value.clone();
            if value.type_of() == "object" {
                let value_type = match &value {
                    JsValue::Null => {
                        return Err(read_properties_error(&value, "getFullyQualifiedType"));
                    }
                    JsValue::Instance(i) => Some(i.class_fqn.clone()),
                    _ => self.mm.model_file_fully_qualified_type_name(
                        &map_declaration.namespace(),
                        map.value_type_name(),
                    ),
                };
                let decl = match value_type {
                    Some(t) => model::get_type(self.mm, &t)?,
                    // `getType(null)`: `ModelUtil.getNamespace(null)` throws.
                    None => {
                        model_util::get_namespace(None)?;
                        unreachable!("getNamespace(null) throws");
                    }
                };
                value = self.accept_declaration(&decl, &value)?;
            }
            // `map.set(key, value)`, then `Object.fromEntries(map)`.
            match result.iter_mut().find(|(k, _)| *k == key) {
                Some(entry) => entry.1 = value,
                None => result.push((key, value)),
            }
        }
        Ok(JsValue::Object(result.into_iter().collect()))
    }

    /// TS: JSONGenerator.visitField, for a field already unboxed by
    /// `getScalarField()` where its type is a scalar.
    fn visit_field(&mut self, field: &Field, obj: &JsValue) -> Result<JsValue> {
        if field.is_array() {
            let mut array = Vec::new();
            for item in for_in_values(obj)? {
                if !field.is_primitive() && !matches!(field.field_type, FieldType::Enum(_)) {
                    // `parameters.stack.push(item, Typed)`
                    let JsValue::Instance(typed) = &item else {
                        return Err(plain_error(
                            "typedstack-push-unexpectedtype",
                            vec![
                                ("type", "Typed".to_string()),
                                ("obj", typed_stack_found(&item)?),
                            ],
                        ));
                    };
                    let declaration = model::get_type(self.mm, &typed.class_fqn)?;
                    array.push(self.accept_declaration(&declaration, &item)?);
                } else {
                    array.push(self.convert_to_json(field, &item)?);
                }
            }
            return Ok(JsValue::Array(array));
        }
        match &field.field_type {
            FieldType::Primitive(_) | FieldType::Scalar { .. } | FieldType::Enum(_) => {
                self.convert_to_json(field, obj)
            }
            FieldType::Map(map_fqn) => {
                let declaration = model::get_type(self.mm, map_fqn)?;
                self.accept_declaration(&declaration, obj)
            }
            _ => {
                let JsValue::Instance(typed) = obj else {
                    return Err(method_error(
                        obj,
                        "obj.getFullyQualifiedType",
                        "getFullyQualifiedType",
                    ));
                };
                let declaration = model::get_type(self.mm, &typed.class_fqn)?;
                self.accept_declaration(&declaration, obj)
            }
        }
    }

    /// TS: JSONGenerator.convertToJSON. No dependency on `self.mm`, so it is
    /// [`convert_primitive`], a free function the concerto-wasm binding
    /// (P4-10, jsongenerator.ts) calls directly per field.
    fn convert_to_json(&mut self, field: &Field, obj: &JsValue) -> Result<JsValue> {
        convert_primitive(&field.type_name(), obj, self.options)
    }

    /// TS: JSONGenerator.visitRelationshipDeclaration.
    fn visit_relationship_declaration(
        &mut self,
        relationship: &Field,
        obj: &JsValue,
    ) -> Result<JsValue> {
        if relationship.is_array() {
            let mut array = Vec::new();
            for item in for_in_values(obj)? {
                array.push(self.relationship_item(relationship, &item)?);
            }
            return Ok(JsValue::Array(array));
        }
        self.relationship_item(relationship, obj)
    }

    /// One relationship value: a resource written in full when
    /// `permitResourcesForRelationships` allows it and it is not already
    /// being written, otherwise its relationship text.
    fn relationship_item(&mut self, relationship: &Field, item: &JsValue) -> Result<JsValue> {
        if self.options.permit_resources_for_relationships
            && let Some(resource) = as_resource(item)
        {
            let fqi = resource.fully_qualified_identifier();
            if self.seen_resources.contains(&fqi) {
                return self.get_relationship_text(relationship, item);
            }
            self.seen_resources.insert(fqi.clone());
            let declaration = model::get_type(self.mm, &relationship.fully_qualified_type_name())?;
            let result = self.accept_declaration(&declaration, item)?;
            self.seen_resources.remove(&fqi);
            return Ok(result);
        }
        self.get_relationship_text(relationship, item)
    }

    /// TS: JSONGenerator.getRelationshipText.
    fn get_relationship_text(&mut self, relationship: &Field, item: &JsValue) -> Result<JsValue> {
        if as_resource(item).is_some()
            && !(self.options.convert_resources_to_relationships
                || self.options.permit_resources_for_relationships)
        {
            return Err(plain_error(
                "jsongenerator-getrelationshiptext-norelationship",
                vec![
                    ("type", relationship.fully_qualified_type_name()),
                    ("obj", item.to_js_string()),
                ],
            ));
        }
        let JsValue::Instance(identifiable) = item else {
            let (expression, method) = if self.options.convert_resources_to_id {
                ("relationshipOrResource.getIdentifier", "getIdentifier")
            } else {
                ("relationshipOrResource.toURI", "toURI")
            };
            return Err(method_error(item, expression, method));
        };
        if self.options.convert_resources_to_id {
            Ok(identifiable.get_identifier().clone())
        } else {
            Ok(JsValue::String(identifiable.to_uri()?))
        }
    }
}

/// `obj?.toString()` in `TypedStack.push`'s message.
fn typed_stack_found(obj: &JsValue) -> Result<String> {
    Ok(match obj {
        JsValue::Undefined => "undefined".to_string(),
        other => other.to_js_string(),
    })
}

/// The generator's options from the serializer's merged options.
/// The generator's options from the serializer's merged options. `pub`
/// (not `pub(crate)`) so the concerto-wasm binding (P4-10) can build a
/// `GeneratorOptions` for [`convert_primitive`].
pub fn generator_options(options: &IndexMap<String, JsValue>) -> GeneratorOptions {
    let is_true = |key: &str| options.get(key) == Some(&JsValue::Bool(true));
    let utc_offset = options
        .get("utcOffset")
        .cloned()
        .unwrap_or(JsValue::Undefined);
    GeneratorOptions {
        convert_resources_to_relationships: is_true("convertResourcesToRelationships"),
        permit_resources_for_relationships: is_true("permitResourcesForRelationships"),
        deduplicate_resources: is_true("deduplicateResources"),
        convert_resources_to_id: is_true("convertResourcesToId"),
        utc_offset: if utc_offset.is_truthy() {
            utc_offset
        } else {
            JsValue::Number(0.0)
        },
    }
}
