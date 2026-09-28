//! P5-12b SPIKE (DO NOT MERGE; accordproject/concerto-rust#292): cheaper
//! TS<->WASM transports for one-call instance validation (P5-12 variant B).
//! Measure only. Every binding here is additive; `validateResource` (P5-12)
//! is unchanged.
//!
//! Every candidate hands the validator the value shape it already reads
//! (`concerto_core::instance::validate`, module doc "Scope": `$class`-tagged
//! objects, `$$dayjs`/`$$relationship`/`$$undefined`/`$$number`/`$$bigint`/
//! `$$map` markers), built on the TS side, so the engine skips the P5-12
//! path's `decode_wire` (wire JSON -> `JsValue`) and `to_validator_value`
//! (`JsValue` -> validator `Value`) steps. They differ only in how that
//! tree crosses:
//!
//! - (a) `validateResourceJson`: JSON text, `serde_json::from_str` straight
//!   into the validator's `Value`;
//! - (a+e) `validateResourceJsonScratch`: the same JSON text, UTF-8 encoded
//!   by TS straight into a reused engine-owned buffer (no per-call
//!   malloc/free);
//! - (b) `validateResourceObject`: the JS object tree itself, through
//!   `serde-wasm-bindgen`;
//! - (c) `validateResourceBinary`: a compact tagged binary layout
//!   (`Uint8Array`, copied in by wasm-bindgen);
//! - (c+e) `validateResourceBinaryScratch`: the same layout written by TS
//!   straight into the reused engine-owned buffer.
//!
//! The options cross as a bit set (`flags`: bit 0
//! `convertResourcesToRelationships`, bit 1
//! `permitResourcesForRelationships`) and the root identifier as a short
//! string, instead of JSON text (candidate (e)). Errors leave exactly as
//! `validateResource`'s do (`run`), so the TS classes are unchanged.
//!
//! `p512bStage`/`p512bStageJson`/`p512bPrepare`/`p512bValidatePrepared`
//! stop part way, for the per-stage cost split.

use std::cell::RefCell;

use concerto_core::error::{ContractError, ErrorKind};
use concerto_core::instance::ValidateOptions;
use concerto_core::instance::validate::validate_instance_from;
use serde::de::IgnoredAny;
use serde_json::{Map, Number, Value};
use wasm_bindgen::prelude::*;

use super::{
    CoreValue, Error, ModelManagerHandle, Result, decode_wire, decode_wire_options, run, wire_error,
};

thread_local! {
    /// The reused transport buffer (candidate (e)).
    static SCRATCH: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
    /// A validator value parsed once, for `p512bValidatePrepared`.
    static PREPARED: RefCell<Option<Value>> = const { RefCell::new(None) };
}

fn options_from_flags(flags: u32) -> ValidateOptions {
    ValidateOptions {
        convert_resources_to_relationships: flags & 1 != 0,
        permit_resources_for_relationships: flags & 2 != 0,
    }
}

/// A parse failure: the TS side falls back to the visitor (it matches
/// `wire value`, as `asUnsupported` does for the P5-12 path). serde_json
/// rejects a lone-surrogate escape, which is how a lone surrogate that
/// `JSON.stringify` escaped reaches here.
fn parse_error(e: impl std::fmt::Display) -> Error {
    wire_error(format!("an unreadable wire value: {e}"))
}

fn validate(mm: &ModelManagerHandle, value: &Value, root_id: &str, flags: u32) -> Result<()> {
    validate_instance_from(
        &mm.manager,
        value,
        &options_from_flags(flags),
        root_id.to_string(),
    )?;
    Ok(())
}

/// A JSON number as `JsValue::to_validator_value` spells it (an integral
/// double below 2^53 as an integer).
fn validator_number(n: f64) -> Value {
    if n.trunc() == n && n.abs() < 9_007_199_254_740_992.0 {
        return Value::Number(Number::from(n as i64));
    }
    Number::from_f64(n).map_or(Value::Null, Value::Number)
}

// ---------------------------------------------------------------------
// (c) the binary layout
// ---------------------------------------------------------------------
//
// One tag byte, then:
//   0 null | 1 false | 2 true
//   3 f64 (8 bytes LE) | 4 i32 (4 bytes LE)
//   5 string: u32 LE byte length, UTF-8 bytes
//   6 array: u32 LE count, items
//   7 object: u32 LE count, then (u32 LE key length, key UTF-8, value)

struct Reader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Result<&[u8]> {
        let end = self
            .pos
            .checked_add(n)
            .ok_or_else(|| wire_error("a truncated binary wire value".to_string()))?;
        let out = self
            .bytes
            .get(self.pos..end)
            .ok_or_else(|| wire_error("a truncated binary wire value".to_string()))?;
        self.pos = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?.first().copied().unwrap_or(0))
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        let mut a = [0u8; 4];
        a.copy_from_slice(b);
        Ok(u32::from_le_bytes(a))
    }

    fn string(&mut self) -> Result<String> {
        let len = self.u32()? as usize;
        let b = self.take(len)?;
        std::str::from_utf8(b)
            .map(str::to_string)
            .map_err(parse_error)
    }

    fn value(&mut self, depth: u32) -> Result<Value> {
        if depth > 512 {
            return Err(wire_error(
                "a binary wire value nested too deeply".to_string(),
            ));
        }
        match self.u8()? {
            0 => Ok(Value::Null),
            1 => Ok(Value::Bool(false)),
            2 => Ok(Value::Bool(true)),
            3 => {
                let b = self.take(8)?;
                let mut a = [0u8; 8];
                a.copy_from_slice(b);
                Ok(validator_number(f64::from_le_bytes(a)))
            }
            4 => {
                let b = self.take(4)?;
                let mut a = [0u8; 4];
                a.copy_from_slice(b);
                Ok(Value::Number(Number::from(i32::from_le_bytes(a))))
            }
            5 => self.string().map(Value::String),
            6 => {
                let n = self.u32()? as usize;
                let mut items = Vec::with_capacity(n.min(1024));
                for _ in 0..n {
                    items.push(self.value(depth + 1)?);
                }
                Ok(Value::Array(items))
            }
            7 => {
                let n = self.u32()? as usize;
                let mut map = Map::with_capacity(n.min(1024));
                for _ in 0..n {
                    let key = self.string()?;
                    let value = self.value(depth + 1)?;
                    map.insert(key, value);
                }
                Ok(Value::Object(map))
            }
            other => Err(wire_error(format!("a binary wire value with tag {other}"))),
        }
    }
}

fn decode_binary(bytes: &[u8]) -> Result<Value> {
    let mut reader = Reader { bytes, pos: 0 };
    reader.value(0)
}

/// The reused buffer, grown to at least `len` bytes, as a `Uint8Array` view
/// into the module's memory (valid until the memory next grows).
#[wasm_bindgen(js_name = p512bScratch)]
pub fn p512b_scratch(len: u32) -> js_sys::Uint8Array {
    SCRATCH.with(|s| {
        let mut buf = s.borrow_mut();
        if buf.len() < len as usize {
            buf.resize(len as usize, 0);
        }
        let memory: js_sys::WebAssembly::Memory = wasm_bindgen::memory().unchecked_into();
        js_sys::Uint8Array::new_with_byte_offset_and_length(
            &memory.buffer(),
            buf.as_ptr() as u32,
            buf.len() as u32,
        )
    })
}

fn with_scratch<T>(len: u32, body: impl FnOnce(&[u8]) -> Result<T>) -> Result<T> {
    SCRATCH.with(|s| {
        let buf = s.borrow();
        let bytes = buf
            .get(..len as usize)
            .ok_or_else(|| wire_error("a scratch length past the buffer".to_string()))?;
        body(bytes)
    })
}

#[wasm_bindgen]
impl ModelManagerHandle {
    /// (a) Validator-shaped JSON text, parsed straight into the validator's
    /// `Value`.
    #[wasm_bindgen(js_name = validateResourceJson)]
    pub fn validate_resource_json(
        &self,
        text: &str,
        root_id: &str,
        flags: u32,
    ) -> std::result::Result<(), JsValue> {
        run(|| {
            let value: Value = serde_json::from_str(text).map_err(parse_error)?;
            validate(self, &value, root_id, flags)
        })
    }

    /// (a+e) As [`Self::validate_resource_json`], from the first `len` bytes
    /// of the reused buffer.
    #[wasm_bindgen(js_name = validateResourceJsonScratch)]
    pub fn validate_resource_json_scratch(
        &self,
        len: u32,
        root_id: &str,
        flags: u32,
    ) -> std::result::Result<(), JsValue> {
        run(|| {
            let value: Value =
                with_scratch(len, |b| serde_json::from_slice(b).map_err(parse_error))?;
            validate(self, &value, root_id, flags)
        })
    }

    /// (b) The validator-shaped JS object tree, through serde-wasm-bindgen.
    #[wasm_bindgen(js_name = validateResourceObject)]
    pub fn validate_resource_object(
        &self,
        value: JsValue,
        root_id: &str,
        flags: u32,
    ) -> std::result::Result<(), JsValue> {
        run(|| {
            let value: Value = serde_wasm_bindgen::from_value(value).map_err(parse_error)?;
            validate(self, &value, root_id, flags)
        })
    }

    /// (c) The validator-shaped tree in the binary layout above.
    #[wasm_bindgen(js_name = validateResourceBinary)]
    pub fn validate_resource_binary(
        &self,
        bytes: &[u8],
        root_id: &str,
        flags: u32,
    ) -> std::result::Result<(), JsValue> {
        run(|| {
            let value = decode_binary(bytes)?;
            validate(self, &value, root_id, flags)
        })
    }

    /// (c+e) As [`Self::validate_resource_binary`], from the first `len`
    /// bytes of the reused buffer.
    #[wasm_bindgen(js_name = validateResourceBinaryScratch)]
    pub fn validate_resource_binary_scratch(
        &self,
        len: u32,
        root_id: &str,
        flags: u32,
    ) -> std::result::Result<(), JsValue> {
        run(|| {
            let value = with_scratch(len, decode_binary)?;
            validate(self, &value, root_id, flags)
        })
    }

    /// Profiling only: P5-12's `validateResource`, stopped after `stage`:
    /// 0 = the call and the two string copies, 1 = + `serde_json` parse,
    /// 2 = + `decode_wire`, 3 = + options decode and `to_validator_value`,
    /// 4 = + validate (all of `validateResource`).
    #[wasm_bindgen(js_name = p512bStage)]
    pub fn p512b_stage(
        &self,
        wire_text: &str,
        options_text: &str,
        stage: u32,
    ) -> std::result::Result<(), JsValue> {
        run(|| {
            if stage == 0 {
                return Ok(());
            }
            let wire_value: Value = serde_json::from_str(wire_text).map_err(parse_error)?;
            if stage == 1 {
                return Ok(());
            }
            let resource = decode_wire(&wire_value)?;
            if stage == 2 {
                return Ok(());
            }
            let CoreValue::Instance(instance) = resource else {
                return Err(ContractError::pre_port(
                    ErrorKind::Error,
                    "typed wire value expected".to_string(),
                    None,
                )
                .into());
            };
            let options = decode_wire_options(options_text)?;
            let truthy = |key: &str| {
                options
                    .as_ref()
                    .and_then(|o| o.get(key))
                    .is_some_and(CoreValue::is_truthy)
            };
            let validate_options = ValidateOptions {
                convert_resources_to_relationships: truthy("convertResourcesToRelationships"),
                permit_resources_for_relationships: truthy("permitResourcesForRelationships"),
            };
            let value = instance.to_validator_value();
            if stage == 3 {
                return Ok(());
            }
            validate_instance_from(
                &self.manager,
                &value,
                &validate_options,
                instance.fully_qualified_identifier(),
            )?;
            Ok(())
        })
    }

    /// Profiling only, for a validator-shaped JSON text: 0 = the call and
    /// the string copy, 1 = + parse into `Value`, 2 = parse into
    /// `IgnoredAny` instead (the tokenizer alone: the floor for a
    /// validator that builds no tree, candidate (d)).
    #[wasm_bindgen(js_name = p512bStageJson)]
    pub fn p512b_stage_json(&self, text: &str, stage: u32) -> std::result::Result<(), JsValue> {
        run(|| {
            match stage {
                0 => {}
                1 => {
                    let _value: Value = serde_json::from_str(text).map_err(parse_error)?;
                }
                _ => {
                    let _ignored: IgnoredAny = serde_json::from_str(text).map_err(parse_error)?;
                }
            }
            Ok(())
        })
    }

    /// Profiling only: parse a validator-shaped JSON text once and keep it.
    #[wasm_bindgen(js_name = p512bPrepare)]
    pub fn p512b_prepare(&self, text: &str) -> std::result::Result<(), JsValue> {
        run(|| {
            let value: Value = serde_json::from_str(text).map_err(parse_error)?;
            PREPARED.with(|p| *p.borrow_mut() = Some(value));
            Ok(())
        })
    }

    /// Profiling only: validate the value [`Self::p512b_prepare`] kept (the
    /// validator alone, in WASM, plus a call with one short string).
    #[wasm_bindgen(js_name = p512bValidatePrepared)]
    pub fn p512b_validate_prepared(
        &self,
        root_id: &str,
        flags: u32,
    ) -> std::result::Result<(), JsValue> {
        run(|| {
            PREPARED.with(|p| match p.borrow().as_ref() {
                Some(value) => validate(self, value, root_id, flags),
                None => Err(wire_error("nothing prepared".to_string())),
            })
        })
    }
}
