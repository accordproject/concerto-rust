//! `Serializer.fromJSON` over a plain JSON document (docs/public-api.md
//! sections 5.4 and 5.7, step 5): the checks and coercions of
//! `Serializer.fromJSON`, `Factory.newResource` and `JSONPopulator`
//! (`src/serializer.ts`, `src/factory.ts`, `src/serializer/jsonpopulator.ts`)
//! for a document that JSON can carry, without building the TS `Resource`
//! objects.
//!
//! The JS layer's serializer (`instance::serializer`, with the
//! `js-compat` feature) populates a JS `Resource` from any JS value,
//! `undefined`, a dayjs or a live `Resource` included, because that is what
//! a JS caller can hand it. A native caller holds JSON, and all it needs is
//! the verdict. This module walks the same steps in the same order over a
//! `serde_json::Value`, and builds what the instance validator
//! ([`super::validate`]) reads: the populated instance in the validator's
//! value shape, a `DateTime` as a parsed date and a relationship URI as a
//! relationship. Each function is the TS method, or the Rust port, its doc
//! names, so that the first error is the same one.
//!
//! [`ModelManager::validate_instance`] and its siblings, and the metamodel
//! checks (`validateAst`, `validateMetaModel`), run on it, so the stable API
//! does not depend on the JS object model (docs/public-api.md F8). The
//! native oracle harness replays every recorded `Serializer.fromJSON` call
//! whose input is plain JSON through both routes and requires the same
//! outcome, error and populated instance.

use std::borrow::Cow;

use serde_json::{Map, Value};

use super::dayjs::{Dayjs, UtcOffset};
use super::model::{self, Field, FieldType, TypeRef};
use super::resource_id::ResourceId;
use super::validate::{self, ValidateOptions, js_map, js_number, js_undefined};
use crate::error::{ContractError, Detail, DetailCode, ErrorKind, Result};
use crate::introspect::Declaration;
use crate::model_manager::{ModelManager, Node, ResolutionContext};
use crate::{Error, ecma, model_util};

js_compat_pub! {
    /// What the TS `Factory` gets from its environment rather than from the
    /// model (D7): a new identifier and the current time.
    pub trait InstanceEnv {
        /// TS: `Factory.newId()`, `uuid.v4()`.
        fn new_id(&mut self) -> String;
        /// TS: the time `dayjs.utc()` reads, in ms since the epoch.
        fn now_ms(&mut self) -> f64;
    }
}

/// The environment of a native check: a fixed identifier and clock. No
/// verdict depends on either; they appear only in some message texts.
pub(crate) struct FixedEnv;

impl InstanceEnv for FixedEnv {
    fn new_id(&mut self) -> String {
        "00000000-0000-4000-8000-000000000000".into()
    }
    fn now_ms(&mut self) -> f64 {
        0.0
    }
}

js_compat_pub! {
    /// The `Serializer.fromJSON` options this route reads, after the
    /// serializer has merged them with its defaults.
    #[derive(Debug, Clone, PartialEq)]
    pub struct FromJsonOptions {
        /// `validate`: validate the populated instance
        /// (`ValidatedResource.validate`).
        pub validate: bool,
        /// `utcOffset || 0`: the offset a non-strict `DateTime` gets.
        pub utc_offset: UtcOffset,
        /// `strictQualifiedDateTimes`.
        pub strict_qualified_date_times: bool,
        /// `acceptResourcesForRelationships`.
        pub accept_resources_for_relationships: bool,
        /// `rejectUnknownKeys` (accordproject/concerto#1273).
        pub reject_unknown_keys: bool,
        /// `rejectRequiredNull` (accordproject/concerto#1273).
        pub reject_required_null: bool,
        /// The options the validator walk runs with. `Serializer.fromJSON`
        /// always validates with the defaults (the `ValidatedResource`'s own
        /// validator).
        pub validator: ValidateOptions,
    }
}

impl Default for FromJsonOptions {
    /// A `Serializer` built with no options: `{validate: true, utcOffset}`,
    /// where the `-0` offset is falsy and becomes `0`.
    fn default() -> Self {
        Self {
            validate: true,
            utc_offset: UtcOffset::Number(0.0),
            strict_qualified_date_times: false,
            accept_resources_for_relationships: false,
            reject_unknown_keys: false,
            reject_required_null: false,
            validator: ValidateOptions::default(),
        }
    }
}

js_compat_pub! {
    /// TS: `Serializer.fromJSON(json, options)` for a plain JSON `json`,
    /// then, when `options.validate` is set, `ValidatedResource.validate`.
    /// Returns the populated instance in the validator's value shape.
    pub fn from_json(
        mm: &ModelManager,
        json: &Value,
        options: &FromJsonOptions,
        env: &mut dyn InstanceEnv,
    ) -> Result<Value> {
        let class_name = get_property(Some(json), "$class")?;
        if !is_truthy(class_name.as_deref()) {
            return Err(plain_error("serializer-fromjson-noclass", Vec::new()));
        }
        // DV-015: see the JS layer's `Serializer::from_json`.
        let Some(Value::String(class_name)) = class_name.as_deref() else {
            return Err(not_a_string_class(class_name.as_deref()));
        };
        from_json_as(mm, json, class_name, options, env)
    }
}

/// [`from_json`] after its `$class` read, with `class_name` as the type: a
/// document with no `$class` of its own can be checked against a named type
/// ([`ModelManager::validate_instance_as`]).
pub(crate) fn from_json_as(
    mm: &ModelManager,
    json: &Value,
    class_name: &str,
    options: &FromJsonOptions,
    env: &mut dyn InstanceEnv,
) -> Result<Value> {
    let class_declaration = model::get_type(mm, class_name)?;
    let ns = class_declaration.namespace();
    let name = class_declaration.name().to_string();
    let id = match class_declaration.identifier_field_name()? {
        Some(field) => get_property(Some(json), &field)?,
        None => get_property(Some(json), "null")?,
    };
    let mut populator = Populator {
        mm,
        env,
        options,
        path: vec!["$".to_string()],
        instances: Vec::new(),
    };
    let resource = if class_declaration.is_transaction() || class_declaration.is_event() {
        // `Factory.newTransaction`/`newEvent`: `ns` and `type` must be
        // truthy, then `newResource`, then the kind check.
        if ns.is_empty() {
            return Err(plain_error("factory-newtransaction-nsnotspecified", Vec::new()));
        }
        if name.is_empty() {
            return Err(plain_error(
                "factory-newtransaction-typenotspecified",
                Vec::new(),
            ));
        }
        let resource = populator.new_resource(&ns, &name, id)?;
        let decl = model::get_type(mm, &resource.class_fqn)?;
        if class_declaration.is_transaction() && !decl.is_transaction() {
            return Err(plain_error(
                "factory-newtransaction-notatransaction",
                vec![("fqn", resource.class_fqn.clone())],
            ));
        }
        if class_declaration.is_event() && !decl.is_event() {
            return Err(plain_error(
                "factory-newevent-notanevent",
                vec![("fqn", resource.class_fqn.clone())],
            ));
        }
        resource
    } else if class_declaration.is_concept() {
        populator.new_resource(&ns, &name, id)?
    } else if class_declaration.is_map_declaration() {
        return Err(plain_error("serializer-fromjson-mapnotsupported", Vec::new()));
    } else if class_declaration.is_enum() {
        return Err(plain_error("serializer-fromjson-enumnotsupported", Vec::new()));
    } else {
        populator.new_resource(&ns, &name, id)?
    };

    let resource = populator.visit_class_declaration(&class_declaration, Some(json), resource)?;
    let root_identifier = resource.fully_qualified_identifier();
    let value = populator.finish(resource);
    if options.validate {
        validate::validate_instance_from(mm, &value, &options.validator, root_identifier)?;
        // `ResourceValidator.visitClassDeclaration` writes each resource's
        // identifier back (the JS layer's `resource::sync_identifiers`),
        // which reads each one's identifying field; only that read can fail.
        for fqn in &populator.instances {
            mm.identifier_field(fqn)?;
        }
    }
    Ok(value)
}

// ---------------------------------------------------------------------
// JS values over JSON
// ---------------------------------------------------------------------

/// A JS value read off a JSON document: `None` is `undefined`. A property
/// read of a string (an index or `length`) makes a new value.
type Js<'v> = Option<Cow<'v, Value>>;

fn is_truthy(value: Option<&Value>) -> bool {
    value.is_some_and(ecma::is_truthy)
}

fn is_nullish(value: Option<&Value>) -> bool {
    matches!(value, None | Some(Value::Null))
}

/// ECMAScript `ToString`.
fn js_string(value: Option<&Value>) -> String {
    value.map_or_else(|| "undefined".to_string(), ecma::to_js_string)
}

/// V8's `TypeError: Cannot read properties of <value> (reading '<property>')`.
fn read_properties_error(value: Option<&Value>, property: &str) -> Error {
    ContractError::new(
        ErrorKind::MalformedInput,
        "engine-typeerror-readproperties",
        vec![
            ("value", js_string(value)),
            ("property", property.to_string()),
        ],
    )
    .into()
}

/// `value[key]`. Reading a property of `undefined` or `null` is V8's
/// `TypeError`.
fn get_property<'v>(value: Option<&'v Value>, key: &str) -> Result<Js<'v>> {
    let index = || key.parse::<usize>().ok().filter(|i| key == i.to_string());
    Ok(match value {
        None | Some(Value::Null) => return Err(read_properties_error(value, key)),
        Some(Value::Object(map)) => map.get(key).map(Cow::Borrowed),
        Some(Value::String(s)) => match index() {
            // An index reads one UTF-16 unit.
            Some(i) => s
                .encode_utf16()
                .nth(i)
                .map(|u| Cow::Owned(Value::String(String::from_utf16_lossy(&[u])))),
            None if key == "length" => Some(Cow::Owned(js_number(s.encode_utf16().count() as f64))),
            None => None,
        },
        Some(Value::Array(items)) => match index() {
            Some(i) => items.get(i).map(Cow::Borrowed),
            None if key == "length" => Some(Cow::Owned(js_number(items.len() as f64))),
            None => None,
        },
        Some(_) => None,
    })
}

/// `Object.keys(value)`: V8's `TypeError` for `undefined` and `null`, and
/// the integer-like keys first, in ascending order.
fn object_keys(value: Option<&Value>) -> Result<Vec<String>> {
    Ok(match value {
        None | Some(Value::Null) => {
            return Err(ContractError::new(
                ErrorKind::MalformedInput,
                "engine-typeerror-convertnulltoobject",
                Vec::new(),
            )
            .into());
        }
        Some(Value::Object(map)) => {
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
            let rest: Vec<String> = map.keys().filter(|k| !keys.contains(k)).cloned().collect();
            keys.extend(rest);
            keys
        }
        Some(Value::String(s)) => (0..s.encode_utf16().count())
            .map(|i| i.to_string())
            .collect(),
        Some(Value::Array(items)) => (0..items.len()).map(|i| i.to_string()).collect(),
        Some(_) => Vec::new(),
    })
}

/// A plain JSON value in the validator's value shape: every number as a JS
/// number (an integral one as an integer, [`js_number`]).
fn plain(value: Option<&Value>) -> Value {
    match value {
        None => js_undefined(),
        Some(Value::Number(n)) => js_number(n.as_f64().unwrap_or(f64::NAN)),
        Some(Value::Array(items)) => Value::Array(items.iter().map(|v| plain(Some(v))).collect()),
        Some(Value::Object(map)) => Value::Object(
            map.iter()
                .map(|(k, v)| (k.clone(), plain(Some(v))))
                .collect(),
        ),
        Some(other) => other.clone(),
    }
}

/// Whether a value in the validator's shape is truthy (`undefined` is not).
fn validator_value_is_truthy(value: &Value) -> bool {
    !validate::is_js_undefined(value) && ecma::is_truthy(value)
}

fn validation(code: &'static str, params: Vec<(&'static str, String)>) -> Error {
    ContractError::new(ErrorKind::Validation, code, params).into()
}

fn plain_error(code: &'static str, params: Vec<(&'static str, String)>) -> Error {
    ContractError::new(ErrorKind::InvalidArgument, code, params).into()
}

/// DV-015: a `$class` that is not a string.
fn not_a_string_class(class_name: Option<&Value>) -> Error {
    ContractError::pre_port(
        ErrorKind::InvalidArgument,
        format!("a $class that is not a string: {}", js_string(class_name)),
        None,
    )
    .into()
}

// ---------------------------------------------------------------------
// Factory
// ---------------------------------------------------------------------

js_compat_pub! {
    /// The identifier `Factory.newResource` is given, as its checks read it.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum IdentifierArg<'a> {
        /// `undefined` or `null`.
        Nullish,
        /// A string.
        String(&'a str),
        /// Any other value, and whether it is truthy.
        Other {
            /// `!!id`.
            truthy: bool,
        },
    }
}

impl<'a> IdentifierArg<'a> {
    fn of(value: Option<&'a Value>) -> Self {
        match value {
            None | Some(Value::Null) => Self::Nullish,
            Some(Value::String(s)) => Self::String(s),
            Some(other) => Self::Other {
                truthy: ecma::is_truthy(other),
            },
        }
    }
}

js_compat_pub! {
    /// What [`check_new_resource`] settles: everything `newResource` needs
    /// from the model before it builds the object.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ResourceCheck {
        /// `classDecl.getFullyQualifiedName()`.
        pub class_fqn: String,
        /// `classDecl.getIdentifierFieldName()`.
        pub identifier_field_name: Option<String>,
        /// The identifier `Factory.newId()` made for a system-identified
        /// type given a nullish one; it replaces the one given.
        pub generated_id: Option<String>,
        /// `classDecl.isTransaction() || classDecl.isEvent()`: the instance
        /// gets `dayjs.utc()` as its `$timestamp`.
        pub timestamped: bool,
    }
}

js_compat_pub! {
    /// The model checks of `Factory.newResource`, in TS order (#32 point 4):
    /// the type lookup, the abstract type, the identifier's type, the empty
    /// identifier and the identifier regex, plus the non-identifiable type
    /// given an identifier. `new_id` is `Factory.newId`, called only for a
    /// system-identified type given a nullish id.
    ///
    /// TS: Factory.newResource (src/factory.ts), up to the construction.
    pub fn check_new_resource(
        mm: &ModelManager,
        ns: &str,
        type_name: &str,
        id: IdentifierArg<'_>,
        new_id: &mut dyn FnMut() -> String,
    ) -> Result<ResourceCheck> {
        let qualified_name = model_util::qualify(ns, type_name);
        let class_decl = model::get_type(mm, &qualified_name)?;
        let ns_and_type = || {
            vec![
                ("namespace", ns.to_string()),
                ("type", type_name.to_string()),
            ]
        };

        if class_decl.is_abstract("classDecl.isAbstract")? {
            return Err(plain_error("factory-newinstance-abstracttype", ns_and_type()));
        }

        let id_field = class_decl.identifier_field_name()?;
        let generated_id = (class_decl.is_system_identified()? && id == IdentifierArg::Nullish)
            .then(new_id);
        let id = match &generated_id {
            Some(generated) => IdentifierArg::String(generated),
            None => id,
        };
        if let Some(id_field) = &id_field {
            let IdentifierArg::String(id_text) = id else {
                return Err(plain_error(
                    "factory-newinstance-invalididentifier",
                    ns_and_type(),
                ));
            };
            if ecma::js_trim(id_text).is_empty() {
                return Err(plain_error(
                    "factory-newinstance-missingidentifier",
                    ns_and_type(),
                ));
            }
            // `if (id)`: a non-empty string here.
            if let Some(regex) = model::identifier_regex(&class_decl, id_field)?
                && !regex.matches_regex(id_text)
            {
                return Err(plain_error(
                    "factory-newresource-idregexmismatch",
                    vec![("regex", regex.regex().unwrap_or_default())],
                ));
            }
        } else if matches!(id, IdentifierArg::String(s) if !s.is_empty())
            || matches!(id, IdentifierArg::Other { truthy: true })
        {
            return Err(plain_error(
                "factory-newresource-notidentifiable",
                vec![("fqn", class_decl.fqn())],
            ));
        }

        Ok(ResourceCheck {
            class_fqn: class_decl.fqn(),
            identifier_field_name: id_field,
            generated_id,
            timestamped: class_decl.is_transaction() || class_decl.is_event(),
        })
    }
}

js_compat_pub! {
    /// The `$identifierFieldName` the `Identifiable` constructor caches:
    /// `modelManager.getModelFile(ns)?.getType(fqt)?.getIdentifierFieldName()
    /// || '$identifier'`, with `fqt` the class declaration's name. `None`
    /// stands for the `'$identifier'` fallback.
    pub fn identifiable_field_name(
        mm: &ModelManager,
        ns: &str,
        class_fqn: &str,
    ) -> Result<Option<String>> {
        let Some(file) = mm.model_file_id(ns) else {
            return Ok(None);
        };
        let Some(Node::Declaration(id)) = mm.get_type(&Node::ModelFile(file), Some(class_fqn))?
        else {
            return Ok(None);
        };
        let decl = TypeRef {
            mm,
            id,
            decl: mm.declaration(id).expect("a live handle"),
        };
        Ok(decl.identifier_field_name()?.filter(|f| !f.is_empty()))
    }
}

js_compat_pub! {
    /// A property default, converted by the field's type as
    /// `Typed.assignFieldDefaults` converts it.
    #[derive(Debug, Clone, PartialEq)]
    pub enum FieldDefault {
        /// An `Integer`, `Long` or `Double` default: `parseInt`/`parseFloat`
        /// of its text.
        Number(f64),
        /// A `Boolean` default: `default === true`.
        Bool(bool),
        /// A `DateTime` default: `dayjs.utc(default)`.
        DateTime(Dayjs),
        /// A `String` or enum default, as it is in the AST.
        Json(Value),
    }
}

js_compat_pub! {
    /// TS: `Typed.assignFieldDefaults` (src/model/typed.ts): each field of
    /// `class_fqn` with a non-null default, converted by the field's type,
    /// handed to `assign` (TS `this.setPropertyValue`) with the property's
    /// name, in `getProperties()` order. A relationship is not a `Field`, and
    /// has none.
    pub fn assign_field_defaults(
        mm: &ModelManager,
        class_fqn: &str,
        assign: &mut dyn FnMut(&str, FieldDefault) -> Result<()>,
    ) -> Result<()> {
        let class_decl = model::get_type(mm, class_fqn)?;
        for (owner_fqn, property) in class_decl.properties("classDeclaration.getProperties")? {
            // `isField?.()`: relationships are not `Field`s.
            if property.is_relationship() || property.is_enum_value() {
                continue;
            }
            let name = crate::Named::name(&property).to_string();
            let field = model::field(mm, &owner_fqn, property)?;
            let (default_value, type_name) = match &field.field_type {
                FieldType::Scalar {
                    default_value,
                    primitive,
                    ..
                } => (
                    default_value.clone(),
                    primitive.map(str::to_string).unwrap_or_default(),
                ),
                _ => (raw_default_value(mm, &owner_fqn, &name), field.type_name()),
            };
            let Some(default_value) = default_value.filter(|v| !v.is_null()) else {
                continue;
            };
            let value = match type_name.as_str() {
                "Integer" | "Long" => {
                    FieldDefault::Number(ecma::parse_int(&ecma::to_js_string(&default_value)))
                }
                "Double" => {
                    FieldDefault::Number(ecma::parse_float(&ecma::to_js_string(&default_value)))
                }
                "Boolean" => FieldDefault::Bool(default_value == Value::Bool(true)),
                "DateTime" => FieldDefault::DateTime(match &default_value {
                    Value::String(s) => Dayjs::utc_parse(s),
                    Value::Number(n) => Dayjs::utc_from_number(n.as_f64().unwrap_or(f64::NAN)),
                    _ => Dayjs::utc_invalid(),
                }),
                // String, and "if we get this far the field should be an enum".
                _ => FieldDefault::Json(default_value),
            };
            assign(&name, value)?;
        }
        Ok(())
    }
}

/// The raw AST `defaultValue` of a property of `owner_fqn`
/// (`Field.getDefaultValue()`, `null` when nullish), read off the AST as TS
/// does: the typed `DateTimeProperty` carries none.
fn raw_default_value(mm: &ModelManager, owner_fqn: &str, name: &str) -> Option<Value> {
    let decl = mm.declaration_id(owner_fqn)?;
    let prop = mm.property_ids(decl).find(|id| {
        mm.property_by_id(*id)
            .is_some_and(|p| crate::Named::name(p) == name)
    })?;
    mm.property_default_value(prop).cloned()
}

// ---------------------------------------------------------------------
// The instance being populated
// ---------------------------------------------------------------------

/// A `ValidatedResource` being populated: its own properties in insertion
/// order, each in the validator's value shape, without the private ones the
/// validator never reads (`$namespace`, `$type`, `$identifierFieldName`).
struct Resource {
    class_fqn: String,
    /// `this.$identifierFieldName`.
    identifier_key: String,
    props: Map<String, Value>,
}

impl Resource {
    /// TS `Identifiable.getFullyQualifiedIdentifier`.
    fn fully_qualified_identifier(&self) -> String {
        match self.props.get(&self.identifier_key) {
            Some(id) if validator_value_is_truthy(id) => {
                format!("{}#{}", self.class_fqn, ecma::to_js_string(id))
            }
            _ => self.class_fqn.clone(),
        }
    }
}

/// `Relationship.fromURI(...)` in the validator's value shape: its
/// `$class` and its identifying field, tagged as a relationship.
fn relationship_value(class_fqn: String, identifier_key: String, id: String) -> Value {
    let mut wire = Map::new();
    wire.insert(validate::RELATIONSHIP_TAG.to_string(), Value::Bool(true));
    wire.insert("$class".to_string(), Value::String(class_fqn));
    wire.insert(identifier_key, Value::String(id));
    Value::Object(wire)
}

// ---------------------------------------------------------------------
// JSONPopulator
// ---------------------------------------------------------------------

/// The visitor's state: its options and `parameters`.
struct Populator<'a> {
    mm: &'a ModelManager,
    env: &'a mut dyn InstanceEnv,
    options: &'a FromJsonOptions,
    /// `parameters.path`, a `TypedStack` that starts as `['$']`.
    path: Vec<String>,
    /// The type of each resource built, for the identifier write-back.
    instances: Vec<String>,
}

impl Populator<'_> {
    fn path_text(&self) -> String {
        self.path.concat()
    }

    /// A finished resource, in the validator's value shape.
    fn finish(&mut self, resource: Resource) -> Value {
        self.instances.push(resource.class_fqn.clone());
        let mut wire = Map::new();
        wire.insert("$class".to_string(), Value::String(resource.class_fqn));
        for (key, value) in resource.props {
            wire.insert(key, value);
        }
        Value::Object(wire)
    }

    /// TS: `Factory.newResource(ns, type, id)` with a falsy
    /// `options.generate`: the checks, the `ValidatedResource` constructor,
    /// `assignFieldDefaults()` (each default validated as it is set), then
    /// the identifying field.
    fn new_resource(&mut self, ns: &str, type_name: &str, id: Js) -> Result<Resource> {
        let env = &mut *self.env;
        let check = check_new_resource(
            self.mm,
            ns,
            type_name,
            IdentifierArg::of(id.as_deref()),
            &mut || env.new_id(),
        )?;
        let id = match &check.generated_id {
            Some(generated) => Value::String(generated.clone()),
            None => plain(id.as_deref()),
        };
        let timestamp = if check.timestamped {
            Dayjs::utc_now(self.env.now_ms()).validator_value()
        } else {
            Value::Null
        };
        let identifier_key = identifiable_field_name(self.mm, ns, &check.class_fqn)?
            .unwrap_or_else(|| "$identifier".to_string());
        let mut props = Map::new();
        props.insert("$identifier".to_string(), id.clone());
        props.insert(identifier_key.clone(), id.clone());
        props.insert("$timestamp".to_string(), timestamp);
        let mut resource = Resource {
            class_fqn: check.class_fqn.clone(),
            identifier_key,
            props,
        };
        let mm = self.mm;
        assign_field_defaults(mm, &check.class_fqn, &mut |name, value| {
            let value = match value {
                FieldDefault::Number(n) => js_number(n),
                FieldDefault::Bool(b) => Value::Bool(b),
                FieldDefault::DateTime(d) => d.validator_value(),
                FieldDefault::Json(v) => plain(Some(&v)),
            };
            set_property_value(mm, &mut resource, name, value)
        })?;
        if let Some(id_field) = check.identifier_field_name {
            resource.props.insert(id_field, id);
        }
        Ok(resource)
    }

    /// TS `declaration.accept(this, parameters)` for a declaration: a class
    /// declaration (or an enum, which is one) to `visitClassDeclaration`,
    /// with the resource it pops, and a map to `visitMapDeclaration`.
    fn accept_declaration(
        &mut self,
        declaration: &TypeRef,
        json: Option<&Value>,
        resource: Option<Resource>,
    ) -> Result<Value> {
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
            let resource = self.visit_class_declaration(declaration, json, resource)?;
            return Ok(self.finish(resource));
        }
        if declaration.is_map_declaration() {
            return self.visit_map_declaration(declaration, json);
        }
        // `throw new Error('Unrecognised ' + JSON.stringify(thing))`: a
        // scalar declaration.
        Err(model::unrecognised())
    }

    /// TS: JSONPopulator.visitClassDeclaration.
    fn visit_class_declaration(
        &mut self,
        class_declaration: &TypeRef,
        json: Option<&Value>,
        mut resource: Resource,
    ) -> Result<Resource> {
        let properties = get_assignable_properties(json, class_declaration)?;
        if self.options.reject_unknown_keys {
            self.reject_unknown_keys(json, class_declaration)?;
        }
        validate_properties(&properties, class_declaration)?;
        if self.options.reject_required_null {
            self.reject_required_null(json, class_declaration)?;
        }
        for property in properties {
            let value = get_property(json, &property)?;
            if value.as_deref() != Some(&Value::Null) {
                self.path.push(format!(".{property}"));
                let (owner_fqn, class_property) = class_declaration
                    .property(&property)?
                    .expect("validateProperties found every property");
                let field = model::field(self.mm, &owner_fqn, class_property)?;
                let populated = self.visit_property(&field, value.as_deref())?;
                resource.props.insert(property, populated);
                self.path.pop();
            }
        }
        Ok(resource)
    }

    /// `rejectUnknownKeys` (accordproject/concerto#1273): every key that is
    /// not a system property and that the declaration does not declare,
    /// whatever its value (`null` included), in one error with one
    /// `UNKNOWN_PROPERTY` detail per key.
    fn reject_unknown_keys(&self, json: Option<&Value>, class_declaration: &TypeRef) -> Result<()> {
        let expected = property_names(class_declaration)?;
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
            .map(|property| Detail {
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
    fn reject_required_null(
        &self,
        json: Option<&Value>,
        class_declaration: &TypeRef,
    ) -> Result<()> {
        for key in object_keys(json)? {
            if model_util::is_system_property(&key)
                || get_property(json, &key)?.as_deref() != Some(&Value::Null)
            {
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
            error.details = vec![Detail {
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
    fn visit_property(&mut self, field: &Field, json: Option<&Value>) -> Result<Value> {
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
        json: Option<&Value>,
    ) -> Result<Value> {
        // Throws if the map holds reserved properties.
        get_assignable_properties(json, map_declaration)?;
        let Declaration::Map(map) = map_declaration.decl else {
            unreachable!("visit_map_declaration is only reached for a map");
        };
        let key_type = map.key_type_name().to_string();
        let value_type = map.value_type_name().to_string();
        let mut result: Vec<(Value, Value)> = Vec::new();
        // `new Map(Object.entries(jsonObj))`
        for key in object_keys(json)? {
            let value = get_property(json, &key)?;
            if key == "$class" {
                map_set(&mut result, Value::String(key), plain(value.as_deref()));
                continue;
            }
            let key_json = Value::String(key);
            let key = if model_util::is_primitive_type(&key_type) {
                key_json
            } else {
                self.process_map_type(map_declaration, Some(&key_json), &key_type)?
            };
            let value = if model_util::is_primitive_type(&value_type) {
                plain(value.as_deref())
            } else {
                self.process_map_type(map_declaration, value.as_deref(), &value_type)?
            };
            map_set(&mut result, key, value);
        }
        Ok(js_map(result))
    }

    /// TS: JSONPopulator.processMapType.
    fn process_map_type(
        &mut self,
        map_declaration: &TypeRef,
        value: Option<&Value>,
        type_name: &str,
    ) -> Result<Value> {
        let namespace = map_declaration.namespace();
        let mm = self.mm;
        // `try { ... } catch (err) { decl = undefined; }`
        let declaration = (|| -> Option<TypeRef<'_>> {
            let class_name = match value {
                Some(Value::Object(_) | Value::Array(_)) => get_property(value, "$class")
                    .ok()
                    .flatten()
                    .filter(|c| ecma::is_truthy(c)),
                _ => None,
            };
            let name = match class_name.as_deref() {
                Some(Value::String(s)) => s.clone(),
                Some(_) => return None,
                None => mm.model_file_fully_qualified_type_name(&namespace, type_name)?,
            };
            model::get_type(mm, &name).ok()
        })();
        if let Some(declaration) = declaration
            && declaration.is_class_declaration()
        {
            // `newConcept(ns, name, decl.getIdentifierFieldName())`: the
            // field's name as the identifier. DV-011
            let id = declaration.identifier_field_name()?.map(Value::String);
            let id = Some(Cow::Owned(id.unwrap_or(Value::Null)));
            let sub_resource =
                self.new_resource(&declaration.namespace(), declaration.name(), id)?;
            return self.accept_declaration(&declaration, value, Some(sub_resource));
        }
        // TS's `catch` leaves `value` exactly as parsed.
        Ok(plain(value))
    }

    /// TS: JSONPopulator.visitField, for a field already unboxed by
    /// `getScalarField()` where its type is a scalar.
    fn visit_field(&mut self, field: &Field, json: Option<&Value>) -> Result<Value> {
        if field.is_array() {
            let Some(Value::Array(items)) = json else {
                return Err(validation(
                    "jsonpopulator-visitfield-notarray",
                    vec![("path", self.path_text()), ("type", field.type_name())],
                ));
            };
            let mut result = Vec::with_capacity(items.len());
            for (n, item) in items.iter().enumerate() {
                self.path.push(format!("[{n}]"));
                result.push(self.convert_item(field, Some(item))?);
                self.path.pop();
            }
            Ok(Value::Array(result))
        } else {
            self.convert_item(field, json)
        }
    }

    /// TS: JSONPopulator.convertItem.
    fn convert_item(&mut self, field: &Field, json_item: Option<&Value>) -> Result<Value> {
        if field.is_primitive() || matches!(field.field_type, FieldType::Enum(_)) {
            return self.convert_to_object(field, json_item);
        }
        let class_name = get_property(json_item, "$class")?;
        let type_name = if is_truthy(class_name.as_deref()) {
            match class_name.as_deref() {
                Some(Value::String(s)) => s.clone(),
                // DV-015: see the JS layer's `Serializer::from_json`.
                other => return Err(not_a_string_class(other)),
            }
        } else {
            field.fully_qualified_type_name()
        };
        let declaration = model::get_type(self.mm, &type_name)?;
        let sub_resource = if declaration.is_map_declaration() {
            None
        } else if declaration.is_identified()? {
            let id_field = declaration
                .identifier_field_name()?
                .expect("an identified declaration names its identifying field");
            let id = get_property(json_item, &id_field)?;
            Some(self.new_resource(&declaration.namespace(), declaration.name(), id)?)
        } else {
            Some(self.new_resource(&declaration.namespace(), declaration.name(), None)?)
        };
        self.accept_declaration(&declaration, json_item, sub_resource)
    }

    /// TS: JSONPopulator.convertToObject: the primitive-type switch.
    fn convert_to_object(&mut self, field: &Field, json: Option<&Value>) -> Result<Value> {
        let type_name = field.type_name();
        let path = self.path_text();
        let wrong_type = || {
            validation(
                "jsonpopulator-converttoobject-wrongtype",
                vec![("path", path.clone()), ("type", type_name.clone())],
            )
        };
        Ok(match type_name.as_str() {
            "DateTime" => {
                let Some(Value::String(s)) = json else {
                    return Err(wrong_type());
                };
                let result = if !self.options.strict_qualified_date_times {
                    Dayjs::utc_parse(s).utc_offset_set(&self.options.utc_offset)
                } else if strict_qualified_date_time(s) {
                    Dayjs::utc_parse(s)
                } else {
                    return Err(validation(
                        "jsonpopulator-converttoobject-datetimeformat",
                        vec![("path", path.clone()), ("type", type_name.clone())],
                    ));
                };
                if !result.is_valid() {
                    return Err(wrong_type());
                }
                result.validator_value()
            }
            "Integer" | "Long" => match json {
                // `Math.trunc(num) !== num` (Infinity passes, NaN does not). DV-012
                Some(Value::Number(n))
                    if n.as_f64().is_some_and(|n| n.trunc() == n) =>
                {
                    plain(json)
                }
                _ => return Err(wrong_type()),
            },
            "Double" => match json {
                Some(Value::Number(_)) => plain(json),
                _ => return Err(wrong_type()),
            },
            "Boolean" => match json {
                Some(Value::Bool(_)) => plain(json),
                _ => return Err(wrong_type()),
            },
            "String" => match json {
                Some(Value::String(_)) => plain(json),
                _ => return Err(wrong_type()),
            },
            // Everything else should be an enumerated value.
            _ => plain(json),
        })
    }

    /// TS: JSONPopulator.visitRelationshipDeclaration.
    fn visit_relationship_declaration(
        &mut self,
        relationship: &Field,
        json: Option<&Value>,
    ) -> Result<Value> {
        let type_fqn = relationship.fully_qualified_type_name();
        let mut default_namespace = model_util::get_namespace(Some(&type_fqn))?.to_string();
        if default_namespace.is_empty() {
            default_namespace =
                model_util::get_namespace(Some(&relationship.owner_fqn))?.to_string();
        }
        let default_type = model_util::short_name(&type_fqn).to_string();

        if relationship.is_array() {
            let Some(Value::Array(items)) = json else {
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
                if let Value::String(uri) = item {
                    result.push(relationship_from_uri(
                        self.mm,
                        uri,
                        &default_namespace,
                        &default_type,
                    )?);
                } else {
                    result.push(self.relationship_resource(relationship, json, Some(item))?);
                }
            }
            Ok(Value::Array(result))
        } else {
            match json {
                Some(Value::String(uri)) => {
                    relationship_from_uri(self.mm, uri, &default_namespace, &default_type)
                }
                Some(Value::Object(_) | Value::Array(_)) => {
                    self.relationship_resource(relationship, json, json)
                }
                _ => Err(plain_error(
                    "jsonpopulator-visitrelationshipdeclaration-notstringorobject",
                    vec![
                        ("value", js_string(json)),
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
        json: Option<&Value>,
        item: Option<&Value>,
    ) -> Result<Value> {
        if !self.options.accept_resources_for_relationships {
            return Err(plain_error(
                "jsonpopulator-visitrelationshipdeclaration-notastring",
                vec![
                    ("value", js_string(json)),
                    ("relationship", relationship.relationship_to_string()),
                ],
            ));
        }
        let class_name = get_property(item, "$class")?;
        if !is_truthy(class_name.as_deref()) {
            return Err(plain_error(
                "jsonpopulator-visitrelationshipdeclaration-noclass",
                vec![
                    ("value", js_string(item)),
                    ("relationship", relationship.relationship_to_string()),
                ],
            ));
        }
        // DV-015: see the JS layer's `Serializer::from_json`.
        let Some(Value::String(class_name)) = class_name.as_deref() else {
            return Err(not_a_string_class(class_name.as_deref()));
        };
        let class_declaration = model::get_type(self.mm, class_name)?;
        let id = match class_declaration.identifier_field_name()? {
            Some(field) => get_property(item, &field)?,
            None => get_property(item, "null")?,
        };
        let sub_resource = self.new_resource(
            &class_declaration.namespace(),
            class_declaration.name(),
            id,
        )?;
        self.accept_declaration(&class_declaration, item, Some(sub_resource))
    }
}

/// TS: `Relationship.fromURI(modelManager, uri, defaultNamespace,
/// defaultType)` (src/model/relationship.ts), in the validator's value
/// shape.
fn relationship_from_uri(
    mm: &ModelManager,
    uri: &str,
    default_namespace: &str,
    default_type: &str,
) -> Result<Value> {
    let resource_id = ResourceId::from_uri(uri, Some(default_namespace), Some(default_type))?;
    let fqt = model_util::qualify(&resource_id.namespace, &resource_id.type_name);
    let class_decl = model::get_type(mm, &fqt)?;
    let class_fqn = class_decl.fqn();
    let identifier_key = identifiable_field_name(mm, &resource_id.namespace, &class_fqn)?
        .unwrap_or_else(|| "$identifier".to_string());
    Ok(relationship_value(class_fqn, identifier_key, resource_id.id))
}

/// TS `ValidatedResource.setPropertyValue(propName, value)`: the value is
/// validated against the property before it is assigned.
fn set_property_value(
    mm: &ModelManager,
    resource: &mut Resource,
    prop_name: &str,
    value: Value,
) -> Result<()> {
    let class_declaration = model::get_type(mm, &resource.class_fqn)?;
    let Some((owner_fqn, field)) = class_declaration.property(prop_name)? else {
        let id = resource
            .props
            .get(&resource.identifier_key)
            .filter(|v| !validate::is_js_undefined(v))
            .map_or_else(|| "undefined".to_string(), ecma::to_js_string);
        return Err(plain_error(
            "validatedresource-setpropertyvalue-undeclaredfield",
            vec![("id", id), ("propName", prop_name.to_string())],
        ));
    };
    validate::validate_property_value(
        mm,
        &owner_fqn,
        &field,
        &value,
        resource.fully_qualified_identifier(),
        &ValidateOptions::default(),
    )?;
    resource.props.insert(prop_name.to_string(), value);
    Ok(())
}

/// The declared property names of a class declaration
/// (`classDeclaration.getProperties()`).
fn property_names(class_declaration: &TypeRef) -> Result<Vec<String>> {
    Ok(class_declaration
        .properties("classDeclaration.getProperties")?
        .iter()
        .map(|(_, p)| crate::Named::name(p).to_string())
        .collect())
}

/// TS `getAssignableProperties(resourceData, classDeclaration)`: the keys
/// that have a value and are not system properties, after the reserved
/// property and `$timestamp` checks.
fn get_assignable_properties(
    resource_data: Option<&Value>,
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
        if is_nullish(get_property(resource_data, &property)?.as_deref()) {
            continue;
        }
        assignable.push(property);
    }
    Ok(assignable)
}

/// TS `validateProperties(properties, classDeclaration)`.
fn validate_properties(properties: &[String], class_declaration: &TypeRef) -> Result<()> {
    let expected = property_names(class_declaration)?;
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

/// `map.set(key, value)`: a key seen before keeps its place.
fn map_set(entries: &mut Vec<(Value, Value)>, key: Value, value: Value) {
    match entries.iter_mut().find(|(k, _)| *k == key) {
        Some(entry) => entry.1 = value,
        None => entries.push((key, value)),
    }
}

js_compat_pub! {
    /// `json.match(/^((?:(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2}(?:\.\d+)?))(Z|[+-]\d{2}:\d{2}))$/)`:
    /// the `strictQualifiedDateTimes` format.
    pub fn strict_qualified_date_time(s: &str) -> bool {
        let re = regress::Regex::new(
            r"^((?:(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2}(?:\.\d+)?))(Z|[+-]\d{2}:\d{2}))$",
        )
        .expect("static pattern");
        re.find(s).is_some()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn manager() -> ModelManager {
        let mut mm = ModelManager::new().unwrap();
        mm.load_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "imports": [],
                "declarations": [
                    {
                        "$class": "concerto.metamodel@1.0.0.AssetDeclaration",
                        "name": "Car",
                        "isAbstract": false,
                        "identified": {
                            "$class": "concerto.metamodel@1.0.0.IdentifiedBy",
                            "name": "vin"
                        },
                        "properties": [
                            { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "vin", "isArray": false, "isOptional": false },
                            { "$class": "concerto.metamodel@1.0.0.DateTimeProperty", "name": "built", "isArray": false, "isOptional": true },
                            { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "doors", "isArray": false, "isOptional": false, "defaultValue": 4 },
                            {
                                "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
                                "name": "owner",
                                "isArray": false,
                                "isOptional": true,
                                "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Car" }
                            }
                        ]
                    }
                ]
            }),
            None,
        )
        .unwrap();
        mm
    }

    fn check(json: Value) -> Result<Value> {
        from_json(&manager(), &json, &FromJsonOptions::default(), &mut FixedEnv)
    }

    #[test]
    fn populates_a_datetime_a_relationship_and_a_default() {
        let value = check(json!({
            "$class": "org.acme@1.0.0.Car",
            "vin": "V1",
            "built": "2024-01-02T03:04:05Z",
            "owner": "resource:org.acme@1.0.0.Car#V2"
        }))
        .unwrap();
        assert_eq!(value["$class"], "org.acme@1.0.0.Car");
        assert_eq!(value["vin"], "V1");
        assert_eq!(value["doors"], 4);
        assert_eq!(value["built"][validate::DAYJS_TAG], "2024-01-02T03:04:05.000Z");
        assert_eq!(value["owner"][validate::RELATIONSHIP_TAG], true);
        assert_eq!(value["owner"]["vin"], "V2");
    }

    #[test]
    fn rejects_what_the_populator_rejects() {
        let err = check(json!({"$class": "org.acme@1.0.0.Car", "vin": "V1", "doors": "4"}))
            .unwrap_err();
        assert_eq!(err.code(), "jsonpopulator-converttoobject-wrongtype");
        let err = check(json!({"$class": "org.acme@1.0.0.Car", "vin": "V1", "built": 5}))
            .unwrap_err();
        assert_eq!(err.code(), "jsonpopulator-converttoobject-wrongtype");
        let err = check(json!({"$class": "org.acme@1.0.0.Car", "vin": " "})).unwrap_err();
        assert_eq!(err.code(), "factory-newinstance-missingidentifier");
        let err = check(json!({"vin": "V1"})).unwrap_err();
        assert_eq!(err.code(), "serializer-fromjson-noclass");
    }

    #[test]
    fn the_strict_options_reject_unknown_keys_and_required_nulls() {
        let strict = FromJsonOptions {
            reject_unknown_keys: true,
            reject_required_null: true,
            ..FromJsonOptions::default()
        };
        let mm = manager();
        let err = from_json(
            &mm,
            &json!({"$class": "org.acme@1.0.0.Car", "vin": "V1", "extra": null}),
            &strict,
            &mut FixedEnv,
        )
        .unwrap_err();
        assert_eq!(err.details()[0].code, DetailCode::UnknownProperty);
        assert_eq!(err.details()[0].path, "$.extra");
        let err = from_json(
            &mm,
            &json!({"$class": "org.acme@1.0.0.Car", "vin": "V1", "doors": null}),
            &strict,
            &mut FixedEnv,
        )
        .unwrap_err();
        assert_eq!(err.details()[0].code, DetailCode::TypeViolation);
        assert_eq!(err.details()[0].path, "$.doors");
    }
}
