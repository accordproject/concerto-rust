//! Reading a model's JSON AST text straight into the typed model, without
//! building a [`serde_json::Value`] for the whole document first (P5-06c
//! spike, accordproject/concerto-rust#234; adopted in P5-06d,
//! accordproject/concerto-rust#239).
//!
//! [`crate::ModelFile::from_json`] reads a `Value` that the caller has
//! already parsed. When the caller holds JSON text (the WASM bindings), that
//! parse is about half of the load. [`parse`] reads the text instead: each
//! class-like declaration (concept, asset, participant, transaction, event)
//! and enum declaration, and each of their properties, is deserialized
//! straight into its generated `mm::*` struct. The rest of the document stays
//! as small `Value` subtrees and goes through the existing loaders: the
//! model's own header keys (`namespace`, `imports`, `decorators`, and so on),
//! every decorator list, the few property and declaration fields the `Value`
//! path reads untyped (a class's `name`, `superType` and `identified`, a
//! property's validators, an `ObjectProperty`'s `type`), and every scalar and
//! map declaration (whose loaders read the raw node throughout).
//!
//! # Error parity
//!
//! The typed path is a fast path only. It never reports an error of its
//! own. [`parse`] returns `None`, and the typed loader returns an error,
//! whenever the document is not in the shape it reads, or would fail any
//! check the `Value` path makes. [`crate::ModelFile::from_json_text`] then
//! throws that result away and loads the same text through the `Value`
//! path, which raises the catalogued, TS-matching error, in TS's order.
//! Error parity therefore only needs one property: **whenever the typed
//! path succeeds, the `Value` path succeeds too, with the same result.**
//! It is kept as follows.
//!
//! - **JSON syntax.** `serde_json` checks less when it skips a value
//!   (`deserialize_ignored_any`) than when it builds one: a skipped `1e400`,
//!   lone surrogate escape or over-deep array is accepted where a `Value`
//!   parse rejects it. Every value the generated structs skip is read
//!   through [`Strict`], which builds and drops a `Value` for it instead, so
//!   typed success implies that the text parses as a `Value` too.
//! - **Shape.** A node's `$class` must be its first key (as every AST that
//!   `concerto-cto` or `JSON.stringify` writes has it). Anything a `Value`
//!   would take but this reader cannot is refused: a duplicate `$class`,
//!   `properties` or `decorators` key, or a duplicate struct field (a
//!   `Value` keeps the last), or a non-object node.
//! - **Checks.** Every check [`crate::introspect::Declaration::from_model_json`]
//!   and [`crate::introspect::Property`]'s loader make is either implied by
//!   the typed read (a required field, a `$class`), made by the very same
//!   code on the same fields ([`normalize_class_fields`],
//!   [`Property::set_ast_validators`]), or made again, as a rejection, in
//!   [`read_property`] and [`crate::introspect::Declaration::from_typed`].
//!   Everything after the read (the implicit super type, the system fields,
//!   the validator checks, the namespace and imports) is the same code on
//!   both paths.
//!
//! The differential test at the end of this module loads every model AST it
//! can find (the in-repo coverage set, the benchmark model sets, and the
//! oracle corpus and its CTO cache when `CONCERTO_ORACLE_FIXTURES` is set),
//! plus mutated copies of each, through both paths and requires identical
//! results. It also fails when a model that loads does not take the typed
//! path (the drift guard: a metamodel change the typed structs miss would
//! otherwise only lose the speedup, silently).

use std::borrow::Cow;
use std::fmt;

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use serde::Deserialize;
use serde::de::value::{BorrowedStrDeserializer, MapAccessDeserializer, StringDeserializer};
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Value};

use crate::introspect::METAMODEL_NAMESPACE;
use crate::introspect::declaration::{ClassKind, ClassNode, normalize_class_fields};
use crate::introspect::decorator::{WithDecorators, parse_decorator_list};
use crate::introspect::property::{
    Property, ast_validator_keys, object_type_placeholder, property_kind,
};
use crate::model_util::{is_system_property, is_valid_identifier};

type Error = serde_json::Error;

/// A model AST read by [`parse`].
pub(crate) struct TypedModel {
    /// Every top-level key but `declarations`, as the `Value` path would
    /// read it.
    pub(crate) header: Value,
    /// The declarations, in order; empty when the AST has none.
    pub(crate) declarations: Vec<TypedDeclaration>,
}

/// One declaration read by [`parse`].
#[allow(clippy::large_enum_variant)]
pub(crate) enum TypedDeclaration {
    /// A class-like declaration, read straight into its generated struct.
    /// The node's own `properties` is left empty; they are in `properties`.
    Class {
        kind: ClassKind,
        node: ClassNode,
        /// Whether the AST gave `superType.name: null`
        /// ([`normalize_class_fields`]).
        explicit_null_super_type: bool,
        properties: Vec<Property>,
        /// For each property, in order, the keys of its AST node that the
        /// `Value` path reads untyped ([`ast_validator_keys`]).
        raw_properties: Vec<Value>,
        /// The node's `decorators` value, if it has that key.
        decorators: Option<Value>,
    },
    /// An enum declaration, read straight into its generated struct, whose
    /// `properties` are the enum values' own nodes.
    Enum {
        node: mm::EnumDeclaration,
        values: Vec<Property>,
        /// The node's `decorators` value, if it has that key.
        decorators: Option<Value>,
    },
    /// Any other declaration (a scalar or map declaration, or anything
    /// unrecognised), as its JSON subtree.
    Ast(Value),
}

/// Reads a model AST from JSON text, or `None` when the typed path does
/// not take it (module doc).
pub(crate) fn parse(text: &str) -> Option<TypedModel> {
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let model = ModelSeed.deserialize(&mut deserializer).ok()?;
    deserializer.end().ok()?;
    Some(model)
}

/// The error the typed path returns to give up; never shown to a caller.
fn refuse(why: &str) -> Error {
    de::Error::custom(format_args!("typed AST path: {why}"))
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
        let mut header = Map::new();
        let mut declarations = None;
        while let Some(key) = map.next_key::<String>()? {
            if key == "declarations" {
                if declarations.is_some() {
                    return Err(de::Error::custom("typed AST path: duplicate declarations"));
                }
                declarations = Some(map.next_value_seed(SeqOf(DeclarationSeed))?);
            } else {
                // A duplicate key replaces the earlier value, as in a `Value`.
                header.insert(key, map.next_value::<Value>()?);
            }
        }
        Ok(TypedModel {
            header: Value::Object(header),
            declarations: declarations.unwrap_or_default(),
        })
    }
}

/// A JSON array of whatever `S` reads.
struct SeqOf<S>(S);

impl<'de, S: DeserializeSeed<'de> + Copy> DeserializeSeed<'de> for SeqOf<S> {
    type Value = Vec<S::Value>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_seq(self)
    }
}

impl<'de, S: DeserializeSeed<'de> + Copy> Visitor<'de> for SeqOf<S> {
    type Value = Vec<S::Value>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an array")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(item) = seq.next_element_seed(self.0)? {
            items.push(item);
        }
        Ok(items)
    }
}

/// Reads a node's first key, which must be `$class`, and its string value.
fn read_class<'de, A: MapAccess<'de, Error = Error>>(map: &mut A) -> Result<Cow<'de, str>, Error> {
    match map.next_key_seed(StrSeed)? {
        Some(key) if key == "$class" => map.next_value_seed(StrSeed),
        _ => Err(refuse("$class is not the first key")),
    }
}

#[derive(Clone, Copy)]
struct DeclarationSeed;

impl<'de> DeserializeSeed<'de> for DeclarationSeed {
    type Value = TypedDeclaration;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<TypedDeclaration, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for DeclarationSeed {
    type Value = TypedDeclaration;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a declaration object")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<TypedDeclaration, A::Error> {
        // The generic `A` has only its own error type; every map this
        // reader is handed comes from `serde_json`, so go through that.
        read_declaration(ErrorBridge(map)).map_err(de::Error::custom)
    }
}

/// A declaration's short `$class` name, when it is a metamodel one.
fn metamodel_kind(class: &str) -> Option<&str> {
    class
        .strip_prefix(METAMODEL_NAMESPACE)
        .and_then(|rest| rest.strip_prefix('.'))
}

fn read_declaration<'de, A: MapAccess<'de, Error = Error>>(
    mut map: A,
) -> Result<TypedDeclaration, Error> {
    let class = read_class(&mut map)?;
    let short = metamodel_kind(&class);
    if short == Some("EnumDeclaration") {
        return read_enum(map);
    }
    let Some(kind) = short.and_then(ClassKind::from_short) else {
        // Neither class-like nor an enum: the subtree the `Value` path would
        // have, `$class` first, for `Declaration::from_model_json`.
        let replay = Replay {
            class: Some(class),
            inner: map,
        };
        return Value::deserialize(MapAccessDeserializer::new(replay)).map(TypedDeclaration::Ast);
    };
    let mut properties = None;
    let mut decorators = None;
    let mut taken = Map::new();
    let mut explicit_null_super_type = false;
    // `ClassDeclaration::from_json` decodes the struct from these three
    // fields as `normalize_class_fields` leaves them.
    let mut normalize = |taken: &Map<String, Value>| {
        let mut fields = taken.clone();
        explicit_null_super_type = normalize_class_fields(&mut fields);
        fields.into_iter().collect()
    };
    let access = Intercept {
        inner: map,
        properties: Some(&mut properties),
        decorators: &mut decorators,
        take: &["name", "superType", "identified"],
        taken: &mut taken,
        put_back: Some(&mut normalize),
        tail: None,
        pending: Pending::Other,
    };
    let de = MapAccessDeserializer::new(access);
    let node = match kind {
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
    let (properties, raw_properties) = properties
        .ok_or_else(|| refuse("no properties"))?
        .into_iter()
        .unzip();
    Ok(TypedDeclaration::Class {
        kind,
        node,
        explicit_null_super_type,
        properties,
        raw_properties,
        decorators,
    })
}

/// An enum declaration, after its `$class`: what `EnumDeclaration::from_json`
/// accepts, where each value is an `EnumProperty`.
fn read_enum<'de, A: MapAccess<'de, Error = Error>>(map: A) -> Result<TypedDeclaration, Error> {
    let mut properties = None;
    let mut decorators = None;
    let mut taken = Map::new();
    let access = Intercept {
        inner: map,
        properties: Some(&mut properties),
        decorators: &mut decorators,
        take: &[],
        taken: &mut taken,
        put_back: None,
        tail: None,
        pending: Pending::Other,
    };
    let mut node = mm::EnumDeclaration::deserialize(MapAccessDeserializer::new(access))?;
    let values: Vec<Property> = properties
        .ok_or_else(|| refuse("no properties"))?
        .into_iter()
        .map(|(value, _)| value)
        .collect();
    // `EnumDeclaration::from_json` decodes each value's node as an
    // `mm::EnumProperty` into the struct as well; that is the node
    // `read_property` read for an `EnumProperty`. A value of any other kind
    // (which that decode may still accept) is left to the `Value` path.
    node.properties = values
        .iter()
        .map(|value| match value {
            Property::Enum(value) => Ok((**value).clone()),
            _ => Err(refuse("enum value is not an EnumProperty")),
        })
        .collect::<Result<_, _>>()?;
    Ok(TypedDeclaration::Enum {
        node,
        values,
        decorators,
    })
}

#[derive(Clone, Copy)]
struct PropertySeed;

impl<'de> DeserializeSeed<'de> for PropertySeed {
    type Value = (Property, Value);

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_map(self)
    }
}

impl<'de> Visitor<'de> for PropertySeed {
    type Value = (Property, Value);

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a property object")
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        read_property(ErrorBridge(map)).map_err(de::Error::custom)
    }
}

/// One property of a class-like or enum declaration: what
/// `parse_properties` and `Property::try_from` accept, and nothing they
/// reject. Also returns the keys of the node that the `Value` path reads
/// untyped ([`ast_validator_keys`]), as an object.
fn read_property<'de, A: MapAccess<'de, Error = Error>>(
    mut map: A,
) -> Result<(Property, Value), Error> {
    let class = read_class(&mut map)?;
    // `Property::try_from` matches the full metamodel `$class`, as TS does
    // (accordproject/concerto-rust#285): anything else is left to the
    // `Value` path, which reports it.
    let kind = property_kind(&class).ok_or_else(|| refuse("unrecognised property $class"))?;
    let mut decorators = None;
    let mut taken = Map::new();
    // `Property::try_from` gives an `ObjectProperty` with no (or a `null`)
    // `type` a placeholder one.
    let mut object_type = |taken: &Map<String, Value>| match taken.get("type") {
        Some(value) if !value.is_null() => vec![("type".to_string(), value.clone())],
        _ => vec![("type".to_string(), object_type_placeholder())],
    };
    let is_object = kind == "ObjectProperty";
    let take: &[&str] = match kind {
        "StringProperty" => &["sizeValidator", "lengthValidator", "validator"],
        "ObjectProperty" => &["sizeValidator", "type"],
        _ => &["sizeValidator"],
    };
    debug_assert!(
        ast_validator_keys(kind)
            .iter()
            .all(|key| take.contains(key))
    );
    let access = Intercept {
        inner: map,
        properties: None,
        decorators: &mut decorators,
        take,
        taken: &mut taken,
        put_back: if is_object {
            Some(&mut object_type)
        } else {
            None
        },
        tail: None,
        pending: Pending::Other,
    };
    macro_rules! read {
        ($variant:ident, $node:ty) => {{
            let node = <$node>::deserialize(MapAccessDeserializer::new(access))?;
            Property::$variant(WithDecorators::new(
                node,
                parse_decorator_list(decorators.as_ref()),
            ))
        }};
    }
    let mut property = match kind {
        "BooleanProperty" => read!(Boolean, mm::BooleanProperty),
        "StringProperty" => read!(String, mm::StringProperty),
        "IntegerProperty" => read!(Integer, mm::IntegerProperty),
        "LongProperty" => read!(Long, mm::LongProperty),
        "DoubleProperty" => read!(Double, mm::DoubleProperty),
        "DateTimeProperty" => read!(DateTime, mm::DateTimeProperty),
        "ObjectProperty" => read!(Object, mm::ObjectProperty),
        // A missing or `null` `type` fails the generated struct here, where
        // the `Value` path rejects it (DV-017): the `Value` path runs.
        "RelationshipProperty" => read!(Relationship, mm::RelationshipProperty),
        "EnumProperty" => {
            // The generated struct has a `$class` field: hand it back.
            let replay = Replay {
                class: Some(class.clone()),
                inner: access,
            };
            let node = mm::EnumProperty::deserialize(MapAccessDeserializer::new(replay))?;
            Property::Enum(WithDecorators::new(
                node,
                parse_decorator_list(decorators.as_ref()),
            ))
        }
        _ => return Err(refuse("unrecognised property $class")),
    };
    // `parse_properties`' system-name check, and `Property::try_from`'s
    // null-decorator, system-name and identifier checks. A missing, `null`
    // or non-string `name` has already failed the generated struct.
    let name = property.name();
    if is_system_property(name)
        || !is_valid_identifier(name)
        || decorators
            .as_ref()
            .and_then(Value::as_array)
            .is_some_and(|items| items.iter().any(Value::is_null))
    {
        return Err(refuse("rejected property"));
    }
    taken.remove("type");
    property.set_ast_validators(|key| taken.get(key));
    Ok((property, Value::Object(taken)))
}

// ---------------------------------------------------------------------------
// Map adapters
// ---------------------------------------------------------------------------

/// What [`Intercept`] has just handed on a key for.
enum Pending {
    Properties,
    Decorators,
    Other,
    /// One of the entries `put_back` gave, handed on after `inner`'s own.
    Tail(Value),
}

/// Makes the entries handed back to the struct from the taken ones.
type PutBack<'a> = dyn FnMut(&Map<String, Value>) -> Vec<(String, Value)> + 'a;

/// The entries of a node after its `$class`, handed to a generated struct,
/// with some keys taken out on the way:
/// - `properties`, when `properties` is set: read as [`Property`]s (the
///   struct sees `[]`, as `ClassDeclaration::from_json` gives it);
/// - `decorators`: read as a `Value`, which both the struct and
///   [`parse_decorator_list`] then read, as on the `Value` path;
/// - every key in `take`: read as a `Value` into `taken` (a repeated key
///   replaces the earlier value, as in a `Value`), and kept from the struct.
///   Once `inner` has no more entries, `put_back`, when set, makes the
///   entries the struct is handed next from them.
///
/// Every other value goes through [`Strict`].
struct Intercept<'a, 'p, A> {
    inner: A,
    /// Where the properties go, or `None` to pass `properties` through.
    properties: Option<&'a mut Option<Vec<(Property, Value)>>>,
    decorators: &'a mut Option<Value>,
    take: &'a [&'a str],
    taken: &'a mut Map<String, Value>,
    put_back: Option<&'a mut PutBack<'p>>,
    /// The entries `put_back` made, once `inner` has run out.
    tail: Option<std::vec::IntoIter<(String, Value)>>,
    pending: Pending,
}

impl<'de, A: MapAccess<'de, Error = Error>> MapAccess<'de> for Intercept<'_, '_, A> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        loop {
            if let Some(tail) = &mut self.tail {
                let Some((key, value)) = tail.next() else {
                    return Ok(None);
                };
                self.pending = Pending::Tail(value);
                return seed.deserialize(StringDeserializer::new(key)).map(Some);
            }
            let Some(key) = self.inner.next_key_seed(StrSeed)? else {
                let tail = match self.put_back.as_deref_mut() {
                    Some(put_back) => put_back(self.taken),
                    None => Vec::new(),
                };
                self.tail = Some(tail.into_iter());
                continue;
            };
            if self.take.contains(&&*key) {
                let value: Value = self.inner.next_value()?;
                self.taken.insert(key.into_owned(), value);
                continue;
            }
            self.pending = match &*key {
                "$class" => return Err(refuse("duplicate $class")),
                "properties" if self.properties.is_some() => Pending::Properties,
                "decorators" => Pending::Decorators,
                _ => Pending::Other,
            };
            return match key {
                Cow::Borrowed(key) => seed.deserialize(BorrowedStrDeserializer::new(key)),
                Cow::Owned(key) => seed.deserialize(StringDeserializer::new(key)),
            }
            .map(Some);
        }
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
                *slot = Some(self.inner.next_value_seed(SeqOf(PropertySeed))?);
                seed.deserialize(Value::Array(Vec::new()))
            }
            Pending::Decorators => {
                if self.decorators.is_some() {
                    return Err(refuse("duplicate decorators"));
                }
                let value: Value = self.inner.next_value()?;
                let read = seed.deserialize(value.clone())?;
                *self.decorators = Some(value);
                Ok(read)
            }
            Pending::Tail(value) => seed.deserialize(value),
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
            Some(class) => seed.deserialize(StringDeserializer::new(class.into_owned())),
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
struct Strict<T>(T);

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

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, D::Error> {
        Value::deserialize(self.0)?;
        visitor.visit_unit()
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
mod tests {
    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use serde_json::{Value, json};

    use crate::ModelFile;
    use crate::introspect::Decorated;

    /// Everything a [`ModelFile`] holds, in a stable order (its own `Debug`
    /// prints a `HashMap` whose order varies between instances).
    fn key(file: &ModelFile) -> String {
        format!(
            "{:?}",
            (
                file.namespace(),
                file.version(),
                file.imports(),
                file.declarations(),
                file.file_name(),
                file.ast(),
                file.decorators(),
                file.concerto_version(),
                file.definitions(),
                file.is_external(),
            )
        )
    }

    /// What a load gives, comparable across the two paths.
    #[derive(Debug, PartialEq)]
    enum Outcome {
        NotJson(String),
        Loaded(String),
        Failed(String),
    }

    fn by_value(text: &str) -> Outcome {
        match serde_json::from_str::<Value>(text) {
            Err(e) => Outcome::NotJson(e.to_string()),
            Ok(value) => outcome(ModelFile::from_owned_json_with_definitions(
                value,
                None,
                Some("m.cto".into()),
            )),
        }
    }

    fn outcome(result: crate::Result<ModelFile>) -> Outcome {
        match result {
            Ok(file) => Outcome::Loaded(key(&file)),
            Err(e) => Outcome::Failed(format!("{e:?} / {e}")),
        }
    }

    /// Loads `text` both ways; panics unless the results are identical.
    /// Returns whether the typed path took it.
    fn check(text: &str) -> bool {
        let (typed, took_typed) = match ModelFile::from_json_text(text, None, Some("m.cto".into()))
        {
            Err(e) => (Outcome::NotJson(e.to_string()), false),
            Ok(result) => {
                let took = result.as_ref().is_ok_and(ModelFile::built_by_typed_path);
                (outcome(result), took)
            }
        };
        assert_eq!(
            typed,
            by_value(text),
            "typed and Value paths differ for {text}"
        );
        took_typed
    }

    fn model(declarations: Value) -> Value {
        json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": declarations,
        })
    }

    fn concept(properties: Value) -> Value {
        json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Thing",
            "isAbstract": false,
            "properties": properties,
        })
    }

    fn string_property(name: &str) -> Value {
        json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": name,
            "isArray": false,
            "isOptional": false,
        })
    }

    #[test]
    fn a_well_formed_model_takes_the_typed_path() {
        let text = model(json!([
            concept(json!([string_property("a"), {
                "$class": "concerto.metamodel@1.0.0.RelationshipProperty",
                "name": "r",
                "type": {"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Thing"},
                "decorators": [{"$class": "concerto.metamodel@1.0.0.Decorator", "name": "d", "arguments": [
                    {"$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "x"}
                ]}],
            }])),
            {"$class": "concerto.metamodel@1.0.0.EnumDeclaration", "name": "E", "properties": [
                {"$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "ONE"}
            ]},
        ]))
        .to_string();
        assert!(check(&text));
    }

    #[test]
    fn the_ast_is_parsed_from_the_kept_text_on_first_use() {
        let value = model(json!([concept(json!([string_property("a")]))]));
        let file = ModelFile::from_json_text(&value.to_string(), None, None)
            .unwrap()
            .unwrap();
        assert!(file.built_by_typed_path());
        assert_eq!(file.ast(), &value);
        let same = ModelFile::from_json(&value, None).unwrap();
        assert!(file.same_ast(&same) && same.same_ast(&file));
    }

    #[test]
    fn a_value_json_rejects_is_rejected_even_where_the_typed_path_skips_it() {
        // `serde_json` skips an unknown field without checking it the way a
        // `Value` parse does; `Strict` must not let these through.
        let concept = r#"{"$class":"concerto.metamodel@1.0.0.ConceptDeclaration","name":"T","properties":[],"x":"#;
        for skipped in [
            "1e400",
            "-1e400",
            r#""\ud800""#,
            r#""\udc00x""#,
            &format!("{}{}", "[".repeat(200), "]".repeat(200)),
        ] {
            let text = format!(
                r#"{{"$class":"concerto.metamodel@1.0.0.Model","namespace":"org.acme@1.0.0","declarations":[{concept}{skipped}}}]}}"#
            );
            assert!(
                ModelFile::from_json_text(&text, None, None).is_err(),
                "{text}"
            );
            assert!(!check(&text));
        }
    }

    #[test]
    fn shapes_the_typed_path_does_not_read_fall_back_to_the_value_path() {
        let property = r#"{"$class":"concerto.metamodel@1.0.0.StringProperty","name":"a"}"#;
        for declaration in [
            // `$class` not first.
            format!(
                r#"{{"name":"T","$class":"concerto.metamodel@1.0.0.ConceptDeclaration","properties":[{property}]}}"#
            ),
            // A duplicate `$class`: a `Value` keeps the last.
            format!(
                r#"{{"$class":"concerto.metamodel@1.0.0.ConceptDeclaration","name":"T","$class":"concerto.metamodel@1.0.0.AssetDeclaration","properties":[{property}]}}"#
            ),
            // Duplicate `properties`.
            format!(
                r#"{{"$class":"concerto.metamodel@1.0.0.ConceptDeclaration","name":"T","properties":[],"properties":[{property}]}}"#
            ),
            // A duplicate struct field: a `Value` keeps the last.
            format!(
                r#"{{"$class":"concerto.metamodel@1.0.0.ConceptDeclaration","name":"T","isAbstract":true,"isAbstract":false,"properties":[{property}]}}"#
            ),
            // An enum value that is not an `EnumProperty`.
            format!(
                r#"{{"$class":"concerto.metamodel@1.0.0.EnumDeclaration","name":"E","properties":[{property}]}}"#
            ),
        ] {
            let text = format!(
                r#"{{"$class":"concerto.metamodel@1.0.0.Model","namespace":"org.acme@1.0.0","declarations":[{declaration}]}}"#
            );
            assert!(!check(&text), "{text}");
        }
    }

    #[test]
    fn fields_the_value_path_reads_untyped_take_the_typed_path() {
        // Each is normalized or rebuilt from the raw AST by the same code on
        // both paths (`normalize_class_fields`, `Property::set_ast_validators`,
        // `object_type_placeholder`), so these shapes need no fallback.
        let declarations = [
            // An `ObjectProperty` with no (or a `null`) `type`.
            concept(json!([{"$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "o"}])),
            concept(
                json!([{"$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "o", "type": null}]),
            ),
            // A wrongly-typed validator, which TS reads untyped.
            concept(json!([{
                "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "s",
                "lengthValidator": {"$class": "concerto.metamodel@1.0.0.StringLengthValidator", "minLength": "2", "maxLength": [1]},
                "validator": {"$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": 1, "flags": null},
                "sizeValidator": {"$class": "concerto.metamodel@1.0.0.CollectionSizeValidator", "minSize": true},
            }])),
            concept(
                json!([{"$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "i", "isArray": true,
                "sizeValidator": {"$class": "concerto.metamodel@1.0.0.CollectionSizeValidator", "minSize": 3, "maxSize": 1}}]),
            ),
            // A class `name` TS coerces, and `superType`/`identified` shapes
            // `normalize_class_fields` rewrites.
            json!({"$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": ["C"], "properties": []}),
            json!({"$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "C", "properties": [],
                "superType": {"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": null}}),
            json!({"$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "C", "properties": [],
                "superType": {"$class": "concerto.metamodel@1.0.0.TypeIdentifier"}}),
            json!({"$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "C", "properties": [],
                "superType": {"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": 0}}),
            json!({"$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "C", "properties": [], "identified": false}),
            json!({"$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "C", "properties": [],
                "identified": {"$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": ""}}),
            json!({"$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "C", "properties": [],
                "identified": {"$class": "Nope"}}),
            json!({"$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "C", "properties": [],
                "identified": "yes"}),
            // An enum, with decorated values.
            json!({"$class": "concerto.metamodel@1.0.0.EnumDeclaration", "name": "E", "properties": [
                {"$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "ONE",
                 "decorators": [{"$class": "concerto.metamodel@1.0.0.Decorator", "name": "d"}]},
                {"$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "TWO", "sizeValidator": 1},
            ]}),
        ];
        let mut loaded = 0;
        for declaration in declarations {
            let text = model(json!([declaration])).to_string();
            let loads = matches!(by_value(&text), Outcome::Loaded(_));
            loaded += usize::from(loads);
            // `check` requires the same result either way; one that loads
            // must come from the typed path.
            assert_eq!(check(&text), loads, "{text}");
        }
        assert!(loaded >= 10, "only {loaded} of these load");
    }

    #[test]
    fn a_rejected_model_reports_the_value_paths_error() {
        for property in [
            json!({"$class": "concerto.metamodel@1.0.0.StringProperty", "name": "1bad"}),
            json!({"$class": "concerto.metamodel@1.0.0.StringProperty", "name": "$identifier"}),
            json!({"$class": "concerto.metamodel@1.0.0.StringProperty"}),
            json!({"$class": "concerto.metamodel@1.0.0.StringProperty", "name": "a", "decorators": [null]}),
            json!({"$class": "concerto.metamodel@1.0.0.RelationshipProperty", "name": "r"}),
            json!({"$class": "concerto.metamodel@1.0.0.NopeProperty", "name": "a"}),
            // accordproject/concerto-rust#285: only the full metamodel class.
            json!({"$class": "StringProperty", "name": "a"}),
            json!({"$class": "foo.StringProperty", "name": "a"}),
            json!({"$class": "concerto.metamodel@1.0.0.StringPropertyconcerto.metamodel@1.0.0.StringProperty", "name": "a"}),
        ] {
            let text = model(json!([concept(json!([property]))])).to_string();
            assert!(!check(&text));
            assert!(matches!(
                ModelFile::from_json_text(&text, None, None),
                Ok(Err(e)) if !e.is_unported_type_not_found()
            ));
        }
        assert!(!check("{\"namespace\": }"));
        assert!(!check("[]"));
    }

    // -----------------------------------------------------------------------
    // Differential test over every model AST available
    // -----------------------------------------------------------------------

    /// Every `concerto.metamodel@1.0.0.Model` object anywhere in `value`.
    fn collect_models(value: &Value, out: &mut BTreeSet<String>) {
        match value {
            Value::Object(map) => {
                if map.get("$class").and_then(Value::as_str)
                    == Some("concerto.metamodel@1.0.0.Model")
                {
                    out.insert(value.to_string());
                }
                map.values().for_each(|v| collect_models(v, out));
            }
            Value::Array(items) => items.iter().for_each(|v| collect_models(v, out)),
            _ => {}
        }
    }

    fn json_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                json_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "json") {
                out.push(path);
            }
        }
    }

    /// Mutated copies of `ast`, one change each, on its first class-like
    /// declaration and its first enum declaration, and each one's first two
    /// properties: each change either leaves the model loadable or makes one
    /// of the loader's checks fail.
    fn mutations(ast: &Value) -> Vec<Value> {
        let mut out = Vec::new();
        let Some(declarations) = ast.get("declarations").and_then(Value::as_array) else {
            return out;
        };
        let is_enum = |d: &Value| {
            d.get("$class")
                .and_then(Value::as_str)
                .is_some_and(|c| c.ends_with("EnumDeclaration"))
        };
        let indexes = [
            declarations
                .iter()
                .position(|d| d.get("properties").is_some() && !is_enum(d)),
            declarations.iter().position(is_enum),
        ];
        for index in indexes.into_iter().flatten() {
            mutate(ast, index, &mut out);
        }
        out
    }

    /// [`mutations`] for the declaration at `index`.
    fn mutate(ast: &Value, index: usize, out: &mut Vec<Value>) {
        let mut edit = |path: &[&str], change: &dyn Fn(&mut serde_json::Map<String, Value>)| {
            let mut copy = ast.clone();
            let mut node = copy.get_mut("declarations").and_then(|d| d.get_mut(index));
            for step in path {
                node = node.and_then(|n| match step.parse::<usize>() {
                    Ok(i) => n.get_mut(i),
                    Err(_) => n.get_mut(*step),
                });
            }
            if let Some(Value::Object(map)) = node {
                change(map);
                out.push(copy);
            }
        };
        type Change = dyn Fn(&mut serde_json::Map<String, Value>);
        let changes: &[&Change] = &[
            &|m| {
                m.remove("name");
            },
            &|m| {
                m.insert("name".into(), json!("1bad"));
            },
            &|m| {
                m.insert("name".into(), json!("$timestamp"));
            },
            &|m| {
                m.insert("name".into(), Value::Null);
            },
            &|m| {
                m.insert("$class".into(), json!("concerto.metamodel@1.0.0.Nope"));
            },
            &|m| {
                m.insert("decorators".into(), json!([null]));
            },
            &|m| {
                m.insert("decorators".into(), json!("ab"));
            },
            &|m| {
                m.insert("decorators".into(), json!([{"$class": "concerto.metamodel@1.0.0.Decorator", "name": "d", "arguments": [{"$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 1.5}]}]));
            },
            &|m| {
                m.insert("extra".into(), json!({"a": [1, -0.0, "x", null]}));
            },
            &|m| {
                // `$class` moved to the end.
                if let Some(class) = m.shift_remove("$class") {
                    m.insert("$class".into(), class);
                }
            },
            &|m| {
                m.remove("type");
            },
            &|m| {
                m.insert("type".into(), Value::Null);
            },
            &|m| {
                m.insert("isArray".into(), json!("yes"));
            },
            &|m| {
                m.insert(
                    "superType".into(),
                    json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Other"}),
                );
            },
            &|m| {
                m.insert("properties".into(), json!({}));
            },
            &|m| {
                m.remove("properties");
            },
            // The fields the `Value` path normalizes or reads untyped.
            &|m| {
                m.insert("name".into(), json!(["C"]));
            },
            &|m| {
                m.insert("name".into(), json!(0));
            },
            &|m| {
                m.insert(
                    "superType".into(),
                    json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": null}),
                );
            },
            &|m| {
                m.insert(
                    "superType".into(),
                    json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier"}),
                );
            },
            &|m| {
                m.insert(
                    "superType".into(),
                    json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": false}),
                );
            },
            &|m| {
                m.insert("identified".into(), json!(0));
            },
            &|m| {
                m.insert(
                    "identified".into(),
                    json!({"$class": "concerto.metamodel@1.0.0.Identified"}),
                );
            },
            &|m| {
                m.insert(
                    "identified".into(),
                    json!({"$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": ""}),
                );
            },
            &|m| {
                m.insert(
                    "identified".into(),
                    json!({"$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "x"}),
                );
            },
            &|m| {
                m.insert(
                    "identified".into(),
                    json!({"$class": "IdentifiedBy", "name": "x"}),
                );
            },
            &|m| {
                m.insert(
                    "sizeValidator".into(),
                    json!({"$class": "concerto.metamodel@1.0.0.CollectionSizeValidator", "minSize": 2, "maxSize": 1}),
                );
            },
            &|m| {
                m.insert(
                    "sizeValidator".into(),
                    json!({"minSize": "1", "maxSize": null}),
                );
            },
            &|m| {
                m.insert(
                    "lengthValidator".into(),
                    json!({"$class": "concerto.metamodel@1.0.0.StringLengthValidator", "minLength": 5, "maxLength": 2}),
                );
            },
            &|m| {
                m.insert(
                    "lengthValidator".into(),
                    json!({"minLength": [], "maxLength": "3"}),
                );
            },
            &|m| {
                m.insert("validator".into(), json!({"pattern": "^a", "flags": 7}));
            },
            &|m| {
                m.insert(
                    "validator".into(),
                    json!({"$class": "concerto.metamodel@1.0.0.IntegerDomainValidator", "lower": 1, "upper": 0}),
                );
            },
            &|m| {
                m.insert("defaultValue".into(), json!(1.5));
            },
            &|m| {
                m.insert(
                    "$class".into(),
                    json!("concerto.metamodel@1.0.0.EnumProperty"),
                );
            },
            &|m| {
                m.insert(
                    "$class".into(),
                    json!("concerto.metamodel@1.0.0.StringProperty"),
                );
            },
            &|m| {
                m.insert(
                    "$class".into(),
                    json!("concerto.metamodel@1.0.0.ObjectProperty"),
                );
            },
            &|m| {
                m.insert(
                    "type".into(),
                    json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": ""}),
                );
            },
        ];
        for change in changes {
            edit(&[], change);
            edit(&["properties", "0"], change);
            edit(&["properties", "1"], change);
        }
    }

    /// The typed and `Value` paths agree on every model AST of the
    /// benchmark sets and the oracle corpus (with `CONCERTO_ORACLE_FIXTURES`
    /// set), and on mutated copies of each.
    #[test]
    fn the_typed_path_agrees_with_the_value_path_on_every_available_model() {
        let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut roots = Vec::new();
        if let Ok(fixtures) = std::env::var("CONCERTO_ORACLE_FIXTURES") {
            let fixtures = PathBuf::from(fixtures);
            if let Some(oracle) = fixtures.parent() {
                roots.push(oracle.join("cto-cache"));
                if let Some(migration) = oracle.parent() {
                    roots.push(migration.join("bench/fixtures/model-sets"));
                }
            }
            roots.push(fixtures);
        }
        roots.push(manifest.join("src"));
        roots.push(manifest.join("tests/typed_ast"));

        let mut files = Vec::new();
        roots.iter().for_each(|root| json_files(root, &mut files));
        let mut models = BTreeSet::new();
        for file in &files {
            if let Ok(text) = std::fs::read_to_string(file)
                && let Ok(value) = serde_json::from_str::<Value>(&text)
            {
                collect_models(&value, &mut models);
            }
        }
        assert!(!models.is_empty());

        let (mut typed, mut loadable, mut mutants, mut typed_mutants) = (0, 0, 0, 0);
        let mut fell_back = Vec::new();
        for text in &models {
            let took_typed = check(text);
            typed += usize::from(took_typed);
            if matches!(by_value(text), Outcome::Loaded(_)) {
                loadable += 1;
                if !took_typed {
                    fell_back.push(text);
                }
            }
            let ast: Value = serde_json::from_str(text).unwrap();
            for mutant in mutations(&ast) {
                mutants += 1;
                typed_mutants += usize::from(check(&mutant.to_string()));
            }
        }
        eprintln!(
            "typed AST differential: {} models from {} files ({loadable} load, {typed} of them typed), {mutants} mutants ({typed_mutants} typed)",
            models.len(),
            files.len()
        );
        // The drift guard: every model that loads takes the typed path. A
        // node the typed structs do not read (a metamodel change, say)
        // would otherwise only lose the speedup, silently.
        assert!(
            fell_back.is_empty(),
            "{} model(s) load but fell back to the Value path, e.g. {}",
            fell_back.len(),
            fell_back[0]
        );
        assert!(typed > 0 && typed == loadable);
    }
}
