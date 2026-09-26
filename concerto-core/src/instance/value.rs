//! The in-memory instances the serializer builds and reads: a port of the
//! state of `Typed`, `Identifiable`, `Resource`, `ValidatedResource` and
//! `Relationship` (`src/model/*.ts`), and of the JS values their fields
//! hold.
//!
//! D7 keeps these objects in TS: on the WASM path the TS classes stay the
//! user-visible objects, and Rust only builds or reads their state in one
//! call (the Serializer fast path, PORTING.md section 5 row 6). This module
//! is that state, as the populator ([`super::populator`]) produces it and
//! the generator ([`super::generator`]) and the validator
//! ([`super::validate`]) consume it. It holds no model data: an instance
//! names its declaration by fully-qualified name, and every operation that
//! needs the model takes the [`ModelManager`](crate::ModelManager).
//!
//! An [`Instance`] keeps every own property of the TS object in insertion
//! order (`$namespace`, `$type`, `$identifierFieldName`, `$identifier`, the
//! identifying field, `$timestamp`, `$class` for a relationship, then the
//! fields), except the three handles `$modelManager`, `$classDeclaration`
//! and `$validator`, which it keeps as the declaration's name and the
//! validator's options. That is the order `Object.getOwnPropertyNames`
//! reports, which `ResourceValidator` walks (first undeclared field wins).

use indexmap::IndexMap;
use serde_json::{Value, json};

use super::dayjs::Dayjs;
use super::resource_id::ResourceId;
use super::validate::{
    DAYJS_TAG, RELATIONSHIP_TAG, ValidateOptions, js_bigint, js_map, js_special_number,
    js_undefined,
};
use crate::ecma;
use crate::error::Result;

/// A JS value held by an instance field or passed to the serializer.
#[derive(Debug, Clone, PartialEq)]
pub enum JsValue {
    Undefined,
    Null,
    Bool(bool),
    /// A JS number (an IEEE double, PORTING.md 3.1).
    Number(f64),
    String(String),
    Array(Vec<JsValue>),
    /// A plain object: its own enumerable properties, in `Object.keys`
    /// order.
    Object(IndexMap<String, JsValue>),
    /// A JS `Map` (a populated `MapDeclaration` value), in insertion order.
    Map(Vec<(JsValue, JsValue)>),
    /// A dayjs object.
    DateTime(Dayjs),
    /// A `Resource`, `ValidatedResource` or `Relationship`.
    Instance(Box<Instance>),
    /// A JS `BigInt`, as its decimal digit string (`toString()`'s
    /// spelling). Not produced by `JSONPopulator` (JSON has no bigint
    /// literal); it reaches an instance only by direct field assignment,
    /// as `Resource.setPropertyValue` allows (task P2-11b-U6).
    BigInt(String),
}

/// Which TS class an [`Instance`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstanceKind {
    Resource,
    ValidatedResource,
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
    pub props: IndexMap<String, JsValue>,
    /// `$validator.options`, for a `ValidatedResource`.
    pub validator_options: ValidateOptions,
}

/// `undefined`, for a property that is not there.
static UNDEFINED: JsValue = JsValue::Undefined;

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
        let mut instance = Self {
            kind,
            class_fqn: class_fqn.into(),
            props: IndexMap::new(),
            validator_options: ValidateOptions::default(),
        };
        instance.set("$namespace", JsValue::String(namespace.to_string()));
        instance.set("$type", JsValue::String(type_name.to_string()));
        instance.set(
            "$identifierFieldName",
            JsValue::String(identifier_field_name.unwrap_or_else(|| "$identifier".to_string())),
        );
        instance.set_identifier(id);
        instance.set("$timestamp", timestamp);
        if kind == InstanceKind::Relationship {
            // `this.$class = 'Relationship'`
            instance.set("$class", JsValue::String("Relationship".to_string()));
        }
        instance
    }

    /// `this[key]`.
    pub fn get(&self, key: &str) -> &JsValue {
        self.props.get(key).unwrap_or(&UNDEFINED)
    }

    /// `this[key] = value`: a new key goes last, an existing one keeps its
    /// place.
    pub fn set(&mut self, key: &str, value: JsValue) {
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
        self.get("$identifierFieldName").to_js_string()
    }

    /// TS `Identifiable.getIdentifier`: `this[this.$identifierFieldName]`.
    pub fn get_identifier(&self) -> &JsValue {
        let field = self.identifier_field_name();
        self.get(&field)
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

    /// This instance in the value shape [`super::validate::validate_instance`]
    /// reads (its module doc, "Scope"): a `Relationship` as a
    /// [`RELATIONSHIP_TAG`]-tagged `{$class, <identifying field>}` object,
    /// anything else as a `$class`-tagged object with every own property
    /// but the private ones.
    pub fn to_validator_value(&self) -> Value {
        if self.kind == InstanceKind::Relationship {
            let mut wire = serde_json::Map::new();
            wire.insert(RELATIONSHIP_TAG.to_string(), Value::Bool(true));
            wire.insert("$class".to_string(), Value::String(self.class_fqn.clone()));
            let field = self.identifier_field_name();
            if let JsValue::String(id) = self.get(&field) {
                wire.insert(field, Value::String(id.clone()));
            }
            return Value::Object(wire);
        }
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
        let mut wire = serde_json::Map::new();
        wire.insert("$class".to_string(), Value::String(self.class_fqn.clone()));
        for (key, value) in &self.props {
            if PRIVATE_ONLY_KEYS.contains(&key.as_str()) {
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
            Self::Number(n) => ecma::number_to_string(*n),
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

    /// This value in the shape [`super::validate::validate_instance`] reads
    /// (its module doc, "Scope"): a dayjs as a [`DAYJS_TAG`]-tagged object,
    /// `undefined` as [`js_undefined`], a `Map` as the list of its
    /// entries ([`js_map`]), an instance through
    /// [`Instance::to_validator_value`], and a non-finite number as
    /// [`js_special_number`].
    pub fn to_validator_value(&self) -> Value {
        match self {
            Self::Undefined => js_undefined(),
            Self::Null => Value::Null,
            Self::Bool(b) => Value::Bool(*b),
            Self::Number(n) => validator_number(*n),
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
            Self::DateTime(d) => json!({ DAYJS_TAG: d.to_iso_string() }),
            Self::Instance(i) => i.to_validator_value(),
            Self::BigInt(s) => js_bigint(s),
        }
    }
}

/// A JS number as a JSON number: an integral one as an integer, so that the
/// messages that print it (`JSON.stringify`, `String`) read `1`, not `1.0`;
/// a non-finite one as [`js_special_number`].
fn validator_number(n: f64) -> Value {
    if !n.is_finite() {
        return js_special_number(&ecma::number_to_string(n));
    }
    if n.trunc() == n && n.abs() < 9_007_199_254_740_992.0 {
        return Value::Number(serde_json::Number::from(n as i64));
    }
    serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
}
