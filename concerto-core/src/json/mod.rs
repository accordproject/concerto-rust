//! JSON values whose objects are hashed with the process-wide secret keys
//! (PORTING.md 3.7).
//!
//! [`Value`] and [`Map`] are `serde_json::Value` and `serde_json::Map` with
//! the `preserve_order` feature (OD-3: an object keeps its keys in the order
//! they were read), with one difference: a [`Map`] hashes its keys with
//! [`SeededState`](crate::hash::SeededState), not the standard library's
//! `RandomState`. On `wasm32-unknown-unknown`, `RandomState` has no entropy
//! source: its keys are derived from memory addresses, the same in every
//! instantiation of a given build, so an attacker who knows the build can
//! craft object keys that all collide and make parsing a document quadratic
//! in its number of keys. Every JSON value concerto-core reads or builds,
//! including the instances, model ASTs and decorator command sets a WASM
//! host hands it as text, is one of these.
//!
//! Numbers are [`serde_json::Number`]s, and text is read and written by
//! `serde_json` (`serde_json::from_str::<Value>`, `serde_json::to_string`),
//! so syntax errors, the recursion limit and number round-tripping
//! (`float_roundtrip`) are `serde_json`'s own. The [`json!`](crate::json!)
//! macro builds a [`Value`] as `serde_json::json!` builds a
//! `serde_json::Value`.
//!
//! The code is adapted from `serde_json` 1.0.150 (`src/value/`, `src/map.rs`
//! and `src/macros.rs`), copyright David Tolnay and the serde_json
//! contributors, licensed under MIT OR Apache-2.0, with the `std`,
//! `preserve_order` and `float_roundtrip` features and without
//! `arbitrary_precision` or `raw_value`.

use std::fmt::{self, Debug, Display};
use std::io;
use std::mem;

use serde::de::DeserializeOwned;
use serde::ser::Serialize;

pub use self::index::Index;
pub use self::map::{Entry, IntoIter, IntoValues, Iter, IterMut, Keys, Map, Values, ValuesMut};
pub use self::ser::Serializer;
pub use serde_json::{Error, Number};

mod de;
mod from;
mod index;
mod macros;
mod map;
mod partial_eq;
mod ser;

/// Any JSON value: `serde_json::Value`, whose objects are [`Map`]s.
#[derive(Clone, Eq, PartialEq, Hash, Default)]
pub enum Value {
    /// JSON `null`.
    #[default]
    Null,
    /// A JSON boolean.
    Bool(bool),
    /// A JSON number.
    Number(Number),
    /// A JSON string.
    String(String),
    /// A JSON array.
    Array(Vec<Value>),
    /// A JSON object.
    Object(Map<String, Value>),
}

impl Debug for Value {
    fn fmt(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Value::Null => formatter.write_str("Null"),
            Value::Bool(boolean) => write!(formatter, "Bool({boolean})"),
            Value::Number(number) => Debug::fmt(number, formatter),
            Value::String(string) => write!(formatter, "String({string:?})"),
            Value::Array(vec) => {
                formatter.write_str("Array ")?;
                Debug::fmt(vec, formatter)
            }
            Value::Object(map) => {
                formatter.write_str("Object ")?;
                Debug::fmt(map, formatter)
            }
        }
    }
}

impl Display for Value {
    /// The value as compact JSON text, or pretty-printed with `{:#}`.
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        struct WriterFormatter<'a, 'b: 'a> {
            inner: &'a mut fmt::Formatter<'b>,
        }

        impl io::Write for WriterFormatter<'_, '_> {
            fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
                // The serializer writes whole UTF-8 strings.
                let s = std::str::from_utf8(buf).map_err(io::Error::other)?;
                self.inner.write_str(s).map_err(io::Error::other)?;
                Ok(buf.len())
            }

            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }

        let alternate = f.alternate();
        let mut wr = WriterFormatter { inner: f };
        if alternate {
            serde_json::to_writer_pretty(&mut wr, self).map_err(|_| fmt::Error)
        } else {
            serde_json::to_writer(&mut wr, self).map_err(|_| fmt::Error)
        }
    }
}

fn parse_index(s: &str) -> Option<usize> {
    if s.starts_with('+') || (s.starts_with('0') && s.len() != 1) {
        return None;
    }
    s.parse().ok()
}

impl Value {
    /// The array element or object entry `index` names, if any.
    pub fn get<I: Index>(&self, index: I) -> Option<&Value> {
        index.index_into(self)
    }

    /// [`Value::get`], mutably.
    pub fn get_mut<I: Index>(&mut self, index: I) -> Option<&mut Value> {
        index.index_into_mut(self)
    }

    /// Whether this is an object.
    #[must_use]
    pub fn is_object(&self) -> bool {
        self.as_object().is_some()
    }

    /// The object's map, if this is an object.
    #[must_use]
    pub fn as_object(&self) -> Option<&Map<String, Value>> {
        match self {
            Value::Object(map) => Some(map),
            _ => None,
        }
    }

    /// The object's map, mutably, if this is an object.
    pub fn as_object_mut(&mut self) -> Option<&mut Map<String, Value>> {
        match self {
            Value::Object(map) => Some(map),
            _ => None,
        }
    }

    /// Whether this is an array.
    #[must_use]
    pub fn is_array(&self) -> bool {
        self.as_array().is_some()
    }

    /// The array's elements, if this is an array.
    #[must_use]
    pub fn as_array(&self) -> Option<&Vec<Value>> {
        match self {
            Value::Array(array) => Some(array),
            _ => None,
        }
    }

    /// The array's elements, mutably, if this is an array.
    pub fn as_array_mut(&mut self) -> Option<&mut Vec<Value>> {
        match self {
            Value::Array(list) => Some(list),
            _ => None,
        }
    }

    /// Whether this is a string.
    #[must_use]
    pub fn is_string(&self) -> bool {
        self.as_str().is_some()
    }

    /// The string, if this is one.
    #[must_use]
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    /// Whether this is a number.
    #[must_use]
    pub fn is_number(&self) -> bool {
        matches!(self, Value::Number(_))
    }

    /// The number, if this is one.
    #[must_use]
    pub fn as_number(&self) -> Option<&Number> {
        match self {
            Value::Number(number) => Some(number),
            _ => None,
        }
    }

    /// Whether this is an integer that fits an `i64`.
    #[must_use]
    pub fn is_i64(&self) -> bool {
        match self {
            Value::Number(n) => n.is_i64(),
            _ => false,
        }
    }

    /// Whether this is an integer that fits a `u64`.
    #[must_use]
    pub fn is_u64(&self) -> bool {
        match self {
            Value::Number(n) => n.is_u64(),
            _ => false,
        }
    }

    /// Whether this is a number that is neither an `i64` nor a `u64`.
    #[must_use]
    pub fn is_f64(&self) -> bool {
        match self {
            Value::Number(n) => n.is_f64(),
            _ => false,
        }
    }

    /// The number as an `i64`, if it is an integer that fits one.
    #[must_use]
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Number(n) => n.as_i64(),
            _ => None,
        }
    }

    /// The number as a `u64`, if it is an integer that fits one.
    #[must_use]
    pub fn as_u64(&self) -> Option<u64> {
        match self {
            Value::Number(n) => n.as_u64(),
            _ => None,
        }
    }

    /// The number as an `f64`, if this is a number.
    #[must_use]
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Number(n) => n.as_f64(),
            _ => None,
        }
    }

    /// Whether this is a boolean.
    #[must_use]
    pub fn is_boolean(&self) -> bool {
        self.as_bool().is_some()
    }

    /// The boolean, if this is one.
    #[must_use]
    pub fn as_bool(&self) -> Option<bool> {
        match *self {
            Value::Bool(b) => Some(b),
            _ => None,
        }
    }

    /// Whether this is `null`.
    #[must_use]
    pub fn is_null(&self) -> bool {
        self.as_null().is_some()
    }

    /// `Some(())` if this is `null`.
    #[must_use]
    pub fn as_null(&self) -> Option<()> {
        match *self {
            Value::Null => Some(()),
            _ => None,
        }
    }

    /// The value a JSON Pointer (RFC 6901) names, if any.
    #[must_use]
    pub fn pointer(&self, pointer: &str) -> Option<&Value> {
        if pointer.is_empty() {
            return Some(self);
        }
        if !pointer.starts_with('/') {
            return None;
        }
        pointer
            .split('/')
            .skip(1)
            .map(|x| x.replace("~1", "/").replace("~0", "~"))
            .try_fold(self, |target, token| match target {
                Value::Object(map) => map.get(&token),
                Value::Array(list) => parse_index(&token).and_then(|x| list.get(x)),
                _ => None,
            })
    }

    /// [`Value::pointer`], mutably.
    pub fn pointer_mut(&mut self, pointer: &str) -> Option<&mut Value> {
        if pointer.is_empty() {
            return Some(self);
        }
        if !pointer.starts_with('/') {
            return None;
        }
        pointer
            .split('/')
            .skip(1)
            .map(|x| x.replace("~1", "/").replace("~0", "~"))
            .try_fold(self, |target, token| match target {
                Value::Object(map) => map.get_mut(&token),
                Value::Array(list) => parse_index(&token).and_then(move |x| list.get_mut(x)),
                _ => None,
            })
    }

    /// Takes the value out, leaving `null` in its place.
    pub fn take(&mut self) -> Value {
        mem::replace(self, Value::Null)
    }

    /// Sorts every object's keys, at any depth.
    pub fn sort_all_objects(&mut self) {
        match self {
            Value::Object(map) => {
                map.sort_keys();
                map.values_mut().for_each(Value::sort_all_objects);
            }
            Value::Array(list) => {
                list.iter_mut().for_each(Value::sort_all_objects);
            }
            _ => {}
        }
    }
}

impl Default for &Value {
    fn default() -> Self {
        const DEFAULT: Value = Value::Null;
        &DEFAULT
    }
}

/// `value` as a [`Value`] (`serde_json::to_value`).
///
/// # Errors
///
/// When `value`'s `Serialize` fails, or it has a map with a key that is not
/// a string.
pub fn to_value<T>(value: T) -> Result<Value, Error>
where
    T: Serialize,
{
    value.serialize(Serializer)
}

/// A `T` from a [`Value`] (`serde_json::from_value`).
///
/// # Errors
///
/// When `value` does not have the shape `T` reads.
pub fn from_value<T>(value: Value) -> Result<T, Error>
where
    T: DeserializeOwned,
{
    T::deserialize(value)
}

#[cfg(test)]
mod tests;
