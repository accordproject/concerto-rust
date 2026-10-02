//! P5-81 spike (accordproject/concerto-rust#425): a table-driven decoder for
//! the metamodel types, behind the `table-decoder` feature. Analysis only;
//! not for merge.
//!
//! `serde`'s derive generates a `Deserialize` impl per type, and the
//! compiler instantiates each one for every deserializer it is used with
//! (JSON text, a `Value`, the typed read's map adapters, its `Strict`
//! wrapper, ...). Here one decode loop, generic only over the deserializer,
//! walks static tables that `build.rs` generates from `metamodel.json` (each
//! type's `$class`, fields, field kinds, optional/array/default flags and
//! variant list), collects a node's fields into [`Slot`]s, and builds the
//! typed struct through a small generated constructor per type.
//!
//! # Accept/reject behaviour
//!
//! The loop mirrors what the derived impls accept, so a caller that used
//! `T::deserialize(Strict(d))` (strict) or `T::deserialize(d)` (lenient) gets
//! the same verdicts:
//! - a struct reads from a map, or from a sequence of its fields in order
//!   (`serde`'s positional form), where a missing field is an error unless it
//!   is optional (map form only) or has a default;
//! - a duplicate field is an error; an unknown key is an error in
//!   [`Mode::Strict`] and is skipped (after a full parse, as `serde`'s
//!   buffering does) in [`Mode::Lenient`];
//! - a `$class`-tagged enum (`#[serde(tag = "$class")]`) reads its tag from
//!   a map (in any position) or as the first element of a sequence; its
//!   variant's payload is read leniently at every depth, as the derived impl
//!   reads it from `serde`'s buffered content; a repeated `$class` is an
//!   error;
//! - `String`, `f64` and `bool` fields take exactly the JSON types the
//!   derived impls take (`f64` from any number).
//!
//! Known difference (prototype): when a tagged node's `$class` is not its
//! first key in JSON *text*, the node is buffered as a `serde_json::Value`
//! (as the typed read's `read_class` does), so a duplicate key nested inside
//! it is collapsed instead of refused. `JSON.stringify` never writes one.

use std::any::Any;
use std::fmt;

use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess, Visitor};

use crate::concerto_metamodel_1_0_0 as mm;

/// A decoded node, before its constructor's caller takes its type back.
pub type Erased = Box<dyn Any>;

/// A struct's generated constructor.
pub type Build = fn(&mut [Slot]) -> Erased;

/// A tagged enum's generated variant constructor.
pub type Wrap = fn(usize, Option<Erased>) -> Erased;

/// Whether an unknown key is an error ([`Mode::Strict`], the typed read's
/// `Strict` wrapper) or skipped ([`Mode::Lenient`], plain `serde`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Strict,
    Lenient,
}

/// A field's JSON kind.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Str,
    F64,
    Bool,
    /// A node of the type at this index of [`TYPES`].
    Node(u16),
}

/// One field of a struct.
#[derive(Debug)]
pub struct Field {
    pub name: &'static str,
    pub kind: Kind,
    pub array: bool,
    pub optional: bool,
    /// `#[serde(default)]`: `false`, or `""` for a `$class`.
    pub default: bool,
}

/// One variant of a `$class`-tagged enum: its `$class` and its payload's
/// type, or `None` for a unit variant.
#[derive(Debug)]
pub struct Variant {
    pub class: &'static str,
    pub payload: Option<u16>,
}

/// What a type is.
pub enum Shape {
    Struct {
        fields: &'static [Field],
        names: &'static [&'static str],
        build: Build,
    },
    Union {
        variants: &'static [Variant],
        names: &'static [&'static str],
        wrap: Wrap,
    },
}

/// One type of the table.
pub struct TypeDesc {
    pub name: &'static str,
    pub shape: Shape,
}

/// The most fields a struct has (`build.rs` checks it).
const MAX_FIELDS: usize = 16;

/// A field's value, as the loop read it.
#[derive(Default)]
pub enum Slot {
    #[default]
    Missing,
    Null,
    Str(String),
    F64(f64),
    Bool(bool),
    Node(Erased),
    List(Vec<Slot>),
}

/// The typed value of an erased node.
pub fn unerase<T: 'static>(node: Erased) -> T {
    *node
        .downcast::<T>()
        .unwrap_or_else(|_| unreachable!("the table builds each node's own type"))
}

const SHAPE: &str = "the table checked the slot's shape";

impl Slot {
    fn take(&mut self) -> Slot {
        std::mem::take(self)
    }

    pub fn str(&mut self) -> String {
        match self.take() {
            Slot::Str(s) => s,
            _ => unreachable!("{SHAPE}"),
        }
    }

    pub fn f64(&mut self) -> f64 {
        match self.take() {
            Slot::F64(n) => n,
            _ => unreachable!("{SHAPE}"),
        }
    }

    pub fn bool(&mut self) -> bool {
        match self.take() {
            Slot::Bool(b) => b,
            _ => unreachable!("{SHAPE}"),
        }
    }

    pub fn node<T: 'static>(&mut self) -> T {
        match self.take() {
            Slot::Node(n) => unerase(n),
            _ => unreachable!("{SHAPE}"),
        }
    }

    fn list(&mut self) -> Vec<Slot> {
        match self.take() {
            Slot::List(items) => items,
            _ => unreachable!("{SHAPE}"),
        }
    }

    fn is_null(&self) -> bool {
        matches!(self, Slot::Null)
    }

    pub fn opt_str(&mut self) -> Option<String> {
        (!self.is_null()).then(|| self.str())
    }

    pub fn opt_f64(&mut self) -> Option<f64> {
        (!self.is_null()).then(|| self.f64())
    }

    pub fn opt_bool(&mut self) -> Option<bool> {
        (!self.is_null()).then(|| self.bool())
    }

    pub fn opt_node<T: 'static>(&mut self) -> Option<T> {
        (!self.is_null()).then(|| self.node())
    }

    pub fn list_str(&mut self) -> Vec<String> {
        self.list().iter_mut().map(Slot::str).collect()
    }

    pub fn list_f64(&mut self) -> Vec<f64> {
        self.list().iter_mut().map(Slot::f64).collect()
    }

    pub fn list_bool(&mut self) -> Vec<bool> {
        self.list().iter_mut().map(Slot::bool).collect()
    }

    pub fn list_node<T: 'static>(&mut self) -> Vec<T> {
        self.list().iter_mut().map(Slot::node).collect()
    }

    pub fn opt_list_str(&mut self) -> Option<Vec<String>> {
        (!self.is_null()).then(|| self.list_str())
    }

    pub fn opt_list_f64(&mut self) -> Option<Vec<f64>> {
        (!self.is_null()).then(|| self.list_f64())
    }

    pub fn opt_list_bool(&mut self) -> Option<Vec<bool>> {
        (!self.is_null()).then(|| self.list_bool())
    }

    pub fn opt_list_node<T: 'static>(&mut self) -> Option<Vec<T>> {
        (!self.is_null()).then(|| self.list_node())
    }
}

/// A type the table decodes.
pub trait TableDecode: Sized {
    /// Decodes `Self` from `d`, as `Self::deserialize(Strict(d))` does
    /// ([`Mode::Strict`]) or `Self::deserialize(d)` ([`Mode::Lenient`]).
    fn decode<'de, D: Deserializer<'de>>(d: D, mode: Mode) -> Result<Self, D::Error>;
}

impl<T: TableDecode> TableDecode for Option<T> {
    fn decode<'de, D: Deserializer<'de>>(d: D, mode: Mode) -> Result<Self, D::Error> {
        struct V<T>(Mode, std::marker::PhantomData<T>);
        impl<'de, T: TableDecode> Visitor<'de> for V<T> {
            type Value = Option<T>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("option")
            }
            fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
                T::decode(d, self.0).map(Some)
            }
        }
        d.deserialize_option(V(mode, std::marker::PhantomData))
    }
}

impl<T: TableDecode> TableDecode for Vec<T> {
    fn decode<'de, D: Deserializer<'de>>(d: D, mode: Mode) -> Result<Self, D::Error> {
        struct S<T>(Mode, std::marker::PhantomData<T>);
        impl<'de, T: TableDecode> DeserializeSeed<'de> for S<T> {
            type Value = T;
            fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<T, D::Error> {
                T::decode(d, self.0)
            }
        }
        struct V<T>(Mode, std::marker::PhantomData<T>);
        impl<'de, T: TableDecode> Visitor<'de> for V<T> {
            type Value = Vec<T>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a sequence")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Vec<T>, A::Error> {
                let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096));
                while let Some(item) =
                    seq.next_element_seed(S::<T>(self.0, std::marker::PhantomData))?
                {
                    items.push(item);
                }
                Ok(items)
            }
        }
        d.deserialize_seq(V(mode, std::marker::PhantomData))
    }
}

/// Decodes the node of type `ty` (an index into [`TYPES`]).
pub fn decode_node<'de, D: Deserializer<'de>>(
    d: D,
    ty: u16,
    mode: Mode,
) -> Result<Erased, D::Error> {
    NodeSeed { ty, mode }.deserialize(d)
}

fn desc(ty: u16) -> &'static TypeDesc {
    &TYPES[usize::from(ty)]
}

// ---------------------------------------------------------------------------
// The loop
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct NodeSeed {
    ty: u16,
    mode: Mode,
}

impl<'de> DeserializeSeed<'de> for NodeSeed {
    type Value = Erased;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Erased, D::Error> {
        let desc = desc(self.ty);
        match desc.shape {
            Shape::Struct { names, .. } => d.deserialize_struct(
                desc.name,
                names,
                StructVisitor {
                    ty: self.ty,
                    mode: self.mode,
                    payload: false,
                },
            ),
            // A variant's payload is read leniently whatever the mode, as
            // the derived impl reads it from `serde`'s buffered content.
            Shape::Union { .. } => d.deserialize_any(UnionVisitor { ty: self.ty }),
        }
    }
}

/// A struct's map or sequence. `payload`: the struct is a tagged enum's
/// variant payload, read after its `$class`, so another `$class` is a
/// duplicate.
struct StructVisitor {
    ty: u16,
    mode: Mode,
    payload: bool,
}

fn struct_parts(ty: u16) -> (&'static [Field], Build) {
    match desc(ty).shape {
        Shape::Struct { fields, build, .. } => (fields, build),
        Shape::Union { .. } => unreachable!("a struct type"),
    }
}

fn empty_slots() -> [Slot; MAX_FIELDS] {
    std::array::from_fn(|_| Slot::Missing)
}

/// Fills in what a node did not give: `None` for an optional field, the
/// default for a defaulted one, an error otherwise.
fn finish<E: de::Error>(
    fields: &'static [Field],
    slots: &mut [Slot; MAX_FIELDS],
    build: Build,
) -> Result<Erased, E> {
    for (field, slot) in fields.iter().zip(slots.iter_mut()) {
        if matches!(slot, Slot::Missing) {
            *slot = if field.optional {
                Slot::Null
            } else if field.default {
                default_slot(field)
            } else {
                return Err(de::Error::missing_field(field.name));
            };
        }
    }
    Ok(build(&mut slots[..fields.len()]))
}

fn default_slot(field: &Field) -> Slot {
    match field.kind {
        Kind::Str => Slot::Str(String::new()),
        Kind::Bool => Slot::Bool(false),
        Kind::F64 => Slot::F64(0.0),
        Kind::Node(_) => unreachable!("no node field has a default"),
    }
}

/// Reads a struct's entries from `map` (after its `$class`, for a payload).
fn read_struct_map<'de, A: MapAccess<'de>>(
    ty: u16,
    mode: Mode,
    payload: bool,
    map: &mut A,
) -> Result<Erased, A::Error> {
    let (fields, build) = struct_parts(ty);
    let mut slots = empty_slots();
    while let Some(key) = map.next_key_seed(KeySeed(fields))? {
        match key {
            Key::Field(i) => {
                let field = &fields[i];
                if !matches!(slots[i], Slot::Missing) {
                    return Err(de::Error::duplicate_field(field.name));
                }
                slots[i] = map.next_value_seed(FieldSeed::new(field, mode))?;
            }
            Key::Class if payload => return Err(de::Error::duplicate_field("$class")),
            Key::Class | Key::Other => match mode {
                Mode::Strict => return Err(de::Error::custom("unknown field")),
                Mode::Lenient => map.next_value_seed(Discard)?,
            },
        }
    }
    finish(fields, &mut slots, build)
}

/// Reads a struct's fields, in order, from `seq` (after its `$class`, for a
/// payload).
fn read_struct_seq<'de, A: SeqAccess<'de>>(
    ty: u16,
    mode: Mode,
    seq: &mut A,
) -> Result<Erased, A::Error> {
    let (fields, build) = struct_parts(ty);
    let mut slots = empty_slots();
    for (i, field) in fields.iter().enumerate() {
        slots[i] = match seq.next_element_seed(FieldSeed::new(field, mode))? {
            Some(slot) => slot,
            None if field.default => default_slot(field),
            None => {
                return Err(de::Error::invalid_length(
                    i,
                    &format_args!("struct {} with {} elements", desc(ty).name, fields.len())
                        .to_string()
                        .as_str(),
                ));
            }
        };
    }
    Ok(build(&mut slots[..fields.len()]))
}

impl<'de> Visitor<'de> for StructVisitor {
    type Value = Erased;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "struct {}", desc(self.ty).name)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Erased, A::Error> {
        read_struct_map(self.ty, self.mode, self.payload, &mut map)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Erased, A::Error> {
        read_struct_seq(self.ty, self.mode, &mut seq)
    }
}

enum Key {
    Field(usize),
    Class,
    Other,
}

/// A map key: a field's index, `$class`, or another key.
#[derive(Clone, Copy)]
struct KeySeed(&'static [Field]);

impl KeySeed {
    fn find(self, key: &str) -> Key {
        match self.0.iter().position(|f| f.name == key) {
            Some(i) => Key::Field(i),
            None if key == "$class" => Key::Class,
            None => Key::Other,
        }
    }
}

impl<'de> DeserializeSeed<'de> for KeySeed {
    type Value = Key;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Key, D::Error> {
        d.deserialize_identifier(self)
    }
}

impl<'de> Visitor<'de> for KeySeed {
    type Value = Key;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("field identifier")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Key, E> {
        Ok(self.find(v))
    }

    fn visit_bytes<E: de::Error>(self, v: &[u8]) -> Result<Key, E> {
        Ok(std::str::from_utf8(v).map_or(Key::Other, |v| self.find(v)))
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Key, E> {
        Ok(usize::try_from(v)
            .ok()
            .filter(|i| *i < self.0.len())
            .map_or(Key::Other, Key::Field))
    }
}

/// One field's value.
#[derive(Clone, Copy)]
struct FieldSeed {
    kind: Kind,
    array: bool,
    optional: bool,
    mode: Mode,
}

impl FieldSeed {
    fn new(field: &Field, mode: Mode) -> Self {
        FieldSeed {
            kind: field.kind,
            array: field.array,
            optional: field.optional,
            mode,
        }
    }
}

impl<'de> DeserializeSeed<'de> for FieldSeed {
    type Value = Slot;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Slot, D::Error> {
        if self.optional {
            return d.deserialize_option(OptionVisitor(FieldSeed {
                optional: false,
                ..self
            }));
        }
        if self.array {
            return d.deserialize_seq(ListVisitor(FieldSeed {
                array: false,
                ..self
            }));
        }
        match self.kind {
            Kind::Str => d.deserialize_string(LeafVisitor(self.kind)),
            Kind::F64 => d.deserialize_f64(LeafVisitor(self.kind)),
            Kind::Bool => d.deserialize_bool(LeafVisitor(self.kind)),
            Kind::Node(ty) => NodeSeed {
                ty,
                mode: self.mode,
            }
            .deserialize(d)
            .map(Slot::Node),
        }
    }
}

struct OptionVisitor(FieldSeed);

impl<'de> Visitor<'de> for OptionVisitor {
    type Value = Slot;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("option")
    }

    fn visit_none<E: de::Error>(self) -> Result<Slot, E> {
        Ok(Slot::Null)
    }

    fn visit_unit<E: de::Error>(self) -> Result<Slot, E> {
        Ok(Slot::Null)
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<Slot, D::Error> {
        self.0.deserialize(d)
    }
}

struct ListVisitor(FieldSeed);

impl<'de> Visitor<'de> for ListVisitor {
    type Value = Slot;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("a sequence")
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Slot, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0).min(4096));
        while let Some(item) = seq.next_element_seed(self.0)? {
            items.push(item);
        }
        Ok(Slot::List(items))
    }
}

/// A string, number or boolean leaf.
struct LeafVisitor(Kind);

impl<'de> Visitor<'de> for LeafVisitor {
    type Value = Slot;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(match self.0 {
            Kind::Str => "a string",
            Kind::F64 => "f64",
            _ => "a boolean",
        })
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<Slot, E> {
        match self.0 {
            Kind::Str => Ok(Slot::Str(v.to_owned())),
            _ => Err(de::Error::invalid_type(de::Unexpected::Str(v), &self)),
        }
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<Slot, E> {
        match self.0 {
            Kind::Str => Ok(Slot::Str(v)),
            _ => Err(de::Error::invalid_type(de::Unexpected::Str(&v), &self)),
        }
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<Slot, E> {
        match self.0 {
            Kind::F64 => Ok(Slot::F64(v)),
            _ => Err(de::Error::invalid_type(de::Unexpected::Float(v), &self)),
        }
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<Slot, E> {
        match self.0 {
            Kind::F64 => Ok(Slot::F64(v as f64)),
            _ => Err(de::Error::invalid_type(de::Unexpected::Unsigned(v), &self)),
        }
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<Slot, E> {
        match self.0 {
            Kind::F64 => Ok(Slot::F64(v as f64)),
            _ => Err(de::Error::invalid_type(de::Unexpected::Signed(v), &self)),
        }
    }

    fn visit_bool<E: de::Error>(self, v: bool) -> Result<Slot, E> {
        match self.0 {
            Kind::Bool => Ok(Slot::Bool(v)),
            _ => Err(de::Error::invalid_type(de::Unexpected::Bool(v), &self)),
        }
    }
}

// ---------------------------------------------------------------------------
// `$class`-tagged enums
// ---------------------------------------------------------------------------

struct UnionVisitor {
    ty: u16,
}

type UnionParts = (&'static [Variant], &'static [&'static str], Wrap);

fn union_parts(ty: u16) -> UnionParts {
    match desc(ty).shape {
        Shape::Union {
            variants,
            names,
            wrap,
        } => (variants, names, wrap),
        Shape::Struct { .. } => unreachable!("a union type"),
    }
}

fn variant_of<E: de::Error>(ty: u16, class: &str) -> Result<usize, E> {
    let (variants, names, _) = union_parts(ty);
    variants
        .iter()
        .position(|v| v.class == class)
        .ok_or_else(|| de::Error::unknown_variant(class, names))
}

impl<'de> Visitor<'de> for UnionVisitor {
    type Value = Erased;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "internally tagged enum {}", desc(self.ty).name)
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Erased, A::Error> {
        let (variants, _, wrap) = union_parts(self.ty);
        let Some(first) = map.next_key::<String>()? else {
            return Err(de::Error::missing_field("$class"));
        };
        if first != "$class" {
            return buffered(self.ty, first, map);
        }
        let class: String = map.next_value_seed(TagSeed)?;
        let v = variant_of(self.ty, &class)?;
        match variants[v].payload {
            Some(p) => {
                let payload = read_struct_map(p, Mode::Lenient, true, &mut map)?;
                Ok(wrap(v, Some(payload)))
            }
            None => {
                while let Some(key) = map.next_key::<String>()? {
                    if key == "$class" {
                        return Err(de::Error::duplicate_field("$class"));
                    }
                    map.next_value_seed(Discard)?;
                }
                Ok(wrap(v, None))
            }
        }
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<Erased, A::Error> {
        let (variants, _, wrap) = union_parts(self.ty);
        let Some(class) = seq.next_element_seed(TagSeed)? else {
            return Err(de::Error::missing_field("$class"));
        };
        let v = variant_of(self.ty, &class)?;
        match variants[v].payload {
            Some(p) => {
                let payload = read_struct_seq(p, Mode::Lenient, &mut seq)?;
                Ok(wrap(v, Some(payload)))
            }
            None => {
                while seq.next_element_seed(Discard)?.is_some() {}
                Ok(wrap(v, None))
            }
        }
    }
}

/// A tagged node whose `$class` was not its first key: the rest of it read
/// into a `Value` (a duplicate `$class` is an error), then read again with
/// `$class` in front.
fn buffered<'de, A: MapAccess<'de>>(
    ty: u16,
    first: String,
    mut map: A,
) -> Result<Erased, A::Error> {
    use serde_json::{Map, Value};
    let mut rest = Map::new();
    rest.insert(first, map.next_value::<Value>()?);
    let mut class = None;
    while let Some(key) = map.next_key::<String>()? {
        let value = map.next_value::<Value>()?;
        if key == "$class" {
            if class.is_some() {
                return Err(de::Error::duplicate_field("$class"));
            }
            class = Some(value);
        } else {
            rest.insert(key, value);
        }
    }
    let Some(class) = class else {
        return Err(de::Error::missing_field("$class"));
    };
    let mut node = Map::with_capacity(rest.len() + 1);
    node.insert("$class".to_string(), class);
    node.extend(rest);
    decode_node(&Value::Object(node), ty, Mode::Lenient).map_err(de::Error::custom)
}

/// A `$class` value.
struct TagSeed;

impl<'de> DeserializeSeed<'de> for TagSeed {
    type Value = String;

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<String, D::Error> {
        d.deserialize_identifier(self)
    }
}

impl<'de> Visitor<'de> for TagSeed {
    type Value = String;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("variant identifier")
    }

    fn visit_str<E: de::Error>(self, v: &str) -> Result<String, E> {
        Ok(v.to_owned())
    }

    fn visit_string<E: de::Error>(self, v: String) -> Result<String, E> {
        Ok(v)
    }
}

/// A value read in full (as `serde`'s buffering reads it) and dropped.
#[derive(Clone, Copy)]
struct Discard;

impl<'de> DeserializeSeed<'de> for Discard {
    type Value = ();

    fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Discard {
    type Value = ();

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("any value")
    }

    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }

    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }

    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }

    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }

    fn visit_str<E: de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }

    fn visit_none<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }

    fn visit_some<D: Deserializer<'de>>(self, d: D) -> Result<(), D::Error> {
        d.deserialize_any(self)
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut seq: A) -> Result<(), A::Error> {
        while seq.next_element_seed(Discard)?.is_some() {}
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        while map.next_key_seed(Discard)?.is_some() {
            map.next_value_seed(Discard)?;
        }
        Ok(())
    }
}

include!(concat!(env!("OUT_DIR"), "/table_metamodel.rs"));
