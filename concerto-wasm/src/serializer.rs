//! The Serializer fast path and its wire codec.

use super::*;

// ---------------------------------------------------------------------------
// Serializer fast path
// ---------------------------------------------------------------------------
//
// `Serializer.fromJSON`/`toJSON` cross the boundary in one call each,
// rather than per field through the TS visitors (the fallback).
//
// Plain JSON crosses unchanged. Anything else is a one-key object tagged
// `@@oracle` ([`WIRE_TAG`]), so a value JSON cannot hold (a non-finite
// number, `undefined`, a `Map`, a dayjs, a `Resource`/`ValidatedResource`/
// `Relationship`) round-trips; `src/engine/serializer-codec.ts` writes and
// reads the same shapes. A `"typed"` value's `fields` holds every own
// property of the TS object, `$`-prefixed ones included, but
// `$modelManager`/`$classDeclaration`/`$validator`, and decodes straight
// into the `Instance`'s `props`. A dayjs crosses as `(epoch ms, utcOffset
// minutes)` (PORTING.md 3.3), which the view rebuilds.

/// The wire tag key, the oracle harness's `M` constant.
pub(crate) const WIRE_TAG: &str = "@@oracle";

/// An engine-side error for a wire shape the codec does not recognise (the
/// view controls what it sends): an [`Error::Unsupported`], whose payload
/// carries `fastPathUnsupported: true`, the caller's fallback signal.
pub(crate) fn wire_error(reason: String) -> Error {
    Error::Unsupported(Box::new(ContractError::pre_port(
        ErrorKind::InvalidArgument,
        reason,
        None,
    )))
}

/// The error for wire JSON text serde_json could not read. Past serde_json's
/// recursion limit (128 levels) the text is valid, only nested deeper than
/// the engine reads text: that is a [`wire_error`], so the shim runs its TS
/// visitor path, as TS 5.0.0 reads any depth (R2D-1). Any other failure is
/// malformed text, a JS `SyntaxError` ([`json_syntax`]).
pub(crate) fn wire_text_error(e: serde_json::Error) -> Error {
    if e.is_syntax() && e.to_string().starts_with("recursion limit exceeded") {
        wire_error(format!(
            "a wire document nested too deeply for the text path: {e}"
        ))
    } else {
        json_syntax(e)
    }
}

/// A JS number that is not finite, or `-0`, in [`WIRE_TAG`]'s `"number"`
/// encoding.
pub(crate) fn decode_wire_number(text: &str) -> Result<f64> {
    match text {
        "NaN" => Ok(f64::NAN),
        "Infinity" => Ok(f64::INFINITY),
        "-Infinity" => Ok(f64::NEG_INFINITY),
        "-0" => Ok(-0.0),
        other => Err(wire_error(format!("an unrecognised wire number {other}"))),
    }
}

/// A `"typed"` wire value (module doc) as an [`Instance`]: `ctor` selects
/// the [`InstanceKind`], `fqn` is `class_fqn`, and every entry of `fields`
/// decodes straight into `props`, in order.
pub(crate) fn decode_wire_typed(map: &concerto_core::json::Map<String, Value>) -> Result<Instance> {
    let kind = match map.get("ctor").and_then(Value::as_str) {
        Some("Resource") => InstanceKind::Resource,
        Some("ValidatedResource") => InstanceKind::ValidatedResource,
        Some("Relationship") => InstanceKind::Relationship,
        other => return Err(wire_error(format!("a typed wire value of class {other:?}"))),
    };
    let fqn = map
        .get("fqn")
        .and_then(Value::as_str)
        .ok_or_else(|| wire_error("a typed wire value without fqn".to_string()))?
        .to_string();
    let fields = map
        .get("fields")
        .and_then(Value::as_object)
        .ok_or_else(|| wire_error("a typed wire value without fields".to_string()))?;
    let mut props = JsObject::default();
    for (key, value) in fields {
        props.insert(key.clone(), decode_wire(value)?);
    }
    Ok(Instance {
        kind,
        class_fqn: fqn,
        props,
        validator_options: ValidateOptions::default(),
    })
}

/// A wire value (module doc) as the [`CoreValue`] it decodes to: plain JSON
/// unchanged, and [`WIRE_TAG`]'s `undefined`, `number`, `bigint`, `dayjs`,
/// `map` and `typed` kinds.
pub(crate) fn decode_wire(value: &Value) -> Result<CoreValue> {
    match value {
        Value::Null => Ok(CoreValue::Null),
        Value::Bool(b) => Ok(CoreValue::Bool(*b)),
        Value::Number(n) => Ok(CoreValue::Number(n.as_f64().unwrap_or(f64::NAN))),
        Value::String(s) => Ok(CoreValue::String(s.clone())),
        Value::Array(items) => items
            .iter()
            .map(decode_wire)
            .collect::<Result<Vec<_>>>()
            .map(CoreValue::Array),
        Value::Object(map) => match map.get(WIRE_TAG).and_then(Value::as_str) {
            None => {
                let mut out = JsObject::default();
                for (key, item) in map {
                    out.insert(key.clone(), decode_wire(item)?);
                }
                Ok(CoreValue::Object(out))
            }
            Some("undefined") => Ok(CoreValue::Undefined),
            Some("number") => {
                let text = map
                    .get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| wire_error("a wire number without value".to_string()))?;
                decode_wire_number(text).map(CoreValue::Number)
            }
            Some("bigint") => {
                let text = map
                    .get("value")
                    .and_then(Value::as_str)
                    .ok_or_else(|| wire_error("a wire bigint without value".to_string()))?;
                Ok(CoreValue::BigInt(text.to_string()))
            }
            Some("map") => {
                let entries = map
                    .get("entries")
                    .and_then(Value::as_array)
                    .ok_or_else(|| wire_error("a wire map without entries".to_string()))?;
                let mut decoded = Vec::with_capacity(entries.len());
                for entry in entries {
                    let pair = entry.as_array().ok_or_else(|| {
                        wire_error("a wire map entry that is not a pair".to_string())
                    })?;
                    let key = pair
                        .first()
                        .ok_or_else(|| wire_error("a wire map entry without a key".to_string()))?;
                    let value = pair.get(1).ok_or_else(|| {
                        wire_error("a wire map entry without a value".to_string())
                    })?;
                    decoded.push((decode_wire(key)?, decode_wire(value)?));
                }
                Ok(CoreValue::Map(decoded))
            }
            Some("dayjs") => {
                let valid = map.get("valid").and_then(Value::as_bool).unwrap_or(false);
                if !valid {
                    return Ok(CoreValue::DateTime(Dayjs::utc_invalid()));
                }
                let ms = map
                    .get("ms")
                    .and_then(Value::as_f64)
                    .ok_or_else(|| wire_error("a valid wire dayjs without ms".to_string()))?;
                let offset = map.get("utcOffset").and_then(Value::as_f64).unwrap_or(0.0);
                let built = Dayjs::utc_from_number(ms);
                let built = if offset == 0.0 {
                    built
                } else {
                    built.utc_offset_set(&UtcOffset::Number(offset))
                };
                Ok(CoreValue::DateTime(built))
            }
            Some("typed") => decode_wire_typed(map).map(|i| CoreValue::Instance(Box::new(i))),
            Some(other) => Err(wire_error(format!(
                "a wire value of kind {other} has no engine counterpart"
            ))),
        },
    }
}

/// A model file from its JSON AST text, through the typed AST read
/// ([`ModelFile::from_json_text`]); malformed JSON throws a JS
/// `SyntaxError`.
pub(crate) fn model_file_from_text(
    ast: &str,
    definitions: Option<String>,
    file_name: Option<String>,
) -> Result<ModelFile> {
    Ok(ModelFile::from_json_text(ast, definitions, file_name).map_err(json_syntax)??)
}

/// The options object a serializer call's `optionsText` (`JSON.stringify`d
/// by the view, `"null"` for no options) decodes to, from its parsed wire
/// encoding.
fn decode_wire_options_of(value: &Value) -> Result<Option<SerializerOptions>> {
    match value {
        Value::Null => Ok(None),
        Value::Object(map) => {
            let mut options = SerializerOptions::default();
            for (key, item) in map {
                options.insert(key.clone(), decode_wire(item)?);
            }
            Ok(Some(options))
        }
        _ => Err(wire_error(
            "serializer options that are not a plain object or null".to_string(),
        )),
    }
}

/// A JS number in [`WIRE_TAG`]'s `"number"` encoding: non-finite or `-0`
/// values only, since JSON already holds every other number.
#[cfg(test)]
pub(crate) fn encode_wire_number(n: f64) -> Value {
    if !n.is_finite() {
        let text = if n.is_nan() {
            "NaN"
        } else if n > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        };
        return json!({ WIRE_TAG: "number", "value": text });
    }
    if n == 0.0 && n.is_sign_negative() {
        return json!({ WIRE_TAG: "number", "value": "-0" });
    }
    serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
}

/// A dayjs as `(epoch ms, utcOffset minutes)` (PORTING.md 3.3): an invalid
/// one crosses as `{valid: false}`, carrying no time value.
#[cfg(test)]
pub(crate) fn encode_wire_dayjs(d: &Dayjs) -> Value {
    if !d.is_valid() {
        return json!({ WIRE_TAG: "dayjs", "valid": false });
    }
    json!({
        WIRE_TAG: "dayjs",
        "valid": true,
        "ms": d.epoch_ms(),
        "utcOffset": d.utc_offset(),
    })
}

/// An [`Instance`] in [`WIRE_TAG`]'s `"typed"` encoding (module doc): every
/// own property, `fields`, plus its class and TS constructor.
#[cfg(test)]
pub(crate) fn encode_wire_instance(i: &Instance) -> Value {
    let fields: concerto_core::json::Map<String, Value> = i
        .props
        .iter()
        .map(|(k, v)| (k.clone(), encode_wire(v)))
        .collect();
    json!({
        WIRE_TAG: "typed",
        "ctor": i.kind.ctor(),
        "fqn": i.class_fqn,
        "fields": fields,
    })
}

/// A [`CoreValue`] as the wire value the view reads back (module doc): the
/// reference [`WireOut`] is tested against.
#[cfg(test)]
pub(crate) fn encode_wire(v: &CoreValue) -> Value {
    match v {
        CoreValue::Undefined => json!({ WIRE_TAG: "undefined" }),
        CoreValue::Null => Value::Null,
        CoreValue::Bool(b) => Value::Bool(*b),
        CoreValue::Number(n) => encode_wire_number(*n),
        CoreValue::String(s) => Value::String(s.clone()),
        CoreValue::Array(items) => Value::Array(items.iter().map(encode_wire).collect()),
        CoreValue::Object(map) => Value::Object(
            map.iter()
                .map(|(k, x)| (k.clone(), encode_wire(x)))
                .collect(),
        ),
        CoreValue::Map(entries) => json!({
            WIRE_TAG: "map",
            "entries": entries
                .iter()
                .map(|(k, x)| Value::Array(vec![encode_wire(k), encode_wire(x)]))
                .collect::<Vec<_>>(),
        }),
        CoreValue::DateTime(d) => encode_wire_dayjs(d),
        CoreValue::Instance(i) => encode_wire_instance(i),
        // The oracle harness's `bigint` shape. No decoded wire value is a
        // `BigInt`; the view's decoder would fall back on one.
        CoreValue::BigInt(s) => json!({ WIRE_TAG: "bigint", "value": s }),
    }
}

// `serializerFromJsonCompact` reads its document straight into a
// [`CoreValue`] ([`parse_wire`]) and writes its result straight to JSON text
// ([`WireOut`]), without an intermediate `concerto_core::json::Value` tree either
// way; the tests check both against the `Value` route.

/// Deserializes one wire value (module doc) directly into a [`CoreValue`],
/// as `decode_wire(&serde_json::from_str(text)?)` would. A wire shape the
/// codec does not recognise is not a JSON syntax error: its [`wire_error`]
/// is kept in `error` (the first one only) and the value read as
/// `undefined`, so parsing goes on and a syntax error anywhere in the text
/// still takes precedence, as it does when the whole text is parsed first.
pub(crate) struct WireSeed<'e> {
    error: &'e RefCell<Option<Error>>,
}

impl WireSeed<'_> {
    pub(crate) fn fail(&self, error: Error) -> CoreValue {
        let mut slot = self.error.borrow_mut();
        if slot.is_none() {
            *slot = Some(error);
        }
        CoreValue::Undefined
    }
}

impl<'de> serde::de::DeserializeSeed<'de> for WireSeed<'_> {
    type Value = CoreValue;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<CoreValue, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for WireSeed<'_> {
    type Value = CoreValue;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, b: bool) -> std::result::Result<CoreValue, E> {
        Ok(CoreValue::Bool(b))
    }

    fn visit_i64<E>(self, n: i64) -> std::result::Result<CoreValue, E> {
        Ok(CoreValue::Number(n as f64))
    }

    fn visit_u64<E>(self, n: u64) -> std::result::Result<CoreValue, E> {
        Ok(CoreValue::Number(n as f64))
    }

    fn visit_f64<E>(self, n: f64) -> std::result::Result<CoreValue, E> {
        // `concerto_core::json::Value` holds a non-finite double as `null`.
        Ok(if n.is_finite() {
            CoreValue::Number(n)
        } else {
            CoreValue::Null
        })
    }

    fn visit_str<E>(self, s: &str) -> std::result::Result<CoreValue, E> {
        Ok(CoreValue::String(s.to_string()))
    }

    fn visit_string<E>(self, s: String) -> std::result::Result<CoreValue, E> {
        Ok(CoreValue::String(s))
    }

    fn visit_unit<E>(self) -> std::result::Result<CoreValue, E> {
        Ok(CoreValue::Null)
    }

    fn visit_none<E>(self) -> std::result::Result<CoreValue, E> {
        Ok(CoreValue::Null)
    }

    fn visit_some<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> std::result::Result<CoreValue, D::Error> {
        deserializer.deserialize_any(self)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(
        self,
        mut seq: A,
    ) -> std::result::Result<CoreValue, A::Error> {
        let mut items = Vec::with_capacity(seq.size_hint().unwrap_or(0));
        while let Some(item) = seq.next_element_seed(WireSeed { error: self.error })? {
            items.push(item);
        }
        Ok(CoreValue::Array(items))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(
        self,
        mut access: A,
    ) -> std::result::Result<CoreValue, A::Error> {
        // serde_json gives no size hint; most documents' objects have a
        // handful of keys, so start with room for 8 rather than regrow.
        let mut map = SerializerOptions::with_capacity_and_hasher(
            access.size_hint().unwrap_or(8),
            Default::default(),
        );
        while let Some(key) = access.next_key::<String>()? {
            let value = access.next_value_seed(WireSeed { error: self.error })?;
            map.insert(key, value);
        }
        let kind = match map.get(WIRE_TAG) {
            Some(CoreValue::String(kind)) => kind.clone(),
            _ => return Ok(CoreValue::Object(map)),
        };
        Ok(match decode_wire_tagged(&kind, map) {
            Ok(value) => value,
            Err(error) => self.fail(error),
        })
    }
}

/// A [`WIRE_TAG`]ged object of kind `kind`, its entries already read into
/// `map`, as the [`CoreValue`] it decodes to: [`decode_wire`]'s tagged arms
/// over already-decoded entries.
pub(crate) fn decode_wire_tagged(kind: &str, mut map: SerializerOptions) -> Result<CoreValue> {
    let number = |map: &SerializerOptions, key: &str| match map.get(key) {
        Some(CoreValue::Number(n)) => Some(*n),
        _ => None,
    };
    match kind {
        "undefined" => Ok(CoreValue::Undefined),
        "number" => match map.get("value") {
            Some(CoreValue::String(text)) => decode_wire_number(text).map(CoreValue::Number),
            _ => Err(wire_error("a wire number without value".to_string())),
        },
        "bigint" => match map.swap_remove("value") {
            Some(CoreValue::String(text)) => Ok(CoreValue::BigInt(text)),
            _ => Err(wire_error("a wire bigint without value".to_string())),
        },
        "map" => {
            let Some(CoreValue::Array(entries)) = map.swap_remove("entries") else {
                return Err(wire_error("a wire map without entries".to_string()));
            };
            let mut decoded = Vec::with_capacity(entries.len());
            for entry in entries {
                let CoreValue::Array(pair) = entry else {
                    return Err(wire_error(
                        "a wire map entry that is not a pair".to_string(),
                    ));
                };
                let mut pair = pair.into_iter();
                let key = pair
                    .next()
                    .ok_or_else(|| wire_error("a wire map entry without a key".to_string()))?;
                let value = pair
                    .next()
                    .ok_or_else(|| wire_error("a wire map entry without a value".to_string()))?;
                decoded.push((key, value));
            }
            Ok(CoreValue::Map(decoded))
        }
        "dayjs" => {
            let valid = matches!(map.get("valid"), Some(CoreValue::Bool(true)));
            if !valid {
                return Ok(CoreValue::DateTime(Dayjs::utc_invalid()));
            }
            let ms = number(&map, "ms")
                .ok_or_else(|| wire_error("a valid wire dayjs without ms".to_string()))?;
            let offset = number(&map, "utcOffset").unwrap_or(0.0);
            let built = Dayjs::utc_from_number(ms);
            let built = if offset == 0.0 {
                built
            } else {
                built.utc_offset_set(&UtcOffset::Number(offset))
            };
            Ok(CoreValue::DateTime(built))
        }
        "typed" => {
            let kind = match map.get("ctor") {
                Some(CoreValue::String(ctor)) if ctor == "Resource" => InstanceKind::Resource,
                Some(CoreValue::String(ctor)) if ctor == "ValidatedResource" => {
                    InstanceKind::ValidatedResource
                }
                Some(CoreValue::String(ctor)) if ctor == "Relationship" => {
                    InstanceKind::Relationship
                }
                other => {
                    let other = match other {
                        Some(CoreValue::String(ctor)) => Some(ctor.as_str()),
                        _ => None,
                    };
                    return Err(wire_error(format!("a typed wire value of class {other:?}")));
                }
            };
            let Some(CoreValue::String(fqn)) = map.swap_remove("fqn") else {
                return Err(wire_error("a typed wire value without fqn".to_string()));
            };
            let Some(CoreValue::Object(props)) = map.swap_remove("fields") else {
                return Err(wire_error("a typed wire value without fields".to_string()));
            };
            Ok(CoreValue::Instance(Box::new(Instance {
                kind,
                class_fqn: fqn,
                props,
                validator_options: ValidateOptions::default(),
            })))
        }
        other => Err(wire_error(format!(
            "a wire value of kind {other} has no engine counterpart"
        ))),
    }
}

/// [`parse_wire`] over the wire value in concerto-core's compact binary
/// layout, read as its JSON text is
/// ([`concerto_core::introspect::compact_deserialize_seed`]). Bytes not in
/// the layout, or an unrecognised wire shape, are a [`wire_error`].
pub(crate) fn parse_wire_bytes(bytes: &[u8]) -> Result<CoreValue> {
    let error = RefCell::new(None);
    let value =
        concerto_core::introspect::compact_deserialize_seed(bytes, WireSeed { error: &error })
            .map_err(|e| wire_error(format!("a binary wire document: {e}")))?;
    match error.into_inner() {
        Some(error) => Err(error),
        None => Ok(value),
    }
}

/// `decode_wire(&serde_json::from_str(text)?)` in one pass (see
/// [`WireSeed`]): malformed JSON throws a JS `SyntaxError`, and an
/// unrecognised wire shape, or one nested past serde_json's recursion limit
/// ([`wire_text_error`]), its [`wire_error`].
pub(crate) fn parse_wire(text: &str) -> Result<CoreValue> {
    use serde::de::DeserializeSeed;
    let error = RefCell::new(None);
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = WireSeed { error: &error }
        .deserialize(&mut deserializer)
        .and_then(|value| deserializer.end().map(|()| value))
        .map_err(wire_text_error)?;
    match error.into_inner() {
        Some(error) => Err(error),
        None => Ok(value),
    }
}

/// A [`CoreValue`] as [`encode_wire`]'s JSON text, without building the
/// `Value` first. With `INTS` (the compact result), an integral finite
/// number below 2^53 is written as an integer, which `JSON.parse` reads
/// faster and as the same number.
pub(crate) struct WireOut<'a, const INTS: bool = false>(pub(crate) &'a CoreValue);

/// An [`Instance`] written as [`encode_wire_instance`]'s JSON text.
pub(crate) struct WireInstanceOut<'a, const INTS: bool = false>(pub(crate) &'a Instance);

impl<const INTS: bool> serde::Serialize for WireOut<'_, INTS> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        match self.0 {
            CoreValue::Undefined => {
                let mut map = s.serialize_map(Some(1))?;
                map.serialize_entry(WIRE_TAG, "undefined")?;
                map.end()
            }
            CoreValue::Null => s.serialize_unit(),
            CoreValue::Bool(b) => s.serialize_bool(*b),
            CoreValue::Number(n) => {
                let n = *n;
                let special = if n.is_nan() {
                    Some("NaN")
                } else if n.is_infinite() {
                    Some(if n > 0.0 { "Infinity" } else { "-Infinity" })
                } else if n == 0.0 && n.is_sign_negative() {
                    Some("-0")
                } else {
                    None
                };
                match special {
                    Some(text) => {
                        let mut map = s.serialize_map(Some(2))?;
                        map.serialize_entry(WIRE_TAG, "number")?;
                        map.serialize_entry("value", text)?;
                        map.end()
                    }
                    // `n` is finite and not `-0` here, so it is exactly
                    // representable as an `i64` when it is integral and
                    // below 2^53.
                    None if INTS && n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 => {
                        s.serialize_i64(n as i64)
                    }
                    None => s.serialize_f64(n),
                }
            }
            CoreValue::String(text) => s.serialize_str(text),
            CoreValue::Array(items) => {
                let mut seq = s.serialize_seq(Some(items.len()))?;
                for item in items {
                    seq.serialize_element(&WireOut::<INTS>(item))?;
                }
                seq.end()
            }
            CoreValue::Object(entries) => {
                let mut map = s.serialize_map(Some(entries.len()))?;
                for (key, item) in entries {
                    map.serialize_entry(key, &WireOut::<INTS>(item))?;
                }
                map.end()
            }
            CoreValue::Map(entries) => {
                struct Pair<'a, const I: bool>(&'a CoreValue, &'a CoreValue);
                impl<const I: bool> serde::Serialize for Pair<'_, I> {
                    fn serialize<S: serde::Serializer>(
                        &self,
                        s: S,
                    ) -> std::result::Result<S::Ok, S::Error> {
                        let mut seq = s.serialize_seq(Some(2))?;
                        seq.serialize_element(&WireOut::<I>(self.0))?;
                        seq.serialize_element(&WireOut::<I>(self.1))?;
                        seq.end()
                    }
                }
                struct Entries<'a, const I: bool>(&'a [(CoreValue, CoreValue)]);
                impl<const I: bool> serde::Serialize for Entries<'_, I> {
                    fn serialize<S: serde::Serializer>(
                        &self,
                        s: S,
                    ) -> std::result::Result<S::Ok, S::Error> {
                        let mut seq = s.serialize_seq(Some(self.0.len()))?;
                        for (key, value) in self.0 {
                            seq.serialize_element(&Pair::<I>(key, value))?;
                        }
                        seq.end()
                    }
                }
                let mut map = s.serialize_map(Some(2))?;
                map.serialize_entry(WIRE_TAG, "map")?;
                map.serialize_entry("entries", &Entries::<INTS>(entries))?;
                map.end()
            }
            CoreValue::DateTime(d) => {
                if !d.is_valid() {
                    let mut map = s.serialize_map(Some(2))?;
                    map.serialize_entry(WIRE_TAG, "dayjs")?;
                    map.serialize_entry("valid", &false)?;
                    return map.end();
                }
                let mut map = s.serialize_map(Some(4))?;
                map.serialize_entry(WIRE_TAG, "dayjs")?;
                map.serialize_entry("valid", &true)?;
                map.serialize_entry("ms", &d.epoch_ms())?;
                map.serialize_entry("utcOffset", &d.utc_offset())?;
                map.end()
            }
            CoreValue::Instance(i) => WireInstanceOut::<INTS>(i).serialize(s),
            CoreValue::BigInt(text) => {
                let mut map = s.serialize_map(Some(2))?;
                map.serialize_entry(WIRE_TAG, "bigint")?;
                map.serialize_entry("value", text)?;
                map.end()
            }
        }
    }
}

impl<const INTS: bool> serde::Serialize for WireInstanceOut<'_, INTS> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        struct Fields<'a, const I: bool>(&'a SerializerOptions);
        impl<const I: bool> serde::Serialize for Fields<'_, I> {
            fn serialize<S: serde::Serializer>(
                &self,
                s: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                let mut map = s.serialize_map(Some(self.0.len()))?;
                for (key, value) in self.0 {
                    map.serialize_entry(key, &WireOut::<I>(value))?;
                }
                map.end()
            }
        }
        let mut map = s.serialize_map(Some(4))?;
        map.serialize_entry(WIRE_TAG, "typed")?;
        map.serialize_entry("ctor", self.0.kind.ctor())?;
        map.serialize_entry("fqn", &self.0.class_fqn)?;
        map.serialize_entry("fields", &Fields::<INTS>(&self.0.props))?;
        map.end()
    }
}

/// The own properties the view's `materializeTyped` reads by name rather
/// than copying, and which [`CompactInstanceOut`] therefore writes by
/// position instead of in its field object.
pub(crate) const COMPACT_HEADER_KEYS: [&str; 5] = [
    "$namespace",
    "$type",
    "$identifierFieldName",
    "$identifier",
    "$timestamp",
];

/// An [`Instance`] as `serializerFromJsonCompact`'s array `[ctor, fqn,
/// $namespace, $type, $identifierFieldName, $identifier, $timestamp,
/// fields]`, in wire encoding; `fields` omits those keys, `$class` and the
/// identifier field the constructor sets. Nested instances keep the
/// `"typed"` shape.
pub(crate) struct CompactInstanceOut<'a>(pub(crate) &'a Instance);

impl serde::Serialize for CompactInstanceOut<'_> {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        use serde::ser::{SerializeMap, SerializeSeq};
        const UNDEFINED: CoreValue = CoreValue::Undefined;
        struct Rest<'a>(&'a SerializerOptions, Option<&'a str>);
        impl serde::Serialize for Rest<'_> {
            fn serialize<S: serde::Serializer>(
                &self,
                s: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                let mut map = s.serialize_map(None)?;
                for (key, value) in self.0 {
                    if COMPACT_HEADER_KEYS.contains(&key.as_str())
                        || key == "$class"
                        || Some(key.as_str()) == self.1
                    {
                        continue;
                    }
                    map.serialize_entry(key, &WireOut::<true>(value))?;
                }
                map.end()
            }
        }
        let props = &self.0.props;
        // The header values, found in one pass over the properties rather
        // than one hashed lookup each.
        let mut header: [&CoreValue; 5] = [&UNDEFINED; 5];
        for (key, value) in props {
            if let Some(slot) = COMPACT_HEADER_KEYS
                .iter()
                .position(|k| k == key)
                .and_then(|i| header.get_mut(i))
            {
                *slot = value;
            }
        }
        let [_, _, identifier_field_name, _, _] = header;
        let identifier_field = match identifier_field_name {
            CoreValue::String(name) => Some(name.as_str()),
            _ => None,
        };
        let mut seq = s.serialize_seq(Some(8))?;
        seq.serialize_element(self.0.kind.ctor())?;
        seq.serialize_element(&self.0.class_fqn)?;
        for value in header {
            seq.serialize_element(&WireOut::<true>(value))?;
        }
        seq.serialize_element(&Rest(props, identifier_field))?;
        seq.end()
    }
}

/// A serializer call's merged options, read once per options text and
/// shared by `serializerFromJsonCompact`, `serializerToJson` and
/// `validateInstance`: the `Serializer` built from them, what `from_json`
/// reads of them, and what `validateInstance`'s walk reads. A call with the
/// same text reuses them ([`caches::SERIALIZER_OPTIONS`]).
pub(crate) struct SerializerOptionsEntry {
    /// The options text the entry was read from (its key).
    text: String,
    /// `new Serializer(factory, modelManager, options)`.
    pub(crate) serializer: Serializer,
    /// The merged options as `from_json` reads them: the serializer's
    /// defaults, built from the same options, so merging again changes
    /// nothing.
    pub(crate) from_json: FromJsonOptions,
    /// The merged options as `validateInstance`'s walk reads them
    /// ([`native_from_json_options`] of [`validator_options_of`]).
    pub(crate) native: FromJsonOptions,
}

impl SerializerOptionsEntry {
    /// Reads `text` (a wire encoding, module doc above "Serializer fast
    /// path", or `"null"`): malformed JSON throws a JS `SyntaxError`, an
    /// unrecognised wire shape its [`wire_error`].
    pub(crate) fn new(text: &str) -> Result<Self> {
        // Parsed once: the serializer's options and the walk's are both read
        // from the one parse.
        let wire = parse_json(text)?;
        let options = decode_wire_options_of(&wire)?;
        let serializer = Serializer::new(true, true, options.as_ref())?;
        let from_json = populator::from_json_options(&serializer.default_options);
        let native = native_from_json_options(&validator_options_of(wire, options.as_ref()));
        Ok(Self {
            text: text.to_string(),
            serializer,
            from_json,
            native,
        })
    }
}

/// Runs `body` with the [`SerializerOptionsEntry`] of `options_text`, the
/// cached one when the last call had the same text. The entry is taken out
/// of the cache for the call, so a call `body` makes back into JS that
/// reaches the serializer again (`env.newId`) builds its own.
pub(crate) fn with_serializer_options<R>(
    options_text: &str,
    body: impl FnOnce(&SerializerOptionsEntry) -> R,
) -> Result<R> {
    let cached = caches::SERIALIZER_OPTIONS.with(|slot| {
        slot.borrow_mut()
            .take()
            .filter(|entry| entry.text == options_text)
    });
    let entry = match cached {
        Some(entry) => entry,
        None => SerializerOptionsEntry::new(options_text)?,
    };
    let out = body(&entry);
    caches::SERIALIZER_OPTIONS.with(|slot| *slot.borrow_mut() = Some(entry));
    Ok(out)
}

/// A Serializer fast path document: the wire encoding's JSON text, or the
/// same wire value written by the TS binary writer (src/engine/wire.ts) in
/// concerto-core's compact layout, which reads as that text reads
/// ([`concerto_core::introspect::compact_deserialize_seed`]).
#[derive(Clone, Copy)]
pub(crate) enum WireDoc<'a> {
    Text(&'a str),
    Bytes(&'a [u8]),
}

impl WireDoc<'_> {
    /// The document as a [`CoreValue`] ([`parse_wire`], [`parse_wire_bytes`]).
    pub(crate) fn parse(self) -> Result<CoreValue> {
        match self {
            WireDoc::Text(text) => parse_wire(text),
            WireDoc::Bytes(bytes) => parse_wire_bytes(bytes),
        }
    }

    /// The document as a `concerto_core::json::Value`, as `serde_json::from_str`
    /// reads its text; `None` when it is not one.
    pub(crate) fn value(self) -> Option<Value> {
        match self {
            WireDoc::Text(text) => serde_json::from_str::<Value>(text).ok(),
            WireDoc::Bytes(bytes) => concerto_core::introspect::compact_value(bytes).ok(),
        }
    }
}

impl ModelManagerHandle {
    /// The resource `serializerFromJsonCompact` builds, in one pass each way
    /// ([`parse_wire`]), with the serializer reused while the options text is
    /// unchanged ([`with_serializer_options`]).
    pub(crate) fn build_from_json(
        &self,
        doc: WireDoc,
        options_text: &str,
        env: JsValue,
    ) -> Result<Instance> {
        let object = doc.parse()?;
        with_serializer_options(options_text, |entry| {
            let mut js_env = JsInstanceEnv { env };
            entry
                .serializer
                .from_json_prepared(&self.manager, &object, &entry.from_json, &mut js_env)
                .map_err(|err| self.instance_error(err, doc, &entry.native))
        })?
    }

    /// `serializerToJson`'s result text for the resource `doc` holds, as for
    /// `serializerFromJsonCompact`.
    pub(crate) fn to_json_text(&self, doc: WireDoc, options_text: &str) -> Result<String> {
        let resource = doc.parse()?;
        let result = with_serializer_options(options_text, |entry| {
            entry.serializer.to_json(&self.manager, &resource, None)
        })??;
        serde_json::to_string(&WireOut::<false>(&result)).map_err(internal)
    }

    /// `err`, an error `serializerFromJsonCompact` raised for `doc` with the
    /// merged `options`, with its diagnostics attached as `details`
    /// (accordproject/concerto#1325): `validateInstance`'s first diagnostic
    /// for the document ([`validator_readings_of`], [`diagnose_read`]). Read
    /// only on a failure.
    pub(crate) fn instance_error(
        &self,
        err: CoreError,
        doc: WireDoc,
        options: &FromJsonOptions,
    ) -> Error {
        match validator_readings_of(doc) {
            Some(readings) => {
                let diagnosis =
                    diagnose_read(&self.manager, None, &readings, options, false, || {
                        Err(err.clone())
                    });
                Error::Instance(
                    Box::new(err.into_contract()),
                    diagnostics_json(diagnosis.report.diagnostics()),
                )
            }
            None => err.into(),
        }
    }
}

/// TS `validateMetaModel(input)` and the other metamodel-instance checks, on
/// the engine's resident metamodel manager
/// (`concerto_core::instance::with_resident_metamodel_manager`), so TS keeps
/// no metamodel handle. Validates `json_text`, in the serializer's wire
/// encoding, as `Serializer.fromJSON` over a metamodel manager would with
/// the options `preset` names:
///
/// - `"strict"`: accordproject/concerto#1273's `STRICT_VALIDATE_OPTIONS`
///   (`validateAst`'s check);
/// - `"default"`: the manager's serializer defaults, `baseDefaultOptions`;
/// - `"serializer"`: a `new Serializer(factory, modelManager)`'s defaults
///   (`validateMetaModel`'s), the same as `"default"`.
///
/// Throws what `validateInstance` (mode 0) throws, with its diagnostics,
/// without building a resource (`validateAst`'s `MetamodelException`
/// wrapping is its caller's); an unknown `preset` is a plain `Error`.
#[wasm_bindgen(js_name = validateMetaModelInstance)]
pub fn validate_meta_model_instance(json_text: &str, preset: &str) -> JsResult<()> {
    run(|| {
        validate_meta_model_wire(preset, || {
            serde_json::from_str::<Value>(json_text).map_err(wire_text_error)
        })
    })
}

/// [`validate_meta_model_instance`] with the wire document in the compact
/// binary layout the TS writer (src/engine/wire.ts) writes from the live
/// object, as `validateInstanceBytes` takes it: the same result and errors
/// as its JSON text. Bytes not in the layout are a fast path fallback
/// (`fastPathUnsupported`).
#[wasm_bindgen(js_name = validateMetaModelInstanceBytes)]
pub fn validate_meta_model_instance_bytes(bytes: &[u8], preset: &str) -> JsResult<()> {
    run(|| {
        validate_meta_model_wire(preset, || {
            concerto_core::introspect::compact_value(bytes)
                .map_err(|e| wire_error(format!("a binary wire document: {e}")))
        })
    })
}

/// The body of [`validate_meta_model_instance`] and
/// [`validate_meta_model_instance_bytes`]: the preset is read before the
/// document (`read_wire`).
fn validate_meta_model_wire(preset: &str, read_wire: impl FnOnce() -> Result<Value>) -> Result<()> {
    use concerto_core::instance::{MetaModelPreset, with_resident_metamodel_manager};
    let preset = match preset {
        "strict" => MetaModelPreset::Strict,
        "default" => MetaModelPreset::Default,
        "serializer" => MetaModelPreset::Serializer,
        other => {
            return Err(CoreError::from(ContractError::pre_port(
                ErrorKind::InvalidArgument,
                format!("unknown metamodel preset: {other}"),
                None,
            ))
            .into());
        }
    };
    let wire = read_wire()?;
    let options = preset.from_json_options();
    let serializer = Serializer::new(true, true, None)?;
    let mut outcome = Ok(String::new());
    with_resident_metamodel_manager(|mm| {
        outcome = validate_wire(mm, &wire, &serializer, &options, &options, None, 0);
        Ok(())
    })?;
    outcome.map(|_| ())
}

/// The document `doc` (a wire encoding, module doc above "Serializer
/// fast path") as the diagnostics walk reads it: plain JSON as itself, and a
/// document with a wire tag (an `undefined` field, `-0`, `NaN`, a `Map`, a
/// dayjs, ...) decoded as `serializerFromJsonCompact` decodes it, in the
/// validator's tagged form ([`validator_readings`]). `None` when it is not
/// a wire encoding.
pub(crate) fn validator_readings_of(doc: WireDoc) -> Option<Vec<Value>> {
    let wire = doc.value()?;
    if !has_wire_tag(&wire) {
        return Some(vec![wire]);
    }
    decode_wire(&wire).ok().map(|v| validator_readings(&v))
}

/// A decoded document in the validator's tagged form
/// (`JsValue::to_validator_value`), both ways [`diagnose_read`] reads it: an
/// `undefined` field of a plain object left out (as `JSON.stringify` and
/// TS's `obj.x === undefined` see it), and then kept as its tag (as
/// `Object.keys`, so a `rejectUnknownKeys` check, sees it). The second is
/// only given when the document has such a field.
pub(crate) fn validator_readings(document: &CoreValue) -> Vec<Value> {
    let mut has_undefined_field = false;
    let left_out = without_undefined_fields(document, &mut has_undefined_field);
    let mut readings = vec![left_out.to_validator_value()];
    if has_undefined_field {
        readings.push(document.to_validator_value());
    }
    readings
}

/// `value` with every `undefined` field of a plain object left out, at any
/// depth (array items and the values of a `Map` or an instance are kept as
/// they are); `found` is set when there was one.
pub(crate) fn without_undefined_fields(value: &CoreValue, found: &mut bool) -> CoreValue {
    match value {
        CoreValue::Object(map) => {
            let mut out = JsObject::default();
            for (key, item) in map {
                if matches!(item, CoreValue::Undefined) {
                    *found = true;
                } else {
                    out.insert(key.clone(), without_undefined_fields(item, found));
                }
            }
            CoreValue::Object(out)
        }
        CoreValue::Array(items) => CoreValue::Array(
            items
                .iter()
                .map(|item| without_undefined_fields(item, found))
                .collect(),
        ),
        other => other.clone(),
    }
}

/// The merged options, as the parsed wire encoding `wire` whose decoding is
/// `decoded`, as the plain JSON [`native_from_json_options`] reads:
/// `wire` itself when it holds no wire-tagged value, else each decoded
/// option, an `undefined` one left out, as `Serializer.fromJSON` reads it
/// (`options.x` is `undefined` either way).
fn validator_options_of(wire: Value, decoded: Option<&SerializerOptions>) -> Value {
    if !has_wire_tag(&wire) {
        return wire;
    }
    Value::Object(
        decoded
            .into_iter()
            .flatten()
            .filter(|(_, v)| !matches!(v, CoreValue::Undefined))
            .map(|(k, v)| (k.clone(), v.to_validator_value()))
            .collect(),
    )
}

/// The document `Serializer.fromJSON` is given for the type `fqn` (the TS
/// `validateInstance` layer's `withClass`): an object without a truthy
/// `$class` gets `fqn` as its `$class`, first, as
/// `Object.assign({ $class: fqn }, object)` builds it.
pub(crate) fn with_class(object: CoreValue, fqn: Option<&str>) -> CoreValue {
    match (fqn, object) {
        (Some(fqn), CoreValue::Object(map))
            if !map.get("$class").is_some_and(CoreValue::is_truthy) =>
        {
            let mut out = JsObject::default();
            out.insert("$class".to_string(), CoreValue::String(fqn.to_string()));
            for (key, value) in map {
                out.insert(key, value);
            }
            CoreValue::Object(out)
        }
        (_, object) => object,
    }
}

/// The environment `validateInstance`'s engine read runs in: a fixed
/// identifier and clock, the same as the native walk's (no verdict depends
/// on either; they appear only in some message texts).
pub(crate) struct ValidationEnv;

impl InstanceEnv for ValidationEnv {
    fn new_id(&mut self) -> String {
        "00000000-0000-4000-8000-000000000000".into()
    }

    fn now_ms(&mut self) -> f64 {
        0.0
    }
}

/// Whether `value` holds a wire-tagged value (module doc above "Serializer
/// fast path"): one JSON cannot hold, which the walk reads only once
/// decoded ([`validator_readings`]).
pub(crate) fn has_wire_tag(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.contains_key(WIRE_TAG) || map.values().any(has_wire_tag),
        Value::Array(items) => items.iter().any(has_wire_tag),
        _ => false,
    }
}
