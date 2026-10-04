//! The in-memory instances the serializer builds and reads: a port of the
//! state of `Typed`, `Identifiable`, `Resource`, `ValidatedResource` and
//! `Relationship` (`src/model/*.ts`), and of the JS values their fields
//! hold.
//!
//! D7 keeps these objects in TS: on the WASM path the TS classes stay the
//! user-visible objects, and Rust only builds or reads their state in one
//! call (the Serializer fast path, PORTING.md section 5 row 6). This module
//! is that state, as the populator (`concerto_core_js::populator`) produces it and
//! the generator (`concerto_core_js::generator`) and the validator
//! ([`concerto_core::instance::validate`]) consume it. It holds no model data: an instance
//! names its declaration by fully-qualified name, and every operation that
//! needs the model takes the [`ModelManager`](concerto_core::ModelManager).
//!
//! An [`Instance`] keeps every own property of the TS object in insertion
//! order (`$namespace`, `$type`, `$identifierFieldName`, `$identifier`, the
//! identifying field, `$timestamp`, `$class` for a relationship, then the
//! fields), except the three handles `$modelManager`, `$classDeclaration`
//! and `$validator`, which it keeps as the declaration's name and the
//! validator's options. That is the order `Object.getOwnPropertyNames`
//! reports, which `ResourceValidator` walks (first undeclared field wins).

use concerto_core::hash::SeededState;
use concerto_core::json::Value;
use indexmap::IndexMap;

/// The key-ordered map behind a plain object, an instance's own properties
/// and the serializer options: insertion (`Object.keys`) order. Its keys
/// come from user JSON, so it hashes with secret-keyed SipHash
/// ([`SeededState`]) against key-collision DoS (tests/hashdos.rs;
/// PORTING.md 3.7).
pub type JsObject = IndexMap<String, JsValue, SeededState>;

use concerto_core::error::Result;
use concerto_core::instance::dayjs::Dayjs;
use concerto_core::instance::resource_id::ResourceId;
use concerto_core::instance::validate::{
    BIGINT_TAG, DAYJS_TAG, MAP_TAG, NUMBER_TAG, RELATIONSHIP_TAG, UNDEFINED_TAG, ValidateOptions,
    ValidatorInput, ValidatorObject, js_bigint, js_map, js_number, js_number_to_string,
    js_undefined,
};

/// A JS value held by an instance field or passed to the serializer.
#[derive(Debug, Clone, PartialEq)]
pub enum JsValue {
    /// JS `undefined`: an absent value, distinct from `null`.
    Undefined,
    /// JS `null`.
    Null,
    /// A JS boolean.
    Bool(bool),
    /// A JS number (an IEEE double, PORTING.md 3.1).
    Number(f64),
    /// A JS string.
    String(String),
    /// A JS array, in index order.
    Array(Vec<JsValue>),
    /// A plain object: its own enumerable properties, in `Object.keys`
    /// order.
    Object(JsObject),
    /// A JS `Map` (a populated `MapDeclaration` value), in insertion order.
    Map(Vec<(JsValue, JsValue)>),
    /// A dayjs object.
    DateTime(Dayjs),
    /// A `Resource`, `ValidatedResource` or `Relationship`.
    Instance(Box<Instance>),
    /// A JS `BigInt`, as its decimal digit string (`toString()`'s
    /// spelling). Not produced by `JSONPopulator` (JSON has no bigint
    /// literal); it reaches an instance only by direct field assignment,
    /// as `Resource.setPropertyValue` allows.
    BigInt(String),
}

/// Which TS class an [`Instance`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceKind {
    /// A TS `Resource`: a concept, asset, participant, transaction or event
    /// instance.
    Resource,
    /// A TS `ValidatedResource`, a `Resource` whose every field assignment
    /// is validated.
    ValidatedResource,
    /// A TS `Relationship`: a reference to an identified resource.
    Relationship,
}

impl InstanceKind {
    /// The TS constructor name.
    pub fn ctor(self) -> &'static str {
        match self {
            Self::Resource => "Resource",
            Self::ValidatedResource => "ValidatedResource",
            Self::Relationship => "Relationship",
        }
    }
}

/// A `Resource`, `ValidatedResource` or `Relationship` (module doc).
#[derive(Debug, Clone, PartialEq)]
pub struct Instance {
    /// The TS class.
    pub kind: InstanceKind,
    /// `$classDeclaration.getFullyQualifiedName()`, which is what TS
    /// `getFullyQualifiedType()` answers.
    pub class_fqn: String,
    /// Every own property other than the three handles, in insertion order.
    pub props: JsObject,
    /// `$validator.options`, for a `ValidatedResource`.
    pub validator_options: ValidateOptions,
}

/// `undefined`, for a property that is not there.
static UNDEFINED: JsValue = JsValue::Undefined;

/// The own properties of a `Resource` that the validator never sees
/// ([`Instance::to_validator_value`] leaves them out).
const PRIVATE_ONLY_KEYS: [&str; 9] = [
    "$modelManager",
    "$classDeclaration",
    "$namespace",
    "$type",
    "$identifierFieldName",
    "$validator",
    "$imports",
    "$superTypes",
    "$id",
];

/// Whether `key` is one of [`PRIVATE_ONLY_KEYS`].
fn is_private_only(key: &str) -> bool {
    key.starts_with('$') && PRIVATE_ONLY_KEYS.contains(&key)
}

impl Instance {
    /// TS: the `Identifiable` constructor (`src/model/identifiable.ts`),
    /// with `Typed`'s before it and, for a `Relationship`, its own after it.
    /// `identifier_field_name` is `typeDeclaration?.getIdentifierFieldName()`
    /// of the instance's own type (the caller resolves it; `None` gives
    /// `'$identifier'`).
    pub fn new(
        kind: InstanceKind,
        class_fqn: impl Into<String>,
        namespace: &str,
        type_name: &str,
        identifier_field_name: Option<String>,
        id: JsValue,
        timestamp: JsValue,
    ) -> Self {
        let identifier_field_name =
            identifier_field_name.unwrap_or_else(|| "$identifier".to_string());
        let mut instance = Self {
            kind,
            class_fqn: class_fqn.into(),
            // The system properties below, then a few fields (sized up
            // front rather than grown).
            props: JsObject::with_capacity_and_hasher(8, Default::default()),
            validator_options: ValidateOptions::default(),
        };
        instance.set("$namespace", JsValue::String(namespace.to_string()));
        instance.set("$type", JsValue::String(type_name.to_string()));
        instance.set(
            "$identifierFieldName",
            JsValue::String(identifier_field_name.clone()),
        );
        // `setIdentifier(id)`, whose `this.$identifierFieldName` is the
        // name just set.
        instance.set("$identifier", id.clone());
        instance.set(&identifier_field_name, id);
        instance.set("$timestamp", timestamp);
        if kind == InstanceKind::Relationship {
            // `this.$class = 'Relationship'`
            instance.set("$class", JsValue::String("Relationship".to_string()));
        }
        instance
    }

    /// An empty stand-in, for while the instance itself is moved out
    /// ([`crate::resource::validate`]). Allocates nothing.
    pub(crate) fn placeholder() -> Self {
        Self {
            kind: InstanceKind::Resource,
            class_fqn: String::new(),
            props: JsObject::default(),
            validator_options: ValidateOptions::default(),
        }
    }

    /// `this[key]`.
    pub fn get(&self, key: &str) -> &JsValue {
        self.props.get(key).unwrap_or(&UNDEFINED)
    }

    /// `this[key] = value`: a new key goes last, an existing one keeps its
    /// place.
    pub fn set(&mut self, key: &str, value: JsValue) {
        // `IndexMap::insert` keeps an existing key where it is and replaces
        // its value, so one hashed insert does both cases (the lookup first
        // hashed every new key twice).
        self.props.insert(key.to_string(), value);
    }

    /// `this.$namespace` (TS `getNamespace()`), as JS `ToString`.
    pub fn namespace(&self) -> String {
        self.get("$namespace").to_js_string()
    }

    /// `this.$type` (TS `getType()`), as JS `ToString`.
    pub fn type_name(&self) -> String {
        self.get("$type").to_js_string()
    }

    /// `this.$identifierFieldName`, as JS `ToString`.
    pub fn identifier_field_name(&self) -> String {
        self.identifier_field_name_ref().into_owned()
    }

    /// [`Instance::identifier_field_name`], borrowed when it is a string.
    fn identifier_field_name_ref(&self) -> std::borrow::Cow<'_, str> {
        match self.get("$identifierFieldName") {
            JsValue::String(name) => std::borrow::Cow::Borrowed(name),
            other => std::borrow::Cow::Owned(other.to_js_string()),
        }
    }

    /// TS `Identifiable.getIdentifier`: `this[this.$identifierFieldName]`.
    pub fn get_identifier(&self) -> &JsValue {
        self.get(&self.identifier_field_name_ref())
    }

    /// TS `Identifiable.setIdentifier`: `this.$identifier = id;
    /// this[this.$identifierFieldName] = id`.
    pub fn set_identifier(&mut self, id: JsValue) {
        self.set("$identifier", id.clone());
        let field = self.identifier_field_name();
        self.set(&field, id);
    }

    /// TS `Identifiable.getFullyQualifiedIdentifier`.
    pub fn fully_qualified_identifier(&self) -> String {
        let id = self.get_identifier();
        if let JsValue::String(id) = id {
            // The usual case, built without the formatting
            // machinery: a non-empty string is truthy and is its own
            // `ToString`.
            if id.is_empty() {
                return self.class_fqn.clone();
            }
            let mut out = String::with_capacity(self.class_fqn.len() + 1 + id.len());
            out.push_str(&self.class_fqn);
            out.push('#');
            out.push_str(id);
            return out;
        }
        if id.is_truthy() {
            format!("{}#{}", self.class_fqn, id.to_js_string())
        } else {
            self.class_fqn.clone()
        }
    }

    /// TS `toString()`: `Resource.toString` for a `Resource` or a
    /// `ValidatedResource`, `Relationship.toString` for a relationship.
    pub fn to_js_string(&self) -> String {
        let ctor = match self.kind {
            InstanceKind::Relationship => "Relationship",
            _ => "Resource",
        };
        format!("{ctor} {{id={}}}", self.fully_qualified_identifier())
    }

    /// TS `Identifiable.toURI`: `new ResourceId(this.getNamespace(),
    /// this.getType(), this.getIdentifier()).toURI()`.
    pub fn to_uri(&self) -> Result<String> {
        // The constructor's `if (!id)` check, then `encodeURI(String(id))`.
        let id = self.get_identifier();
        let id = if id.is_truthy() {
            id.to_js_string()
        } else {
            String::new()
        };
        Ok(ResourceId::new(self.namespace(), self.type_name(), id)?.to_uri())
    }

    /// This instance in the value shape [`concerto_core::instance::validate::validate_instance`]
    /// reads (its module doc, "Scope"): a `Relationship` as a
    /// [`RELATIONSHIP_TAG`]-tagged `{$class, <identifying field>}` object,
    /// anything else as a `$class`-tagged object with every own property
    /// but the private ones.
    pub fn to_validator_value(&self) -> Value {
        if self.kind == InstanceKind::Relationship {
            let mut wire = concerto_core::json::Map::new();
            wire.insert(RELATIONSHIP_TAG.to_string(), Value::Bool(true));
            wire.insert("$class".to_string(), Value::String(self.class_fqn.clone()));
            let field = self.identifier_field_name();
            if let JsValue::String(id) = self.get(&field) {
                wire.insert(field, Value::String(id.clone()));
            }
            return Value::Object(wire);
        }
        let mut wire = concerto_core::json::Map::with_capacity(self.props.len() + 1);
        wire.insert("$class".to_string(), Value::String(self.class_fqn.clone()));
        for (key, value) in &self.props {
            if is_private_only(key) {
                continue;
            }
            wire.insert(key.clone(), value.to_validator_value());
        }
        Value::Object(wire)
    }
}

impl JsValue {
    /// JS truthiness (`!!value`).
    pub fn is_truthy(&self) -> bool {
        match self {
            Self::Undefined | Self::Null => false,
            Self::Bool(b) => *b,
            Self::Number(n) => *n != 0.0 && !n.is_nan(),
            Self::String(s) => !s.is_empty(),
            // `!!0n === false`; `BigInt.prototype.toString()` never spells
            // zero any other way (no `-0n`).
            Self::BigInt(s) => s != "0",
            _ => true,
        }
    }

    /// `Util.isNull`: `undefined` or `null`.
    pub fn is_nullish(&self) -> bool {
        matches!(self, Self::Undefined | Self::Null)
    }

    /// `typeof value`.
    pub fn type_of(&self) -> &'static str {
        match self {
            Self::Undefined => "undefined",
            Self::Bool(_) => "boolean",
            Self::Number(_) => "number",
            Self::String(_) => "string",
            Self::BigInt(_) => "bigint",
            _ => "object",
        }
    }

    /// The string, when this is one.
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }

    /// ECMAScript `ToString`, as `'' + value` and template literals apply
    /// it (a `Resource` through its own `toString`, a dayjs through
    /// `toUTCString`).
    pub fn to_js_string(&self) -> String {
        match self {
            Self::Undefined => "undefined".to_string(),
            Self::Null => "null".to_string(),
            Self::Bool(b) => b.to_string(),
            Self::Number(n) => js_number_to_string(*n),
            Self::String(s) => s.clone(),
            Self::Array(items) => items
                .iter()
                .map(|item| {
                    if item.is_nullish() {
                        String::new()
                    } else {
                        item.to_js_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(","),
            Self::Object(_) => "[object Object]".to_string(),
            Self::Map(_) => "[object Map]".to_string(),
            Self::DateTime(d) => d.to_js_string(),
            Self::Instance(i) => i.to_js_string(),
            Self::BigInt(s) => s.clone(),
        }
    }

    /// Plain JSON as `JSON.parse` would have produced it.
    pub fn from_json(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(b) => Self::Bool(*b),
            Value::Number(n) => Self::Number(n.as_f64().unwrap_or(f64::NAN)),
            Value::String(s) => Self::String(s.clone()),
            Value::Array(items) => Self::Array(items.iter().map(Self::from_json).collect()),
            Value::Object(map) => Self::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), Self::from_json(v)))
                    .collect(),
            ),
        }
    }

    /// This value in the shape [`concerto_core::instance::validate::validate_instance`] reads
    /// (its module doc, "Scope"): a dayjs as a [`DAYJS_TAG`]-tagged object,
    /// `undefined` as [`js_undefined`], a `Map` as the list of its
    /// entries ([`js_map`]), an instance through
    /// [`Instance::to_validator_value`], and a non-finite number as
    /// [`js_special_number`](concerto_core::instance::validate::js_special_number).
    pub fn to_validator_value(&self) -> Value {
        match self {
            Self::Undefined => js_undefined(),
            Self::Null => Value::Null,
            Self::Bool(b) => Value::Bool(*b),
            Self::Number(n) => js_number(*n),
            Self::String(s) => Value::String(s.clone()),
            Self::Array(items) => {
                Value::Array(items.iter().map(Self::to_validator_value).collect())
            }
            Self::Object(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| (k.clone(), v.to_validator_value()))
                    .collect(),
            ),
            Self::Map(entries) => js_map(
                entries
                    .iter()
                    .map(|(k, v)| (k.to_validator_value(), v.to_validator_value()))
                    .collect(),
            ),
            Self::DateTime(d) => d.validator_value(),
            Self::Instance(i) => i.to_validator_value(),
            Self::BigInt(s) => js_bigint(s),
        }
    }
}

// ---------------------------------------------------------------------
// The validator's view of a JS value
// ---------------------------------------------------------------------

/// `true`, for a relationship's [`RELATIONSHIP_TAG`] property.
static TRUE: JsValue = JsValue::Bool(true);

/// The one-key tagged object `{tag: <one value>}` of the validator's
/// plain-JSON shape, as a plain object holding it (`$$undefined`,
/// `$$number`, `$$bigint` or `$$map`) is read there.
fn sole_tag<'a>(map: &'a JsObject, tag: &str) -> Option<&'a JsValue> {
    if map.len() == 1 { map.get(tag) } else { None }
}

/// A JS object of the validator's walk ([`ValidatorObject`]), read in
/// place: each answers what its plain-JSON shape
/// ([`JsValue::to_validator_value`]) would.
#[derive(Debug, Clone, Copy)]
pub enum JsObjectView<'a> {
    /// A `Resource` or `ValidatedResource`: `$class`, then every own
    /// property but the private ones.
    Resource(&'a Instance),
    /// A `Relationship`: [`RELATIONSHIP_TAG`], `$class`, and its
    /// identifying field when that holds a string.
    Relationship(&'a Instance),
    /// A plain object: its own properties.
    Plain(&'a JsObject),
    /// A dayjs, a `Map` or a `BigInt`: the one tag key its shape has
    /// ([`DAYJS_TAG`], [`MAP_TAG`] or [`BIGINT_TAG`]), so no `$class`, and
    /// so no property the walk reads.
    Tagged(&'static str),
}

impl Instance {
    /// A relationship's identifying field, when it holds a string: the key
    /// and the value [`Instance::to_validator_value`] writes.
    fn relationship_identifier(&self) -> Option<(&str, &JsValue)> {
        let (key, value) = self
            .props
            .get_key_value(&*self.identifier_field_name_ref())?;
        matches!(value, JsValue::String(_)).then_some((key.as_str(), value))
    }
}

impl<'a> ValidatorObject<'a, JsValue> for JsObjectView<'a> {
    fn class(&self) -> Option<&'a str> {
        match *self {
            // A `$class` own property is written over the declaration's.
            Self::Resource(i) => match i.props.get("$class") {
                Some(class) => class.as_str(),
                None => Some(&i.class_fqn),
            },
            Self::Relationship(i) => Some(&i.class_fqn),
            Self::Plain(map) => map.get("$class").and_then(JsValue::as_str),
            Self::Tagged(_) => None,
        }
    }

    fn is_relationship(&self) -> bool {
        match *self {
            Self::Resource(i) => i.props.contains_key(RELATIONSHIP_TAG),
            Self::Relationship(_) => true,
            Self::Plain(map) => map.contains_key(RELATIONSHIP_TAG),
            Self::Tagged(_) => false,
        }
    }

    fn get(&self, key: &str) -> Option<&'a JsValue> {
        match *self {
            Self::Resource(i) => {
                if key == "$class" || is_private_only(key) {
                    None
                } else {
                    i.props.get(key)
                }
            }
            Self::Relationship(i) => {
                if key == RELATIONSHIP_TAG {
                    return Some(&TRUE);
                }
                i.relationship_identifier()
                    .filter(|(field, _)| *field == key)
                    .map(|(_, value)| value)
            }
            Self::Plain(map) => map.get(key),
            Self::Tagged(_) => None,
        }
    }

    fn keys(&self) -> impl Iterator<Item = &'a str> {
        let (head, props, own_only, tail): ([Option<&'a str>; 2], Option<&'a JsObject>, bool, _) =
            match *self {
                Self::Resource(i) => ([Some("$class"), None], Some(&i.props), true, None),
                Self::Relationship(i) => (
                    [Some(RELATIONSHIP_TAG), Some("$class")],
                    None,
                    false,
                    i.relationship_identifier().map(|(field, _)| field),
                ),
                Self::Plain(map) => ([None, None], Some(map), false, None),
                Self::Tagged(tag) => ([Some(tag), None], None, false, None),
            };
        head.into_iter()
            .flatten()
            .chain(
                props
                    .into_iter()
                    .flat_map(|map| map.keys())
                    .map(String::as_str)
                    .filter(move |key| !own_only || (*key != "$class" && !is_private_only(key))),
            )
            .chain(tail)
    }
}

impl ValidatorInput for JsValue {
    type Object<'a> = JsObjectView<'a>;

    fn is_undefined(&self) -> bool {
        match self {
            Self::Undefined => true,
            Self::Object(map) => sole_tag(map, UNDEFINED_TAG).is_some(),
            _ => false,
        }
    }

    fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    fn as_object(&self) -> Option<JsObjectView<'_>> {
        match self {
            Self::Object(map) => {
                let special = sole_tag(map, UNDEFINED_TAG).is_some()
                    || sole_tag(map, NUMBER_TAG).is_some_and(|n| n.as_str().is_some());
                (!special).then_some(JsObjectView::Plain(map))
            }
            Self::Instance(i) if i.kind == InstanceKind::Relationship => {
                Some(JsObjectView::Relationship(i))
            }
            Self::Instance(i) => Some(JsObjectView::Resource(i)),
            Self::DateTime(_) => Some(JsObjectView::Tagged(DAYJS_TAG)),
            Self::Map(_) => Some(JsObjectView::Tagged(MAP_TAG)),
            Self::BigInt(_) => Some(JsObjectView::Tagged(BIGINT_TAG)),
            // A non-finite number is a one-key tagged object, but not a JS
            // object.
            Self::Undefined
            | Self::Null
            | Self::Bool(_)
            | Self::Number(_)
            | Self::String(_)
            | Self::Array(_) => None,
        }
    }

    fn as_array(&self) -> Option<&[JsValue]> {
        match self {
            Self::Array(items) => Some(items),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        JsValue::as_str(self)
    }

    fn as_f64(&self) -> Option<f64> {
        match self {
            // `js_number`: a finite number, `-0` written as the integer `0`.
            Self::Number(n) if n.is_finite() => Some(if *n == 0.0 { 0.0 } else { *n }),
            _ => None,
        }
    }

    fn is_boolean(&self) -> bool {
        matches!(self, Self::Bool(_))
    }

    fn is_dayjs(&self) -> bool {
        match self {
            Self::DateTime(_) => true,
            Self::Object(map) => map.contains_key(DAYJS_TAG),
            Self::Instance(i) => {
                i.kind != InstanceKind::Relationship && i.props.contains_key(DAYJS_TAG)
            }
            _ => false,
        }
    }

    fn map_entries(&self) -> Option<impl Iterator<Item = (&JsValue, &JsValue)>> {
        // A JS `Map`'s own entries, or a `MAP_TAG` object's pairs: one
        // iterator over whichever the value is (no `Vec` per map).
        let (own, tagged) = match self {
            Self::Map(entries) => (Some(entries.iter().map(|(k, v)| (k, v))), None),
            Self::Object(map) => {
                let JsValue::Array(entries) = sole_tag(map, MAP_TAG)? else {
                    return None;
                };
                (
                    None,
                    Some(entries.iter().filter_map(|entry| match entry {
                        JsValue::Array(pair) => Some((pair.first()?, pair.get(1)?)),
                        _ => None,
                    })),
                )
            }
            _ => return None,
        };
        Some(
            own.into_iter()
                .flatten()
                .chain(tagged.into_iter().flatten()),
        )
    }

    fn to_value(&self) -> std::borrow::Cow<'_, Value> {
        std::borrow::Cow::Owned(self.to_validator_value())
    }
}

#[cfg(test)]
mod validator_input_tests;
