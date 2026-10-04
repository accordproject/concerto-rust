//! A model's JSON AST in the compact binary layout, read
//! straight into the typed model.
//!
//! The TS side (concerto-core `src/engine/ast-codec.ts`) writes an AST that
//! already exists as a JS object (the CTO parser's output, or an
//! `addModel`/`addModelFile`/`fromAst` input) straight into the layout
//! below, and [`Compact`] hands it to the typed read (`typed_ast`) as a
//! `serde` deserializer, without JSON text or a [`Value`] of the whole
//! document.
//!
//! It is the layout of the instance fast path (concerto-wasm
//! `validate_resource.rs`). One tag byte, then:
//!
//! | tag | value |
//! |---|---|
//! | 0 | `null` |
//! | 1 | `false` |
//! | 2 | `true` |
//! | 3 | a double, 8 bytes LE |
//! | 4 | an `i32`, 4 bytes LE |
//! | 5 | a string: `u32` LE byte length, then UTF-8 |
//! | 6 | an array: `u32` LE count, then the items |
//! | 7 | an object: `u32` LE count, then (`u32` LE key length, UTF-8 key, value) per entry |
//!
//! # The same AST as its JSON text
//!
//! The TS writer sends only what `JSON.stringify` would write as plain JSON
//! (it hands anything else to the text path), so the bytes describe the
//! very document `JSON.parse(JSON.stringify(ast))` is. [`Compact`] reads them
//! as `&Value`'s deserializer reads that document, method by method, so the
//! typed read gives the same result, and the same verdict, as from the text:
//!
//! - **Numbers.** `JSON.stringify` writes a double as `Number::toString`
//!   does: an integral one below `1e21` as a decimal integer (past `2^53`,
//!   its shortest round-trip digits padded with zeros), which `serde_json`
//!   reads as a `u64` (an `i64` when negative) when it fits, and as an
//!   `f64` otherwise; any other finite double in its shortest round-trip
//!   form, which `serde_json` (`float_roundtrip`) reads back as that very
//!   double. A number is handed to the visitor the same way here
//!   ([`number`], with `ryu-js` for the digits): `-0` as `0`.
//! - **Objects** keep their entries in order (`preserve_order`); a JS object
//!   has no duplicate keys.
//! - **Depth.** The TS writer leaves an AST nested deeper than `serde_json`'s
//!   text reader allows to the text path, which then rejects it as before.
//!   [`MAX_DEPTH`] only guards the stack against bytes not written by it.
//!
//! Bytes that are not in this layout (truncated, trailing bytes, an unknown
//! tag, a string that is not UTF-8, a number that is not finite, or nested
//! past [`MAX_DEPTH`]) are an error, which [`to_value`] tells apart from a
//! data error of the typed read; the TS writer never writes them. Every
//! read checks them, the skip of a value the typed read ignores included,
//! so bytes the typed read accepts are bytes [`to_value`] accepts, and no
//! count in them makes a visitor reserve more than the bytes left can hold:
//! malformed bytes are an error, never a panic (a trap in WASM).

use serde::Deserialize;
use serde::de::value::BorrowedStrDeserializer;
use serde::de::{self, DeserializeSeed, Deserializer, MapAccess, SeqAccess};
use serde::de::{Unexpected, Visitor};
use serde_json::Value;

type Error = serde_json::Error;

const NULL: u8 = 0;
const FALSE: u8 = 1;
const TRUE: u8 = 2;
const F64: u8 = 3;
const I32: u8 = 4;
const STR: u8 = 5;
const ARRAY: u8 = 6;
const OBJECT: u8 = 7;

/// How deep the bytes may nest (a stack guard; module doc, "Depth").
const MAX_DEPTH: usize = 512;

/// The fewest bytes an array item (a tag) and an object entry (a key's
/// length, then a value's tag) take. An array's or an object's
/// `size_hint` is its count bounded by the bytes left over these, so a
/// visitor that reserves its size hint (`kept::KeptSeed`) never reserves
/// more than the bytes can hold, whatever count bytes not written by the TS
/// writer give.
const MIN_ITEM_LEN: usize = 1;
const MIN_ENTRY_LEN: usize = 5;

/// `2^53`, past which not every integer is a double.
const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_992.0;

/// Why bytes are not in the layout (module doc).
fn malformed(why: &str) -> Error {
    de::Error::custom(format_args!(
        "a compact AST that is not in the layout: {why}"
    ))
}

/// A number, as `serde_json` reads `JSON.stringify`'s text of it (module
/// doc, "Numbers").
#[derive(Clone, Copy)]
enum Number {
    PosInt(u64),
    NegInt(i64),
    Float(f64),
}

/// The number `serde_json` reads from the text `JSON.stringify` writes for
/// the finite double `v` (module doc, "Numbers").
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn number(v: f64) -> Number {
    if v.trunc() != v || v.abs() >= 1e21 {
        // Written in its shortest round-trip form (with an exponent from
        // `1e21`), which `serde_json` reads back as `v` itself.
        return Number::Float(v);
    }
    if v.abs() <= MAX_SAFE_INTEGER {
        // Exact. `-0.0 >= 0.0`, so `-0` is `0`, as `JSON.stringify` writes it.
        return if v >= 0.0 {
            Number::PosInt(v as u64)
        } else {
            Number::NegInt(v as i64)
        };
    }
    // Past 2^53, JS writes the shortest round-trip digits padded with zeros
    // (`2 ** 60` is `1152921504606847000`), which `serde_json` reads as that
    // integer when it fits, and as `v` otherwise.
    let mut buffer = ryu_js::Buffer::new();
    let text = buffer.format_finite(v);
    let integer = if v > 0.0 {
        text.parse().ok().map(Number::PosInt)
    } else {
        text.parse().ok().map(Number::NegInt)
    };
    integer.unwrap_or(Number::Float(v))
}

/// The finite double `v` as the instance validator spells a JS number
/// (`instance::validate::js_number`): an integral one below `2^53` in
/// magnitude as an integer, any other as itself.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn validator_number(v: f64) -> Number {
    if v.trunc() == v && v.abs() < MAX_SAFE_INTEGER {
        if v >= 0.0 {
            Number::PosInt(v as u64)
        } else {
            Number::NegInt(v as i64)
        }
    } else {
        Number::Float(v)
    }
}

impl Number {
    fn visit<'de, V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self {
            Number::PosInt(n) => visitor.visit_u64(n),
            Number::NegInt(n) => visitor.visit_i64(n),
            Number::Float(n) => visitor.visit_f64(n),
        }
    }

    fn unexpected(self) -> Unexpected<'static> {
        match self {
            Number::PosInt(n) => Unexpected::Unsigned(n),
            Number::NegInt(n) => Unexpected::Signed(n),
            Number::Float(n) => Unexpected::Float(n),
        }
    }
}

/// A reader over bytes in the layout (module doc), and the deserializer of
/// the value at its position.
pub(crate) struct Compact<'de> {
    bytes: &'de [u8],
    pos: usize,
    depth: usize,
    /// A double read as the instance validator spells a JS number
    /// ([`to_validator_value`]) rather than as `serde_json` reads
    /// `JSON.stringify`'s text of it.
    validator_numbers: bool,
}

impl<'de> Compact<'de> {
    pub(crate) fn new(bytes: &'de [u8]) -> Self {
        Self {
            bytes,
            pos: 0,
            depth: 0,
            validator_numbers: false,
        }
    }

    /// An error unless every byte has been read.
    pub(crate) fn end(&self) -> Result<(), Error> {
        if self.pos == self.bytes.len() {
            Ok(())
        } else {
            Err(malformed("trailing bytes"))
        }
    }

    /// How many bytes are left to read.
    fn left(&self) -> usize {
        self.bytes.len().saturating_sub(self.pos)
    }

    fn take(&mut self, n: usize) -> Result<&'de [u8], Error> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|end| *end <= self.bytes.len())
            .ok_or_else(|| malformed("truncated"))?;
        let out = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N], Error> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn u32(&mut self) -> Result<usize, Error> {
        Ok(u32::from_le_bytes(self.array()?) as usize)
    }

    fn peek(&self) -> Result<u8, Error> {
        self.bytes
            .get(self.pos)
            .copied()
            .ok_or_else(|| malformed("truncated"))
    }

    /// A length-prefixed string (no tag).
    fn raw_str(&mut self) -> Result<&'de str, Error> {
        let len = self.u32()?;
        std::str::from_utf8(self.take(len)?).map_err(|_| malformed("a string that is not UTF-8"))
    }

    /// The number after a number tag the caller has peeked.
    fn number(&mut self, tag: u8) -> Result<Number, Error> {
        self.pos += 1;
        Ok(if tag == I32 {
            let n = i32::from_le_bytes(self.array()?);
            if n >= 0 {
                Number::PosInt(n.unsigned_abs().into())
            } else {
                Number::NegInt(n.into())
            }
        } else {
            let v = f64::from_le_bytes(self.array()?);
            if !v.is_finite() {
                return Err(malformed("a number that is not finite"));
            }
            if self.validator_numbers {
                validator_number(v)
            } else {
                number(v)
            }
        })
    }

    /// What `&Value`'s deserializer tells a visitor the value at the
    /// position is, for an error; reads nothing past it.
    fn unexpected(&mut self) -> Result<Unexpected<'de>, Error> {
        let start = self.pos;
        let unexpected = match self.peek()? {
            NULL => Unexpected::Unit,
            FALSE => Unexpected::Bool(false),
            TRUE => Unexpected::Bool(true),
            tag @ (F64 | I32) => self.number(tag)?.unexpected(),
            STR => {
                self.pos += 1;
                Unexpected::Str(self.raw_str()?)
            }
            ARRAY => Unexpected::Seq,
            OBJECT => Unexpected::Map,
            _ => return Err(malformed("an unknown tag")),
        };
        self.pos = start;
        Ok(unexpected)
    }

    fn invalid_type<V: Visitor<'de>>(&mut self, visitor: &V) -> Error {
        match self.unexpected() {
            Ok(unexpected) => de::Error::invalid_type(unexpected, visitor),
            Err(err) => err,
        }
    }

    /// Skips the value at the position (`deserialize_ignored_any`).
    fn skip(&mut self) -> Result<(), Error> {
        let tag = self.peek()?;
        self.pos += 1;
        match tag {
            NULL | FALSE | TRUE => {}
            F64 => {
                // The finiteness check `number` makes, so that bytes the
                // typed read accepts are bytes `to_value` accepts.
                let v = f64::from_le_bytes(self.array()?);
                if !v.is_finite() {
                    return Err(malformed("a number that is not finite"));
                }
            }
            I32 => {
                self.take(4)?;
            }
            STR => {
                self.raw_str()?;
            }
            ARRAY | OBJECT => {
                let count = self.u32()?;
                self.enter()?;
                for _ in 0..count {
                    if tag == OBJECT {
                        self.raw_str()?;
                    }
                    self.skip()?;
                }
                self.depth -= 1;
            }
            _ => return Err(malformed("an unknown tag")),
        }
        Ok(())
    }

    fn enter(&mut self) -> Result<(), Error> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(malformed("nested too deeply"));
        }
        Ok(())
    }

    /// An array whose tag the caller has peeked, handed to `visitor` as
    /// `&Value`'s deserializer hands one (an error for items the visitor
    /// leaves unread).
    fn visit_array<V: Visitor<'de>>(&mut self, visitor: V) -> Result<V::Value, Error> {
        self.pos += 1;
        let len = self.u32()?;
        self.enter()?;
        let mut access = Items {
            de: self,
            remaining: len,
        };
        let value = visitor.visit_seq(&mut access)?;
        if access.remaining != 0 {
            return Err(de::Error::invalid_length(len, &"fewer elements in array"));
        }
        self.depth -= 1;
        Ok(value)
    }

    /// An object whose tag the caller has peeked, handed to `visitor` as
    /// `&Value`'s deserializer hands one (keys as borrowed strings; an error
    /// for entries the visitor leaves unread).
    fn visit_object<V: Visitor<'de>>(&mut self, visitor: V) -> Result<V::Value, Error> {
        self.pos += 1;
        let len = self.u32()?;
        self.enter()?;
        let mut access = Entries {
            de: self,
            remaining: len,
        };
        let value = visitor.visit_map(&mut access)?;
        if access.remaining != 0 {
            return Err(de::Error::invalid_length(len, &"fewer elements in map"));
        }
        self.depth -= 1;
        Ok(value)
    }
}

/// An array's items (`Compact::visit_array`).
struct Items<'a, 'de> {
    de: &'a mut Compact<'de>,
    remaining: usize,
}

impl<'de> SeqAccess<'de> for Items<'_, 'de> {
    type Error = Error;

    fn next_element_seed<T: DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Error> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        seed.deserialize(&mut *self.de).map(Some)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining.min(self.de.left() / MIN_ITEM_LEN))
    }
}

/// An object's entries (`Compact::visit_object`).
struct Entries<'a, 'de> {
    de: &'a mut Compact<'de>,
    remaining: usize,
}

impl<'de> MapAccess<'de> for Entries<'_, 'de> {
    type Error = Error;

    fn next_key_seed<K: DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        if self.remaining == 0 {
            return Ok(None);
        }
        self.remaining -= 1;
        let key = self.de.raw_str()?;
        seed.deserialize(BorrowedStrDeserializer::new(key))
            .map(Some)
    }

    fn next_value_seed<V: DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        seed.deserialize(&mut *self.de)
    }

    fn size_hint(&self) -> Option<usize> {
        Some(self.remaining.min(self.de.left() / MIN_ENTRY_LEN))
    }
}

/// `&Value`'s deserializer for the numeric methods: a number goes to the
/// visitor as its own kind, anything else is an error.
macro_rules! compact_number {
    ($($method:ident)*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            match self.peek()? {
                tag @ (F64 | I32) => self.number(tag)?.visit(visitor),
                _ => Err(self.invalid_type(&visitor)),
            }
        }
    )*};
}

/// `&Value`'s deserializer for the string methods.
macro_rules! compact_str {
    ($($method:ident)*) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            match self.peek()? {
                STR => {
                    self.pos += 1;
                    visitor.visit_borrowed_str(self.raw_str()?)
                }
                _ => Err(self.invalid_type(&visitor)),
            }
        }
    )*};
}

/// `&Value`'s deserializer for the methods that take an array, an object,
/// or either.
macro_rules! compact_container {
    ($($method:ident($($arg:ident: $ty:ty),*) => $array:ident, $object:ident;)*) => {$(
        fn $method<V: Visitor<'de>>(self, $($arg: $ty,)* visitor: V) -> Result<V::Value, Error> {
            $(let _ = $arg;)*
            match self.peek()? {
                ARRAY if compact_container!(@$array) => self.visit_array(visitor),
                OBJECT if compact_container!(@$object) => self.visit_object(visitor),
                _ => Err(self.invalid_type(&visitor)),
            }
        }
    )*};
    (@yes) => { true };
    (@no) => { false };
}

impl<'de> Deserializer<'de> for &mut Compact<'de> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.peek()? {
            NULL => {
                self.pos += 1;
                visitor.visit_unit()
            }
            FALSE | TRUE => self.deserialize_bool(visitor),
            tag @ (F64 | I32) => self.number(tag)?.visit(visitor),
            STR => self.deserialize_str(visitor),
            ARRAY => self.visit_array(visitor),
            OBJECT => self.visit_object(visitor),
            _ => Err(malformed("an unknown tag")),
        }
    }

    fn deserialize_bool<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.peek()? {
            tag @ (FALSE | TRUE) => {
                self.pos += 1;
                visitor.visit_bool(tag == TRUE)
            }
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    compact_number! {
        deserialize_i8 deserialize_i16 deserialize_i32 deserialize_i64 deserialize_i128
        deserialize_u8 deserialize_u16 deserialize_u32 deserialize_u64 deserialize_u128
        deserialize_f32 deserialize_f64
    }

    compact_str! {
        deserialize_char deserialize_str deserialize_string deserialize_identifier
    }

    fn deserialize_bytes<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.peek()? {
            STR => self.deserialize_str(visitor),
            ARRAY => self.visit_array(visitor),
            _ => Err(self.invalid_type(&visitor)),
        }
    }

    fn deserialize_byte_buf<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_bytes(visitor)
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        if self.peek()? == NULL {
            self.pos += 1;
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_unit<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        if self.peek()? == NULL {
            self.pos += 1;
            visitor.visit_unit()
        } else {
            Err(self.invalid_type(&visitor))
        }
    }

    fn deserialize_unit_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_unit(visitor)
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        visitor.visit_newtype_struct(self)
    }

    compact_container! {
        deserialize_seq() => yes, no;
        deserialize_tuple(len: usize) => yes, no;
        deserialize_tuple_struct(name: &'static str, len: usize) => yes, no;
        deserialize_map() => no, yes;
        deserialize_struct(name: &'static str, fields: &'static [&'static str]) => yes, yes;
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        // Never reached by the generated structs (no externally tagged
        // enum); the same verdict through the `Value`.
        Value::deserialize(self)?.deserialize_enum(name, variants, visitor)
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        self.skip()?;
        visitor.visit_unit()
    }
}

/// The document `bytes` hold, as a [`Value`]: the one
/// `serde_json::from_str` gives for `JSON.stringify`'s text of it (module
/// doc). An error for bytes not in the layout.
pub fn to_value(bytes: &[u8]) -> Result<Value, Error> {
    let mut compact = Compact::new(bytes);
    let value = Value::deserialize(&mut compact)?;
    compact.end()?;
    Ok(value)
}

/// `seed` run over the document `bytes` hold, as over `serde_json`'s
/// deserializer of `JSON.stringify`'s text of it (module doc), for the
/// Serializer fast path's binary input (concerto-wasm `parse_wire_bytes`,
/// whose TS writer, src/engine/wire.ts, is the AST's too). An error for
/// bytes not in the layout, or the seed's own error.
pub fn deserialize_seed<'de, S: DeserializeSeed<'de>>(
    bytes: &'de [u8],
    seed: S,
) -> Result<S::Value, Error> {
    let mut compact = Compact::new(bytes);
    let value = seed.deserialize(&mut compact)?;
    compact.end()?;
    Ok(value)
}

/// The value `bytes` hold as the instance validator reads it, for the
/// instance fast path: as [`to_value`], except that a double is spelled as
/// `instance::validate::js_number` spells a finite JS number. An error for
/// bytes not in the layout.
pub fn to_validator_value(bytes: &[u8]) -> Result<Value, Error> {
    let mut compact = Compact::new(bytes);
    compact.validator_numbers = true;
    let value = Value::deserialize(&mut compact)?;
    compact.end()?;
    Ok(value)
}

#[cfg(test)]
pub(crate) mod tests;
