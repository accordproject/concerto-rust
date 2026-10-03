//! Instance validation in one engine call per resource (task P5-12c,
//! accordproject/concerto-rust#293; the P5-12 spike's variant B on the
//! P5-12b transport, accordproject/concerto-rust#289 and #292).
//!
//! `ValidatedResource.validate()`, `setPropertyValue` and `addArrayValue`
//! (concerto-core src/model/validatedresource.ts) hand the value they check
//! across once, and the existing instance validator
//! ([`concerto_core::instance::validate`]) runs over it. Additive: nothing
//! else in this crate calls these bindings.
//!
//! # Transport
//!
//! The TS side (src/engine/validate-resource.ts) writes the value straight
//! from the live object into the compact binary layout below, already in
//! the shape the validator reads (the `validate.rs` module doc, "Scope": a
//! `$class`-tagged object per Resource, `{$$relationship, $class, <id
//! field>}` per Relationship, and `$$dayjs`/`$$undefined`/`$$number`/`$$map`
//! markers), exactly what `Instance::to_validator_value` builds from the
//! Serializer's wire value. So there is no JSON text, no `decode_wire` and
//! no `to_validator_value` on this path. wasm-bindgen copies the bytes in as
//! a `&[u8]`.
//!
//! One tag byte, then:
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
//! The validator options cross as a bit set (bit 0
//! `convertResourcesToRelationships`, bit 1
//! `permitResourcesForRelationships`) and the root identifier as a string.
//!
//! # Result codes
//!
//! Each binding returns a code instead of throwing, so the common outcomes
//! cost no exception object built in WASM (P5-12b, candidate (e)):
//!
//! - [`CODE_VALID`]: the value is valid;
//! - [`CODE_VALIDATION`]: a `Validation` error that is not a validator's
//!   (a validator's carries an `errorType`, BC-39, so it is a
//!   [`CODE_ERROR`]). TS reads its message with
//!   [`validate_error_message`] and throws `new ValidationException(message)`
//!   itself, which is exactly what the error factory builds for that kind
//!   (src/engine/errors.ts);
//! - [`CODE_ERROR`]: any other error. TS throws what [`validate_take_error`]
//!   returns: the exception the unchanged `throw`/error-factory path builds,
//!   so its class is the one every other binding would throw;
//! - [`CODE_UNSUPPORTED`]: the bytes are not a value this layout can carry,
//!   or the engine's model does not have the property TS found. TS throws
//!   `EngineFastPathUnsupported` and runs the `ResourceValidator` visitor
//!   instead, as it does for a value it cannot encode at all.
//!
//! The error stays in a thread-local slot until TS takes it, or until the
//! next call replaces it.

use std::cell::RefCell;

use concerto_core::error::ErrorKind;
use concerto_core::instance::ValidateOptions;
use concerto_core::instance::validate::{validate_instance_from, validate_property_value};
use serde_json::{Map, Number, Value};
use wasm_bindgen::prelude::*;

use super::{Error, ModelManagerHandle, Result, throw, wire_error};

/// The value is valid.
const CODE_VALID: u32 = 0;
/// A `Validation` error (module doc).
const CODE_VALIDATION: u32 = 1;
/// Any other error (module doc).
const CODE_ERROR: u32 = 2;
/// The transport cannot carry the value (module doc).
const CODE_UNSUPPORTED: u32 = 3;

/// How deep a value may nest before the transport gives up on it (the TS
/// visitor then runs instead).
const MAX_DEPTH: u32 = 512;

thread_local! {
    /// The error behind the last non-zero code.
    static LAST_ERROR: RefCell<Option<Error>> = const { RefCell::new(None) };
}

/// A failure of the transport itself, not of the validator.
struct Unsupported(Error);

/// The code for a validation outcome, keeping the error for
/// [`validate_error_message`] or [`validate_take_error`].
fn code_of(result: std::result::Result<Result<()>, Unsupported>) -> u32 {
    let (code, err) = match result {
        Ok(Ok(())) => return CODE_VALID,
        Ok(Err(err)) => {
            let code = match &err {
                // A validator error (BC-39) is a `Validation` error too, but
                // it carries an `errorType`, which only the full `throw` path
                // sets on the exception.
                Error::Contract(c) if c.kind == ErrorKind::Validation && c.validator.is_none() => {
                    CODE_VALIDATION
                }
                _ => CODE_ERROR,
            };
            (code, err)
        }
        Err(Unsupported(err)) => (CODE_UNSUPPORTED, err),
    };
    LAST_ERROR.with(|l| *l.borrow_mut() = Some(err));
    code
}

/// The rendered message of the error behind the last non-zero code, and
/// drops it. Empty when there is none.
#[wasm_bindgen(js_name = validateErrorMessage)]
pub fn validate_error_message() -> String {
    LAST_ERROR.with(|l| match l.borrow_mut().take() {
        Some(Error::Contract(c)) => c.message(),
        _ => String::new(),
    })
}

/// The error behind the last non-zero code, as the exception every other
/// binding throws for it (`run`'s mapping), and drops it. `undefined` when
/// there is none.
#[wasm_bindgen(js_name = validateTakeError)]
pub fn validate_take_error() -> JsValue {
    LAST_ERROR.with(|l| match l.borrow_mut().take() {
        Some(err) => throw(err, None),
        None => JsValue::UNDEFINED,
    })
}

fn options_from_flags(flags: u32) -> ValidateOptions {
    ValidateOptions {
        convert_resources_to_relationships: flags & 1 != 0,
        permit_resources_for_relationships: flags & 2 != 0,
    }
}

fn unsupported(reason: &str) -> Unsupported {
    Unsupported(wire_error(format!("a binary wire value: {reason}")))
}

/// A double as `validate::js_number` spells a finite one: an integral value
/// below 2^53 as a JSON integer.
fn validator_number(n: f64) -> Value {
    if n.trunc() == n && n.abs() < 9_007_199_254_740_992.0 {
        return Value::Number(Number::from(n as i64));
    }
    // The TS side only writes finite doubles here (a non-finite number
    // crosses as a `$$number` marker object), so `from_f64` succeeds.
    Number::from_f64(n).map_or(Value::Null, Value::Number)
}

/// A reader over the binary layout (module doc).
struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> std::result::Result<&'a [u8], Unsupported> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| unsupported("truncated"))?;
        let out = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| unsupported("truncated"))?;
        self.pos = end;
        Ok(out)
    }

    fn array<const N: usize>(&mut self) -> std::result::Result<[u8; N], Unsupported> {
        let mut a = [0u8; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn u32(&mut self) -> std::result::Result<usize, Unsupported> {
        Ok(u32::from_le_bytes(self.array()?) as usize)
    }

    fn string(&mut self) -> std::result::Result<String, Unsupported> {
        let len = self.u32()?;
        let bytes = self.take(len)?;
        std::str::from_utf8(bytes)
            .map(str::to_string)
            .map_err(|_| unsupported("invalid UTF-8"))
    }

    fn value(&mut self, depth: u32) -> std::result::Result<Value, Unsupported> {
        if depth > MAX_DEPTH {
            return Err(unsupported("nested too deeply"));
        }
        let [tag] = self.array()?;
        match tag {
            0 => Ok(Value::Null),
            1 => Ok(Value::Bool(false)),
            2 => Ok(Value::Bool(true)),
            3 => Ok(validator_number(f64::from_le_bytes(self.array()?))),
            4 => Ok(Value::Number(Number::from(i32::from_le_bytes(
                self.array()?,
            )))),
            5 => self.string().map(Value::String),
            6 => {
                let n = self.u32()?;
                // Each item is at least one byte: a count past what is left
                // is truncated input, not a reason to allocate.
                let mut items = Vec::with_capacity(n.min(self.bytes.len() - self.pos));
                for _ in 0..n {
                    items.push(self.value(depth + 1)?);
                }
                Ok(Value::Array(items))
            }
            7 => {
                let n = self.u32()?;
                let mut map = Map::with_capacity(n.min(self.bytes.len() - self.pos));
                for _ in 0..n {
                    let key = self.string()?;
                    let value = self.value(depth + 1)?;
                    // `IndexMap` insert: a repeated key keeps its first
                    // position and takes the last value, as a JS object does.
                    map.insert(key, value);
                }
                Ok(Value::Object(map))
            }
            _ => Err(unsupported("unknown tag")),
        }
    }
}

/// The whole of `bytes` as one value.
fn decode(bytes: &[u8]) -> std::result::Result<Value, Unsupported> {
    let mut reader = Reader { bytes, pos: 0 };
    let value = reader.value(0)?;
    if reader.pos != bytes.len() {
        return Err(unsupported("trailing bytes"));
    }
    Ok(value)
}

#[wasm_bindgen]
impl ModelManagerHandle {
    /// `ValidatedResource.validate()`: the resource in the binary layout
    /// (module doc), validated as `Resource.validate` does
    /// (`validate_instance_from` with the instance's
    /// `getFullyQualifiedIdentifier()` as `root_id`). Returns a result code
    /// (module doc). The `$identifier` write-back of `visitClassDeclaration`
    /// is left to the caller, which owns the live object.
    #[wasm_bindgen(js_name = validateResourceBinary)]
    pub fn validate_resource_binary(&self, bytes: &[u8], root_id: &str, flags: u32) -> u32 {
        code_of(decode(bytes).map(|value| {
            validate_instance_from(
                &self.manager,
                &value,
                &options_from_flags(flags),
                root_id.to_string(),
            )
            .map_err(Error::from)
        }))
    }

    /// `field.accept(this.$validator, parameters)` in
    /// `ValidatedResource.setPropertyValue` and `addArrayValue`: `bytes` is
    /// the value (for `addArrayValue`, the whole new array) in the binary
    /// layout, `class_fqn` the instance's type and `prop_name` the property
    /// TS already found on it. Returns a result code (module doc);
    /// [`CODE_UNSUPPORTED`] when this manager has no such property.
    #[wasm_bindgen(js_name = validatePropertyBinary)]
    pub fn validate_property_binary(
        &self,
        bytes: &[u8],
        class_fqn: &str,
        prop_name: &str,
        root_id: &str,
        flags: u32,
    ) -> u32 {
        code_of(decode(bytes).and_then(|value| {
            // Validation plan (P5-88, accordproject/concerto-rust#434): the
            // property from the plan's name index, validated over the plan.
            // A type whose plan (its chain) does not resolve, or that has
            // no such property, is not one this manager can check.
            let Ok(class_plan) =
                concerto_core::instance::plan::class_plan_by_name(&self.manager, class_fqn)
            else {
                return Err(unsupported("no such property in the engine's model"));
            };
            let Some(index) = class_plan.find(prop_name) else {
                return Err(unsupported("no such property in the engine's model"));
            };
            Ok(validate_property_value(
                &self.manager,
                &class_plan,
                index,
                &value,
                root_id.to_string(),
                &options_from_flags(flags),
            )
            .map_err(Error::from))
        }))
    }
}

#[cfg(test)]
mod tests {
    // Host-side tests of the decoder alone (no `js_sys` call is reached).
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn str_bytes(out: &mut Vec<u8>, s: &str) {
        out.extend_from_slice(&(s.len() as u32).to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    }

    #[test]
    fn decodes_every_tag() {
        let mut b = vec![7];
        b.extend_from_slice(&7u32.to_le_bytes());
        str_bytes(&mut b, "n");
        b.push(0);
        str_bytes(&mut b, "f");
        b.push(1);
        str_bytes(&mut b, "t");
        b.push(2);
        str_bytes(&mut b, "d");
        b.push(3);
        b.extend_from_slice(&1.5f64.to_le_bytes());
        str_bytes(&mut b, "i");
        b.push(4);
        b.extend_from_slice(&(-7i32).to_le_bytes());
        str_bytes(&mut b, "s");
        b.push(5);
        str_bytes(&mut b, "é");
        str_bytes(&mut b, "a");
        b.push(6);
        b.extend_from_slice(&2u32.to_le_bytes());
        b.push(3);
        b.extend_from_slice(&2.0f64.to_le_bytes());
        b.push(0);
        let v = decode(&b).ok().unwrap();
        assert_eq!(
            v,
            serde_json::json!({"n": null, "f": false, "t": true, "d": 1.5, "i": -7, "s": "é", "a": [2, null]})
        );
        // An integral double reads as an integer, as `js_number` spells it.
        assert!(v["a"][0].is_i64());
    }

    #[test]
    fn repeated_key_keeps_first_position_and_last_value() {
        let mut b = vec![7];
        b.extend_from_slice(&3u32.to_le_bytes());
        for (k, v) in [("a", 1u8), ("b", 2), ("a", 1)] {
            str_bytes(&mut b, k);
            b.push(v);
        }
        let v = decode(&b).ok().unwrap();
        let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        assert_eq!(keys, ["a", "b"]);
        assert_eq!(v["a"], Value::Bool(false));
    }

    #[test]
    fn rejects_malformed_input() {
        // Truncated, unknown tag, trailing bytes, bad UTF-8, a huge count.
        for b in [
            vec![],
            vec![3, 0, 0],
            vec![9],
            vec![0, 0],
            vec![5, 1, 0, 0, 0, 0xff],
            vec![6, 0xff, 0xff, 0xff, 0xff],
        ] {
            assert!(decode(&b).is_err(), "{b:?}");
        }
        // Too deep.
        let mut deep = Vec::new();
        for _ in 0..=MAX_DEPTH + 1 {
            deep.push(6);
            deep.extend_from_slice(&1u32.to_le_bytes());
        }
        deep.push(0);
        assert!(decode(&deep).is_err());
    }

    #[test]
    fn codes() {
        assert_eq!(code_of(Ok(Ok(()))), CODE_VALID);
        let validation: Error = concerto_core::error::ContractError::pre_port(
            ErrorKind::Validation,
            "bad".to_string(),
            None,
        )
        .into();
        assert_eq!(code_of(Ok(Err(validation))), CODE_VALIDATION);
        // A validator's `Validation` error (BC-39) keeps its `errorType`
        // through the full `throw` path.
        let mut validator = concerto_core::error::ContractError::pre_port(
            ErrorKind::Validation,
            "too long".to_string(),
            None,
        );
        validator.validator = Some(concerto_core::error::ValidatorReport {
            id: "null".to_string(),
            fqn: "org.acme@1.0.0.C.s".to_string(),
            error_type: "DefaultValidatorException",
        });
        assert_eq!(code_of(Ok(Err(validator.into()))), CODE_ERROR);
        let other: Error = concerto_core::error::ContractError::pre_port(
            ErrorKind::TypeNotFound,
            "missing".to_string(),
            None,
        )
        .into();
        assert_eq!(code_of(Ok(Err(other))), CODE_ERROR);
        assert_eq!(code_of(Err(unsupported("x"))), CODE_UNSUPPORTED);
        assert_eq!(validate_error_message(), "a binary wire value: x");
        assert_eq!(validate_error_message(), "");
    }
}
