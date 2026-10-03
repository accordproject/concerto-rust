//! Reading a model's JSON AST into the typed model: the only model loader
//! (P5-06c spike, accordproject/concerto-rust#234; adopted in P5-06d,
//! accordproject/concerto-rust#239; made the only loader by P5-61,
//! accordproject/concerto-rust#393, BR-09).
//!
//! [`parse`] reads JSON text without building a [`serde_json::Value`] for
//! the whole document first (when the caller holds JSON text, as the WASM
//! bindings do, that parse was about half of the load); [`from_value`]
//! reads a `Value` the caller has already parsed, through the very same
//! readers. Each class-like declaration (concept, asset, participant,
//! transaction, event) and enum declaration, and each of their properties,
//! is deserialized straight into its generated `mm::*` struct. The rest of
//! the document stays as small `Value` subtrees: the model's own header keys
//! (`namespace`, `imports`, `decorators`, and so on), every decorator list
//! (which the generated structs also decode, strictly), and every scalar
//! and map declaration, each of which its own loader decodes into its
//! generated struct ([`crate::introspect::Declaration`]).
//!
//! Since P5-93 (accordproject/concerto-rust#443) the read allocates as
//! little as it can: a node's `$class` is interned
//! (`concerto_metamodel::ClassName`); a `name` read from JSON text shares
//! the copy of the text the model file keeps (`concerto_metamodel::Name`,
//! [`parse`] is run inside `concerto_metamodel::with_source`); a decorator
//! list is read straight into its generated and processed decorators
//! (`kept::DecoratorsSeed`); and a property node's own keys are read by
//! [`read_property`] itself, the generated struct reading only what else a
//! node has. Each reads exactly what the generated structs read, and fails
//! as they fail.
//!
//! # Strictness (BC-19, BR-09)
//!
//! Since BC-19 (P5-49) a model loaded through the JS API has its shape
//! checked against the metamodel first (`instance::check_ast_shape`, unless
//! the manager opts out with `metamodelValidation: false`), so a malformed
//! AST never reaches this reader there. The reader is therefore strict: a
//! node that does not decode into its generated struct is an error (the
//! caller's `modelfile-load-unreadable` `IllegalModelException`), never a
//! TS-style coercion. Before P5-61, a failure here fell back to an untyped
//! walk over the whole `Value` that reproduced TS 5.0.0's handling of
//! malformed nodes (#217, #230); that path is gone. Every semantic check
//! (identifiers, reserved names, validators, and everything
//! `ModelFile.validate` does) runs after the read, on the typed result.
//!
//! Since P5-69 (BC-19-b, accordproject/concerto-rust#408) that shape check
//! is folded into this read: `crate::introspect::shape` decides it from
//! what the read has decoded, and only an AST it cannot vouch for is checked
//! again in full (`ModelFile::from_json_text_checked_with_imports`, and
//! `instance::check_ast_shape` itself). For that, the read keeps the
//! `Value`s the check needs that the generated structs drop: a class's
//! `identified`, each property's `decorators`, and the `defaultValue` of a
//! `DateTimeProperty`.
//!
//! Since P5-61 (accordproject/concerto-rust#393) there is no exception:
//! a class's `identified` and a property's `sizeValidator`,
//! `lengthValidator` and `validator` are decoded as strictly as every other
//! field. BC-19's shape check requires a node there (an object with a
//! `$class`, or `null`; `modelfile-load-nodenotobject`), where the
//! metamodel check alone accepts any value with no own keys (a number, a
//! boolean, `""`, `[]`, `{}`) and an object without a `$class`.
//!
//! With the shape check off (`metamodelValidation: false`, an escape hatch
//! for trusted input), this read is the only check. A malformed AST it
//! cannot read is an error, never a trap or a panic; the read is not a full
//! metamodel check, though (see "Not checked" below).
//!
//! - **Unknown keys.** A key the generated struct for a node does not
//!   declare is an error (`Strict`), on every node decoded into a
//!   generated struct: declarations, properties, the structs under them,
//!   decorators, locations, scalar and map declarations, and the model's
//!   own header. The one exception is BC-19's tolerance: the
//!   `defaultValue` the reference parser writes on a `DateTimeProperty`
//!   (read, as before, by BC-45's check when it is applied).
//! - **Key order.** A node's `$class` is read first when it is the first
//!   key (as every AST that `concerto-cto` or `JSON.stringify` writes has
//!   it); otherwise the node is buffered and read again with `$class` in
//!   front, so key order never changes the result.
//! - **JSON syntax.** `serde_json` checks less when it skips a value
//!   (`deserialize_ignored_any`) than when it builds one: a skipped `1e400`,
//!   lone surrogate escape or over-deep array is accepted where a `Value`
//!   parse rejects it. The generated structs never skip a value: [`Strict`]
//!   refuses one (an unknown key), so text is read as JSON exactly when it
//!   parses as a `Value`.
//! - **Duplicate keys.** A `Value` keeps the last of two equal keys. This
//!   reader refuses a duplicate `$class`, `properties`, `decorators` or
//!   `location` key, or a duplicate struct field, in JSON text
//!   (`JSON.stringify` never writes one).
//! - **Not checked.** With the shape check off, the read still accepts some
//!   ASTs the metamodel check rejects: an unknown key inside a node of a
//!   polymorphic type that `serde` buffers before it picks the variant (an
//!   `IdentifiedBy`, a decorator argument, a map key or value type), an
//!   import node (read untyped by `Import::try_from`), a `$class` naming
//!   the wrong type on a node whose type has no subtypes, a model `$class`
//!   of another metamodel version, and a fraction in an `Integer` field
//!   (the generated structs read every number as `f64`).

use std::borrow::Cow;
use std::cell::Cell;
use std::fmt;
use std::marker::PhantomData;
use std::thread::LocalKey;

use concerto_metamodel::Name;
use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use serde::Deserialize;
use serde::de::value::{
    BoolDeserializer, BorrowedStrDeserializer, MapAccessDeserializer, StringDeserializer,
};
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::introspect::METAMODEL_NAMESPACE;
use crate::introspect::declaration::{ClassKind, ClassNode};
use crate::introspect::decorator::{Decorator, WithDecorators};
use crate::introspect::kept::{
    Decorators, DecoratorsSeed, IdentifiedSeed, Kept, KeptSeed, Location, LocationSeed,
};
use crate::introspect::property::{Property, property_kind};
use crate::introspect::shape;

type Error = serde_json::Error;

/// A model AST read by [`parse`] or [`from_value`].
pub(crate) struct TypedModel {
    /// Every top-level key but `declarations`.
    pub(crate) header: ModelHeader,
    /// The declarations, in order; empty when the AST has none.
    pub(crate) declarations: Vec<TypedDeclaration>,
}

/// A model's own keys (every top-level key but `declarations`), each the
/// `Value` a `Value` parse builds for it, or `None` when the AST has no
/// such key (a repeated key's last value, as in a `Value`). P5-93: read
/// into its own fields, where a `Map` of every key (each key an owned
/// `String`) used to be built.
#[derive(Default)]
pub(crate) struct ModelHeader {
    pub(crate) class: Option<ModelClass>,
    pub(crate) namespace: Option<Value>,
    pub(crate) source_uri: Option<Value>,
    pub(crate) concerto_version: Option<Value>,
    pub(crate) imports: Option<Value>,
    /// The model's `decorators`, as a declaration's are read (P5-93;
    /// before, a `Value`): its decorators ([`DecoratorsSeed`]), or else the
    /// value as a [`Kept`], decoded by the loader.
    pub(crate) decorators: Option<Result<Decorators, Kept>>,
    /// The first key the metamodel's `Model` does not declare, if any (its
    /// value is read as JSON, and dropped).
    pub(crate) unknown: Option<String>,
}

/// A model's `$class` value, as the read keeps it for BC-19's shape check
/// (P5-93: the metamodel's own without an allocation).
pub(crate) enum ModelClass {
    /// The string `concerto.metamodel@1.0.0.Model`.
    Model,
    /// Any other value, as a `Value` parse builds it.
    Other(Value),
}

/// The metamodel's `Model` `$class`.
const MODEL_CLASS: &str = "concerto.metamodel@1.0.0.Model";

/// Reads a [`ModelClass`].
struct ModelClassSeed;

impl<'de> DeserializeSeed<'de> for ModelClassSeed {
    type Value = ModelClass;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<ModelClass, D::Error> {
        d.deserialize_any(self)
    }
}

/// `serde_json`'s own `Value` visitor, but for the `Model` string.
impl<'de> Visitor<'de> for ModelClassSeed {
    type Value = ModelClass;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any valid JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<ModelClass, E> {
        Ok(ModelClass::Other(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<ModelClass, E> {
        Ok(ModelClass::Other(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<ModelClass, E> {
        Ok(ModelClass::Other(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<ModelClass, E> {
        Ok(ModelClass::Other(
            serde_json::Number::from_f64(value).map_or(Value::Null, Value::Number),
        ))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<ModelClass, E> {
        Ok(if value == MODEL_CLASS {
            ModelClass::Model
        } else {
            ModelClass::Other(Value::String(value.to_owned()))
        })
    }

    fn visit_string<E>(self, value: String) -> Result<ModelClass, E> {
        Ok(if value == MODEL_CLASS {
            ModelClass::Model
        } else {
            ModelClass::Other(Value::String(value))
        })
    }

    fn visit_none<E>(self) -> Result<ModelClass, E> {
        Ok(ModelClass::Other(Value::Null))
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<ModelClass, D::Error> {
        ModelClassSeed.deserialize(d)
    }

    fn visit_unit<E>(self) -> Result<ModelClass, E> {
        Ok(ModelClass::Other(Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<ModelClass, A::Error> {
        Value::deserialize(de::value::SeqAccessDeserializer::new(seq)).map(ModelClass::Other)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<ModelClass, A::Error> {
        Value::deserialize(MapAccessDeserializer::new(map)).map(ModelClass::Other)
    }
}

/// One declaration read by [`parse`] or [`from_value`]. P5-93: the
/// generated nodes are held inline (boxed from P5-76): the declarations
/// are read into a per-thread buffer the loader hands back once it has
/// taken them ([`recycle_declarations`]), and each declaration's
/// properties are collected into a `Vec` of exactly their number
/// ([`exact`]), not one that grows (and moves every element) as they are
/// read.
pub(crate) enum TypedDeclaration {
    /// A class-like declaration, read straight into its generated struct.
    /// The node's own `properties` is left empty; they are in `properties`.
    Class {
        kind: ClassKind,
        node: ClassNode,
        properties: TypedProperties,
        /// The node's `decorators`, if it has that key.
        decorators: Option<ReadDecorators>,
        /// The node's `location` value, as given (for an error).
        location: Option<Location>,
        /// Whether BC-19's shape check certainly accepts the node's
        /// `identified` value ([`crate::introspect::shape`]; P5-93: decided
        /// as it is read, where the value used to be kept for it).
        identified_conforms: bool,
    },
    /// An enum declaration, read straight into its generated struct, whose
    /// `properties` are the enum values' own nodes (in `values`; the
    /// node's own `properties` is left empty).
    Enum {
        node: mm::EnumDeclaration,
        values: TypedProperties,
        /// The node's `decorators`, if it has that key.
        decorators: Option<ReadDecorators>,
        /// The node's `location` value, as given (for an error).
        location: Option<Location>,
    },
    /// A map declaration, as its JSON node (P5-93: a [`Kept`], whose
    /// objects are each one list of entries where a `Value`'s are each a
    /// hash map, every key an owned `String`; before, an `Ast` `Value`).
    Map(Kept),
    /// A scalar declaration of one of the six metamodel scalar kinds, as
    /// its JSON node, read the way a map declaration is (A-10,
    /// accordproject/concerto-rust#458; before, an `Ast` `Value`).
    Scalar(Kept),
    /// Any other declaration (anything unrecognised), as its JSON subtree.
    Ast(Value),
}

/// The properties of a class-like declaration, or the values of an enum
/// declaration, read by [`parse`] or [`from_value`]: each [`Property`], and
/// what the read kept of each property's node.
pub(crate) struct TypedProperties {
    pub(crate) properties: Vec<Property>,
    /// In step with `properties`, or empty when the read kept nothing of
    /// any of them (P5-93: no allocation for a declaration whose
    /// properties have no `location`, `decorators` or `defaultValue`).
    kept: Vec<PropertyKept>,
}

impl TypedProperties {
    /// Each property, with what the read kept of its node.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&Property, &PropertyKept)> {
        const NOTHING: &PropertyKept = &PropertyKept {
            location: None,
            unusual_decorators: false,
            date_time_default: None,
        };
        self.properties
            .iter()
            .enumerate()
            .map(|(i, property)| (property, self.kept.get(i).unwrap_or(NOTHING)))
    }
}

/// A node's `decorators` value, as the read keeps it (P5-93; the value
/// itself, as a [`Kept`], from P5-76): the decorators it gives
/// ([`crate::introspect::decorator::parse_decorator_list`]), and whether
/// BC-19's shape check certainly
/// accepts it ([`crate::introspect::shape`]).
pub(crate) struct ReadDecorators {
    pub(crate) list: Vec<Decorator>,
    pub(crate) conforms: bool,
}

impl ReadDecorators {
    /// The decorators of a node (none when it has no `decorators` key),
    /// taken from `read`: `parse_decorator_list` of the value.
    pub(crate) fn list(read: &mut Option<ReadDecorators>) -> Vec<Decorator> {
        read.as_mut()
            .map(|read| std::mem::take(&mut read.list))
            .unwrap_or_default()
    }

    /// Whether the shape check certainly accepts a node's `decorators`
    /// (`true` when it has no such key).
    pub(crate) fn conforms(read: Option<&ReadDecorators>) -> bool {
        read.is_none_or(|read| read.conforms)
    }
}

/// What the read keeps of a property's node: its `location` value as given
/// (for an error), and the values BC-19's shape check reads
/// ([`crate::introspect::shape`]).
#[derive(Default)]
pub(crate) struct PropertyKept {
    pub(crate) location: Option<Location>,
    /// Whether the node has a `decorators` value BC-19's shape check may
    /// reject (P5-93: decided as it is read, where the value used to be
    /// kept for it).
    pub(crate) unusual_decorators: bool,
    /// The `defaultValue` the reference parser writes on a
    /// `DateTimeProperty`, which the generated struct does not declare.
    pub(crate) date_time_default: Option<Value>,
}

impl PropertyKept {
    fn is_empty(&self) -> bool {
        self.location.is_none() && !self.unusual_decorators && self.date_time_default.is_none()
    }
}

/// Reads a model AST from JSON text. The error is a `serde_json` syntax
/// error when `text` is not JSON, and a data error when it is JSON but not
/// in the model's shape.
pub(crate) fn parse(text: &str) -> Result<TypedModel, Error> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let model = ModelSeed.deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(model)
}

/// Reads a model AST from the compact binary layout (P5-92,
/// `introspect::compact`), as [`from_value`] reads the document the bytes
/// hold. An error for bytes not in the layout too, which the caller tells
/// apart with `compact::to_value`.
#[cfg(feature = "js-compat")]
pub(crate) fn from_compact(bytes: &[u8]) -> Result<TypedModel, Error> {
    let mut deserializer = crate::introspect::compact::Compact::new(bytes);
    let model = ModelSeed.deserialize(&mut deserializer)?;
    deserializer.end()?;
    Ok(model)
}

/// Reads a model AST from a parsed `Value`, as [`parse`] reads its text.
pub(crate) fn from_value(value: &Value) -> Result<TypedModel, Error> {
    ModelSeed.deserialize(value)
}

/// Reads one declaration node, as [`from_value`] reads each element of a
/// model's `declarations`.
pub(crate) fn declaration_from_value(value: &Value) -> Result<TypedDeclaration, Error> {
    let mut out = Vec::with_capacity(1);
    DeclarationSeed { out: &mut out }.deserialize(value)?;
    out.pop().ok_or_else(|| refuse("no declaration"))
}

/// Reads one property node, as [`from_value`] reads each element of a
/// class-like or enum declaration's `properties`.
pub(crate) fn property_from_value(value: &Value) -> Result<Property, Error> {
    let mut properties = Vec::with_capacity(1);
    PropertySeed {
        properties: &mut properties,
        kept: &mut Vec::new(),
    }
    .deserialize(value)?;
    properties.pop().ok_or_else(|| refuse("no property"))
}

/// The error the reader raises for a node it cannot read.
fn refuse(why: &str) -> Error {
    de::Error::custom(format_args!("{why}"))
}

// ---------------------------------------------------------------------------
// The model, its declarations and their properties
// ---------------------------------------------------------------------------

struct ModelSeed;

impl<'de> DeserializeSeed<'de> for ModelSeed {
    type Value = TypedModel;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<TypedModel, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for ModelSeed {
    type Value = TypedModel;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a model AST object")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<TypedModel, A::Error> {
        let mut header = ModelHeader::default();
        let mut declarations = None;
        while let Some(key) = map.next_key_seed(StrSeed)? {
            // A duplicate key replaces the earlier value, as in a `Value`.
            let slot = match &*key {
                "declarations" => {
                    if declarations.is_some() {
                        return Err(de::Error::custom("duplicate declarations"));
                    }
                    declarations = Some(map.next_value_seed(DeclarationsSeed)?);
                    continue;
                }
                "$class" => {
                    header.class = Some(map.next_value_seed(ModelClassSeed)?);
                    continue;
                }
                "namespace" => &mut header.namespace,
                "sourceUri" => &mut header.source_uri,
                "concertoVersion" => &mut header.concerto_version,
                "imports" => &mut header.imports,
                "decorators" => {
                    header.decorators = Some(map.next_value_seed(DecoratorsSeed)?);
                    continue;
                }
                _ => {
                    map.next_value::<Value>()?;
                    if header.unknown.is_none() {
                        header.unknown = Some(key.into_owned());
                    }
                    continue;
                }
            };
            *slot = Some(map.next_value::<Value>()?);
        }
        Ok(TypedModel {
            header,
            declarations: declarations.unwrap_or_default(),
        })
    }
}

/// A model's declarations, once the loader has taken them all: the
/// (empty) per-thread buffer [`DeclarationsSeed`] read them into, kept for
/// the next read unless it has grown past [`BUFFER_BYTES`] (P5-93: the
/// declarations are only ever moved out of it one by one, so they are
/// never copied into a `Vec` of their own).
pub(crate) fn recycle_declarations(mut items: Vec<TypedDeclaration>) {
    items.clear();
    if items.capacity() * std::mem::size_of::<TypedDeclaration>() <= BUFFER_BYTES {
        DECLARATIONS.set(items);
    }
}

/// The items of `items` (a buffer filled by the read), in a `Vec` of
/// exactly their number plus `reserve`; `items` goes back to `buffer`
/// empty, unless it has grown past [`BUFFER_BYTES`].
fn exact<T: 'static>(
    buffer: &'static LocalKey<Cell<Vec<T>>>,
    mut items: Vec<T>,
    reserve: usize,
) -> Vec<T> {
    let mut out = Vec::new();
    if !items.is_empty() || reserve > 0 {
        out.reserve_exact(items.len() + reserve);
        out.append(&mut items);
    }
    if items.capacity() * std::mem::size_of::<T>() <= BUFFER_BYTES {
        buffer.set(items);
    }
    out
}

/// The largest buffer [`exact`] keeps for the next read.
const BUFFER_BYTES: usize = 1024 * 1024;

thread_local! {
    /// [`DeclarationsSeed`]'s buffer for a model's declarations.
    static DECLARATIONS: Cell<Vec<TypedDeclaration>> = const { Cell::new(Vec::new()) };
    /// [`PropertiesSeed`]'s buffer for a declaration's properties.
    static PROPERTIES: Cell<Vec<Property>> = const { Cell::new(Vec::new()) };
}

/// A model's `declarations` array.
struct DeclarationsSeed;

impl<'de> DeserializeSeed<'de> for DeclarationsSeed {
    type Value = Vec<TypedDeclaration>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for DeclarationsSeed {
    type Value = Vec<TypedDeclaration>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an array")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut out = DECLARATIONS.take();
        while seq
            .next_element_seed(DeclarationSeed { out: &mut out })?
            .is_some()
        {}
        Ok(out)
    }
}

/// A class-like or enum declaration's `properties` array, as read: the
/// properties in [`PROPERTIES`]' buffer (for the declaration's reader to
/// pass to [`exact`] once it knows how many system properties the loader
/// will add), and what the read kept of each, as [`TypedProperties`] holds
/// it.
struct PropertiesSeed;

type ReadProperties = (Vec<Property>, Vec<PropertyKept>);

impl<'de> DeserializeSeed<'de> for PropertiesSeed {
    type Value = ReadProperties;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de> Visitor<'de> for PropertiesSeed {
    type Value = ReadProperties;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an array")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut properties = PROPERTIES.take();
        let mut kept = Vec::new();
        while seq
            .next_element_seed(PropertySeed {
                properties: &mut properties,
                kept: &mut kept,
            })?
            .is_some()
        {}
        Ok((properties, kept))
    }
}

/// [`TypedProperties`] from what [`PropertiesSeed`] read, with room for
/// `reserve` more properties.
fn typed_properties((properties, kept): ReadProperties, reserve: usize) -> TypedProperties {
    TypedProperties {
        properties: exact(&PROPERTIES, properties, reserve),
        kept,
    }
}

/// A node's `$class`, as [`read_class`] finds it.
enum Class<'de> {
    /// `$class` was the node's first key; the rest of the node is still to
    /// be read from the map.
    First(Cow<'de, str>),
    /// `$class` was not the first key: the whole node, read into a `Value`
    /// with `$class` moved to the front, to be read again.
    Reordered(Value),
}

/// Reads a node's `$class`. When it is the first key, only it has been read;
/// otherwise the node is buffered and handed back with `$class` in front
/// (module doc, "Key order"). A node with no string `$class` is an error.
fn read_class<'de, A: MapAccess<'de, Error = Error>>(map: &mut A) -> Result<Class<'de>, Error> {
    let Some(first) = map.next_key_seed(StrSeed)? else {
        return Err(refuse("missing $class"));
    };
    if first == "$class" {
        return map.next_value_seed(StrSeed).map(Class::First);
    }
    let mut rest = Map::new();
    rest.insert(first.into_owned(), map.next_value::<Value>()?);
    while let Some(key) = map.next_key::<String>()? {
        // A duplicate key replaces the earlier value, as in a `Value`.
        rest.insert(key, map.next_value::<Value>()?);
    }
    match rest.shift_remove("$class") {
        Some(class @ Value::String(_)) => {
            let mut node = Map::with_capacity(rest.len() + 1);
            node.insert("$class".to_string(), class);
            node.extend(rest);
            Ok(Class::Reordered(Value::Object(node)))
        }
        Some(_) => Err(refuse("$class is not a string")),
        None => Err(refuse("missing $class")),
    }
}

/// Reads one declaration onto `out` (P5-93: pushed where it is read, not
/// returned through each layer of the read, a move of the whole node at
/// each).
struct DeclarationSeed<'a> {
    out: &'a mut Vec<TypedDeclaration>,
}

impl<'de> DeserializeSeed<'de> for DeclarationSeed<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for DeclarationSeed<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a declaration object")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<(), A::Error> {
        // The generic `A` has only its own error type; every map this
        // reader is handed comes from `serde_json`, so go through that.
        read_declaration(ErrorBridge(map), self.out).map_err(de::Error::custom)
    }
}

/// Whether `short` is one of the six metamodel scalar declaration kinds.
pub(crate) fn is_scalar_kind(short: &str) -> bool {
    matches!(
        short,
        "BooleanScalar"
            | "IntegerScalar"
            | "LongScalar"
            | "DoubleScalar"
            | "StringScalar"
            | "DateTimeScalar"
    )
}

/// A declaration's short `$class` name, when it is a metamodel one.
fn metamodel_kind(class: &str) -> Option<&str> {
    class
        .strip_prefix(METAMODEL_NAMESPACE)
        .and_then(|rest| rest.strip_prefix('.'))
}

fn read_declaration<'de, A: MapAccess<'de, Error = Error>>(
    mut map: A,
    out: &mut Vec<TypedDeclaration>,
) -> Result<(), Error> {
    let class = match read_class(&mut map)? {
        Class::First(class) => class,
        Class::Reordered(node) => return DeclarationSeed { out }.deserialize(&node),
    };
    let short = metamodel_kind(&class);
    if short == Some("EnumDeclaration") {
        return read_enum(map, out);
    }
    let Some(kind) = short.and_then(ClassKind::from_short) else {
        // Neither class-like nor an enum: the subtree, `$class` first, for
        // `Declaration::from_model_json`; a map or scalar declaration's as a
        // `Kept` (P5-93, A-10; before, a `Value`), for
        // `Declaration::from_typed`.
        let is_map = short == Some("MapDeclaration");
        let is_scalar = short.is_some_and(is_scalar_kind);
        let replay = MapAccessDeserializer::new(Replay {
            class: Some(class),
            inner: map,
        });
        out.push(if is_map {
            TypedDeclaration::Map(KeptSeed.deserialize(replay)?)
        } else if is_scalar {
            TypedDeclaration::Scalar(KeptSeed.deserialize(replay)?)
        } else {
            TypedDeclaration::Ast(Value::deserialize(replay)?)
        });
        return Ok(());
    };
    let mut properties = None;
    let mut decorators = None;
    let mut node_decorators = None;
    let mut node_identified = None;
    let mut location = None;
    let mut identified_conforms = true;
    let mut taken = Map::new();
    let access = Intercept {
        inner: map,
        properties: Some(&mut properties),
        decorators: &mut decorators,
        node_decorators: &mut node_decorators,
        node_identified: &mut node_identified,
        location: &mut location,
        identified_conforms: Some(&mut identified_conforms),
        take: &[],
        taken: &mut taken,
        pending: Pending::Other,
    };
    let de = MapAccessDeserializer::new(access);
    let mut node = match kind {
        ClassKind::Concept => ClassNode::Concept(mm::ConceptDeclaration::deserialize(de)?),
        ClassKind::Asset => ClassNode::Asset(mm::AssetDeclaration::deserialize(de)?),
        ClassKind::Participant => {
            ClassNode::Participant(mm::ParticipantDeclaration::deserialize(de)?)
        }
        ClassKind::Transaction => {
            ClassNode::Transaction(mm::TransactionDeclaration::deserialize(de)?)
        }
        ClassKind::Event => ClassNode::Event(mm::EventDeclaration::deserialize(de)?),
    };
    // The generated struct requires `properties`, so it was read.
    let properties = properties.ok_or_else(|| refuse("missing field `properties`"))?;
    node.set_identified(node_identified);
    node.normalize_identified();
    node.set_decorators(node_decorators);
    node.set_location(read_location(location.as_ref())?);
    // Room for the `$identifier` property the loader adds to a class with
    // a system identifier (`ClassDeclaration::finish`).
    let properties = typed_properties(properties, usize::from(node.is_system_identified()));
    out.push(TypedDeclaration::Class {
        kind,
        node,
        properties,
        decorators,
        location,
        identified_conforms,
    });
    Ok(())
}

/// An enum declaration, after its `$class`, whose values are each an
/// `EnumProperty`.
fn read_enum<'de, A: MapAccess<'de, Error = Error>>(
    map: A,
    out: &mut Vec<TypedDeclaration>,
) -> Result<(), Error> {
    let mut properties = None;
    let mut decorators = None;
    let mut node_decorators = None;
    let mut node_identified = None;
    let mut location = None;
    let mut taken = Map::new();
    let access = Intercept {
        inner: map,
        properties: Some(&mut properties),
        decorators: &mut decorators,
        node_decorators: &mut node_decorators,
        node_identified: &mut node_identified,
        location: &mut location,
        identified_conforms: None,
        take: &[],
        taken: &mut taken,
        pending: Pending::Other,
    };
    let mut node = mm::EnumDeclaration::deserialize(MapAccessDeserializer::new(access))?;
    node.decorators = node_decorators;
    node.location = read_location(location.as_ref())?;
    let values = typed_properties(
        properties.ok_or_else(|| refuse("missing field `properties`"))?,
        0,
    );
    // The values' nodes are each `Property::Enum`'s, which `read_property`
    // read; the generated struct's own `properties` is left empty (P5-93:
    // it used to hold a copy of each, which nothing read).
    if values
        .properties
        .iter()
        .any(|value| !matches!(value, Property::Enum(_)))
    {
        return Err(refuse("an enum value is not an EnumProperty"));
    }
    out.push(TypedDeclaration::Enum {
        node,
        values,
        decorators,
        location,
    });
    Ok(())
}

/// The generated `location` of a node whose `location` value [`Intercept`]
/// kept: `None` for no value (or `null`).
fn read_location(location: Option<&Location>) -> Result<Option<mm::Range>, Error> {
    match location {
        None => Ok(None),
        Some(location) => location.decode(),
    }
}

/// Decodes `value` into a generated struct as the typed read decodes a node:
/// through [`Strict`], so a key the struct does not declare is an error.
/// (P5-93: only the tests' reference since the model's own decorators are
/// read as a [`Kept`].)
#[cfg(test)]
pub(crate) fn strict_from_value<'de, T: Deserialize<'de>>(value: &'de Value) -> Result<T, Error> {
    T::deserialize(Strict(value))
}

/// `strict_from_value`, for the generated struct of one variant of a
/// polymorphic type (a scalar or map declaration), which has no `$class`
/// field of its own: `value`'s `$class` (which picked the variant) is left
/// out. A test helper: the loader decodes the node as a [`Kept`]
/// (`Kept::strict_variant_decode`, A-10).
#[cfg(test)]
pub(crate) fn strict_variant_from_value<T: de::DeserializeOwned>(
    value: &Value,
) -> Result<T, Error> {
    match value {
        // P5-76: the object's other entries, read in place, where a copy of
        // the object without `$class` used to be built and read.
        Value::Object(map) => T::deserialize(Strict(serde::de::value::MapDeserializer::new(
            map.iter()
                .filter(|(key, _)| *key != "$class")
                .map(|(key, value)| (key.as_str(), value)),
        ))),
        other => T::deserialize(Strict(other)),
    }
}

/// Reads one property onto `properties`, and what the read keeps of its
/// node onto `kept` (P5-93: pushed where they are read, as
/// [`DeclarationSeed`] pushes a declaration). `kept` is in step with
/// `properties` from the first property that has anything kept, and empty
/// until then.
struct PropertySeed<'a> {
    properties: &'a mut Vec<Property>,
    kept: &'a mut Vec<PropertyKept>,
}

impl PropertySeed<'_> {
    /// What the read keeps of the property just pushed onto `properties`.
    fn push_kept(self, kept: PropertyKept) {
        if !kept.is_empty() && self.kept.is_empty() {
            self.kept
                .resize_with(self.properties.len() - 1, PropertyKept::default);
        }
        if !self.kept.is_empty() || !kept.is_empty() {
            self.kept.push(kept);
        }
    }
}

impl<'de> DeserializeSeed<'de> for PropertySeed<'_> {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for PropertySeed<'_> {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a property object")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<(), A::Error> {
        read_property(ErrorBridge(map), self).map_err(de::Error::custom)
    }
}

/// One property of a class-like or enum declaration, read into the
/// generated struct its full metamodel `$class` names. The name checks
/// (identifier, reserved system name) are the loader's, after the read
/// ([`crate::introspect::Declaration`]).
///
/// P5-93: the keys almost every property node has (`name`, `isArray`,
/// `isOptional`, an object or relationship property's `type`, `decorators`
/// and `location`) are read here, each value through the very call the
/// generated struct would make for it (so with the same result, and the
/// same error), and the struct is built from them. From the first other
/// key (a validator, a `defaultValue`, a key the struct does not declare),
/// the node is read on by the generated struct, as before, with the
/// values read so far handed back to it first ([`Resume`]), so it
/// accepts and rejects exactly what it always has.
fn read_property<'de, A: MapAccess<'de, Error = Error>>(
    mut map: A,
    out: PropertySeed<'_>,
) -> Result<(), Error> {
    let class = match read_class(&mut map)? {
        Class::First(class) => class,
        Class::Reordered(node) => return out.deserialize(&node),
    };
    // TS matches the full metamodel `$class` (accordproject/concerto-rust#285).
    let Some(kind) = property_kind(&class) else {
        return Err(de::Error::custom(format_args!(
            "unrecognised property $class {class}"
        )));
    };
    let mut decorators = None;
    let mut node_decorators = None;
    let mut location = None;
    let mut read = PropertyKeys::default();
    // Which of the keys read here the generated struct declares.
    let flags = kind != "EnumProperty";
    let typed = matches!(kind, "ObjectProperty" | "RelationshipProperty");
    let next = loop {
        let Some(key) = map.next_key_seed(StrSeed)? else {
            break None;
        };
        match &*key {
            "name" => {
                if read.name.is_some() {
                    return Err(de::Error::duplicate_field("name"));
                }
                read.name = Some(map.next_value_seed(Strict(PhantomData::<Name>))?);
                read.order.push(KEY_NAME);
            }
            "isArray" if flags => {
                if read.is_array.is_some() {
                    return Err(de::Error::duplicate_field("isArray"));
                }
                read.is_array = Some(map.next_value_seed(Strict(PhantomData::<bool>))?);
                read.order.push(KEY_IS_ARRAY);
            }
            "isOptional" if flags => {
                if read.is_optional.is_some() {
                    return Err(de::Error::duplicate_field("isOptional"));
                }
                read.is_optional = Some(map.next_value_seed(Strict(PhantomData::<bool>))?);
                read.order.push(KEY_IS_OPTIONAL);
            }
            "type" if typed => {
                if read.type_.is_some() {
                    return Err(de::Error::duplicate_field("type"));
                }
                read.type_ = Some(map.next_value_seed(Strict(PhantomData::<mm::TypeIdentifier>))?);
                read.order.push(KEY_TYPE);
            }
            // As `Intercept` reads them.
            "decorators" => {
                // The generated struct's error for a repeated key.
                if decorators.is_some() {
                    return Err(de::Error::duplicate_field("decorators"));
                }
                let decoded = read_decorators(&mut map)?;
                node_decorators = decoded.node;
                decorators = Some(ReadDecorators {
                    list: decoded.list,
                    conforms: decoded.conforms,
                });
            }
            "location" => {
                if location.is_some() {
                    return Err(de::Error::duplicate_field("location"));
                }
                location = Some(map.next_value_seed(LocationSeed)?);
            }
            _ => break Some(key),
        }
    };
    let date_time_default = match next {
        None => {
            read.build(
                out.properties,
                &class,
                kind,
                node_decorators,
                &location,
                &mut decorators,
            )?;
            None
        }
        Some(key) => {
            let resume = Resume {
                read: Some(read),
                next: 0,
                pending: None,
                key: Some(key),
                inner: map,
            };
            let (property, date_time_default) = read_property_struct(
                resume,
                &class,
                kind,
                &mut decorators,
                node_decorators,
                &mut location,
            )?;
            out.properties.push(property);
            date_time_default
        }
    };
    out.push_kept(PropertyKept {
        location,
        unusual_decorators: !ReadDecorators::conforms(decorators.as_ref()),
        date_time_default,
    });
    Ok(())
}

/// A node's `decorators` value, as [`Intercept`] reads it.
fn read_decorators<'de, A: MapAccess<'de, Error = Error>>(
    map: &mut A,
) -> Result<Decorators, Error> {
    match map.next_value_seed(DecoratorsSeed)? {
        Ok(decoded) => Ok(decoded),
        Err(value) => Decorators::from_kept(value),
    }
}

/// The rest of a property node after the keys [`read_property`] read,
/// read by its generated struct through [`Intercept`], as every property
/// node was before P5-93: the property and the `defaultValue` the read
/// keeps of a `DateTimeProperty`.
// `class` is the `Cow` the read gave, which `Replay` hands back as it is
// (borrowed from the text, or owned).
#[allow(clippy::ptr_arg)]
fn read_property_struct<'de, A: MapAccess<'de, Error = Error>>(
    map: A,
    class: &Cow<'de, str>,
    kind: &str,
    decorators: &mut Option<ReadDecorators>,
    mut node_decorators: Option<Vec<mm::Decorator>>,
    location: &mut Option<Location>,
) -> Result<(Property, Option<Value>), Error> {
    let mut node_identified = None;
    let mut taken = Map::new();
    let access = Intercept {
        inner: map,
        properties: None,
        decorators,
        node_decorators: &mut node_decorators,
        node_identified: &mut node_identified,
        location,
        identified_conforms: None,
        // The reference parser writes a `defaultValue` on a
        // `DateTimeProperty`, which the metamodel does not declare (BC-19's
        // one tolerance, `instance::check_ast_shape`). It is kept out of the
        // struct, as before P5-61; BC-45 checks it when it is applied.
        take: if kind == "DateTimeProperty" {
            &["defaultValue"]
        } else {
            &[]
        },
        taken: &mut taken,
        pending: Pending::Other,
    };
    macro_rules! read {
        ($variant:ident, $node:ty) => {{
            let mut node = <$node>::deserialize(MapAccessDeserializer::new(access))?;
            node.decorators = node_decorators;
            node.location = read_location(location.as_ref())?;
            Property::$variant(WithDecorators::new(node, ReadDecorators::list(decorators)))
        }};
    }
    let property = match kind {
        "BooleanProperty" => read!(Boolean, mm::BooleanProperty),
        "StringProperty" => read!(String, mm::StringProperty),
        "IntegerProperty" => read!(Integer, mm::IntegerProperty),
        "LongProperty" => read!(Long, mm::LongProperty),
        "DoubleProperty" => read!(Double, mm::DoubleProperty),
        "DateTimeProperty" => read!(DateTime, mm::DateTimeProperty),
        "ObjectProperty" => read!(Object, mm::ObjectProperty),
        "RelationshipProperty" => read!(Relationship, mm::RelationshipProperty),
        // The generated struct has a `$class` field: hand it back.
        _ => {
            let replay = Replay {
                class: Some(class.clone()),
                inner: access,
            };
            let mut node = mm::EnumProperty::deserialize(MapAccessDeserializer::new(replay))?;
            node.decorators = node_decorators;
            node.location = read_location(location.as_ref())?;
            Property::Enum(WithDecorators::new(node, ReadDecorators::list(decorators)))
        }
    };
    Ok((property, taken.shift_remove("defaultValue")))
}

/// The keys [`read_property`] reads itself, by their index in
/// [`PROPERTY_KEYS`].
const KEY_NAME: u8 = 0;
const KEY_IS_ARRAY: u8 = 1;
const KEY_IS_OPTIONAL: u8 = 2;
const KEY_TYPE: u8 = 3;
const PROPERTY_KEYS: [&str; 4] = ["name", "isArray", "isOptional", "type"];

/// The values [`read_property`] has read of a property node's own keys,
/// and the order it read them in.
#[derive(Default)]
struct PropertyKeys {
    order: KeyOrder,
    name: Option<Name>,
    is_array: Option<bool>,
    is_optional: Option<bool>,
    type_: Option<mm::TypeIdentifier>,
}

/// The keys read, in order, each once.
#[derive(Default, Clone, Copy)]
struct KeyOrder {
    keys: [u8; 4],
    len: u8,
}

impl KeyOrder {
    fn push(&mut self, key: u8) {
        if let Some(slot) = self.keys.get_mut(usize::from(self.len)) {
            *slot = key;
            self.len += 1;
        }
    }

    fn get(&self, index: u8) -> Option<u8> {
        (index < self.len)
            .then(|| self.keys.get(usize::from(index)).copied())
            .flatten()
    }
}

impl PropertyKeys {
    /// The property, from a node that has no key but those
    /// [`read_property`] reads: the generated struct of `kind` as it
    /// decodes such a node (the same fields, the same missing-field
    /// errors, in its field order), with the node's decorators and
    /// location as [`read_property_struct`] sets them.
    fn build(
        self,
        out: &mut Vec<Property>,
        class: &str,
        kind: &str,
        node_decorators: Option<Vec<mm::Decorator>>,
        location: &Option<Location>,
        decorators: &mut Option<ReadDecorators>,
    ) -> Result<(), Error> {
        let name = self.name.ok_or_else(|| de::Error::missing_field("name"))?;
        let is_array = self.is_array.unwrap_or_default();
        let is_optional = self.is_optional.unwrap_or_default();
        let type_ = match kind {
            "ObjectProperty" | "RelationshipProperty" => {
                Some(self.type_.ok_or_else(|| de::Error::missing_field("type"))?)
            }
            _ => None,
        };
        let location = read_location(location.as_ref())?;
        let list = ReadDecorators::list(decorators);
        out.reserve(1);
        macro_rules! scalar {
            ($variant:ident, $node:ident { $($field:ident),* }) => {
                Property::$variant(WithDecorators::new(
                    mm::$node {
                        name,
                        is_array,
                        is_optional,
                        size_validator: None,
                        decorators: node_decorators,
                        location,
                        $($field: None,)*
                    },
                    list,
                ))
            };
        }
        out.push(match (kind, type_) {
            ("BooleanProperty", _) => scalar!(Boolean, BooleanProperty { default_value }),
            ("StringProperty", _) => scalar!(
                String,
                StringProperty {
                    default_value,
                    validator,
                    length_validator
                }
            ),
            ("IntegerProperty", _) => {
                scalar!(
                    Integer,
                    IntegerProperty {
                        default_value,
                        validator
                    }
                )
            }
            ("LongProperty", _) => scalar!(
                Long,
                LongProperty {
                    default_value,
                    validator
                }
            ),
            ("DoubleProperty", _) => scalar!(
                Double,
                DoubleProperty {
                    default_value,
                    validator
                }
            ),
            ("DateTimeProperty", _) => scalar!(DateTime, DateTimeProperty {}),
            ("ObjectProperty", Some(type_)) => Property::Object(WithDecorators::new(
                mm::ObjectProperty {
                    name,
                    is_array,
                    is_optional,
                    size_validator: None,
                    decorators: node_decorators,
                    location,
                    default_value: None,
                    type_,
                },
                list,
            )),
            ("RelationshipProperty", Some(type_)) => Property::Relationship(WithDecorators::new(
                mm::RelationshipProperty {
                    name,
                    is_array,
                    is_optional,
                    size_validator: None,
                    decorators: node_decorators,
                    location,
                    type_,
                },
                list,
            )),
            _ => Property::Enum(WithDecorators::new(
                mm::EnumProperty {
                    _class: concerto_metamodel::utils::class_name(class),
                    name,
                    decorators: node_decorators,
                    location,
                },
                list,
            )),
        });
        Ok(())
    }

    /// The value read for `key`, taken, to hand back to the generated
    /// struct ([`Resume`]).
    fn take(&mut self, key: u8) -> Result<Resumed, Error> {
        Ok(match key {
            KEY_NAME => Resumed::Str(self.name.take().unwrap_or_default().into_string()),
            KEY_IS_ARRAY => Resumed::Bool(self.is_array.unwrap_or_default()),
            KEY_IS_OPTIONAL => Resumed::Bool(self.is_optional.unwrap_or_default()),
            // An already-decoded node, handed back as the `Value` it
            // serializes to, which decodes to the same node.
            _ => Resumed::Value(serde_json::to_value(self.type_.take())?),
        })
    }
}

/// A value [`read_property`] read, handed back to the generated struct.
enum Resumed {
    Str(String),
    Bool(bool),
    Value(Value),
}

/// A property node's entries for its generated struct, after
/// [`read_property`] has read some of them: the keys it read (but for
/// `decorators` and `location`, which `Intercept` would hand the struct as
/// `null`, and which the caller sets on the node), each with its value, in
/// the order read, then the key it stopped at, then the rest of the node.
struct Resume<'de, A> {
    read: Option<PropertyKeys>,
    /// The next of `read`'s keys to hand over.
    next: u8,
    /// The value of the key just handed over, if it is one of `read`'s.
    pending: Option<Resumed>,
    /// The key [`read_property`] stopped at.
    key: Option<Cow<'de, str>>,
    inner: A,
}

impl<'de, A: MapAccess<'de, Error = Error>> MapAccess<'de> for Resume<'de, A> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        if let Some(read) = self.read.as_mut() {
            match read.order.get(self.next) {
                Some(key) => {
                    self.next += 1;
                    self.pending = Some(read.take(key)?);
                    let name = PROPERTY_KEYS.get(usize::from(key)).copied().unwrap_or("");
                    return seed
                        .deserialize(BorrowedStrDeserializer::new(name))
                        .map(Some);
                }
                None => self.read = None,
            }
        }
        match self.key.take() {
            Some(Cow::Borrowed(key)) => seed
                .deserialize(BorrowedStrDeserializer::new(key))
                .map(Some),
            Some(Cow::Owned(key)) => seed.deserialize(StringDeserializer::new(key)).map(Some),
            None => self.inner.next_key_seed(seed),
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        match self.pending.take() {
            Some(Resumed::Str(value)) => seed.deserialize(StringDeserializer::new(value)),
            Some(Resumed::Bool(value)) => seed.deserialize(BoolDeserializer::new(value)),
            Some(Resumed::Value(value)) => seed.deserialize(value),
            None => self.inner.next_value_seed(seed),
        }
    }
}

// ---------------------------------------------------------------------------
// Map adapters
// ---------------------------------------------------------------------------

/// What [`Intercept`] has just handed on a key for.
enum Pending {
    Properties,
    Decorators,
    Location,
    Identified,
    Other,
}

/// The entries of a node after its `$class`, handed to a generated struct,
/// with some keys intercepted on the way:
/// - `properties`, when `properties` is set: read as [`Property`]s (the
///   struct sees `[]`);
/// - `decorators`: read by [`DecoratorsSeed`] (P5-93; a [`Kept`] from P5-76,
///   a `Value` before), decoded strictly once for the caller to set on the
///   node (the struct is handed `null` for it; before P5-76 it decoded a
///   copy of the value again), with its processed decorators and BC-19's
///   verdict on it;
/// - `location`: read as a [`Location`] (P5-76; before, a `Value`), kept as
///   given for an error's location; the struct is handed `null` for it, and
///   [`read_location`] reads it;
/// - `identified`, when `identified` is set: read as a [`Kept`] (P5-76;
///   before, a `Value`), kept as given for BC-19's shape check, and decoded
///   strictly (through [`Strict`]) once, for the caller to set on the node
///   (the struct is handed `null` for it);
/// - every key in `take`: read as a `Value` into `taken` (a repeated key
///   replaces the earlier value, as in a `Value`), and kept from the struct,
///   for the loader to read as TS does (the module doc, "Strictness").
///
/// Every other value goes through [`Strict`].
struct Intercept<'a, A> {
    inner: A,
    /// Where the properties go, or `None` to pass `properties` through.
    properties: Option<&'a mut Option<ReadProperties>>,
    decorators: &'a mut Option<ReadDecorators>,
    /// The generated `decorators` of the node, decoded from `decorators`
    /// (the struct is handed `null` for it, and the caller sets it).
    node_decorators: &'a mut Option<Vec<mm::Decorator>>,
    location: &'a mut Option<Location>,
    /// Where the shape check's verdict on the `identified` value goes, or
    /// `None` to pass `identified` through.
    identified_conforms: Option<&'a mut bool>,
    /// The generated `identified` of the node, decoded from `identified`
    /// (the struct is handed `null` for it, and the caller sets it).
    node_identified: &'a mut Option<mm::Identified>,
    take: &'static [&'static str],
    taken: &'a mut Map<String, Value>,
    pending: Pending,
}

impl<'de, A: MapAccess<'de, Error = Error>> MapAccess<'de> for Intercept<'_, A> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        let key = loop {
            let Some(key) = self.inner.next_key_seed(StrSeed)? else {
                return Ok(None);
            };
            if !self.take.contains(&&*key) {
                break key;
            }
            let value: Value = self.inner.next_value()?;
            self.taken.insert(key.into_owned(), value);
        };
        self.pending = match &*key {
            "$class" => return Err(refuse("duplicate $class")),
            "properties" if self.properties.is_some() => Pending::Properties,
            "decorators" => Pending::Decorators,
            "location" => Pending::Location,
            "identified" if self.identified_conforms.is_some() => Pending::Identified,
            _ => Pending::Other,
        };
        match key {
            Cow::Borrowed(key) => seed.deserialize(BorrowedStrDeserializer::new(key)),
            Cow::Owned(key) => seed.deserialize(StringDeserializer::new(key)),
        }
        .map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        match std::mem::replace(&mut self.pending, Pending::Other) {
            Pending::Properties => {
                let slot = self
                    .properties
                    .as_deref_mut()
                    .expect("Pending::Properties is only set when intercepting properties");
                if slot.is_some() {
                    return Err(refuse("duplicate properties"));
                }
                *slot = Some(self.inner.next_value_seed(PropertiesSeed)?);
                seed.deserialize(Value::Array(Vec::new()))
            }
            Pending::Decorators => {
                // The generated struct's own error for a repeated key, which
                // it raises first, unless `read_property` read the first
                // one (P5-93).
                if self.decorators.is_some() {
                    return Err(de::Error::duplicate_field("decorators"));
                }
                // P5-76: read as a [`Kept`], not a `Value` (the `kept`
                // module doc), and decoded strictly (as the model's own
                // decorators are) once, for the caller to set on the node,
                // where the struct used to decode a copy of the value again
                // (it is handed `null`, as for `location`). P5-93: the
                // decorators and the shape check's verdict are taken from
                // it here, and the decode moves its strings out.
                // From P5-93, an array of plain decorator nodes is read
                // field by field into both, each string once
                // (`DecoratorsSeed`), with no `Kept` of it.
                let decoded = match self.inner.next_value_seed(DecoratorsSeed)? {
                    Ok(decoded) => decoded,
                    Err(value) => Decorators::from_kept(value)?,
                };
                *self.node_decorators = decoded.node;
                *self.decorators = Some(ReadDecorators {
                    list: decoded.list,
                    conforms: decoded.conforms,
                });
                seed.deserialize(Value::Null)
            }
            Pending::Location => {
                // As for `decorators`.
                if self.location.is_some() {
                    return Err(de::Error::duplicate_field("location"));
                }
                // The struct is handed `null`, and its `location` is read
                // from this value once it has been read ([`read_location`]),
                // so the value is kept without a copy. P5-76: kept as a
                // [`Location`], not a `Value` (the `kept` module doc).
                *self.location = Some(self.inner.next_value_seed(LocationSeed)?);
                seed.deserialize(Value::Null)
            }
            Pending::Identified => {
                // P5-76: decoded strictly once, for the caller to set on the
                // node, where the struct used to decode a copy of the value
                // (it is handed `null`, as for `location`). P5-93: the
                // metamodel's own two nodes are read field by field
                // ([`IdentifiedSeed`]), and the shape check's verdict is
                // taken here, where the value used to be kept for it.
                let (identified, kept) = self.inner.next_value_seed(IdentifiedSeed)?.decode()?;
                *self.node_identified = identified;
                if let Some(slot) = self.identified_conforms.as_deref_mut() {
                    *slot = kept.as_ref().is_none_or(shape::identified_conforms);
                }
                seed.deserialize(Value::Null)
            }
            Pending::Other => self.inner.next_value_seed(Strict(seed)),
        }
    }
}

/// A node's entries with its already-read `$class` put back in front, for
/// reading the whole node as a `Value`, or a struct with a `$class` field.
struct Replay<'de, A> {
    class: Option<Cow<'de, str>>,
    inner: A,
}

impl<'de, A: MapAccess<'de, Error = Error>> MapAccess<'de> for Replay<'de, A> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        if self.class.is_some() {
            return seed
                .deserialize(BorrowedStrDeserializer::new("$class"))
                .map(Some);
        }
        self.inner.next_key_seed(seed)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        match self.class.take() {
            // P5-93: a `$class` borrowed from the text is replayed as it
            // is (a generated struct interns it), not copied.
            Some(Cow::Borrowed(class)) => seed.deserialize(BorrowedStrDeserializer::new(class)),
            Some(Cow::Owned(class)) => seed.deserialize(StringDeserializer::new(class)),
            None => self.inner.next_value_seed(seed),
        }
    }
}

/// Gives a `serde_json` map (whose error type a generic visitor cannot
/// name) the concrete `serde_json::Error` type the readers above need.
struct ErrorBridge<A>(A);

impl<'de, A: MapAccess<'de>> MapAccess<'de> for ErrorBridge<A> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        self.0.next_key_seed(seed).map_err(de::Error::custom)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        self.0.next_value_seed(seed).map_err(de::Error::custom)
    }

    fn size_hint(&self) -> Option<usize> {
        self.0.size_hint()
    }
}

/// A string, borrowed from the text when it has no escapes.
#[derive(Clone, Copy)]
struct StrSeed;

impl<'de> DeserializeSeed<'de> for StrSeed {
    type Value = Cow<'de, str>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_str(self)
    }
}

impl<'de> Visitor<'de> for StrSeed {
    type Value = Cow<'de, str>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a string")
    }

    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Self::Value, E> {
        Ok(Cow::Borrowed(v))
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Self::Value, E> {
        Ok(Cow::Owned(v.to_string()))
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Self::Value, E> {
        Ok(Cow::Owned(v))
    }
}

// ---------------------------------------------------------------------------
// Strict: skip a value only after checking it as a `Value` parse would
// ---------------------------------------------------------------------------

/// Wraps a deserializer, at every depth, so that a value its visitor would
/// skip (`deserialize_ignored_any`) is parsed into a `Value` and dropped
/// instead (module doc, "JSON syntax").
pub(crate) struct Strict<T>(pub(crate) T);

impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for Strict<S> {
    type Value = S::Value;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<S::Value, D::Error> {
        self.0.deserialize(Strict(d))
    }
}

macro_rules! forward_deserialize {
    ($($method:ident($($arg:ident: $ty:ty),*);)*) => {$(
        fn $method<V: Visitor<'de>>(self, $($arg: $ty,)* visitor: V) -> Result<V::Value, D::Error> {
            self.0.$method($($arg,)* Strict(visitor))
        }
    )*};
}

impl<'de, D: Deserializer<'de>> Deserializer<'de> for Strict<D> {
    type Error = D::Error;

    forward_deserialize! {
        deserialize_any();
        deserialize_bool();
        deserialize_i8();
        deserialize_i16();
        deserialize_i32();
        deserialize_i64();
        deserialize_i128();
        deserialize_u8();
        deserialize_u16();
        deserialize_u32();
        deserialize_u64();
        deserialize_u128();
        deserialize_f32();
        deserialize_f64();
        deserialize_char();
        deserialize_str();
        deserialize_string();
        deserialize_bytes();
        deserialize_byte_buf();
        deserialize_option();
        deserialize_unit();
        deserialize_unit_struct(name: &'static str);
        deserialize_newtype_struct(name: &'static str);
        deserialize_seq();
        deserialize_tuple(len: usize);
        deserialize_tuple_struct(name: &'static str, len: usize);
        deserialize_map();
        deserialize_struct(name: &'static str, fields: &'static [&'static str]);
        deserialize_enum(name: &'static str, variants: &'static [&'static str]);
        deserialize_identifier();
    }

    /// A generated struct skips the value of a key it does not declare:
    /// that is an error (the module doc, "Unknown keys").
    fn deserialize_ignored_any<V: Visitor<'de>>(self, _visitor: V) -> Result<V::Value, D::Error> {
        Err(de::Error::custom("unknown field"))
    }

    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}

macro_rules! forward_visit {
    ($($method:ident($ty:ty);)*) => {$(
        fn $method<E: de::Error>(self, v: $ty) -> Result<V::Value, E> {
            self.0.$method(v)
        }
    )*};
}

impl<'de, V: Visitor<'de>> Visitor<'de> for Strict<V> {
    type Value = V::Value;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        self.0.expecting(f)
    }

    forward_visit! {
        visit_bool(bool);
        visit_i8(i8);
        visit_i16(i16);
        visit_i32(i32);
        visit_i64(i64);
        visit_i128(i128);
        visit_u8(u8);
        visit_u16(u16);
        visit_u32(u32);
        visit_u64(u64);
        visit_u128(u128);
        visit_f32(f32);
        visit_f64(f64);
        visit_char(char);
        visit_str(&str);
        visit_borrowed_str(&'de str);
        visit_string(String);
        visit_bytes(&[u8]);
        visit_borrowed_bytes(&'de [u8]);
        visit_byte_buf(Vec<u8>);
    }

    fn visit_none<E: de::Error>(self) -> Result<V::Value, E> {
        self.0.visit_none()
    }

    fn visit_unit<E: de::Error>(self) -> Result<V::Value, E> {
        self.0.visit_unit()
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<V::Value, D::Error> {
        self.0.visit_some(Strict(d))
    }

    fn visit_newtype_struct<D: Deserializer<'de>>(self, d: D) -> Result<V::Value, D::Error> {
        self.0.visit_newtype_struct(Strict(d))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<V::Value, A::Error> {
        self.0.visit_seq(Strict(seq))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<V::Value, A::Error> {
        self.0.visit_map(Strict(map))
    }

    fn visit_enum<A: de::EnumAccess<'de>>(self, data: A) -> Result<V::Value, A::Error> {
        self.0.visit_enum(Strict(data))
    }
}

impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for Strict<A> {
    type Error = A::Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, A::Error> {
        self.0.next_element_seed(Strict(seed))
    }

    fn size_hint(&self) -> Option<usize> {
        self.0.size_hint()
    }
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for Strict<A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, A::Error> {
        self.0.next_key_seed(Strict(seed))
    }

    fn next_value_seed<T: DeserializeSeed<'de>>(&mut self, seed: T) -> Result<T::Value, A::Error> {
        self.0.next_value_seed(Strict(seed))
    }

    fn size_hint(&self) -> Option<usize> {
        self.0.size_hint()
    }
}

impl<'de, A: de::EnumAccess<'de>> de::EnumAccess<'de> for Strict<A> {
    type Error = A::Error;
    type Variant = Strict<A::Variant>;

    fn variant_seed<T: DeserializeSeed<'de>>(
        self,
        seed: T,
    ) -> Result<(T::Value, Self::Variant), A::Error> {
        self.0
            .variant_seed(Strict(seed))
            .map(|(value, variant)| (value, Strict(variant)))
    }
}

impl<'de, A: de::VariantAccess<'de>> de::VariantAccess<'de> for Strict<A> {
    type Error = A::Error;

    fn unit_variant(self) -> Result<(), A::Error> {
        self.0.unit_variant()
    }

    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, A::Error> {
        self.0.newtype_variant_seed(Strict(seed))
    }

    fn tuple_variant<V: Visitor<'de>>(self, len: usize, visitor: V) -> Result<V::Value, A::Error> {
        self.0.tuple_variant(len, Strict(visitor))
    }

    fn struct_variant<V: Visitor<'de>>(
        self,
        fields: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, A::Error> {
        self.0.struct_variant(fields, Strict(visitor))
    }
}

#[cfg(test)]
mod property_read_tests;
#[cfg(test)]
mod tests;
