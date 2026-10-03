//! A JSON node the typed read keeps as given, without a `serde_json::Value`
//! for its objects (P5-76, accordproject/concerto-rust#418).
//!
//! The typed read ([`super::typed_ast`]) keeps a declaration's and a
//! property's `location` exactly as the AST gives it: for an error's
//! location, for BC-19's shape check ([`super::shape`]), and to decode the
//! node's typed `location` from. A `Value` of a `Range` is three
//! insertion-ordered maps (`preserve_order`) and eleven owned keys, which
//! made a `location` the most expensive part of reading a model the
//! reference parser wrote with locations. [`Kept`] holds the same JSON: an
//! object is a short list of its entries, in order, each key a `&'static
//! str` when it is one of the metamodel's location keys; anything else is
//! the `Value` a `Value` parse builds for it.
//!
//! A [`Kept`] is read from the same text, by the same parser, as a `Value`
//! would be, and holds what that `Value` holds: [`Kept::to_value`] is that
//! `Value`, exactly (key order, a repeated key's last value at its first
//! position, every number as `serde_json` parsed it). Decoding a generated
//! struct from it ([`Kept::strict_decode`]) runs the same `serde` code over
//! the same entries as decoding it from that `Value`, so it accepts and
//! rejects exactly what the `Value` decode does.

use std::borrow::Cow;
use std::fmt;

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use concerto_metamodel::utils::class_name;
use concerto_metamodel::{ClassName, Name};

use super::decorator::Decorator;
use serde::de::value::{MapDeserializer, SeqDeserializer};
use serde::de::{
    self, DeserializeSeed, Deserializer, IntoDeserializer, MapAccess, SeqAccess, Unexpected,
    Visitor,
};
use serde_json::{Map, Number, Value};

type Error = serde_json::Error;

/// The keys of a `Range`, a `Position`, a `Decorator` and its arguments,
/// kept without an allocation.
const KNOWN_KEYS: [&str; 15] = [
    "$class",
    "start",
    "end",
    "source",
    "offset",
    "line",
    "column",
    "name",
    "arguments",
    "location",
    "value",
    "type",
    "isArray",
    "namespace",
    "resolvedName",
];

/// A JSON value as the typed read keeps it (the module doc).
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Kept {
    /// A JSON object: its entries in order, each key once (a repeated key's
    /// last value, at its first position, as in a `Value`).
    Object(Vec<(Cow<'static, str>, Kept)>),
    /// A JSON array.
    Array(Vec<Kept>),
    /// Any other JSON value, as a `Value` parse builds it.
    Other(Value),
}

impl Kept {
    /// The `Value` a `Value` parse of the same text builds.
    pub(crate) fn to_value(&self) -> Value {
        match self {
            Kept::Other(value) => value.clone(),
            Kept::Object(entries) => {
                let mut map = Map::with_capacity(entries.len());
                for (key, value) in entries {
                    map.insert(key.to_string(), value.to_value());
                }
                Value::Object(map)
            }
            Kept::Array(items) => Value::Array(items.iter().map(Kept::to_value).collect()),
        }
    }

    /// The value at `key`, for an object.
    pub(crate) fn get(&self, key: &str) -> Option<&Kept> {
        match self {
            Kept::Object(entries) => entries.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            // An object is always read as a `Kept::Object`.
            _ => None,
        }
    }

    /// Whether this is JSON `null`.
    pub(crate) fn is_null(&self) -> bool {
        matches!(self, Kept::Other(Value::Null))
    }

    /// [`Kept::strict_decode`], for the generated struct of one variant of
    /// a polymorphic type (a map declaration), which has no `$class` field
    /// of its own: as `typed_ast`'s `strict_variant_from_value` decodes it
    /// from [`Kept::to_value`], the object's `$class` left out.
    pub(crate) fn strict_variant_decode<T: de::DeserializeOwned>(&self) -> Result<T, Error> {
        match self {
            Kept::Object(entries) => {
                T::deserialize(super::typed_ast::Strict(MapDeserializer::<_, Error>::new(
                    entries
                        .iter()
                        .filter(|(key, _)| key != "$class")
                        .map(|(key, value)| (BorrowedKey(key.as_ref()), value)),
                )))
            }
            other => other.strict_decode(),
        }
    }

    /// Decodes a generated struct from this node as `typed_ast`'s
    /// `strict_from_value` decodes it from [`Kept::to_value`]: the same
    /// `serde` code over the same entries, with every value the struct
    /// would skip an error.
    pub(crate) fn strict_decode<T: de::DeserializeOwned>(&self) -> Result<T, Error> {
        T::deserialize(super::typed_ast::Strict(self))
    }
}

/// Reads a [`Kept`]: an object entry by entry, anything else as a `Value`.
#[derive(Clone, Copy)]
pub(crate) struct KeptSeed;

impl<'de> DeserializeSeed<'de> for KeptSeed {
    type Value = Kept;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Kept, D::Error> {
        d.deserialize_any(self)
    }
}

/// A key, as a `&'static str` when it is one of [`KNOWN_KEYS`].
fn kept_key(key: Cow<'_, str>) -> Cow<'static, str> {
    match KNOWN_KEYS.iter().find(|known| **known == key) {
        Some(known) => Cow::Borrowed(known),
        None => Cow::Owned(key.into_owned()),
    }
}

/// A map key, borrowed from the text when it has no escapes.
struct KeySeed;

impl<'de> DeserializeSeed<'de> for KeySeed {
    type Value = Cow<'de, str>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_str(self)
    }
}

impl<'de> Visitor<'de> for KeySeed {
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

/// `serde_json`'s own `Value` visitor, but for an object, which becomes a
/// [`Kept::Object`]. Every other arm builds exactly what that visitor
/// builds.
impl<'de> Visitor<'de> for KeptSeed {
    type Value = Kept;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any valid JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Kept, E> {
        Ok(Kept::Other(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Kept, E> {
        Ok(Kept::Other(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Kept, E> {
        Ok(Kept::Other(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Kept, E> {
        Ok(Kept::Other(
            Number::from_f64(value).map_or(Value::Null, Value::Number),
        ))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Kept, E> {
        Ok(Kept::Other(Value::String(value.to_owned())))
    }

    fn visit_string<E>(self, value: String) -> Result<Kept, E> {
        Ok(Kept::Other(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Kept, E> {
        Ok(Kept::Other(Value::Null))
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Kept, D::Error> {
        KeptSeed.deserialize(d)
    }

    fn visit_unit<E>(self) -> Result<Kept, E> {
        Ok(Kept::Other(Value::Null))
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Kept, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(item) = seq.next_element_seed(KeptSeed)? {
            items.push(item);
        }
        Ok(Kept::Array(items))
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Kept, A::Error> {
        read_entries(Vec::with_capacity(4), map)
    }
}

/// Adds `key` and its `value` to an object's `entries`: a repeated key
/// replaces the earlier value in place, as in a `Value` (`preserve_order`'s
/// `IndexMap::insert`).
fn insert_entry(entries: &mut Vec<(Cow<'static, str>, Kept)>, key: Cow<'_, str>, value: Kept) {
    match entries.iter_mut().find(|(k, _)| **k == *key) {
        Some(entry) => entry.1 = value,
        None => entries.push((kept_key(key), value)),
    }
}

/// Reads the rest of an object into `entries` (what has been read of it so
/// far).
fn read_entries<'de, A: MapAccess<'de>>(
    mut entries: Vec<(Cow<'static, str>, Kept)>,
    mut map: A,
) -> Result<Kept, A::Error> {
    while let Some(key) = map.next_key_seed(KeySeed)? {
        let value = map.next_value_seed(KeptSeed)?;
        insert_entry(&mut entries, key, value);
    }
    Ok(Kept::Object(entries))
}

// ---------------------------------------------------------------------------
// A `location`: a `Range` read field by field
// ---------------------------------------------------------------------------

/// The metamodel's `Range` and `Position` `$class`es.
const RANGE_CLASS: &str = "concerto.metamodel@1.0.0.Range";
const POSITION_CLASS: &str = "concerto.metamodel@1.0.0.Position";

/// A node's `location`, as the typed read keeps it: the same JSON as a
/// [`Kept`] (and so as a `Value`), read without an allocation for anything
/// but the node itself when it is the `Range` every AST with locations
/// holds (an object of `$class`, `start`, `end` and `source`, each once,
/// with a string `$class`, `Position` objects of a string `$class` and
/// numbers, and a string or `null` `source`). Any other value is a [`Kept`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Location {
    /// The `Range` object, field by field (P5-93: held inline, boxed from
    /// P5-76: the read now keeps a property's `location` in a list apart
    /// from the properties, which has an element only when some property
    /// of the declaration has one, `typed_ast::TypedProperties`).
    Range(RangeRead),
    /// Any other value.
    Kept(Kept),
}

/// A `$class` string: the metamodel's own for its node, without an
/// allocation, or any other string.
#[derive(Debug, Clone, PartialEq)]
enum ClassRead {
    Canonical,
    Other(String),
}

impl ClassRead {
    fn new(class: Cow<'_, str>, canonical: &str) -> Self {
        if class == canonical {
            ClassRead::Canonical
        } else {
            ClassRead::Other(class.into_owned())
        }
    }

    /// The `$class` read, as the generated struct keeps it (P5-93: the
    /// canonical one interned, without an allocation).
    fn to_class(&self, canonical: &'static str) -> ClassName {
        match self {
            ClassRead::Canonical => Cow::Borrowed(canonical),
            ClassRead::Other(class) => class_name(class),
        }
    }
}

/// The keys of an object read field by field, in the order read (each an
/// index into its field list), each once.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Order {
    keys: [u8; 4],
    len: u8,
}

impl Order {
    fn push(&mut self, key: u8) {
        self.keys[usize::from(self.len)] = key;
        self.len += 1;
    }

    fn iter(&self) -> impl Iterator<Item = u8> + '_ {
        self.keys[..usize::from(self.len)].iter().copied()
    }
}

/// A `Position` object, field by field: its `$class` and its three
/// numbers, each as read, or `None` when the object has no such key.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct PositionRead {
    order: Order,
    class: Option<ClassRead>,
    numbers: [Option<Number>; 3],
}

/// A `Position`'s keys, by their index in [`PositionRead`].
const POSITION_KEYS: [&str; 4] = ["$class", "line", "column", "offset"];

/// A `Range` object, field by field.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct RangeRead {
    order: Order,
    class: Option<ClassRead>,
    start: Option<PositionRead>,
    end: Option<PositionRead>,
    /// `Some(None)` for a `null` `source`.
    source: Option<Option<String>>,
}

/// A `Range`'s keys, by their index in [`RangeRead`].
const RANGE_KEYS: [&str; 4] = ["$class", "start", "end", "source"];

impl PositionRead {
    fn has(&self, key: u8) -> bool {
        match key {
            0 => self.class.is_some(),
            _ => self.numbers[usize::from(key - 1)].is_some(),
        }
    }

    /// The same object as a [`Kept`].
    fn to_kept(&self) -> Kept {
        Kept::Object(self.entries())
    }

    fn entries(&self) -> Vec<(Cow<'static, str>, Kept)> {
        self.order
            .iter()
            .map(|key| {
                let value = match key {
                    0 => Value::String(
                        self.class
                            .as_ref()
                            .expect("an ordered key is set")
                            .to_class(POSITION_CLASS)
                            .into_owned(),
                    ),
                    _ => Value::Number(
                        self.numbers[usize::from(key - 1)]
                            .clone()
                            .expect("an ordered key is set"),
                    ),
                };
                (
                    Cow::Borrowed(POSITION_KEYS[usize::from(key)]),
                    Kept::Other(value),
                )
            })
            .collect()
    }

    /// The generated `Position`, as `serde` decodes it from the object's
    /// `Value`: `$class` defaults to `""`, and each number is required.
    fn decode(&self) -> Result<mm::Position, Error> {
        let number = |i: usize| {
            self.numbers[i]
                .as_ref()
                .and_then(Number::as_f64)
                .ok_or_else(|| de::Error::missing_field(POSITION_KEYS[i + 1]))
        };
        Ok(mm::Position {
            _class: self
                .class
                .as_ref()
                .map_or_else(ClassName::default, |class| class.to_class(POSITION_CLASS)),
            line: number(0)?,
            column: number(1)?,
            offset: number(2)?,
        })
    }

    /// BC-19's shape rule for a `Position` node: the metamodel's `$class`,
    /// and three integral numbers.
    fn conforms(&self) -> bool {
        self.class == Some(ClassRead::Canonical)
            && self.numbers.iter().all(|number| {
                number
                    .as_ref()
                    .and_then(Number::as_f64)
                    .is_some_and(|n| n.is_finite() && n.trunc() == n)
            })
    }
}

impl RangeRead {
    fn has(&self, key: u8) -> bool {
        match key {
            0 => self.class.is_some(),
            1 => self.start.is_some(),
            2 => self.end.is_some(),
            _ => self.source.is_some(),
        }
    }

    fn entries(&self) -> Vec<(Cow<'static, str>, Kept)> {
        let mut entries = Vec::with_capacity(4);
        for key in self.order.iter() {
            let value = match key {
                0 => Kept::Other(Value::String(
                    self.class
                        .as_ref()
                        .expect("an ordered key is set")
                        .to_class(RANGE_CLASS)
                        .into_owned(),
                )),
                1 => self
                    .start
                    .as_ref()
                    .expect("an ordered key is set")
                    .to_kept(),
                2 => self.end.as_ref().expect("an ordered key is set").to_kept(),
                _ => Kept::Other(
                    self.source
                        .clone()
                        .expect("an ordered key is set")
                        .map_or(Value::Null, Value::String),
                ),
            };
            entries.push((Cow::Borrowed(RANGE_KEYS[usize::from(key)]), value));
        }
        entries
    }

    /// The generated `Range`, as `serde` decodes it from the object's
    /// `Value`: `$class` defaults to `""`, `start` and `end` are required,
    /// and `source` is optional.
    fn decode(&self) -> Result<mm::Range, Error> {
        let position = |position: Option<&PositionRead>, key: &'static str| {
            position
                .ok_or_else(|| de::Error::missing_field(key))
                .and_then(PositionRead::decode)
        };
        Ok(mm::Range {
            _class: self
                .class
                .as_ref()
                .map_or_else(ClassName::default, |class| class.to_class(RANGE_CLASS)),
            start: position(self.start.as_ref(), "start")?,
            end: position(self.end.as_ref(), "end")?,
            source: self.source.clone().flatten(),
        })
    }

    /// BC-19's shape rule for a `Range` node (`shape::node_conforms`): the
    /// metamodel's `$class`, `Position` nodes at `start` and `end`, and a
    /// string `source` or none (every key it has is a `Range` field).
    pub(crate) fn conforms(&self) -> bool {
        self.class == Some(ClassRead::Canonical)
            && self.start.as_ref().is_some_and(PositionRead::conforms)
            && self.end.as_ref().is_some_and(PositionRead::conforms)
    }
}

impl Location {
    /// The `Value` a `Value` parse of the same text builds.
    pub(crate) fn to_value(&self) -> Value {
        match self {
            Location::Range(range) => Kept::Object(range.entries()).to_value(),
            Location::Kept(kept) => kept.to_value(),
        }
    }

    /// The generated `location`, decoded as `typed_ast`'s
    /// `strict_from_value` decodes it from [`Location::to_value`]: `None`
    /// for `null`, an error for anything that is not a `Range`.
    pub(crate) fn decode(&self) -> Result<Option<mm::Range>, Error> {
        match self {
            Location::Range(range) => range.decode().map(Some),
            Location::Kept(kept) => kept.strict_decode(),
        }
    }
}

/// A string, borrowed from the text when it has no escapes, or any other
/// value as a [`Kept`].
enum Leaf<'de> {
    Str(Cow<'de, str>),
    Other(Kept),
}

impl Leaf<'_> {
    fn into_kept(self) -> Kept {
        match self {
            Leaf::Str(s) => Kept::Other(Value::String(s.into_owned())),
            Leaf::Other(kept) => kept,
        }
    }
}

/// Reads a [`Leaf`].
struct LeafSeed;

impl<'de> DeserializeSeed<'de> for LeafSeed {
    type Value = Leaf<'de>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Leaf<'de>, D::Error> {
        d.deserialize_any(self)
    }
}

/// Each arm is [`KeptSeed`]'s, but for a string.
macro_rules! kept_arms {
    ($wrap:path; $($method:ident($ty:ty);)*) => {$(
        fn $method<E: de::Error>(self, value: $ty) -> Result<Self::Value, E> {
            KeptSeed.$method(value).map($wrap)
        }
    )*};
}

impl<'de> Visitor<'de> for LeafSeed {
    type Value = Leaf<'de>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any valid JSON value")
    }

    kept_arms! {
        Leaf::Other;
        visit_bool(bool);
        visit_i64(i64);
        visit_u64(u64);
        visit_f64(f64);
    }

    fn visit_borrowed_str<E: de::Error>(self, value: &'de str) -> Result<Leaf<'de>, E> {
        Ok(Leaf::Str(Cow::Borrowed(value)))
    }

    fn visit_str<E: de::Error>(self, value: &str) -> Result<Leaf<'de>, E> {
        Ok(Leaf::Str(Cow::Owned(value.to_owned())))
    }

    fn visit_string<E: de::Error>(self, value: String) -> Result<Leaf<'de>, E> {
        Ok(Leaf::Str(Cow::Owned(value)))
    }

    fn visit_none<E: de::Error>(self) -> Result<Leaf<'de>, E> {
        KeptSeed.visit_none().map(Leaf::Other)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Leaf<'de>, D::Error> {
        KeptSeed.visit_some(d).map(Leaf::Other)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Leaf<'de>, E> {
        KeptSeed.visit_unit().map(Leaf::Other)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Leaf<'de>, A::Error> {
        KeptSeed.visit_seq(seq).map(Leaf::Other)
    }

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Leaf<'de>, A::Error> {
        KeptSeed.visit_map(map).map(Leaf::Other)
    }
}

/// Reads a [`Location`].
#[derive(Clone, Copy)]
pub(crate) struct LocationSeed;

impl<'de> DeserializeSeed<'de> for LocationSeed {
    type Value = Location;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Location, D::Error> {
        d.deserialize_any(self)
    }
}

/// A `Position` read field by field, or any other value.
enum PositionOrKept {
    Position(PositionRead),
    Kept(Kept),
}

/// Reads a `Range`'s `start` or `end`.
struct PositionSeed;

impl<'de> DeserializeSeed<'de> for PositionSeed {
    type Value = PositionOrKept;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<PositionOrKept, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for PositionSeed {
    type Value = PositionOrKept;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any valid JSON value")
    }

    kept_arms! {
        PositionOrKept::Kept;
        visit_bool(bool);
        visit_i64(i64);
        visit_u64(u64);
        visit_f64(f64);
        visit_str(&str);
        visit_string(String);
    }

    fn visit_none<E: de::Error>(self) -> Result<PositionOrKept, E> {
        KeptSeed.visit_none().map(PositionOrKept::Kept)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<PositionOrKept, D::Error> {
        KeptSeed.visit_some(d).map(PositionOrKept::Kept)
    }

    fn visit_unit<E: de::Error>(self) -> Result<PositionOrKept, E> {
        KeptSeed.visit_unit().map(PositionOrKept::Kept)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<PositionOrKept, A::Error> {
        KeptSeed.visit_seq(seq).map(PositionOrKept::Kept)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<PositionOrKept, A::Error> {
        let mut read = PositionRead::default();
        while let Some(key) = map.next_key_seed(KeySeed)? {
            let index = POSITION_KEYS.iter().position(|k| *k == key);
            let value = match index {
                Some(i) if !read.has(i as u8) => {
                    let i = i as u8;
                    match (i, map.next_value_seed(LeafSeed)?) {
                        (0, Leaf::Str(class)) => {
                            read.class = Some(ClassRead::new(class, POSITION_CLASS));
                            read.order.push(i);
                            continue;
                        }
                        (1..=3, Leaf::Other(Kept::Other(Value::Number(n)))) => {
                            read.numbers[usize::from(i - 1)] = Some(n);
                            read.order.push(i);
                            continue;
                        }
                        (_, leaf) => leaf.into_kept(),
                    }
                }
                // Not a `Position` key, or one already read: read as a
                // `Kept` from here on.
                _ => map.next_value_seed(KeptSeed)?,
            };
            let mut entries = read.entries();
            insert_entry(&mut entries, key, value);
            return read_entries(entries, map).map(PositionOrKept::Kept);
        }
        Ok(PositionOrKept::Position(read))
    }
}

impl<'de> Visitor<'de> for LocationSeed {
    type Value = Location;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any valid JSON value")
    }

    kept_arms! {
        Location::Kept;
        visit_bool(bool);
        visit_i64(i64);
        visit_u64(u64);
        visit_f64(f64);
        visit_str(&str);
        visit_string(String);
    }

    fn visit_none<E: de::Error>(self) -> Result<Location, E> {
        KeptSeed.visit_none().map(Location::Kept)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Location, D::Error> {
        KeptSeed.visit_some(d).map(Location::Kept)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Location, E> {
        KeptSeed.visit_unit().map(Location::Kept)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Location, A::Error> {
        KeptSeed.visit_seq(seq).map(Location::Kept)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Location, A::Error> {
        let mut read = RangeRead::default();
        while let Some(key) = map.next_key_seed(KeySeed)? {
            let index = RANGE_KEYS.iter().position(|k| *k == key);
            let value = match index {
                Some(i) if !read.has(i as u8) => {
                    let i = i as u8;
                    match i {
                        0 | 3 => match (i, map.next_value_seed(LeafSeed)?) {
                            (0, Leaf::Str(class)) => {
                                read.class = Some(ClassRead::new(class, RANGE_CLASS));
                                read.order.push(i);
                                continue;
                            }
                            (3, Leaf::Str(source)) => {
                                read.source = Some(Some(source.into_owned()));
                                read.order.push(i);
                                continue;
                            }
                            (3, Leaf::Other(Kept::Other(Value::Null))) => {
                                read.source = Some(None);
                                read.order.push(i);
                                continue;
                            }
                            (_, leaf) => leaf.into_kept(),
                        },
                        _ => match map.next_value_seed(PositionSeed)? {
                            PositionOrKept::Position(position) => {
                                if i == 1 {
                                    read.start = Some(position);
                                } else {
                                    read.end = Some(position);
                                }
                                read.order.push(i);
                                continue;
                            }
                            PositionOrKept::Kept(kept) => kept,
                        },
                    }
                }
                // Not a `Range` key, or one already read: read as a `Kept`
                // from here on.
                _ => map.next_value_seed(KeptSeed)?,
            };
            let mut entries = read.entries();
            insert_entry(&mut entries, key, value);
            return read_entries(entries, map).map(Location::Kept);
        }
        Ok(Location::Range(read))
    }
}

// ---------------------------------------------------------------------------
// A class's `identified`, read field by field (P5-93)
// ---------------------------------------------------------------------------

/// The metamodel's `Identified` and `IdentifiedBy` `$class`es.
const IDENTIFIED_CLASS: &str = "concerto.metamodel@1.0.0.Identified";
const IDENTIFIED_BY_CLASS: &str = "concerto.metamodel@1.0.0.IdentifiedBy";

/// A class's `identified` value, as the typed read reads it: one of the
/// metamodel's own two nodes, read field by field without an allocation
/// but for the name, or any other value as a [`Kept`].
pub(crate) enum IdentifiedRead {
    /// `{"$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": <a
    /// string>}`, its two keys in either order (P5-93: the name shares
    /// the text it is read from, `Name::from_source`).
    By(Name),
    /// `{"$class": "concerto.metamodel@1.0.0.Identified"}`.
    System,
    /// Any other value.
    Kept(Kept),
}

impl IdentifiedRead {
    /// The generated `identified`, as [`Kept::strict_decode`] decodes it
    /// from the same JSON, and the value as a [`Kept`] unless it is one of
    /// the metamodel's own two nodes (which BC-19's shape check accepts).
    pub(crate) fn decode(self) -> Result<(Option<mm::Identified>, Option<Kept>), Error> {
        match self {
            IdentifiedRead::By(name) => Ok((
                Some(mm::Identified::IdentifiedBy(mm::IdentifiedBy { name })),
                None,
            )),
            IdentifiedRead::System => Ok((Some(mm::Identified::Identified), None)),
            IdentifiedRead::Kept(kept) => Ok((kept.strict_decode()?, Some(kept))),
        }
    }
}

/// The entries an `identified` object read so far holds, in the order
/// read: its `$class` (`by`: `IdentifiedBy`'s, or `Identified`'s) and its
/// `name`.
fn identified_entries(
    by: Option<bool>,
    name: Option<Name>,
    class_first: bool,
) -> Vec<(Cow<'static, str>, Kept)> {
    let class = by.map(|by| {
        let class = if by {
            IDENTIFIED_BY_CLASS
        } else {
            IDENTIFIED_CLASS
        };
        (
            Cow::Borrowed("$class"),
            Kept::Other(Value::String(class.to_string())),
        )
    });
    let name = name.map(|name| {
        (
            Cow::Borrowed("name"),
            Kept::Other(Value::String(name.into_string())),
        )
    });
    let (first, second) = if class_first {
        (class, name)
    } else {
        (name, class)
    };
    first.into_iter().chain(second).collect()
}

/// A string read from the text as a generated struct's [`Name`]: sharing
/// the text when it is borrowed from it (`Name::from_source`), a copy
/// otherwise.
pub(crate) fn leaf_name(value: Cow<'_, str>) -> Name {
    match value {
        Cow::Borrowed(value) => Name::from_source(value),
        Cow::Owned(value) => Name::from(value),
    }
}

/// Reads an [`IdentifiedRead`].
#[derive(Clone, Copy)]
pub(crate) struct IdentifiedSeed;

impl<'de> DeserializeSeed<'de> for IdentifiedSeed {
    type Value = IdentifiedRead;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<IdentifiedRead, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for IdentifiedSeed {
    type Value = IdentifiedRead;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any valid JSON value")
    }

    kept_arms! {
        IdentifiedRead::Kept;
        visit_bool(bool);
        visit_i64(i64);
        visit_u64(u64);
        visit_f64(f64);
        visit_str(&str);
        visit_string(String);
    }

    fn visit_none<E: de::Error>(self) -> Result<IdentifiedRead, E> {
        KeptSeed.visit_none().map(IdentifiedRead::Kept)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<IdentifiedRead, D::Error> {
        KeptSeed.visit_some(d).map(IdentifiedRead::Kept)
    }

    fn visit_unit<E: de::Error>(self) -> Result<IdentifiedRead, E> {
        KeptSeed.visit_unit().map(IdentifiedRead::Kept)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<IdentifiedRead, A::Error> {
        KeptSeed.visit_seq(seq).map(IdentifiedRead::Kept)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<IdentifiedRead, A::Error> {
        // The `$class` read (`Some(true)` for `IdentifiedBy`'s, `Some(false)`
        // for `Identified`'s) and the `name`, each once.
        let mut by = None;
        let mut name = None;
        let mut class_first = false;
        while let Some(key) = map.next_key_seed(KeySeed)? {
            let value = match &*key {
                "$class" if by.is_none() => match map.next_value_seed(LeafSeed)? {
                    Leaf::Str(class)
                        if class == IDENTIFIED_BY_CLASS || class == IDENTIFIED_CLASS =>
                    {
                        by = Some(class == IDENTIFIED_BY_CLASS);
                        class_first = name.is_none();
                        continue;
                    }
                    leaf => leaf.into_kept(),
                },
                "name" if name.is_none() => match map.next_value_seed(LeafSeed)? {
                    Leaf::Str(value) => {
                        name = Some(leaf_name(value));
                        continue;
                    }
                    leaf => leaf.into_kept(),
                },
                // Not an `identified` key, or one already read: read as a
                // `Kept` from here on.
                _ => map.next_value_seed(KeptSeed)?,
            };
            let mut entries = identified_entries(by, name, class_first);
            insert_entry(&mut entries, key, value);
            return read_entries(entries, map).map(IdentifiedRead::Kept);
        }
        Ok(match (by, name) {
            (Some(true), Some(name)) => IdentifiedRead::By(name),
            (Some(false), None) => IdentifiedRead::System,
            (by, name) => {
                IdentifiedRead::Kept(Kept::Object(identified_entries(by, name, class_first)))
            }
        })
    }
}

// ---------------------------------------------------------------------------
// A `decorators` value, decoded by moving its strings out (P5-93)
// ---------------------------------------------------------------------------

/// The metamodel's `DecoratorString`, `DecoratorNumber` and
/// `DecoratorBoolean` `$class`es.
const DECORATOR_STRING_CLASS: &str = "concerto.metamodel@1.0.0.DecoratorString";
const DECORATOR_NUMBER_CLASS: &str = "concerto.metamodel@1.0.0.DecoratorNumber";
const DECORATOR_BOOLEAN_CLASS: &str = "concerto.metamodel@1.0.0.DecoratorBoolean";

impl Kept {
    /// A node's `decorators` value, decoded as [`Kept::strict_decode`]
    /// decodes it into the generated structs, but by moving the value's
    /// strings into them where it is an array of plain decorator nodes
    /// ([`plain_decorator`]), as every AST of the reference parser's
    /// holds; any other value is decoded by [`Kept::strict_decode`].
    pub(crate) fn into_decorators(self) -> Result<Option<Vec<mm::Decorator>>, Error> {
        match self {
            Kept::Array(items) if items.iter().all(plain_decorator) => {
                Ok(Some(items.into_iter().map(into_decorator).collect()))
            }
            other => other.strict_decode(),
        }
    }

    /// This value's string, if it is one.
    fn into_string(self) -> String {
        match self {
            Kept::Other(Value::String(s)) => s,
            // Never reached: [`plain_decorator`] has checked the value.
            _ => String::new(),
        }
    }
}

/// A `Decorator` node the generated struct decodes field by field as it
/// is: an object of a string `$class`, a string `name`, and, if any, an
/// `arguments` that is `null` or an array of plain arguments
/// ([`plain_argument`]), and no other key.
fn plain_decorator(node: &Kept) -> bool {
    let Kept::Object(entries) = node else {
        return false;
    };
    let mut required = 0;
    entries
        .iter()
        .all(|(key, value)| match (key.as_ref(), value) {
            ("$class" | "name", Kept::Other(Value::String(_))) => {
                required += 1;
                true
            }
            ("arguments", Kept::Other(Value::Null)) => true,
            ("arguments", Kept::Array(arguments)) => arguments.iter().all(plain_argument),
            _ => false,
        })
        && required == 2
}

/// A decorator argument of the metamodel's `DecoratorString`,
/// `DecoratorNumber` or `DecoratorBoolean` `$class` and a `value` of its
/// JSON type, and no other key.
fn plain_argument(node: &Kept) -> bool {
    let Kept::Object(entries) = node else {
        return false;
    };
    let [(k1, v1), (k2, v2)] = entries.as_slice() else {
        return false;
    };
    let (class, value) = match (k1.as_ref(), k2.as_ref()) {
        ("$class", "value") => (v1, v2),
        ("value", "$class") => (v2, v1),
        _ => return false,
    };
    matches!(
        (class, value),
        (Kept::Other(Value::String(class)), Kept::Other(Value::String(_)))
            if class == DECORATOR_STRING_CLASS
    ) || matches!(
        (class, value),
        (Kept::Other(Value::String(class)), Kept::Other(Value::Number(n)))
            if class == DECORATOR_NUMBER_CLASS && n.as_f64().is_some()
    ) || matches!(
        (class, value),
        (Kept::Other(Value::String(class)), Kept::Other(Value::Bool(_)))
            if class == DECORATOR_BOOLEAN_CLASS
    )
}

/// The generated `Decorator` of a [`plain_decorator`] node.
fn into_decorator(node: Kept) -> mm::Decorator {
    let mut decorator = mm::Decorator {
        _class: ClassName::default(),
        name: Name::default(),
        arguments: None,
        location: None,
    };
    if let Kept::Object(entries) = node {
        for (key, value) in entries {
            match (key.as_ref(), value) {
                ("$class", value) => {
                    let class = value.into_string();
                    decorator._class = concerto_metamodel::utils::class_name_owned(class);
                }
                ("name", value) => decorator.name = value.into_string().into(),
                ("arguments", Kept::Array(arguments)) => {
                    decorator.arguments =
                        Some(arguments.into_iter().filter_map(into_argument).collect());
                }
                _ => {}
            }
        }
    }
    decorator
}

/// The generated argument of a [`plain_argument`] node (always `Some`).
fn into_argument(node: Kept) -> Option<mm::DecoratorLiteral> {
    let Kept::Object(entries) = node else {
        return None;
    };
    let mut class = String::new();
    let mut value = None;
    for (key, item) in entries {
        match (key.as_ref(), item) {
            ("$class", item) => class = item.into_string(),
            (_, Kept::Other(item)) => value = Some(item),
            _ => {}
        }
    }
    match (class.as_str(), value?) {
        (DECORATOR_STRING_CLASS, Value::String(value)) => {
            Some(mm::DecoratorLiteral::DecoratorString(mm::DecoratorString {
                location: None,
                value,
            }))
        }
        (DECORATOR_NUMBER_CLASS, Value::Number(value)) => {
            Some(mm::DecoratorLiteral::DecoratorNumber(mm::DecoratorNumber {
                location: None,
                value: value.as_f64()?,
            }))
        }
        (DECORATOR_BOOLEAN_CLASS, Value::Bool(value)) => Some(
            mm::DecoratorLiteral::DecoratorBoolean(mm::DecoratorBoolean {
                location: None,
                value,
            }),
        ),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Decoding a generated struct from a `Kept`, as from its `Value`
// ---------------------------------------------------------------------------

impl<'a> IntoDeserializer<'a, Error> for &'a Kept {
    type Deserializer = Self;

    fn into_deserializer(self) -> Self {
        self
    }
}

/// The entries of a [`Kept::Object`], handed to a visitor as `&Value`'s
/// deserializer hands an object's (keys as borrowed strings; an error for
/// entries the visitor leaves unread).
fn visit_object<'a, V: Visitor<'a>>(
    entries: &'a [(Cow<'static, str>, Kept)],
    visitor: V,
) -> Result<V::Value, Error> {
    let len = entries.len();
    let mut map = MapDeserializer::new(
        entries
            .iter()
            .map(|(key, value)| (BorrowedKey(key.as_ref()), value)),
    );
    let value = visitor.visit_map(&mut map)?;
    if map.end().is_err() {
        return Err(de::Error::invalid_length(len, &"fewer elements in map"));
    }
    Ok(value)
}

/// The items of a [`Kept::Array`], handed to a visitor as `&Value`'s
/// deserializer hands an array's.
fn visit_array<'a, V: Visitor<'a>>(items: &'a [Kept], visitor: V) -> Result<V::Value, Error> {
    let len = items.len();
    let mut seq = SeqDeserializer::new(items.iter());
    let value = visitor.visit_seq(&mut seq)?;
    if seq.end().is_err() {
        return Err(de::Error::invalid_length(len, &"fewer elements in array"));
    }
    Ok(value)
}

/// A key handed to a visitor as `&Value`'s deserializer hands it, a
/// borrowed string.
struct BorrowedKey<'a>(&'a str);

impl<'a> IntoDeserializer<'a, Error> for BorrowedKey<'a> {
    type Deserializer = de::value::BorrowedStrDeserializer<'a, Error>;

    fn into_deserializer(self) -> Self::Deserializer {
        de::value::BorrowedStrDeserializer::new(self.0)
    }
}

impl Kept {
    /// What a `Value` of an object or an array tells a visitor it is.
    fn unexpected(&self) -> Unexpected<'_> {
        match self {
            Kept::Object(_) => Unexpected::Map,
            Kept::Array(_) => Unexpected::Seq,
            Kept::Other(_) => Unexpected::Other("a JSON value"),
        }
    }
}

/// `&Value`'s deserializer, method by method: a [`Kept::Other`] goes to it
/// as it is, and an object or an array is handled as it handles one.
macro_rules! kept_deserialize {
    // A method that takes an object (`map`), an array (`seq`), both, or
    // neither (an error for both, as `&Value`'s `invalid_type`).
    ($($method:ident($($arg:ident: $ty:ty),*) => $object:ident, $array:ident;)*) => {$(
        fn $method<V: Visitor<'a>>(self, $($arg: $ty,)* visitor: V) -> Result<V::Value, Error> {
            match self {
                Kept::Other(value) => value.$method($($arg,)* visitor),
                Kept::Object(entries) => kept_deserialize!(@$object self, entries, visitor, visit_object),
                Kept::Array(items) => kept_deserialize!(@$array self, items, visitor, visit_array),
            }
        }
    )*};
    (@yes $kept:ident, $inner:ident, $visitor:ident, $visit:ident) => {
        $visit($inner, $visitor)
    };
    (@no $kept:ident, $inner:ident, $visitor:ident, $visit:ident) => {{
        let _ = $inner;
        Err(de::Error::invalid_type($kept.unexpected(), &$visitor))
    }};
}

impl<'a> Deserializer<'a> for &'a Kept {
    type Error = Error;

    kept_deserialize! {
        deserialize_any() => yes, yes;
        deserialize_bool() => no, no;
        deserialize_i8() => no, no;
        deserialize_i16() => no, no;
        deserialize_i32() => no, no;
        deserialize_i64() => no, no;
        deserialize_i128() => no, no;
        deserialize_u8() => no, no;
        deserialize_u16() => no, no;
        deserialize_u32() => no, no;
        deserialize_u64() => no, no;
        deserialize_u128() => no, no;
        deserialize_f32() => no, no;
        deserialize_f64() => no, no;
        deserialize_char() => no, no;
        deserialize_str() => no, no;
        deserialize_string() => no, no;
        deserialize_bytes() => no, yes;
        deserialize_byte_buf() => no, yes;
        deserialize_unit() => no, no;
        deserialize_unit_struct(name: &'static str) => no, no;
        deserialize_seq() => no, yes;
        deserialize_tuple(len: usize) => no, yes;
        deserialize_tuple_struct(name: &'static str, len: usize) => no, yes;
        deserialize_map() => yes, no;
        deserialize_struct(name: &'static str, fields: &'static [&'static str]) => yes, yes;
        deserialize_identifier() => no, no;
    }

    fn deserialize_option<V: Visitor<'a>>(self, visitor: V) -> Result<V::Value, Error> {
        match self {
            Kept::Other(value) => value.deserialize_option(visitor),
            _ => visitor.visit_some(self),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'a>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self {
            Kept::Other(value) => value.deserialize_newtype_struct(name, visitor),
            _ => visitor.visit_newtype_struct(self),
        }
    }

    fn deserialize_enum<V: Visitor<'a>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        match self {
            Kept::Other(value) => value.deserialize_enum(name, variants, visitor),
            // Never reached by the structs a `Kept` is decoded into (an
            // externally tagged enum); the same verdict through the `Value`.
            Kept::Object(_) => self.to_value().deserialize_enum(name, variants, visitor),
            Kept::Array(_) => Err(de::Error::invalid_type(Unexpected::Seq, &"string or map")),
        }
    }

    fn deserialize_ignored_any<V: Visitor<'a>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }
}

// ---------------------------------------------------------------------------
// A `decorators` value read straight into its decorators (P5-93)
// ---------------------------------------------------------------------------

/// What a node's `decorators` value gives the read: the generated
/// decorators, the processed ones, and BC-19's shape verdict on the value.
pub(crate) struct Decorators {
    /// The generated `decorators` of the node ([`Kept::into_decorators`]).
    pub(crate) node: Option<Vec<mm::Decorator>>,
    /// The processed decorators
    /// ([`super::decorator::parse_decorator_list`]).
    pub(crate) list: Vec<Decorator>,
    /// Whether BC-19's shape check accepts the value
    /// ([`super::shape::decorators_conform`]).
    pub(crate) conforms: bool,
}

impl Decorators {
    /// The decorators of a value read as a [`Kept`].
    pub(crate) fn from_kept(value: Kept) -> Result<Self, Error> {
        Ok(Decorators {
            list: super::decorator::parse_decorator_list(Some(&value)),
            conforms: super::shape::decorators_conform(&value),
            node: value.into_decorators()?,
        })
    }
}

/// The metamodel's `Decorator` `$class`.
const DECORATOR_CLASS: &str = "concerto.metamodel@1.0.0.Decorator";

/// Reads a node's `decorators` value into its [`Decorators`]. An array of
/// decorator nodes as the reference parser and `JSON.stringify` write them
/// (each a `$class` string, a `name` string and, if any, an `arguments`
/// array of `DecoratorString`, `DecoratorNumber` or `DecoratorBoolean`
/// nodes, each a `$class` and then a `value` of its type, every key in that
/// order and no other key) is read straight into the generated and the
/// processed decorators, each string read once. Any other value, from the
/// first node or key that is not so, is read on as a [`Kept`] (what
/// [`KeptSeed`] reads from the same text, with what has been read so far
/// put back as it was given, but that a number is put back as the `f64` it
/// was read as), and decoded from it ([`Decorators::from_kept`]). Either
/// way the decorators are those [`Decorators::from_kept`] gives for the
/// value as [`KeptSeed`] reads it: each decorator read here is a
/// [`plain_decorator`], and a decorator argument's number is only ever
/// read as an `f64`.
#[derive(Clone, Copy)]
pub(crate) struct DecoratorsSeed;

impl<'de> DeserializeSeed<'de> for DecoratorsSeed {
    type Value = Result<Decorators, Kept>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }
}

/// Each arm is [`KeptSeed`]'s, the value wrapped in `$wrap`.
macro_rules! kept_value_arms {
    ($wrap:expr) => {
        fn visit_bool<E: de::Error>(self, value: bool) -> Result<Self::Value, E> {
            KeptSeed.visit_bool(value).map($wrap)
        }

        fn visit_i64<E: de::Error>(self, value: i64) -> Result<Self::Value, E> {
            KeptSeed.visit_i64(value).map($wrap)
        }

        fn visit_u64<E: de::Error>(self, value: u64) -> Result<Self::Value, E> {
            KeptSeed.visit_u64(value).map($wrap)
        }

        fn visit_f64<E: de::Error>(self, value: f64) -> Result<Self::Value, E> {
            KeptSeed.visit_f64(value).map($wrap)
        }

        fn visit_str<E: de::Error>(self, value: &str) -> Result<Self::Value, E> {
            KeptSeed.visit_str(value).map($wrap)
        }

        fn visit_string<E: de::Error>(self, value: String) -> Result<Self::Value, E> {
            KeptSeed.visit_string(value).map($wrap)
        }

        fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
            KeptSeed.visit_none().map($wrap)
        }

        fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            KeptSeed.visit_some(d).map($wrap)
        }

        fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
            KeptSeed.visit_unit().map($wrap)
        }
    };
}

impl<'de> Visitor<'de> for DecoratorsSeed {
    type Value = Result<Decorators, Kept>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any valid JSON value")
    }

    kept_value_arms!(Err);

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        KeptSeed.visit_map(map).map(Err)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut node = Vec::new();
        let mut list = Vec::new();
        let mut conforms = true;
        while let Some(item) = seq.next_element_seed(PlainDecoratorSeed)? {
            match item {
                Ok((generated, processed)) => {
                    // `Decorator` is the one `$class` the shape check takes
                    // for a decorator, and every argument read here has
                    // one it takes for an argument.
                    conforms &= generated._class == DECORATOR_CLASS;
                    push_sized(&mut node, generated);
                    push_sized(&mut list, processed);
                }
                Err(kept) => {
                    // Not as read here: the array from here on is a `Kept`.
                    let mut items: Vec<Kept> = Vec::with_capacity(node.len() + 1);
                    items.extend(node.into_iter().map(decorator_kept));
                    items.push(kept);
                    while let Some(item) = seq.next_element_seed(KeptSeed)? {
                        items.push(item);
                    }
                    return Ok(Err(Kept::Array(items)));
                }
            }
        }
        Ok(Ok(Decorators {
            node: Some(node),
            list,
            conforms,
        }))
    }
}

/// Pushes `item` onto `items`, with room for just it when `items` has none
/// (most decorator lists, and argument lists, have one item), where a
/// first push makes room for four.
fn push_sized<T>(items: &mut Vec<T>, item: T) {
    if items.capacity() == 0 {
        items.reserve_exact(1);
    }
    items.push(item);
}

/// A decorator node read by [`PlainDecoratorSeed`], as a [`Kept`].
fn decorator_kept(decorator: mm::Decorator) -> Kept {
    let mut entries = decorator_entries(Some(decorator._class), Some(decorator.name));
    if let Some(arguments) = decorator.arguments {
        entries.push((
            Cow::Borrowed("arguments"),
            Kept::Array(arguments.into_iter().map(argument_kept).collect()),
        ));
    }
    Kept::Object(entries)
}

/// The entries of a decorator node read so far: its `$class`, and then its
/// `name`, if read.
fn decorator_entries(
    class: Option<ClassName>,
    name: Option<Name>,
) -> Vec<(Cow<'static, str>, Kept)> {
    let mut entries = Vec::with_capacity(4);
    if let Some(class) = class {
        entries.push((
            Cow::Borrowed("$class"),
            Kept::Other(Value::String(class.into_owned())),
        ));
    }
    if let Some(name) = name {
        entries.push((
            Cow::Borrowed("name"),
            Kept::Other(Value::String(name.into_string())),
        ));
    }
    entries
}

/// An argument node read by [`PlainArgumentSeed`], as a [`Kept`].
fn argument_kept(argument: mm::DecoratorLiteral) -> Kept {
    let (class, value) = match argument {
        mm::DecoratorLiteral::DecoratorString(literal) => {
            (DECORATOR_STRING_CLASS, Value::String(literal.value))
        }
        mm::DecoratorLiteral::DecoratorNumber(literal) => (
            DECORATOR_NUMBER_CLASS,
            Number::from_f64(literal.value).map_or(Value::Null, Value::Number),
        ),
        mm::DecoratorLiteral::DecoratorBoolean(literal) => {
            (DECORATOR_BOOLEAN_CLASS, Value::Bool(literal.value))
        }
        // Never read by `PlainArgumentSeed`.
        _ => ("", Value::Null),
    };
    Kept::Object(vec![
        (
            Cow::Borrowed("$class"),
            Kept::Other(Value::String(class.to_string())),
        ),
        (Cow::Borrowed("value"), Kept::Other(value)),
    ])
}

/// Reads one element of a `decorators` array: a decorator node as
/// [`DecoratorsSeed`] reads it, as its generated and its processed
/// decorator, or else as a [`Kept`].
struct PlainDecoratorSeed;

impl<'de> DeserializeSeed<'de> for PlainDecoratorSeed {
    type Value = Result<(mm::Decorator, Decorator), Kept>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for PlainDecoratorSeed {
    type Value = Result<(mm::Decorator, Decorator), Kept>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any valid JSON value")
    }

    kept_value_arms!(Err);

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        KeptSeed.visit_seq(seq).map(Err)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        // `$class`.
        let Some(key) = map.next_key_seed(KeySeed)? else {
            return Ok(Err(Kept::Object(Vec::new())));
        };
        if key != "$class" {
            return read_entries(Vec::with_capacity(4), Prefixed::new(key, map)).map(Err);
        }
        let class = match map.next_value_seed(LeafSeed)? {
            Leaf::Str(Cow::Borrowed(class)) => class_name(class),
            Leaf::Str(Cow::Owned(class)) => concerto_metamodel::utils::class_name_owned(class),
            leaf => {
                let entries = vec![(Cow::Borrowed("$class"), leaf.into_kept())];
                return read_entries(entries, map).map(Err);
            }
        };
        // `name`.
        let Some(key) = map.next_key_seed(KeySeed)? else {
            return Ok(Err(Kept::Object(decorator_entries(Some(class), None))));
        };
        if key != "name" {
            let entries = decorator_entries(Some(class), None);
            return read_entries(entries, Prefixed::new(key, map)).map(Err);
        }
        let name = match map.next_value_seed(LeafSeed)? {
            Leaf::Str(name) => leaf_name(name),
            leaf => {
                let mut entries = decorator_entries(Some(class), None);
                entries.push((Cow::Borrowed("name"), leaf.into_kept()));
                return read_entries(entries, map).map(Err);
            }
        };
        // `arguments`, if any.
        let (literals, arguments) = match map.next_key_seed(KeySeed)? {
            None => (None, Vec::new()),
            Some(key) if key == "arguments" => match map.next_value_seed(PlainArgumentsSeed)? {
                Ok((literals, arguments)) => (Some(literals), arguments),
                Err(kept) => {
                    let mut entries = decorator_entries(Some(class), Some(name));
                    entries.push((Cow::Borrowed("arguments"), kept));
                    return read_entries(entries, map).map(Err);
                }
            },
            Some(key) => {
                let entries = decorator_entries(Some(class), Some(name));
                return read_entries(entries, Prefixed::new(key, map)).map(Err);
            }
        };
        let generated = mm::Decorator {
            _class: class,
            name,
            arguments: literals,
            location: None,
        };
        // No other key.
        if let Some(key) = map.next_key_seed(KeySeed)? {
            let Kept::Object(entries) = decorator_kept(generated) else {
                unreachable!("a decorator is an object");
            };
            return read_entries(entries, Prefixed::new(key, map)).map(Err);
        }
        let processed = Decorator::from_read(generated.name.clone(), arguments);
        Ok(Ok((generated, processed)))
    }
}

/// A map whose first key has been read already: that key, then the rest.
struct Prefixed<'de, A> {
    key: Option<Cow<'de, str>>,
    inner: A,
}

impl<'de, A> Prefixed<'de, A> {
    fn new(key: Cow<'de, str>, inner: A) -> Self {
        Prefixed {
            key: Some(key),
            inner,
        }
    }
}

impl<'de, A: MapAccess<'de>> MapAccess<'de> for Prefixed<'de, A> {
    type Error = A::Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, A::Error> {
        match self.key.take() {
            Some(Cow::Borrowed(key)) => seed
                .deserialize(de::value::BorrowedStrDeserializer::new(key))
                .map(Some),
            Some(Cow::Owned(key)) => seed
                .deserialize(de::value::StringDeserializer::new(key))
                .map(Some),
            None => self.inner.next_key_seed(seed),
        }
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, A::Error> {
        self.inner.next_value_seed(seed)
    }
}

/// The processed arguments of a decorator, in order.
type Arguments = Vec<super::decorator::DecoratorArgument>;

/// Reads a decorator's `arguments` value: an array of argument nodes as
/// [`PlainArgumentSeed`] reads them, as the generated and the processed
/// arguments, or else as a [`Kept`].
struct PlainArgumentsSeed;

impl<'de> DeserializeSeed<'de> for PlainArgumentsSeed {
    type Value = Result<(Vec<mm::DecoratorLiteral>, Arguments), Kept>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for PlainArgumentsSeed {
    type Value = Result<(Vec<mm::DecoratorLiteral>, Arguments), Kept>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any valid JSON value")
    }

    kept_value_arms!(Err);

    fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
        KeptSeed.visit_map(map).map(Err)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Self::Value, A::Error> {
        let mut literals = Vec::new();
        let mut arguments = Vec::new();
        while let Some(item) = seq.next_element_seed(PlainArgumentSeed)? {
            match item {
                Ok((literal, argument)) => {
                    push_sized(&mut literals, literal);
                    push_sized(&mut arguments, argument);
                }
                Err(kept) => {
                    let mut items: Vec<Kept> = Vec::with_capacity(literals.len() + 1);
                    items.extend(literals.into_iter().map(argument_kept));
                    items.push(kept);
                    while let Some(item) = seq.next_element_seed(KeptSeed)? {
                        items.push(item);
                    }
                    return Ok(Err(Kept::Array(items)));
                }
            }
        }
        Ok(Ok((literals, arguments)))
    }
}

/// Reads one element of an `arguments` array: a `DecoratorString`,
/// `DecoratorNumber` or `DecoratorBoolean` node of a `$class` and then a
/// `value` of its type, and no other key ([`plain_argument`]), as its
/// generated and its processed argument, or else as a [`Kept`].
struct PlainArgumentSeed;

impl<'de> DeserializeSeed<'de> for PlainArgumentSeed {
    type Value = Result<(mm::DecoratorLiteral, super::decorator::DecoratorArgument), Kept>;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for PlainArgumentSeed {
    type Value = Result<(mm::DecoratorLiteral, super::decorator::DecoratorArgument), Kept>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any valid JSON value")
    }

    kept_value_arms!(Err);

    fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<Self::Value, A::Error> {
        KeptSeed.visit_seq(seq).map(Err)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
        use super::decorator::DecoratorArgument;

        // `$class`, one of the three.
        let Some(key) = map.next_key_seed(KeySeed)? else {
            return Ok(Err(Kept::Object(Vec::new())));
        };
        if key != "$class" {
            return read_entries(Vec::with_capacity(4), Prefixed::new(key, map)).map(Err);
        }
        let class = match map.next_value_seed(LeafSeed)? {
            Leaf::Str(class) => match &*class {
                DECORATOR_STRING_CLASS => DECORATOR_STRING_CLASS,
                DECORATOR_NUMBER_CLASS => DECORATOR_NUMBER_CLASS,
                DECORATOR_BOOLEAN_CLASS => DECORATOR_BOOLEAN_CLASS,
                _ => {
                    let entries = vec![(Cow::Borrowed("$class"), Leaf::Str(class).into_kept())];
                    return read_entries(entries, map).map(Err);
                }
            },
            leaf => {
                let entries = vec![(Cow::Borrowed("$class"), leaf.into_kept())];
                return read_entries(entries, map).map(Err);
            }
        };
        let class_entry = || {
            (
                Cow::Borrowed("$class"),
                Kept::Other(Value::String(class.to_string())),
            )
        };
        // `value`, of the type `$class` names.
        let Some(key) = map.next_key_seed(KeySeed)? else {
            return Ok(Err(Kept::Object(vec![class_entry()])));
        };
        if key != "value" {
            return read_entries(vec![class_entry()], Prefixed::new(key, map)).map(Err);
        }
        let value = map.next_value_seed(LeafSeed)?;
        let read = match (class, value) {
            (DECORATOR_STRING_CLASS, Leaf::Str(value)) => {
                let value = value.into_owned();
                Ok((
                    mm::DecoratorLiteral::DecoratorString(mm::DecoratorString {
                        location: None,
                        value: value.clone(),
                    }),
                    DecoratorArgument::String(value),
                ))
            }
            (DECORATOR_NUMBER_CLASS, Leaf::Other(Kept::Other(Value::Number(number)))) => {
                match number.as_f64() {
                    Some(value) => Ok((
                        mm::DecoratorLiteral::DecoratorNumber(mm::DecoratorNumber {
                            location: None,
                            value,
                        }),
                        DecoratorArgument::Number(value),
                    )),
                    None => Err(Kept::Other(Value::Number(number))),
                }
            }
            (DECORATOR_BOOLEAN_CLASS, Leaf::Other(Kept::Other(Value::Bool(value)))) => Ok((
                mm::DecoratorLiteral::DecoratorBoolean(mm::DecoratorBoolean {
                    location: None,
                    value,
                }),
                DecoratorArgument::Boolean(value),
            )),
            (_, value) => Err(value.into_kept()),
        };
        // No other key.
        match (read, map.next_key_seed(KeySeed)?) {
            (Ok(read), None) => Ok(Ok(read)),
            (read, key) => {
                let value = match read {
                    Ok((literal, _)) => {
                        let Kept::Object(mut entries) = argument_kept(literal) else {
                            unreachable!("an argument is an object");
                        };
                        entries
                            .pop()
                            .map_or(Kept::Other(Value::Null), |(_, value)| value)
                    }
                    Err(value) => value,
                };
                let entries = vec![class_entry(), (Cow::Borrowed("value"), value)];
                match key {
                    None => Ok(Err(Kept::Object(entries))),
                    Some(key) => read_entries(entries, Prefixed::new(key, map)).map(Err),
                }
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests;
