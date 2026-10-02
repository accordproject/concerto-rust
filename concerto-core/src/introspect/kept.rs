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

    fn to_string(&self, canonical: &str) -> String {
        match self {
            ClassRead::Canonical => canonical.to_string(),
            ClassRead::Other(class) => class.clone(),
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
                            .to_string(POSITION_CLASS),
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
                .map_or_else(String::new, |class| class.to_string(POSITION_CLASS)),
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
                        .to_string(RANGE_CLASS),
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
                .map_or_else(String::new, |class| class.to_string(RANGE_CLASS)),
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
    /// string>}`, its two keys in either order.
    By(String),
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
    name: Option<String>,
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
    let name = name.map(|name| (Cow::Borrowed("name"), Kept::Other(Value::String(name))));
    let (first, second) = if class_first {
        (class, name)
    } else {
        (name, class)
    };
    first.into_iter().chain(second).collect()
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
                        name = Some(value.into_owned());
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
        _class: String::new(),
        name: String::new(),
        arguments: None,
        location: None,
    };
    if let Kept::Object(entries) = node {
        for (key, value) in entries {
            match (key.as_ref(), value) {
                ("$class", value) => decorator._class = value.into_string(),
                ("name", value) => decorator.name = value.into_string(),
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

#[cfg(test)]
pub(crate) mod tests {
    use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
    use serde::de::DeserializeSeed;
    use serde_json::Value;

    use super::{Kept, KeptSeed, Location, LocationSeed};
    use crate::introspect::typed_ast::strict_from_value;

    /// `location` values: the usual `Range`, and every way one can differ
    /// from it (each read field by field as far as it can be, then as a
    /// [`Kept`]).
    pub(crate) const CASES: [&str; 24] = [
        r#"{"$class":"concerto.metamodel@1.0.0.Range","start":{"offset":78,"line":4,"column":3,"$class":"concerto.metamodel@1.0.0.Position"},"end":{"offset":105,"line":5,"column":1,"$class":"concerto.metamodel@1.0.0.Position"}}"#,
        r#"{"start":{"line":1,"column":2,"offset":3},"end":{"line":1.0,"column":-2,"offset":3e2},"source":null}"#,
        r#"{"start":{"line":1,"column":2,"offset":3},"end":{"line":1,"column":2,"offset":3},"source":"x.cto"}"#,
        r#"{"$class":"concerto.metamodel@1.0.0.Range","start":{"$class":"concerto.metamodel@1.0.0.Position","line":1,"column":2,"offset":3},"end":{"$class":"Position","line":1,"column":2,"offset":3}}"#,
        r#"{"$class":"x","start":{"line":1.5,"column":2,"offset":3},"end":{"line":1,"column":2,"offset":3},"source":"x"}"#,
        r#"{"start":{"line":1,"column":2,"offset":3,"line":7},"end":{"line":1,"column":2,"offset":3},"start":{"line":9,"column":2,"offset":3}}"#,
        r#"{"start":"x","end":{"line":1,"column":2,"offset":3},"start":{"line":9,"column":2,"offset":3}}"#,
        r#"{"start":{"line":1,"column":2,"offset":3},"end":{"line":1,"column":2,"offset":3},"extra":1}"#,
        r#"{"start":{"line":1,"column":2,"offset":3,"extra":{}},"end":{"line":1,"column":2,"offset":3}}"#,
        r#"{"start":{"line":1,"column":2},"end":{"line":1,"column":2,"offset":3}}"#,
        r#"{"start":{"line":null,"column":2,"offset":3},"end":{"line":1,"column":2,"offset":3}}"#,
        r#"{"start":{"line":"1","column":2,"offset":3},"end":{"line":1,"column":2,"offset":3}}"#,
        r#"{"$class":null,"start":{"line":1,"column":2,"offset":3},"end":{"line":1,"column":2,"offset":3}}"#,
        r#"{"$class":"a","$class":"b","start":{"line":1,"column":2,"offset":3},"end":{"line":1,"column":2,"offset":3}}"#,
        r#"{"start":["concerto.metamodel@1.0.0.Position",1,2,3],"end":{"line":1,"column":2,"offset":3}}"#,
        r#"["a",["b",1,2,3],["c",4,5,6]]"#,
        r#"{"start":{"line":1,"column":2,"offset":3},"end":{"line":1,"column":2,"offset":3}}"#,
        r#"{"start":{"line":18446744073709551616,"column":2,"offset":-3},"end":{"line":1,"column":2,"offset":3}}"#,
        r#"{"end":{"line":1,"column":2,"offset":3}}"#,
        r#"{}"#,
        r#"null"#,
        r#"7"#,
        r#""x""#,
        r#"true"#,
    ];

    /// Text that is not JSON.
    const INVALID: [&str; 5] = [
        r#"{"start":1e400}"#,
        r#"{"start":{"line":1e400}}"#,
        r#"{"start":"\ud800"}"#,
        r#"{"start":{"line":1,}}"#,
        r#"{"start" 1}"#,
    ];

    fn kept(text: &str) -> Result<Kept, serde_json::Error> {
        let mut d = serde_json::Deserializer::from_str(text);
        let kept = KeptSeed.deserialize(&mut d)?;
        d.end()?;
        Ok(kept)
    }

    fn location(text: &str) -> Result<Location, serde_json::Error> {
        let mut d = serde_json::Deserializer::from_str(text);
        let location = LocationSeed.deserialize(&mut d)?;
        d.end()?;
        Ok(location)
    }

    /// What `typed_ast` did before P5-76: the `Value`, decoded strictly.
    fn decoded_from_value(value: &Value) -> Result<String, ()> {
        strict_from_value::<Option<mm::Range>>(value)
            .map(|range| format!("{range:?}"))
            .map_err(|_| ())
    }

    /// Each case read as a [`Kept`] is its `Value` parse, and decodes into
    /// `Option<Range>` exactly as that `Value` does (the same result, or an
    /// error for both).
    #[test]
    fn kept_is_the_value_and_decodes_as_it() {
        for text in CASES {
            let value: Value = serde_json::from_str(text).unwrap();
            let kept = kept(text).unwrap();
            assert_eq!(kept.to_value(), value, "{text}");
            assert_eq!(
                serde_json::to_string(&kept.to_value()).unwrap(),
                serde_json::to_string(&value).unwrap(),
                "{text}"
            );
            let decoded = kept
                .strict_decode::<Option<mm::Range>>()
                .map(|range| format!("{range:?}"))
                .map_err(|_| ());
            assert_eq!(decoded, decoded_from_value(&value), "{text}");
        }
    }

    /// The same for a [`Location`], read from the text and from its
    /// parsed `Value` (`typed_ast::from_value`); the usual cases are read
    /// field by field.
    #[test]
    fn location_is_the_value_and_decodes_as_it() {
        let mut field_by_field = 0;
        for text in CASES {
            let value: Value = serde_json::from_str(text).unwrap();
            for location in [
                location(text).unwrap(),
                LocationSeed.deserialize(&value).unwrap(),
            ] {
                if matches!(location, Location::Range(_)) {
                    field_by_field += 1;
                }
                assert_eq!(location.to_value(), value, "{text}");
                assert_eq!(
                    serde_json::to_string(&location.to_value()).unwrap(),
                    serde_json::to_string(&value).unwrap(),
                    "{text}"
                );
                let decoded = location
                    .decode()
                    .map(|range| format!("{range:?}"))
                    .map_err(|_| ());
                assert_eq!(decoded, decoded_from_value(&value), "{text}");
            }
        }
        // Ten cases from both reads (the first five; a missing field, an
        // escaped key and a number past `u64`), and the three with a
        // repeated key from the `Value`, which has only the last.
        assert_eq!(field_by_field, 23);
    }

    /// Text that is not JSON is the same kind of error for both reads as
    /// for a `Value` parse.
    #[test]
    fn kept_and_location_reject_what_a_value_parse_rejects() {
        for text in INVALID {
            let value = serde_json::from_str::<Value>(text).unwrap_err();
            let kept = kept(text).unwrap_err();
            let location = location(text).unwrap_err();
            assert_eq!(value.classify(), kept.classify(), "{text}");
            assert_eq!(value.classify(), location.classify(), "{text}");
        }
    }

    /// `decorators` values: the usual lists, and every way one can differ
    /// from them.
    pub(crate) const DECORATOR_CASES: [&str; 28] = [
        r#"[]"#,
        r#"null"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d"}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":"x"},{"$class":"concerto.metamodel@1.0.0.DecoratorNumber","value":1.5},{"$class":"concerto.metamodel@1.0.0.DecoratorBoolean","value":true},{"$class":"concerto.metamodel@1.0.0.DecoratorTypeReference","type":{"$class":"concerto.metamodel@1.0.0.TypeIdentifier","name":"T"},"isArray":true},{"$class":"concerto.metamodel@1.0.0.DecoratorTypeReference","type":{"$class":"concerto.metamodel@1.0.0.TypeIdentifier","name":"U","namespace":"org.x@1.0.0"}}]}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","location":{"$class":"concerto.metamodel@1.0.0.Range","start":{"offset":1,"line":1,"column":2,"$class":"concerto.metamodel@1.0.0.Position"},"end":{"offset":5,"line":1,"column":6,"$class":"concerto.metamodel@1.0.0.Position"}},"arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorNumber","value":-0,"location":null}]}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"a"},{"$class":"concerto.metamodel@1.0.0.Decorator","name":"b","arguments":[]}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","extra":1}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":"x","extra":1}]}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorFoo","value":"x"}]}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[null]}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"value":"x","$class":"concerto.metamodel@1.0.0.DecoratorString"}]}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":"x"}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":1}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"a","name":"b"}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","n\u0061me":"d"}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator"}]"#,
        r#"[{"name":"d"}]"#,
        r#"[null]"#,
        r#""ab""#,
        r#"{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d"}"#,
        // P5-93: lists `Kept::into_decorators` reads by moving strings out,
        // and ones it leaves to the strict decode.
        r#"[{"name":"d","arguments":null,"$class":"concerto.metamodel@1.0.0.Decorator"},{"$class":"Decorator","name":"e\u0021","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorNumber","value":18446744073709551615},{"value":-3,"$class":"concerto.metamodel@1.0.0.DecoratorNumber"},{"$class":"concerto.metamodel@1.0.0.DecoratorBoolean","value":false}]}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorNumber","value":1e300},{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":"\ud83d\ude00"}]}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":1}]}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorBoolean","value":"true"}]}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString"}]}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":"d","arguments":[{"$class":"concerto.metamodel@1.0.0.DecoratorString","value":"x","value":"y"}]}]"#,
        r#"[{"$class":7,"name":"d"}]"#,
        r#"[{"$class":"concerto.metamodel@1.0.0.Decorator","name":null}]"#,
    ];

    /// `identified` values.
    pub(crate) const IDENTIFIED_CASES: [&str; 20] = [
        r#"{"$class":"concerto.metamodel@1.0.0.IdentifiedBy","name":"id"}"#,
        r#"{"$class":"concerto.metamodel@1.0.0.Identified"}"#,
        r#"null"#,
        r#"{}"#,
        r#"{"$class":"concerto.metamodel@1.0.0.IdentifiedBy"}"#,
        r#"{"$class":"concerto.metamodel@1.0.0.IdentifiedBy","name":"id","extra":1}"#,
        r#"{"name":"id","$class":"concerto.metamodel@1.0.0.IdentifiedBy"}"#,
        r#"{"$class":"x","name":"id"}"#,
        r#"[]"#,
        r#"0"#,
        // P5-93: values `IdentifiedSeed` reads field by field, or as far
        // as it can before reading the rest as a `Kept`.
        r#"{"name":"id"}"#,
        r#"{"$class":"concerto.metamodel@1.0.0.Identified","name":"id"}"#,
        r#"{"name":"id","$class":"concerto.metamodel@1.0.0.Identified"}"#,
        r#"{"name":1,"$class":"concerto.metamodel@1.0.0.IdentifiedBy"}"#,
        r#"{"$class":"concerto.metamodel@1.0.0.IdentifiedBy","name":null}"#,
        r#"{"$class":"concerto.metamodel@1.0.0.IdentifiedBy","$class":"concerto.metamodel@1.0.0.Identified"}"#,
        r#"{"$class":"concerto.metamodel@1.0.0.IdentifiedBy","name":"a","name":"b"}"#,
        r#"{"n\u0061me":"id","$cl\u0061ss":"concerto.metamodel@1.0.0.IdentifiedBy"}"#,
        r#"{"$class":"concerto.metamodel@1.0.0.IdentifiedBy","name":"\u0069d"}"#,
        r#"{"extra":1,"$class":"concerto.metamodel@1.0.0.Identified"}"#,
    ];

    /// P5-76: a `decorators` value read as a [`Kept`] is its `Value`, and
    /// decodes into the generated decorators, and gives the processed
    /// [`Decorator`]s, exactly as that `Value` does.
    ///
    /// [`Decorator`]: crate::introspect::Decorator
    #[test]
    fn kept_decorators_are_the_value_and_decode_as_it() {
        use crate::introspect::decorator::{parse_decorator_list, parse_decorators};

        for text in DECORATOR_CASES {
            let value: Value = serde_json::from_str(text).unwrap();
            let kept = kept(text).unwrap();
            assert_eq!(kept.to_value(), value, "{text}");
            let decoded = kept
                .strict_decode::<Option<Vec<mm::Decorator>>>()
                .map(|d| format!("{d:?}"))
                .map_err(|_| ());
            let from_value = strict_from_value::<Option<Vec<mm::Decorator>>>(&value)
                .map(|d| format!("{d:?}"))
                .map_err(|_| ());
            assert_eq!(decoded, from_value, "{text}");
            assert_eq!(
                parse_decorator_list(Some(&kept)),
                parse_decorators(&serde_json::json!({ "decorators": value })),
                "{text}"
            );
            // P5-93: decoded by moving the strings out, the same result.
            let moved = kept
                .into_decorators()
                .map(|d| format!("{d:?}"))
                .map_err(|_| ());
            assert_eq!(moved, from_value, "{text}");
        }
    }

    /// P5-76: the same for an `identified` value.
    #[test]
    fn kept_identified_is_the_value_and_decodes_as_it() {
        for text in IDENTIFIED_CASES {
            let value: Value = serde_json::from_str(text).unwrap();
            let kept = kept(text).unwrap();
            assert_eq!(kept.to_value(), value, "{text}");
            let decoded = kept
                .strict_decode::<Option<mm::Identified>>()
                .map(|d| format!("{d:?}"))
                .map_err(|_| ());
            let from_value = strict_from_value::<Option<mm::Identified>>(&value)
                .map(|d| format!("{d:?}"))
                .map_err(|_| ());
            assert_eq!(decoded, from_value, "{text}");
        }
    }

    /// P5-93: an `identified` value read by [`IdentifiedSeed`], from the
    /// text and from its `Value`, decodes as its `Value` does; it is kept
    /// as a [`Kept`] (its `Value`) unless it is one of the metamodel's own
    /// two nodes, which BC-19's shape check accepts.
    ///
    /// [`IdentifiedSeed`]: super::IdentifiedSeed
    #[test]
    fn identified_is_read_field_by_field_and_decodes_as_its_value() {
        use super::{IdentifiedRead, IdentifiedSeed};
        use crate::instance::metamodel::check_ast_shape;

        let mut field_by_field = 0;
        for text in IDENTIFIED_CASES {
            let value: Value = serde_json::from_str(text).unwrap();
            let from_value = strict_from_value::<Option<mm::Identified>>(&value)
                .map(|d| format!("{d:?}"))
                .map_err(|_| ());
            let mut d = serde_json::Deserializer::from_str(text);
            for read in [
                IdentifiedSeed.deserialize(&mut d).unwrap(),
                IdentifiedSeed.deserialize(&value).unwrap(),
            ] {
                if !matches!(read, IdentifiedRead::Kept(_)) {
                    field_by_field += 1;
                }
                match read.decode() {
                    Ok((identified, kept)) => {
                        assert_eq!(Ok(format!("{identified:?}")), from_value, "{text}");
                        match kept {
                            Some(kept) => assert_eq!(kept.to_value(), value, "{text}"),
                            None => {
                                let ast = serde_json::json!({
                                    "$class": "concerto.metamodel@1.0.0.Model",
                                    "namespace": "org.acme@1.0.0",
                                    "declarations": [{
                                        "$class": "concerto.metamodel@1.0.0.AssetDeclaration",
                                        "name": "A", "isAbstract": false, "properties": [],
                                        "identified": value,
                                    }],
                                });
                                assert!(check_ast_shape(&ast).is_ok(), "{text}");
                            }
                        }
                    }
                    Err(_) => assert_eq!(from_value, Err(()), "{text}"),
                }
            }
        }
        // From both reads: the two nodes, `IdentifiedBy` in the other key
        // order, with an escaped name, and with escaped keys; from the
        // `Value`, the two with a repeated key (it has only the last).
        assert_eq!(field_by_field, 12);
    }
}
