//! `Serializer.fromJSON` over a plain JSON document (docs/public-api.md
//! sections 5.4 and 5.7, step 5): the checks and coercions of
//! `Serializer.fromJSON`, `Factory.newResource` and `JSONPopulator`
//! (`src/serializer.ts`, `src/factory.ts`, `src/serializer/jsonpopulator.ts`)
//! for a document that JSON can carry, without building the TS `Resource`
//! objects.
//!
//! The JS layer's serializer (`concerto_core_js::serializer`, in the
//! `concerto-core-js` crate) populates a JS `Resource` from any JS value,
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
use std::fmt::Write as _;

use serde_json::{Map, Value};

use super::dayjs::{Dayjs, UtcOffset};
use super::model::{self, Field, FieldType, RelationshipSlot, TypeRef};
use super::plan::{self, ClassPlan, Prepared};
use super::resource_id::ResourceId;
use super::validate::{self, ValidateOptions, js_map, js_number, js_undefined};
use crate::error::{ContractError, Detail, DetailCode, ErrorKind, Result};
use crate::introspect::Declaration;
use crate::model_manager::{DeclId, ModelManager, Node, ResolutionContext};
use crate::{Error, ecma, model_util};

/// What the TS `Factory` gets from its environment rather than from the
/// model (D7): a new identifier and the current time.
pub trait InstanceEnv {
    /// TS: `Factory.newId()`, `uuid.v4()`.
    fn new_id(&mut self) -> String;
    /// TS: the time `dayjs.utc()` reads, in ms since the epoch.
    fn now_ms(&mut self) -> f64;
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

/// The `Serializer.fromJSON` options this route reads, after the
/// serializer has merged them with its defaults.
#[derive(Debug, Clone, PartialEq)]
pub struct FromJsonOptions {
    /// `validate`: validate the populated instance
    /// (`ValidatedResource.validate`).
    pub validate: bool,
    /// `utcOffset || 0`: the offset a `DateTime` gets unless
    /// `strictQualifiedDateTimes` is `true`.
    pub utc_offset: UtcOffset,
    /// `strictQualifiedDateTimes === true`. Since P5-24 (BC-07, R1)
    /// every `DateTime` string must have the strict format either way;
    /// the flag only decides whether `utc_offset` is applied.
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

/// TS: `Serializer.fromJSON(json, options)` for a plain JSON `json`,
/// then, when `options.validate` is set, `ValidatedResource.validate`.
/// Returns the populated instance in the validator's value shape.
pub fn from_json(
    mm: &ModelManager,
    json: &Value,
    options: &FromJsonOptions,
    env: &mut dyn InstanceEnv,
) -> Result<Value> {
    with_document_class(json, |class_name| from_json_as(mm, json, class_name, options, env))
}

/// `fromJSON`'s read of the document's own `$class`, handed to `then`.
fn with_document_class<R>(json: &Value, then: impl FnOnce(&str) -> Result<R>) -> Result<R> {
    let class_name = get_property(Some(json), "$class")?;
    if !is_truthy(class_name.as_deref()) {
        return Err(Error::new(ErrorKind::InvalidArgument, "serializer-fromjson-noclass", Vec::new()));
    }
    // DV-015: see the JS layer's `Serializer::from_json`.
    let Some(Value::String(class_name)) = class_name.as_deref() else {
        return Err(not_a_string_class(class_name.as_deref()));
    };
    then(class_name)
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
    let populated = populate_as(mm, json, class_name, options, env)?;
    if options.validate {
        validate::validate_instance_from(
            mm,
            &populated.value,
            &options.validator,
            populated.root_identifier,
        )?;
        identifier_reads(mm, &populated.instances)?;
    }
    Ok(populated.value)
}

/// `ResourceValidator.visitClassDeclaration` writes each resource's
/// identifier back (the JS layer's `resource::sync_identifiers`), which
/// reads each one's identifying field; only that read can fail.
fn identifier_reads(mm: &ModelManager, instances: &[DeclId]) -> Result<()> {
    for id in instances {
        mm.identifier_field_of(*id)?;
    }
    Ok(())
}

/// What [`populate_as`] read.
struct Populated {
    /// The instance, in the validator's value shape.
    value: Value,
    /// Its `getFullyQualifiedIdentifier()`.
    root_identifier: String,
    /// The declaration of each resource built.
    instances: Vec<DeclId>,
}

/// [`from_json_as`] up to the validation: `Serializer.fromJSON` with a falsy
/// `validate`.
fn populate_as(
    mm: &ModelManager,
    json: &Value,
    class_name: &str,
    options: &FromJsonOptions,
    env: &mut dyn InstanceEnv,
) -> Result<Populated> {
    let class_declaration = model::get_type(mm, class_name)?;
    let ns = class_declaration.namespace();
    let name = class_declaration.name();
    let id = match class_declaration.identifier_field_name()? {
        Some(field) => get_property(Some(json), field)?,
        None => get_property(Some(json), "null")?,
    };
    let mut populator = Populator {
        mm,
        env,
        options,
        path: "$".to_string(),
        path_marks: Vec::new(),
        instances: Vec::new(),
    };
    let resource = if class_declaration.is_transaction() || class_declaration.is_event() {
        // `Factory.newTransaction`/`newEvent`: `ns` and `type` must be
        // truthy, then `newResource`, then the kind check.
        if ns.is_empty() {
            return Err(Error::new(ErrorKind::InvalidArgument, "factory-newtransaction-nsnotspecified", Vec::new()));
        }
        if name.is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "factory-newtransaction-typenotspecified",
                Vec::new(),
            ));
        }
        let resource = populator.new_resource(ns, name, id)?;
        let decl = model::get_type(mm, &resource.class_fqn)?;
        if class_declaration.is_transaction() && !decl.is_transaction() {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "factory-newtransaction-notatransaction",
                vec![("fqn", resource.class_fqn.clone())],
            ));
        }
        if class_declaration.is_event() && !decl.is_event() {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "factory-newevent-notanevent",
                vec![("fqn", resource.class_fqn.clone())],
            ));
        }
        resource
    } else if class_declaration.is_map_declaration() {
        return Err(Error::new(ErrorKind::InvalidArgument, "serializer-fromjson-mapnotsupported", Vec::new()));
    } else if class_declaration.is_enum() {
        return Err(Error::new(ErrorKind::InvalidArgument, "serializer-fromjson-enumnotsupported", Vec::new()));
    } else {
        // A concept, or any other class declaration:
        // `this.factory.newResource(ns, name, id)`.
        populator.new_resource_of(&class_declaration, id)?
    };

    let resource = populator.visit_class_declaration(&class_declaration, Some(json), resource)?;
    let root_identifier = resource.fully_qualified_identifier();
    let value = populator.finish(resource);
    Ok(Populated {
        value,
        root_identifier,
        instances: populator.instances,
    })
}

/// The collect-all read (P5-99, accordproject/concerto#1239): `json` read as
/// `Serializer.fromJSON` reads it (as its own `$class` when it has one, or
/// else as `fqn`), then the validation walk collecting every violation (or,
/// without `all`, the first), each with the JSON Pointer of the value it
/// was found at. The first is the error [`from_json`] with `validate`
/// throws. A document that cannot be read fails with the read's error.
pub(crate) fn collect_violations(
    mm: &ModelManager,
    json: &Value,
    fqn: Option<&str>,
    options: &FromJsonOptions,
    all: bool,
) -> Result<Vec<(String, Error)>> {
    let own_class = json.get("$class").filter(|c| ecma::is_truthy(c));
    let populated = match (own_class, fqn) {
        (None, Some(fqn)) => populate_as(mm, json, fqn, options, &mut FixedEnv)?,
        _ => with_document_class(json, |class_name| {
            populate_as(mm, json, class_name, options, &mut FixedEnv)
        })?,
    };
    let mut found = validate::collect_instance_violations(
        mm,
        &populated.value,
        &options.validator,
        populated.root_identifier,
        all,
    );
    if found.is_empty()
        && let Err(err) = identifier_reads(mm, &populated.instances)
    {
        found.push((String::new(), err));
    }
    Ok(found)
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
/// the integer-like keys first, in ascending order. Each key an object
/// holds is borrowed (P5-13).
fn object_keys(value: Option<&Value>) -> Result<Vec<Cow<'_, str>>> {
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
            let mut keys: Vec<Cow<'_, str>> = indices
                .into_iter()
                .map(|(_, k)| Cow::Borrowed(k.as_str()))
                .collect();
            let leading = keys.len();
            let rest: Vec<Cow<'_, str>> = map
                .keys()
                .filter(|k| !keys[..leading].iter().any(|seen| seen == k.as_str()))
                .map(|k| Cow::Borrowed(k.as_str()))
                .collect();
            keys.extend(rest);
            keys
        }
        Some(Value::String(s)) => (0..s.encode_utf16().count())
            .map(|i| Cow::Owned(i.to_string()))
            .collect(),
        Some(Value::Array(items)) => (0..items.len())
            .map(|i| Cow::Owned(i.to_string()))
            .collect(),
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
    check_new_resource_of(&class_decl, ns, type_name, id, new_id)
}

/// [`check_new_resource`] once its type lookup found `class_decl`
/// (P5-13): a caller that already holds the declaration of `ns` and
/// `type_name` need not look it up again.
pub fn check_new_resource_of(
    class_decl: &TypeRef,
    ns: &str,
    type_name: &str,
    id: IdentifierArg<'_>,
    new_id: &mut dyn FnMut() -> String,
) -> Result<ResourceCheck> {
    let ns_and_type = || {
        vec![
            ("namespace", ns.to_string()),
            ("type", type_name.to_string()),
        ]
    };

    if class_decl.is_abstract("classDecl.isAbstract")? {
        return Err(Error::new(ErrorKind::InvalidArgument, "factory-newinstance-abstracttype", ns_and_type()));
    }

    let id_field = class_decl.identifier_field_name()?;
    // `isSystemIdentified()`: the same inherited identifying field.
    let generated_id =
        (id_field == Some("$identifier") && id == IdentifierArg::Nullish).then(new_id);
    let id = match &generated_id {
        Some(generated) => IdentifierArg::String(generated),
        None => id,
    };
    if id_field.is_some() {
        let IdentifierArg::String(id_text) = id else {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "factory-newinstance-invalididentifier",
                ns_and_type(),
            ));
        };
        if ecma::js_trim(id_text).is_empty() {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "factory-newinstance-missingidentifier",
                ns_and_type(),
            ));
        }
        // `if (id)`: a non-empty string here. The regex validator comes
        // from the validation plan, built once (P5-88), as does the
        // error building it.
        let planned = plan::class_plan(class_decl.mm, class_decl.id)?;
        let regex = match &planned.id_regex {
            Prepared::Built(v) => Some(v),
            Prepared::None => None,
            Prepared::Failed(err) => return Err(err.clone()),
        };
        if let Some(regex) = regex
            && !regex.matches_regex(id_text)
        {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "factory-newresource-idregexmismatch",
                vec![("regex", regex.regex().unwrap_or_default())],
            ));
        }
    } else if matches!(id, IdentifierArg::String(s) if !s.is_empty())
        || matches!(id, IdentifierArg::Other { truthy: true })
    {
        return Err(Error::new(
            ErrorKind::InvalidArgument,
            "factory-newresource-notidentifiable",
            vec![("fqn", class_decl.fqn().to_string())],
        ));
    }

    Ok(ResourceCheck {
        class_fqn: class_decl.fqn().to_string(),
        identifier_field_name: id_field.map(str::to_string),
        generated_id,
        timestamped: class_decl.is_transaction() || class_decl.is_event(),
    })
}

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
    Ok(decl
        .identifier_field_name()?
        .filter(|f| !f.is_empty())
        .map(str::to_string))
}

/// A property default, converted by the field's type as
/// `Typed.assignFieldDefaults` converts it.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldDefault {
    /// An `Integer`, `Long` or `Double` default: `parseInt`/`parseFloat`
    /// of its text.
    Number(f64),
    /// A `Boolean` default: `default === true`.
    Bool(bool),
    /// A `DateTime` default: `dayjs.utc(default)`, of a strict
    /// `DateTime` string (BC-45).
    DateTime(Dayjs),
    /// P5-24 (BC-45, R1; accordproject/concerto-rust#328): a `DateTime`
    /// default that is not a strict `DateTime` string. It is not
    /// rejected at model load but when it is applied: the error to
    /// throw then, a `ValidationException`
    /// (`typed-assignfielddefaults-datetime`). Instance creation
    /// (`Factory.newResource`) always applies it; population
    /// (`fromJSON`) only when the document gives the field no value.
    InvalidDateTime(Error),
    /// A `String` or enum default, as it is in the AST.
    Json(Value),
}

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
    assign_field_defaults_of(&class_decl, assign)
}

/// [`assign_field_defaults`] for the declaration `class_fqn` names.
///
/// Every step but `assign` depends only on the model, so the converted
/// defaults are cached per declaration (P5-13,
/// `ModelManager::cached_field_defaults`). TS resolves each field's
/// type and assigns its default in one pass, so a field whose type does
/// not resolve fails only after every earlier default was assigned: the
/// list is cached only when every field resolves, and otherwise this
/// runs that same pass field by field, so the first error is TS's.
pub fn assign_field_defaults_of(
    class_decl: &TypeRef,
    assign: &mut dyn FnMut(&str, FieldDefault) -> Result<()>,
) -> Result<()> {
    let defaults = class_decl
        .mm
        .cached_field_defaults(class_decl.id, || field_defaults(class_decl, &mut |_, _| Ok(())));
    let Ok(defaults) = defaults else {
        field_defaults(class_decl, assign)?;
        return Ok(());
    };
    for (name, value) in &defaults.0 {
        assign(name, value.clone())?;
    }
    Ok(())
}

/// P5-24 (BC-45, R1): the fields of `class_decl` whose `DateTime`
/// default is not a strict `DateTime` string
/// ([`FieldDefault::InvalidDateTime`]), each with the error applying it
/// throws, in `getProperties()` order: read off the cached defaults, and
/// empty for almost every declaration (and when a field's type does not
/// resolve, which fails the instance's creation first).
pub fn invalid_date_time_defaults_of(class_decl: &TypeRef) -> Vec<(String, Error)> {
    let Ok(defaults) = class_decl
        .mm
        .cached_field_defaults(class_decl.id, || field_defaults(class_decl, &mut |_, _| Ok(())))
    else {
        return Vec::new();
    };
    defaults
        .0
        .iter()
        .filter_map(|(name, value)| match value {
            FieldDefault::InvalidDateTime(err) => Some((name.clone(), err.clone())),
            _ => None,
        })
        .collect()
}

/// The defaults [`assign_field_defaults_of`] caches for one declaration.
#[derive(Debug)]
pub(crate) struct FieldDefaults(Vec<(String, FieldDefault)>);

/// `assignFieldDefaults`' pass over `class_decl`'s fields: each converted
/// default is handed to `assign` as it is found, and also returned.
fn field_defaults(
    class_decl: &TypeRef,
    assign: &mut dyn FnMut(&str, FieldDefault) -> Result<()>,
) -> Result<FieldDefaults> {
    let mm = class_decl.mm;
    let mut defaults = Vec::new();
    for (owner_fqn, property) in class_decl
        .properties("classDeclaration.getProperties")?
        .iter()
    {
        // `isField?.()`: relationships are not `Field`s.
        if property.is_relationship() || property.is_enum_value() {
            continue;
        }
        let name = crate::Named::name(property);
        let field = model::field(mm, owner_fqn, property)?;
        let (default_value, type_name) = match &field.field_type {
            FieldType::Scalar {
                default_value,
                primitive,
                ..
            } => (*default_value, primitive.unwrap_or_default()),
            _ => (raw_default_value(mm, owner_fqn, name), field.type_name()),
        };
        let Some(default_value) = default_value.filter(|v| !v.is_null()) else {
            continue;
        };
        let value = match type_name {
            "Integer" | "Long" => {
                FieldDefault::Number(ecma::parse_int(&ecma::to_js_string(default_value)))
            }
            "Double" => FieldDefault::Number(ecma::parse_float(&ecma::to_js_string(default_value))),
            "Boolean" => FieldDefault::Bool(*default_value == Value::Bool(true)),
            // P5-24 (BC-45, R1; accordproject/concerto-rust#328): the
            // default must be a strict `DateTime` string, the rule a field
            // value follows, checked when it is applied (instance creation
            // or population), not at model load. TS builds
            // `dayjs.utc(default)` whatever it is.
            "DateTime" => strict_date_time_default(default_value, owner_fqn, name),
            // String, and "if we get this far the field should be an enum".
            _ => FieldDefault::Json(default_value.clone()),
        };
        assign(name, value.clone())?;
        defaults.push((name.to_string(), value));
    }
    Ok(FieldDefaults(defaults))
}

/// A `DateTime` default read with the strict rule (BC-45): a string that
/// [`Dayjs::utc_parse`] reads as a valid instant, or else
/// [`FieldDefault::InvalidDateTime`] with a `ValidationException` (the class
/// a strict `DateTime` field value's rejection has) naming the field. A
/// number, which `dayjs.utc(n)` used to read, is not a `DateTime` string
/// either.
fn strict_date_time_default(default_value: &Value, owner_fqn: &str, name: &str) -> FieldDefault {
    if let Some(parsed) = default_value
        .as_str()
        .map(Dayjs::utc_parse)
        .filter(Dayjs::is_valid)
    {
        return FieldDefault::DateTime(parsed);
    }
    FieldDefault::InvalidDateTime(ContractError::new(
        ErrorKind::Validation,
        "typed-assignfielddefaults-datetime",
        vec![
            ("value", ecma::to_js_string(default_value)),
            ("fqn", format!("{owner_fqn}.{name}")),
        ],
    )
    .into())
}

/// The raw AST `defaultValue` of a property of `owner_fqn`
/// (`Field.getDefaultValue()`, `null` when nullish), read off the AST as TS
/// does: the typed `DateTimeProperty` carries none.
fn raw_default_value<'a>(mm: &'a ModelManager, owner_fqn: &str, name: &str) -> Option<&'a Value> {
    let decl = mm.declaration_id(owner_fqn)?;
    let prop = mm.property_ids(decl).find(|id| {
        mm.property_by_id(*id)
            .is_some_and(|p| crate::Named::name(p) == name)
    })?;
    mm.property_default_value(prop)
}

// ---------------------------------------------------------------------
// The instance being populated
// ---------------------------------------------------------------------

/// A `ValidatedResource` being populated: its own properties in insertion
/// order, each in the validator's value shape, without the private ones the
/// validator never reads (`$namespace`, `$type`, `$identifierFieldName`).
struct Resource {
    /// Its declaration, for the identifier write-back.
    decl: DeclId,
    class_fqn: String,
    /// `this.$identifierFieldName`.
    identifier_key: String,
    props: Map<String, Value>,
    /// P5-24 (BC-45): each property whose `DateTime` default is not strict,
    /// with the error applying it throws. Population throws it only when
    /// the document gives the property no value, so the default stays
    /// ([`Populator::visit_class_declaration`]).
    invalid_defaults: Vec<(String, Error)>,
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
    /// `parameters.path`, a `TypedStack` that starts as `['$']`, kept
    /// joined (P5-13): what `path.stack.join('')` reads, with
    /// [`Self::path_marks`] recording where each pushed segment starts.
    path: String,
    path_marks: Vec<usize>,
    /// The declaration of each resource built, for the identifier
    /// write-back.
    instances: Vec<DeclId>,
}

impl Populator<'_> {
    /// `parameters.path.stack.join('')`.
    fn path_text(&self) -> String {
        self.path.clone()
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

    /// A finished resource, in the validator's value shape.
    fn finish(&mut self, resource: Resource) -> Value {
        self.instances.push(resource.decl);
        let mut wire = Map::with_capacity(resource.props.len() + 1);
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
        let class_decl = model::get_type(self.mm, &model_util::qualify(ns, type_name))?;
        self.new_resource_at(&class_decl, ns, type_name, id)
    }

    /// [`Self::new_resource`] for a declaration already found (P5-13):
    /// `newResource(decl.getNamespace(), decl.getName(), id)`, whose own
    /// type lookup finds `decl` again.
    fn new_resource_of(&mut self, decl: &TypeRef, id: Js) -> Result<Resource> {
        self.new_resource_at(decl, decl.namespace(), decl.name(), id)
    }

    /// [`Self::new_resource`] once its type lookup found `class_decl`.
    fn new_resource_at(
        &mut self,
        class_decl: &TypeRef,
        ns: &str,
        type_name: &str,
        id: Js,
    ) -> Result<Resource> {
        let env = &mut *self.env;
        let check = check_new_resource_of(
            class_decl,
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
        // `identifiable_field_name`: `getModelFile(ns).getType(fqn)` is
        // `class_decl`.
        let identifier_key = class_decl
            .identifier_field_name()?
            .filter(|f| !f.is_empty())
            .unwrap_or("$identifier")
            .to_string();
        let mut props = Map::new();
        props.insert("$identifier".to_string(), id.clone());
        props.insert(identifier_key.clone(), id.clone());
        props.insert("$timestamp".to_string(), timestamp);
        let mut resource = Resource {
            decl: class_decl.id,
            class_fqn: check.class_fqn,
            identifier_key,
            props,
            invalid_defaults: Vec::new(),
        };
        let mm = self.mm;
        // The validation plan (P5-88), for each default's check (B-5,
        // P5-99), found at the first default. `check_new_resource_of` has
        // read the declaration's identifier field, so its chain resolves.
        let mut class_plan: Option<std::sync::Arc<ClassPlan>> = None;
        assign_field_defaults_of(class_decl, &mut |name, value| {
            let value = match value {
                FieldDefault::Number(n) => js_number(n),
                FieldDefault::Bool(b) => Value::Bool(b),
                FieldDefault::DateTime(d) => d.validator_value(),
                // BC-45: held until the document's own value is known. The
                // property keeps its place with an invalid date, what TS's
                // `dayjs.utc(default)` gave, so a value the document gives
                // later lands where TS puts it.
                FieldDefault::InvalidDateTime(err) => {
                    resource.invalid_defaults.push((name.to_string(), err));
                    Dayjs::utc_invalid().validator_value()
                }
                FieldDefault::Json(v) => plain(Some(&v)),
            };
            let class_plan = match &class_plan {
                Some(found) => found,
                None => class_plan.insert(plan::class_plan(mm, class_decl.id)?),
            };
            set_property_value(mm, class_plan, &mut resource, name, value)
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
        // `visit`'s `Unrecognised` fallthrough: a scalar declaration (BC-08).
        Err(model::unrecognised(declaration.fqn()))
    }

    /// TS: JSONPopulator.visitClassDeclaration.
    fn visit_class_declaration(
        &mut self,
        class_declaration: &TypeRef,
        json: Option<&Value>,
        mut resource: Resource,
    ) -> Result<Resource> {
        let properties = get_assignable_properties(json, class_declaration)?;
        // `classDeclaration.getProperties()` (and each `getProperty` below)
        // from the validation plan (P5-88, P5-99): the same answer every
        // time, and its chain's error first, as `getProperties()` raises it.
        let class_plan = plan::class_plan(self.mm, class_declaration.id)?;
        if self.options.reject_unknown_keys {
            self.reject_unknown_keys(json, class_declaration, &class_plan)?;
        }
        validate_properties(&properties, class_declaration, &class_plan)?;
        if self.options.reject_required_null {
            self.reject_required_null(json, &class_plan)?;
        }
        for property in properties {
            let value = get_property(json, &property)?;
            if value.as_deref() != Some(&Value::Null) {
                self.push_path(format_args!(".{property}"));
                let index = class_plan
                    .find(&property)
                    .expect("validateProperties found every property");
                let field = class_plan.field(self.mm, index)?;
                let populated = self.visit_property(&field, value.as_deref())?;
                resource
                    .invalid_defaults
                    .retain(|(name, _)| *name != *property);
                resource.props.insert(property.into_owned(), populated);
                self.pop_path();
            }
        }
        // BC-45: a non-strict `DateTime` default the document did not
        // replace is applied, so it throws.
        if let Some((_, err)) = resource.invalid_defaults.drain(..).next() {
            return Err(err);
        }
        Ok(resource)
    }

    /// `rejectUnknownKeys` (accordproject/concerto#1273): every key that is
    /// not a system property and that the declaration does not declare,
    /// whatever its value (`null` included), in one error with one
    /// `UNKNOWN_PROPERTY` detail per key.
    fn reject_unknown_keys(
        &self,
        json: Option<&Value>,
        class_declaration: &TypeRef,
        class_plan: &ClassPlan,
    ) -> Result<()> {
        let unknown: Vec<String> = object_keys(json)?
            .into_iter()
            .filter(|p| !model_util::is_system_property(p) && !class_plan.contains(p))
            .map(Cow::into_owned)
            .collect();
        if unknown.is_empty() {
            return Ok(());
        }
        Err(unknown_keys_error(
            class_declaration.fqn(),
            &self.path_text(),
            &unknown,
        ))
    }

    /// `rejectRequiredNull` (accordproject/concerto#1273): the first
    /// declared, required property (in the document's key order) whose value
    /// is `null` fails at once with its path and declared type, and a
    /// `TYPE_VIOLATION` detail.
    fn reject_required_null(&self, json: Option<&Value>, class_plan: &ClassPlan) -> Result<()> {
        for key in object_keys(json)? {
            if model_util::is_system_property(&key)
                || get_property(json, &key)?.as_deref() != Some(&Value::Null)
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
            return Err(required_null_error(&self.path_text(), &key, property));
        }
        Ok(())
    }

    /// TS `classProperty.accept(this, parameters)`, through `visit`: a
    /// relationship to `visitRelationshipDeclaration`, a scalar field to
    /// `visitField(thing.getScalarField())`, any other field to `visitField`.
    fn visit_property(&mut self, field: &Field, json: Option<&Value>) -> Result<Value> {
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
        json: Option<&Value>,
    ) -> Result<Value> {
        // Throws if the map holds reserved properties.
        get_assignable_properties(json, map_declaration)?;
        let Declaration::Map(map) = map_declaration.decl else {
            unreachable!("visit_map_declaration is only reached for a map");
        };
        let key_type = map.key_type_name().to_string();
        let value_type = map.value_type_name().to_string();
        // P5-58 (BC-05, R1; DV-007): a relationship-typed value is read as
        // a relationship property is, not as an embedded concept. Its
        // target type, and its slot, are resolved once (B-5, P5-99); an
        // error resolving them is raised at the first value, as TS resolves
        // it there.
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
        // of its own: the same for every entry, so resolved once (B-5).
        let mut key_declaration = None;
        let mut value_declaration = None;
        let mut result: Vec<(Value, Value)> = Vec::new();
        // `new Map(Object.entries(jsonObj))`. The keys are an object's, so
        // each is new, and each `map.set` appends (B-4, P5-99): a key or
        // value's `processMapType` hands a primitive or scalar key back as
        // it is.
        for key in object_keys(json)? {
            let value = get_property(json, &key)?;
            if key == "$class" {
                result.push((Value::String(key.into_owned()), plain(value.as_deref())));
                continue;
            }
            let key_json = Value::String(key.into_owned());
            let key = if model_util::is_primitive_type(&key_type) {
                key_json
            } else {
                self.process_map_type(
                    map_declaration,
                    Some(&key_json),
                    &key_type,
                    &mut key_declaration,
                )?
            };
            let value = match &relationship {
                Some(resolved) => {
                    let (_, default_namespace, default_type) =
                        resolved.as_ref().map_err(Clone::clone)?;
                    let slot = slot.as_ref().expect("resolved with the target");
                    self.convert_relationship(slot, default_namespace, default_type, value.as_deref())?
                }
                None if model_util::is_primitive_type(&value_type) => plain(value.as_deref()),
                None => self.process_map_type(
                    map_declaration,
                    value.as_deref(),
                    &value_type,
                    &mut value_declaration,
                )?,
            };
            result.push((key, value));
        }
        Ok(js_map(result))
    }

    /// TS: JSONPopulator.processMapType. `declaration` holds the
    /// declaration `type_name` names in the map's model file, resolved at
    /// the first entry that needs it (B-5, P5-99).
    fn process_map_type<'d>(
        &mut self,
        map_declaration: &TypeRef<'d>,
        value: Option<&Value>,
        type_name: &str,
        declaration: &mut Option<Option<TypeRef<'d>>>,
    ) -> Result<Value> {
        let namespace = map_declaration.namespace();
        let mm = map_declaration.mm;
        // `try { ... } catch (err) { decl = undefined; }`
        let class_name = match value {
            Some(Value::Object(_) | Value::Array(_)) => get_property(value, "$class")
                .ok()
                .flatten()
                .filter(|c| ecma::is_truthy(c)),
            _ => None,
        };
        let found = match class_name.as_deref() {
            Some(Value::String(s)) => model::get_type(mm, s).ok(),
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
            let id = declaration
                .identifier_field_name()?
                .map(|name| Value::String(name.to_string()));
            let id = Some(Cow::Owned(id.unwrap_or(Value::Null)));
            let sub_resource = self.new_resource_of(&declaration, id)?;
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
                return Err(Error::new(
                    ErrorKind::Validation,
                    "jsonpopulator-visitfield-notarray",
                    vec![
                        ("path", self.path_text()),
                        ("type", field.type_name().to_string()),
                    ],
                ));
            };
            let mut result = Vec::with_capacity(items.len());
            for (n, item) in items.iter().enumerate() {
                self.push_path(format_args!("[{n}]"));
                result.push(self.convert_item(field, Some(item))?);
                self.pop_path();
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
                Some(Value::String(s)) => s.as_str(),
                // DV-015: see the JS layer's `Serializer::from_json`.
                other => return Err(not_a_string_class(other)),
            }
        } else {
            field.fully_qualified_type_name()
        };
        let declaration = model::get_type(self.mm, type_name)?;
        let sub_resource = if declaration.is_map_declaration() {
            None
        } else if let Some(id_field) = declaration.identifier_field_name()? {
            // `isIdentified()`, then `getIdentifierFieldName()`.
            let id = get_property(json_item, id_field)?;
            Some(self.new_resource_of(&declaration, id)?)
        } else {
            Some(self.new_resource_of(&declaration, None)?)
        };
        self.accept_declaration(&declaration, json_item, sub_resource)
    }

    /// TS: JSONPopulator.convertToObject: the primitive-type switch.
    fn convert_to_object(&mut self, field: &Field, json: Option<&Value>) -> Result<Value> {
        let type_name = field.type_name();
        let path = self.path.as_str();
        let wrong_type = || {
            Error::new(
                ErrorKind::Validation,
                "jsonpopulator-converttoobject-wrongtype",
                vec![("path", path.to_string()), ("type", type_name.to_string())],
            )
        };
        Ok(match type_name {
            "DateTime" => {
                let Some(Value::String(s)) = json else {
                    return Err(wrong_type());
                };
                // P5-24 (BC-07, R1): only the strict format, whatever
                // `strictQualifiedDateTimes` says; the flag now decides
                // only whether `utcOffset` applies, as it did before.
                if !strict_qualified_date_time(s) {
                    return Err(Error::new(
                        ErrorKind::Validation,
                        "jsonpopulator-converttoobject-datetimeformat",
                        vec![("path", path.to_string()), ("type", type_name.to_string())],
                    ));
                }
                let parsed = Dayjs::utc_parse(s);
                let result = if self.options.strict_qualified_date_times {
                    parsed
                } else {
                    parsed.utc_offset_set(&self.options.utc_offset)
                };
                if !result.is_valid() {
                    return Err(wrong_type());
                }
                result.validator_value()
            }
            "Integer" | "Long" => match json {
                // P5-51 (BC-10, R1; DV-012): an integral, finite number.
                // A serde_json number is always finite; the check says so.
                Some(Value::Number(n))
                    if n.as_f64().is_some_and(|n| n.is_finite() && n.trunc() == n) =>
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
        let slot = relationship
            .relationship_slot()
            .expect("visit_relationship_declaration is only reached for a relationship");
        let (default_namespace, default_type) = relationship_defaults(&slot)?;

        if slot.is_array {
            let Some(Value::Array(items)) = json else {
                return Err(Error::new(
                    ErrorKind::Validation,
                    "jsonpopulator-visitfield-notarray",
                    vec![
                        ("path", self.path_text()),
                        ("type", relationship.type_name().to_string()),
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
                    result.push(self.relationship_resource(&slot, json, Some(item))?);
                }
            }
            Ok(Value::Array(result))
        } else {
            self.convert_relationship(&slot, &default_namespace, &default_type, json)
        }
    }

    /// One relationship value, `visitRelationshipDeclaration`'s non-array
    /// branch: a URI string becomes a relationship, and an object an
    /// embedded resource when `acceptResourcesForRelationships` allows it.
    /// A relationship-typed map value goes through here too (P5-58, BC-05).
    fn convert_relationship(
        &mut self,
        slot: &RelationshipSlot,
        default_namespace: &str,
        default_type: &str,
        json: Option<&Value>,
    ) -> Result<Value> {
        match json {
            Some(Value::String(uri)) => {
                relationship_from_uri(self.mm, uri, default_namespace, default_type)
            }
            Some(Value::Object(_) | Value::Array(_)) => self.relationship_resource(slot, json, json),
            _ => Err(Error::new(
                ErrorKind::InvalidArgument,
                "jsonpopulator-visitrelationshipdeclaration-notstringorobject",
                vec![
                    ("value", js_string(json)),
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
        json: Option<&Value>,
        item: Option<&Value>,
    ) -> Result<Value> {
        if !self.options.accept_resources_for_relationships {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "jsonpopulator-visitrelationshipdeclaration-notastring",
                vec![
                    ("value", js_string(json)),
                    ("relationship", slot.relationship_to_string()),
                ],
            ));
        }
        let class_name = get_property(item, "$class")?;
        if !is_truthy(class_name.as_deref()) {
            return Err(Error::new(
                ErrorKind::InvalidArgument,
                "jsonpopulator-visitrelationshipdeclaration-noclass",
                vec![
                    ("value", js_string(item)),
                    ("relationship", slot.relationship_to_string()),
                ],
            ));
        }
        // DV-015: see the JS layer's `Serializer::from_json`.
        let Some(Value::String(class_name)) = class_name.as_deref() else {
            return Err(not_a_string_class(class_name.as_deref()));
        };
        let class_declaration = model::get_type(self.mm, class_name)?;
        let id = match class_declaration.identifier_field_name()? {
            Some(field) => get_property(item, field)?,
            None => get_property(item, "null")?,
        };
        let sub_resource = self.new_resource_of(&class_declaration, id)?;
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
    Ok((default_namespace, model_util::short_name(type_fqn).to_string()))
}

/// The `rejectUnknownKeys` rejection (accordproject/concerto#1273): the
/// keys of the object at `path` that `fqn` does not declare, in one
/// `ValidationException` with one `UNKNOWN_PROPERTY` detail per key.
pub fn unknown_keys_error(fqn: &str, path: &str, unknown: &[String]) -> Error {
    let mut error = ContractError::new(
        ErrorKind::Validation,
        "jsonpopulator-rejectunknownkeys-unknownproperties",
        vec![("fqn", fqn.to_string()), ("properties", unknown.join(", "))],
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
    error.into()
}

/// The `rejectRequiredNull` rejection (accordproject/concerto#1273): the
/// required `property`, set to `null` under the key `key` of the object
/// at `path`, with its path and declared type and a `TYPE_VIOLATION`
/// detail.
pub fn required_null_error(path: &str, key: &str, property: &crate::Property) -> Error {
    let path = format!("{path}.{key}");
    let mut type_name = crate::introspect::Typed::type_name(property)
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
    error.into()
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
    let class_fqn = class_decl.fqn().to_string();
    let identifier_key = identifiable_field_name(mm, &resource_id.namespace, &class_fqn)?
        .unwrap_or_else(|| "$identifier".to_string());
    Ok(relationship_value(class_fqn, identifier_key, resource_id.id))
}

/// TS `ValidatedResource.setPropertyValue(propName, value)`: the value is
/// validated against the property before it is assigned, over the
/// declaration's validation plan (B-5, P5-99).
fn set_property_value(
    mm: &ModelManager,
    class_plan: &ClassPlan,
    resource: &mut Resource,
    prop_name: &str,
    value: Value,
) -> Result<()> {
    let Some(index) = class_plan.find(prop_name) else {
        let id = resource
            .props
            .get(&resource.identifier_key)
            .filter(|v| !validate::is_js_undefined(v))
            .map_or_else(|| "undefined".to_string(), ecma::to_js_string);
        return Err(Error::new(
            ErrorKind::InvalidArgument,
            "validatedresource-setpropertyvalue-undeclaredfield",
            vec![("id", id), ("propName", prop_name.to_string())],
        ));
    };
    validate::validate_property_value(
        mm,
        class_plan,
        index,
        &value,
        resource.fully_qualified_identifier(),
        &ValidateOptions::default(),
    )?;
    resource.props.insert(prop_name.to_string(), value);
    Ok(())
}

/// TS `getAssignableProperties(resourceData, classDeclaration)`: the keys
/// that have a value and are not system properties, after the reserved
/// property and `$timestamp` checks.
fn get_assignable_properties<'v>(
    resource_data: Option<&'v Value>,
    declaration: &TypeRef,
) -> Result<Vec<Cow<'v, str>>> {
    let properties = object_keys(resource_data)?;
    let private: Vec<&str> = properties
        .iter()
        .filter(|p| model_util::is_private_system_property(p))
        .map(|p| &**p)
        .collect();
    if !private.is_empty() {
        return Err(Error::new(
            ErrorKind::Validation,
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
        return Err(Error::new(
            ErrorKind::Validation,
            "jsonpopulator-getassignableproperties-timestamp",
            vec![("fqn", declaration.fqn().to_string())],
        ));
    }
    let mut assignable = Vec::with_capacity(properties.len());
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

/// TS `validateProperties(properties, classDeclaration)`, against the
/// declaration's `getProperties()` (the validation plan's name index, P5-88).
fn validate_properties(
    properties: &[Cow<'_, str>],
    class_declaration: &TypeRef,
    class_plan: &ClassPlan,
) -> Result<()> {
    if properties.iter().all(|p| class_plan.contains(p)) {
        return Ok(());
    }
    let invalid: Vec<&str> = properties
        .iter()
        .filter(|p| !class_plan.contains(p))
        .map(|p| &**p)
        .collect();
    Err(Error::new(
        ErrorKind::Validation,
        "jsonpopulator-validateproperties-unexpectedproperties",
        vec![
            ("fqn", class_declaration.fqn().to_string()),
            ("properties", invalid.join(", ")),
        ],
    ))
}

/// `json.match(/^((?:(\d{4}-\d{2}-\d{2})T(\d{2}:\d{2}:\d{2}(?:\.\d+)?))(Z|[+-]\d{2}:\d{2}))$/)`:
/// the `strictQualifiedDateTimes` format, the only `DateTime` string
/// form accepted (P5-24, BC-07). A string with this format can still
/// name an impossible instant ([`Dayjs::utc_parse`] is then invalid).
pub fn strict_qualified_date_time(s: &str) -> bool {
    super::dayjs::is_strict_date_time_format(s)
}

#[cfg(test)]
mod tests;
