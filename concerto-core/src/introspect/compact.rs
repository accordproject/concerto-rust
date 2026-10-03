//! A model's JSON AST in the compact binary layout (P5-92,
//! accordproject/concerto-rust#438), read straight into the typed model.
//!
//! The TS `ModelFile` constructor used to `JSON.stringify` an AST that
//! already exists as a JS object (the CTO parser's output, or an
//! `addModel`/`addModelFile`/`fromAst` input) for the engine to parse the
//! text again before its typed read. The TS side (concerto-core
//! `src/engine/ast-codec.ts`) now writes such an AST straight from the
//! object into the layout below, and [`Compact`] hands it to the typed read
//! (`typed_ast`) as a `serde` deserializer, without JSON text or a
//! [`Value`] of the whole document.
//!
//! It is the layout of the instance fast path (concerto-wasm
//! `validate_resource.rs`, P5-12b/P5-12c). One tag byte, then:
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
//! read checks them, the skip of a value the typed read ignores included
//! (P5-95, accordproject/concerto-rust#445), so bytes the typed read
//! accepts are bytes [`to_value`] accepts, and no count in them makes a
//! visitor reserve more than the bytes left can hold: malformed bytes are
//! an error, never a panic (a trap in WASM).

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
/// length, then a value's tag) take. P5-95: an array's or an object's
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
/// magnitude as an integer, any other as itself (P5-101, F-8).
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
    /// P5-101 (F-8): a double read as the instance validator spells a JS
    /// number ([`to_validator_value`]) rather than as `serde_json` reads
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
                // P5-95: the finiteness check `number` makes, so that bytes
                // the typed read accepts are bytes `to_value` accepts.
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
pub(crate) fn to_value(bytes: &[u8]) -> Result<Value, Error> {
    let mut compact = Compact::new(bytes);
    let value = Value::deserialize(&mut compact)?;
    compact.end()?;
    Ok(value)
}

/// P5-101 (F-8, accordproject/concerto-rust#455): the value `bytes` hold
/// as the instance validator reads it, for the instance fast path
/// (concerto-wasm `validate_resource.rs`, whose TS writer, src/engine/
/// wire.ts, is the AST's too): the one reader of the layout, with a double
/// spelled as `instance::validate::js_number` spells a finite JS number (an
/// integral one below `2^53` as an integer, any other as itself) rather
/// than as `JSON.stringify`'s text of it reads. Everything else is
/// [`to_value`]'s. An error for bytes not in the layout.
pub fn to_validator_value(bytes: &[u8]) -> Result<Value, Error> {
    let mut compact = Compact::new(bytes);
    compact.validator_numbers = true;
    let value = Value::deserialize(&mut compact)?;
    compact.end()?;
    Ok(value)
}

#[cfg(test)]
pub(crate) mod tests {
    use serde_json::Value;

    use super::{ARRAY, F64, FALSE, I32, NULL, OBJECT, STR, TRUE, to_value};

    /// `value` in the layout, as the TS writer (concerto-core
    /// `src/engine/ast-codec.ts`) writes it: an `i32` for a number that
    /// fits one, a double otherwise.
    pub(crate) fn encode(value: &Value) -> Vec<u8> {
        let mut out = Vec::new();
        write(value, &mut out);
        out
    }

    fn write_str(s: &str, out: &mut Vec<u8>) {
        out.extend_from_slice(&u32::try_from(s.len()).unwrap().to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    fn write(value: &Value, out: &mut Vec<u8>) {
        match value {
            Value::Null => out.push(NULL),
            Value::Bool(b) => out.push(if *b { TRUE } else { FALSE }),
            Value::Number(n) => {
                if let Some(i) = n.as_i64().and_then(|i| i32::try_from(i).ok()) {
                    out.push(I32);
                    out.extend_from_slice(&i.to_le_bytes());
                } else {
                    out.push(F64);
                    out.extend_from_slice(&n.as_f64().unwrap().to_le_bytes());
                }
            }
            Value::String(s) => {
                out.push(STR);
                write_str(s, out);
            }
            Value::Array(items) => {
                out.push(ARRAY);
                out.extend_from_slice(&u32::try_from(items.len()).unwrap().to_le_bytes());
                for item in items {
                    write(item, out);
                }
            }
            Value::Object(map) => {
                out.push(OBJECT);
                out.extend_from_slice(&u32::try_from(map.len()).unwrap().to_le_bytes());
                for (key, item) in map {
                    write_str(key, out);
                    write(item, out);
                }
            }
        }
    }

    /// P5-101 (F-8): the validator's reading is [`to_value`]'s, but for a
    /// double, spelt as `instance::validate::js_number` spells a finite JS
    /// number: an integer below `2^53` in magnitude, itself otherwise.
    #[test]
    fn to_validator_value_spells_doubles_as_the_validator_does() {
        use super::to_validator_value;
        use crate::instance::validate::js_number;
        for v in [
            -0.0,
            0.5,
            3.0e9,
            -3.0e9,
            2f64.powi(53) - 1.0,
            2f64.powi(53),
            2f64.powi(60),
            1e21,
            -2.5e-7,
        ] {
            let mut bytes = vec![F64];
            bytes.extend_from_slice(&v.to_le_bytes());
            assert_eq!(to_validator_value(&bytes).unwrap(), js_number(v), "{v}");
        }
        let value: Value =
            serde_json::from_str(r#"{"a":1,"b":[true,false,null,"x"],"c":{"d":-2}}"#).unwrap();
        assert_eq!(to_validator_value(&encode(&value)).unwrap(), value);
        assert!(to_validator_value(&[9]).is_err());
        assert!(to_validator_value(&[NULL, NULL]).is_err());
    }

    #[test]
    fn decodes_the_document_json_text_gives() {
        for text in [
            r#"{"a":1,"b":[true,false,null],"c":"xé😀","d":{"e":-2,"f":1.5}}"#,
            r#"[0,-1,2147483647,-2147483648,2147483648,-2147483649,9007199254740992,-9007199254740992,1e21,1.7976931348623157e308,5e-324,0.1,-2.5e-7]"#,
            r#"{"z":1,"a":2,"$class":"x"}"#,
            "[]",
            "{}",
            r#""""#,
        ] {
            let value: Value = serde_json::from_str(text).unwrap();
            assert_eq!(to_value(&encode(&value)).unwrap(), value, "{text}");
        }
    }

    #[test]
    fn integral_doubles_read_as_json_text_reads_them() {
        // What `JSON.stringify` writes for each double, read by serde_json.
        for (v, text) in [
            (-0.0, "0"),
            (3.0e9, "3000000000"),
            (-3.0e9, "-3000000000"),
            (2f64.powi(53), "9007199254740992"),
            (2f64.powi(53) + 2.0, "9007199254740994"),
            (2f64.powi(60), "1152921504606847000"),
            (-(2f64.powi(60)), "-1152921504606847000"),
            (2f64.powi(64), "18446744073709552000"),
            (-(2f64.powi(63)), "-9223372036854776000"),
            (1e20, "100000000000000000000"),
            (1e21, "1e+21"),
            (-1e21, "-1e+21"),
            (0.1, "0.1"),
            (1e-7, "1e-7"),
        ] {
            let mut bytes = vec![F64];
            bytes.extend_from_slice(&f64::to_le_bytes(v));
            let expected: Value = serde_json::from_str(text).unwrap();
            let got = to_value(&bytes).unwrap();
            assert_eq!(got, expected, "{v}");
            assert_eq!(got.to_string(), expected.to_string(), "{v}");
        }
    }

    #[test]
    fn rejects_bytes_not_in_the_layout() {
        for bytes in [
            &[][..],
            &[9][..],
            &[STR, 2, 0, 0, 0, b'a'][..],
            &[STR, 1, 0, 0, 0, 0xff][..],
            &[NULL, NULL][..],
            &[ARRAY, 2, 0, 0, 0, NULL][..],
            &[OBJECT, 1, 0, 0, 0, 1, 0, 0, 0, b'a'][..],
            &[F64, 0, 0, 0, 0, 0, 0, 0xf0, 0x7f][..],
        ] {
            assert!(to_value(bytes).is_err(), "{bytes:?}");
        }
        let mut deep = [ARRAY, 1, 0, 0, 0].repeat(600);
        deep.push(NULL);
        assert!(to_value(&deep).is_err());
    }

    /// P5-95 (accordproject/concerto-rust#445): a skipped value
    /// (`deserialize_ignored_any`, which no generated struct reaches: the
    /// typed read refuses an unknown key before its value) is checked as a
    /// read one is, a double's finiteness included, so that bytes the typed
    /// read accepts are bytes [`to_value`] accepts.
    #[test]
    fn a_skipped_value_is_checked_as_a_read_one() {
        use serde::Deserialize;
        use serde::de::IgnoredAny;

        use super::Compact;

        let skip = |bytes: &[u8]| {
            let mut compact = Compact::new(bytes);
            IgnoredAny::deserialize(&mut compact).and_then(|_| compact.end())
        };
        for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let mut bytes = vec![ARRAY, 2, 0, 0, 0, I32, 1, 0, 0, 0, F64];
            bytes.extend_from_slice(&v.to_le_bytes());
            assert!(skip(&bytes).is_err(), "{v}");
            assert!(to_value(&bytes).is_err(), "{v}");
        }
        let mut bytes = vec![OBJECT, 1, 0, 0, 0, 1, 0, 0, 0, b'k', F64];
        bytes.extend_from_slice(&1.5f64.to_le_bytes());
        assert!(skip(&bytes).is_ok());
        for bytes in [
            &[F64, 0, 0][..],
            &[9][..],
            &[STR, 0xff, 0xff, 0xff, 0xff][..],
            &[ARRAY, 0xff, 0xff, 0xff, 0xff, NULL][..],
            &[OBJECT, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff][..],
        ] {
            assert!(skip(bytes).is_err(), "{bytes:?}");
        }
    }

    /// P5-95: an array's or an object's size hint is its count bounded by
    /// the bytes left, so a visitor that reserves it (`kept::KeptSeed`)
    /// never reserves for a count the bytes cannot hold (in WASM, a count
    /// of `u32::MAX` times a `Kept` overflowed the reservation: a trap).
    #[test]
    fn a_size_hint_is_bounded_by_the_bytes_left() {
        use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};

        use super::Compact;

        // The hint the visitor is given (the read then fails: it leaves
        // the items unread).
        struct Hint<'a>(&'a std::cell::Cell<Option<usize>>);
        impl<'de> Visitor<'de> for Hint<'_> {
            type Value = ();
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a container")
            }
            fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<(), A::Error> {
                self.0.set(seq.size_hint());
                Ok(())
            }
            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<(), A::Error> {
                self.0.set(map.size_hint());
                Ok(())
            }
        }
        let hint = |bytes: &[u8]| {
            let given = std::cell::Cell::new(None);
            let _ = (&mut Compact::new(bytes)).deserialize_any(Hint(&given));
            given.get()
        };
        assert_eq!(hint(&[ARRAY, 0xff, 0xff, 0xff, 0xff, NULL, NULL]), Some(2));
        assert_eq!(hint(&[ARRAY, 2, 0, 0, 0, NULL, NULL]), Some(2));
        assert_eq!(hint(&[ARRAY, 2, 0, 0, 0, NULL, NULL, NULL]), Some(2));
        let mut entries = vec![OBJECT, 0xff, 0xff, 0xff, 0xff];
        entries.extend_from_slice(&[0; 11]);
        assert_eq!(hint(&entries), Some(2));
        assert_eq!(hint(&[OBJECT, 0xff, 0xff, 0xff, 0xff]), Some(0));
    }

    /// P5-95 (accordproject/concerto-rust#445): bytes not in the layout,
    /// written by hand, with each malformed value at each place a model
    /// reads one (the whole AST, its namespace, an unknown key, a decorator
    /// argument's value, a declaration's kept `location`). Each is the outer
    /// error of both staging paths (a `TypeError` at the JS boundary), never
    /// a load and never a panic. An oversized array count in a `location`
    /// used to make `KeptSeed` reserve it.
    #[test]
    fn malformed_bytes_are_an_error_at_every_place() {
        use crate::introspect::ModelFile;

        let u32le = |n: u32| n.to_le_bytes().to_vec();
        let key = |k: &str| {
            [
                u32le(u32::try_from(k.len()).unwrap()),
                k.as_bytes().to_vec(),
            ]
            .concat()
        };
        let string = |v: &str| [vec![STR], key(v)].concat();
        let object = |entries: &[(&str, Vec<u8>)]| {
            let mut out = vec![OBJECT];
            out.extend(u32le(u32::try_from(entries.len()).unwrap()));
            for (k, v) in entries {
                out.extend(key(k));
                out.extend(v.iter().copied());
            }
            out
        };
        let array = |items: &[Vec<u8>]| {
            let mut out = vec![ARRAY];
            out.extend(u32le(u32::try_from(items.len()).unwrap()));
            for item in items {
                out.extend(item.iter().copied());
            }
            out
        };
        let double = |v: f64| [vec![F64], v.to_le_bytes().to_vec()].concat();
        let mm = |name: &str| string(&format!("concerto.metamodel@1.0.0.{name}"));

        // The model, with `inject` at `place`.
        let model = |place: &str, inject: &[u8]| -> Vec<u8> {
            if place == "ast" {
                return inject.to_vec();
            }
            let at = |p: &str, valid: Vec<u8>| if place == p { inject.to_vec() } else { valid };
            let argument = object(&[
                ("$class", mm("DecoratorNumber")),
                ("value", at("argument", double(1.5))),
            ]);
            let decorator = object(&[
                ("$class", mm("Decorator")),
                ("name", string("d")),
                ("arguments", array(&[argument])),
            ]);
            let declaration = object(&[
                ("$class", mm("ConceptDeclaration")),
                ("name", string("C")),
                ("isAbstract", vec![FALSE]),
                ("properties", array(&[])),
                ("location", at("location", vec![NULL])),
            ]);
            let mut entries = vec![
                ("$class", mm("Model")),
                ("namespace", at("namespace", string("org.malformed@1.0.0"))),
                ("imports", array(&[])),
                ("decorators", array(&[decorator])),
                ("declarations", array(&[declaration])),
            ];
            if place == "unknownKey" {
                entries.push(("unknownKey", inject.to_vec()));
            }
            object(&entries)
        };
        let load = |bytes: &[u8], checked: bool| {
            if checked {
                ModelFile::from_compact_checked_with_imports(bytes, None, None)
            } else {
                ModelFile::from_compact_with_imports(bytes, None, None)
            }
        };

        // Each place is read: a valid value there loads, or, for an
        // unknown key, is the typed read's data error, not the layout one.
        for (place, valid) in [
            ("ast", model("", &[])),
            ("namespace", string("org.valid@1.0.0")),
            (
                "unknownKey",
                array(&[double(0.5), object(&[("k", vec![TRUE])])]),
            ),
            ("argument", double(-2.5)),
            ("location", vec![NULL]),
        ] {
            let bytes = model(place, &valid);
            let loaded = load(&bytes, false);
            if place == "unknownKey" {
                assert!(matches!(loaded, Ok(Err(_))), "{place}");
            } else {
                assert!(matches!(loaded, Ok(Ok(_))), "{place}");
            }
        }

        let mut deep = [ARRAY, 1, 0, 0, 0].repeat(600);
        deep.push(NULL);
        let malformed: Vec<(&str, Vec<u8>)> = vec![
            ("NaN", double(f64::NAN)),
            ("+Inf", double(f64::INFINITY)),
            ("-Inf", double(f64::NEG_INFINITY)),
            ("a NaN item", array(&[double(f64::NAN)])),
            ("a truncated double", vec![F64, 0, 0, 0]),
            ("a truncated i32", vec![I32, 0]),
            ("a missing value", vec![]),
            ("an unknown tag", vec![8]),
            ("an unknown tag 0xff", vec![0xff]),
            ("an unknown tag in an array", array(&[vec![9]])),
            (
                "an oversized string length",
                [vec![STR], u32le(u32::MAX), b"x".to_vec()].concat(),
            ),
            (
                "an oversized array count",
                [vec![ARRAY], u32le(u32::MAX), vec![NULL]].concat(),
            ),
            (
                "an oversized object count",
                [vec![OBJECT], u32le(u32::MAX)].concat(),
            ),
            (
                "an oversized key length",
                [vec![OBJECT], u32le(1), u32le(u32::MAX)].concat(),
            ),
            (
                "a string that is not UTF-8",
                [vec![STR], u32le(2), vec![0xc3, 0x28]].concat(),
            ),
            (
                "a key that is not UTF-8",
                [vec![OBJECT], u32le(1), u32le(1), vec![0xff, NULL]].concat(),
            ),
            ("nested too deeply", deep),
        ];
        for place in ["ast", "namespace", "unknownKey", "argument", "location"] {
            for (what, inject) in &malformed {
                let bytes = model(place, inject);
                for checked in [false, true] {
                    assert!(
                        load(&bytes, checked).is_err(),
                        "{what} at {place} (checked: {checked}) is not the layout error"
                    );
                }
            }
        }

        // Trailing bytes, and every proper prefix of a valid model.
        let valid = model("", &[]);
        let mut trailing = valid.clone();
        trailing.push(NULL);
        for checked in [false, true] {
            assert!(load(&trailing, checked).is_err(), "trailing bytes");
            for len in 0..valid.len() {
                assert!(load(&valid[..len], checked).is_err(), "truncated to {len}");
            }
        }

        // Each byte of a valid model replaced by a tag, a count's or a
        // double's byte, or an unknown tag: never a panic, and a load gives
        // the `ast()` the bytes hold.
        for at in 0..valid.len() {
            for byte in [NULL, F64, OBJECT, 8, 0x7f, 0xf0, 0xff] {
                let mut bytes = valid.clone();
                bytes[at] = byte;
                for checked in [false, true] {
                    if let Ok(Ok((model_file, _))) = load(&bytes, checked) {
                        assert_eq!(model_file.ast(), &to_value(&bytes).unwrap(), "{at} {byte}");
                    }
                }
            }
        }
    }

    /// P5-92 property test: the same ASTs through both staging paths,
    /// [`ModelFile::from_json_text_with_imports`] over `JSON.stringify`'s
    /// text and [`ModelFile::from_compact_with_imports`] over the bytes the
    /// TS writer writes, with and without BC-19's shape check, give the same
    /// result: an equal model file (its typed load and its `ast()`) and
    /// `imports` node, or the same error (an unreadable AST's message
    /// quotes `serde_json`, whose text errors carry a position, so only its
    /// kind and code are compared). The ASTs are the system models, the
    /// metamodel, and, when `CONCERTO_ORACLE_FIXTURES` is set, every AST of
    /// the CTO cache next to the oracle corpus, each as it is and after
    /// seeded random mutations (a key dropped, `$class` moved last, an
    /// unknown key, a value replaced, an array item doubled).
    #[test]
    fn text_and_compact_paths_agree() {
        use crate::introspect::ModelFile;

        let mut bases: Vec<Value> = crate::rootmodel::system_model_json_texts()
            .iter()
            .map(|(_, text)| serde_json::from_str(text).unwrap())
            .collect();
        bases.push(serde_json::from_str(include_str!("../dcs/metamodel.json")).unwrap());
        if let Ok(fixtures) = std::env::var("CONCERTO_ORACLE_FIXTURES") {
            let cache = std::path::Path::new(&fixtures).join("../cto-cache");
            let mut files = Vec::new();
            for dir in std::fs::read_dir(&cache).into_iter().flatten().flatten() {
                for file in std::fs::read_dir(dir.path())
                    .into_iter()
                    .flatten()
                    .flatten()
                {
                    files.push(file.path());
                }
            }
            files.sort();
            for file in files {
                let entry: Value =
                    serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
                if let Some(ast) = entry.get("ast").filter(|ast| ast.is_object()) {
                    bases.push(ast.clone());
                }
            }
        }

        let mut rng = 0x9e37_79b9_7f4a_7c15_u64;
        let mut cases = 0;
        let mut loaded = 0;
        for base in &bases {
            for round in 0..8 {
                let mut ast = base.clone();
                for _ in 0..round.min(3) {
                    mutate(&mut ast, &mut rng);
                }
                let text = js_stringify(&ast);
                let bytes = encode(&ast);
                for checked in [false, true] {
                    let (from_text, from_compact) = if checked {
                        (
                            ModelFile::from_json_text_checked_with_imports(&text, None, None),
                            ModelFile::from_compact_checked_with_imports(&bytes, None, None),
                        )
                    } else {
                        (
                            ModelFile::from_json_text_with_imports(&text, None, None),
                            ModelFile::from_compact_with_imports(&bytes, None, None),
                        )
                    };
                    cases += 1;
                    match (from_text.unwrap(), from_compact.unwrap()) {
                        (Ok((a, ai)), Ok((b, bi))) => {
                            loaded += 1;
                            assert_eq!(format!("{a:?}"), format!("{b:?}"), "{text}");
                            assert_eq!(a.ast(), b.ast(), "{text}");
                            assert_eq!(ai, bi, "{text}");
                        }
                        (Err(a), Err(b)) => {
                            assert_eq!((a.kind(), a.code()), (b.kind(), b.code()), "{text}");
                            if a.code() != "modelfile-load-unreadable" {
                                assert_eq!(a, b, "{text}");
                            }
                        }
                        (a, b) => panic!(
                            "the paths disagree (checked: {checked}) on {text}: text {:?}, compact {:?}",
                            a.err(),
                            b.err()
                        ),
                    }
                }
            }
        }
        assert!(loaded > 0 && loaded < cases, "{loaded} of {cases} loaded");
    }

    /// `JSON.stringify`'s text of `value`, as a JS object (every number a
    /// double, written by `Number::toString`).
    fn js_stringify(value: &Value) -> String {
        match value {
            Value::Number(n) => {
                let mut buffer = ryu_js::Buffer::new();
                buffer.format_finite(n.as_f64().unwrap()).to_string()
            }
            Value::Array(items) => {
                let items: Vec<String> = items.iter().map(js_stringify).collect();
                format!("[{}]", items.join(","))
            }
            Value::Object(map) => {
                let entries: Vec<String> = map
                    .iter()
                    .map(|(key, item)| {
                        format!("{}:{}", Value::String(key.clone()), js_stringify(item))
                    })
                    .collect();
                format!("{{{}}}", entries.join(","))
            }
            other => other.to_string(),
        }
    }

    fn next(rng: &mut u64) -> u64 {
        *rng ^= *rng << 13;
        *rng ^= *rng >> 7;
        *rng ^= *rng << 17;
        *rng
    }

    fn pick<T: Clone>(items: &[T], rng: &mut u64) -> T {
        items[usize::try_from(next(rng) % items.len() as u64).unwrap()].clone()
    }

    /// The JSON pointers of every node of `value`.
    fn pointers(value: &Value, at: &str, out: &mut Vec<String>) {
        out.push(at.to_string());
        match value {
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    pointers(item, &format!("{at}/{i}"), out);
                }
            }
            Value::Object(map) => {
                for (key, item) in map {
                    let key = key.replace('~', "~0").replace('/', "~1");
                    pointers(item, &format!("{at}/{key}"), out);
                }
            }
            _ => {}
        }
    }

    /// One random change to a random node of `ast`.
    fn mutate(ast: &mut Value, rng: &mut u64) {
        let mut all = Vec::new();
        pointers(ast, "", &mut all);
        let at = pick(&all, rng);
        let replacements = [
            Value::Null,
            Value::Bool(true),
            Value::Bool(false),
            serde_json::json!(0),
            serde_json::json!(-1),
            serde_json::json!(1.5),
            serde_json::json!(2_147_483_648_i64),
            serde_json::json!(1_152_921_504_606_847_000_f64),
            serde_json::json!(1e21),
            serde_json::json!(""),
            serde_json::json!("x"),
            serde_json::json!("concerto.metamodel@1.0.0.StringProperty"),
            serde_json::json!("concerto.metamodel@1.0.0.ConceptDeclaration"),
            serde_json::json!([]),
            serde_json::json!({}),
            serde_json::json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "C"}),
        ];
        let node = ast.pointer_mut(&at).unwrap();
        match (next(rng) % 5, node) {
            (0, Value::Object(map)) if !map.is_empty() => {
                let keys: Vec<String> = map.keys().cloned().collect();
                map.shift_remove(&pick(&keys, rng));
            }
            (1, Value::Object(map)) => {
                if let Some(class) = map.shift_remove("$class") {
                    map.insert("$class".to_string(), class);
                }
            }
            (2, Value::Object(map)) => {
                map.insert("unknownKey".to_string(), pick(&replacements, rng));
            }
            (3, Value::Array(items)) if !items.is_empty() => {
                let item = pick(items, rng);
                items.push(item);
            }
            (_, node) => *node = pick(&replacements, rng),
        }
    }
}
