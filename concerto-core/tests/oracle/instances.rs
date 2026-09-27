//! The instance-layer ops of task P3-01b (`accordproject/concerto-rust#124`):
//! `Serializer.new`/`fromJSON`/`toJSON`, `Factory.newResource`/`newConcept`/
//! `newRelationship`/`newTransaction`/`newEvent`, and the members that
//! change or serialize a `Resource` (`Resource.setPropertyValue`,
//! `addArrayValue`, `toJSON`, `Identifiable.setIdentifier`), replayed over
//! `concerto_core::instance`.
//!
//! It also holds the oracle's encoding of instances in both directions:
//! [`decode_instance`] reads an input `"typed"` node (`codec.js`
//! `encTyped`: the class, the handles and every own property in order) into
//! an [`Instance`], and [`encode_instance`] writes an output one (`codec.js`
//! `encodeOut`: `{ctor, fqn, ns, type, id, timestamp, fields}`, `fields`
//! being every own property but `TYPED_INTERNAL`). `Relationship.fromURI`
//! (`ops.rs`) writes its result through the same encoder.
//!
//! A mutating op records the receiver after the call as `effects.target`
//! (README "Fixture schema"), whether the call returned or threw.
//!
//! `Factory` calls with a truthy `options.generate` run `InstanceGenerator`
//! with a sample or empty value generator, which the ledger keeps in TS
//! (D7, "Factory generate path"): those fixtures are blocked on
//! `InstanceGenerator.visit`, owned `stays-ts`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use concerto_core::error::ConcertoError;
use concerto_core::instance::dayjs::Dayjs;
use concerto_core::instance::{
    Instance, InstanceEnv, InstanceKind, JsValue, Serializer, SerializerOptions, ValidateOptions,
    factory, resource,
};
use serde_json::{Map, Value, json};

use super::Harness;
use super::fixture::Inputs;
use super::ops::{Dispatch, to_oracle_error};
use super::recipe::{self, Fault, Faulty, M, Session};

/// The ops this module dispatches.
pub fn handles(class: &str, member: &str) -> bool {
    matches!(
        (class, member),
        ("Serializer", "new" | "fromJSON" | "toJSON")
            | (
                "Factory",
                "newResource" | "newConcept" | "newRelationship" | "newTransaction" | "newEvent"
            )
            | ("Resource", "setPropertyValue" | "addArrayValue" | "toJSON")
            | ("Identifiable", "setIdentifier")
    )
}

/// The clock and identifiers TS takes from `dayjs.utc()` and `uuid.v4()`.
/// The fixtures record them canonicalised (`<now>`, `<uuid>`), and the
/// judge canonicalises this run's the same way (`compare.rs`).
pub struct HarnessEnv;

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Milliseconds since the epoch, now.
pub fn now_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_millis() as f64)
}

impl InstanceEnv for HarnessEnv {
    fn new_id(&mut self) -> String {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let t = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos() as u64);
        let a = t ^ n.rotate_left(32);
        format!(
            "{:08x}-{:04x}-4{:03x}-8{:03x}-{:012x}",
            (a >> 32) as u32,
            (a >> 16) as u16,
            (a & 0xfff) as u16,
            (n & 0xfff) as u16,
            (t ^ 0x5DEE_CE66_D00D) & 0xffff_ffff_ffff
        )
    }

    fn now_ms(&mut self) -> f64 {
        now_ms()
    }
}

// ---------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------

/// The JS number an oracle `{"@@oracle":"number"}` node stands for.
fn special_number(v: &Value) -> Result<f64, String> {
    match v.get("value").and_then(Value::as_str) {
        Some("NaN") => Ok(f64::NAN),
        Some("Infinity") => Ok(f64::INFINITY),
        Some("-Infinity") => Ok(f64::NEG_INFINITY),
        Some("-0") => Ok(-0.0),
        other => Err(format!("an encoded number {other:?}")),
    }
}

/// An oracle input value as the [`JsValue`] it decodes to (`codec.js`
/// `decode`): plain JSON, and the `undefined`, `number`, `bigint`, `dayjs`,
/// `map` and `typed` kinds. Anything else has no Rust counterpart.
pub fn js_value(v: &Value) -> Result<JsValue, String> {
    match v {
        Value::Array(items) => items
            .iter()
            .map(js_value)
            .collect::<Result<_, _>>()
            .map(JsValue::Array),
        Value::Object(map) => match map.get(M).and_then(Value::as_str) {
            None => map
                .iter()
                .map(|(k, x)| Ok((k.clone(), js_value(x)?)))
                .collect::<Result<_, String>>()
                .map(JsValue::Object),
            Some("undefined") => Ok(JsValue::Undefined),
            Some("number") => special_number(v).map(JsValue::Number),
            Some("bigint") => map
                .get("value")
                .and_then(Value::as_str)
                .map(|s| JsValue::BigInt(s.to_string()))
                .ok_or_else(|| "a bigint without value".to_string()),
            Some("dayjs") => {
                let valid = map.get("valid").and_then(Value::as_bool).unwrap_or(false);
                let iso = map.get("iso").and_then(Value::as_str);
                let offset = map.get("offset").and_then(Value::as_f64).unwrap_or(0.0);
                let utc = map.get("utc").and_then(Value::as_bool).unwrap_or(false);
                Ok(JsValue::DateTime(Dayjs::from_recorded(
                    valid, iso, offset, utc,
                )))
            }
            Some("map") => {
                let entries = map
                    .get("entries")
                    .and_then(Value::as_array)
                    .ok_or("a map without entries")?;
                entries
                    .iter()
                    .map(|e| {
                        let pair = e.as_array().ok_or("a map entry that is not a pair")?;
                        let key = js_value(pair.first().unwrap_or(&Value::Null))?;
                        let value = js_value(pair.get(1).unwrap_or(&Value::Null))?;
                        Ok((key, value))
                    })
                    .collect::<Result<_, String>>()
                    .map(JsValue::Map)
            }
            Some("typed") => decode_instance(v).map(|i| JsValue::Instance(Box::new(i))),
            Some(other) => Err(format!("a value of kind {other} has no Rust counterpart")),
        },
        other => Ok(JsValue::from_json(other)),
    }
}

/// A `"typed"` node's class declaration name: its `declref`'s namespace
/// and name, or the instance's own `$namespace` and `$type`.
fn typed_class_fqn(v: &Value, fields: &Map<String, Value>) -> String {
    let decl = v.get("decl");
    let name = decl.and_then(|d| d.get("name")).and_then(Value::as_str);
    let ns = decl.and_then(|d| d.get("mf")).and_then(|mf| {
        mf.get("ns").and_then(Value::as_str).or_else(|| {
            mf.get("ast")
                .and_then(|a| a.get("namespace"))
                .and_then(Value::as_str)
        })
    });
    match (ns, name) {
        (Some(ns), Some(name)) => format!("{ns}.{name}"),
        _ => format!(
            "{}.{}",
            fields
                .get("$namespace")
                .and_then(Value::as_str)
                .unwrap_or_default(),
            fields
                .get("$type")
                .and_then(Value::as_str)
                .unwrap_or_default()
        ),
    }
}

/// Decodes an input `"typed"` node into an [`Instance`] (module doc).
pub fn decode_instance(v: &Value) -> Result<Instance, String> {
    let kind = match v.get("ctor").and_then(Value::as_str) {
        Some("Resource") => InstanceKind::Resource,
        Some("ValidatedResource") => InstanceKind::ValidatedResource,
        Some("Relationship") => InstanceKind::Relationship,
        other => return Err(format!("a typed value of class {other:?}")),
    };
    let fields = v
        .get("fields")
        .and_then(Value::as_object)
        .ok_or("a typed value without fields")?;
    let keys = v
        .get("keys")
        .and_then(Value::as_array)
        .ok_or("a typed value without keys")?;
    let mut props = indexmap::IndexMap::new();
    for key in keys {
        let key = key.as_str().ok_or("a typed key that is not a string")?;
        if matches!(key, "$modelManager" | "$classDeclaration" | "$validator") {
            continue;
        }
        let value = fields.get(key).map_or(Ok(JsValue::Undefined), js_value)?;
        props.insert(key.to_string(), value);
    }
    let validator_options = v
        .get("validatorOptions")
        .map_or_else(ValidateOptions::default, |o| ValidateOptions {
            convert_resources_to_relationships: o
                .get("convertResourcesToRelationships")
                .is_some_and(recipe::truthy),
            permit_resources_for_relationships: o
                .get("permitResourcesForRelationships")
                .is_some_and(recipe::truthy),
        });
    Ok(Instance {
        kind,
        class_fqn: typed_class_fqn(v, fields),
        props,
        validator_options,
    })
}

/// An input value as a [`JsValue`]: a `"typed"` node's model manager is
/// replayed into the session (so that later `mmref`s resolve), and the
/// instance decoded with [`decode_instance`]; anything else through
/// [`js_value`].
fn decode_value(session: &mut Session, v: &Value) -> Faulty<JsValue> {
    if v.get(M).and_then(Value::as_str) == Some("typed") {
        let mm = v
            .get("mm")
            .ok_or_else(|| Fault::Harness("typed value without mm".into()))?;
        session.mm_index(mm)?;
        return decode_instance(v)
            .map(|i| JsValue::Instance(Box::new(i)))
            .map_err(Fault::Unsupported);
    }
    js_value(v).map_err(Fault::Unsupported)
}

/// The decoded arguments, `undefined` past the end.
fn decode_args(session: &mut Session, inputs: &Inputs) -> Faulty<Vec<JsValue>> {
    inputs
        .args
        .iter()
        .map(|a| decode_value(session, a))
        .collect()
}

fn nth(args: &[JsValue], i: usize) -> JsValue {
    args.get(i).cloned().unwrap_or(JsValue::Undefined)
}

/// A JS options object (`undefined`/`null`/`false` give `None`, as `options
/// ?` does), or unsupported for any other non-object.
fn options_object(value: &JsValue) -> Faulty<Option<SerializerOptions>> {
    match value {
        v if !v.is_truthy() => Ok(None),
        JsValue::Object(map) => Ok(Some(map.clone())),
        _ => Err(Fault::Unsupported(
            "serializer options that are not a plain object".into(),
        )),
    }
}

// ---------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------

/// `codec.js` `TYPED_INTERNAL`: the properties a typed summary leaves out
/// of `fields`.
const TYPED_INTERNAL: [&str; 8] = [
    "$modelManager",
    "$classDeclaration",
    "$namespace",
    "$type",
    "$identifierFieldName",
    "$identifier",
    "$timestamp",
    "$validator",
];

/// A JS number in the oracle's encoding.
fn encode_number(n: f64) -> Value {
    if !n.is_finite() {
        let text = if n.is_nan() {
            "NaN"
        } else if n > 0.0 {
            "Infinity"
        } else {
            "-Infinity"
        };
        return json!({ M: "number", "value": text });
    }
    if n == 0.0 && n.is_sign_negative() {
        return json!({ M: "number", "value": "-0" });
    }
    serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
}

/// A [`JsValue`] in the oracle's output encoding (`codec.js` `encodeOut`).
pub fn encode(v: &JsValue) -> Value {
    match v {
        JsValue::Undefined => recipe::undefined(),
        JsValue::Null => Value::Null,
        JsValue::Bool(b) => Value::Bool(*b),
        JsValue::Number(n) => encode_number(*n),
        JsValue::String(s) => Value::String(s.clone()),
        JsValue::Array(items) => Value::Array(items.iter().map(encode).collect()),
        JsValue::Object(map) => {
            Value::Object(map.iter().map(|(k, x)| (k.clone(), encode(x))).collect())
        }
        JsValue::Map(entries) => json!({
            M: "map",
            "entries": entries.iter().map(|(k, x)| json!([encode(k), encode(x)])).collect::<Vec<_>>(),
        }),
        JsValue::DateTime(d) => {
            let valid = d.is_valid();
            json!({
                M: "dayjs",
                "valid": valid,
                "iso": if valid { d.to_iso_string().map_or(Value::Null, Value::String) } else { Value::Null },
                "offset": if valid { encode_offset(d.utc_offset()) } else { Value::Null },
                "utc": d.is_utc(),
            })
        }
        JsValue::Instance(i) => encode_instance(i),
        JsValue::BigInt(s) => json!({ M: "bigint", "value": s }),
    }
}

/// A dayjs `utcOffset()` as `JSON.stringify` writes it (`-0` as `0`).
fn encode_offset(offset: f64) -> Value {
    serde_json::Number::from_f64(offset + 0.0).map_or(Value::Null, Value::Number)
}

/// An [`Instance`] as the oracle's output `"typed"` summary (module doc).
pub fn encode_instance(i: &Instance) -> Value {
    let fields: Map<String, Value> = i
        .props
        .iter()
        .filter(|(k, _)| !TYPED_INTERNAL.contains(&k.as_str()))
        .map(|(k, v)| (k.clone(), encode(v)))
        .collect();
    json!({
        M: "typed",
        "ctor": i.kind.ctor(),
        "fqn": i.class_fqn,
        "ns": encode(i.get("$namespace")),
        "type": encode(i.get("$type")),
        "id": encode(i.get("$identifier")),
        "timestamp": encode(i.get("$timestamp")),
        "fields": fields,
    })
}

// ---------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------

fn outcome(result: Result<Value, ConcertoError>) -> Value {
    match result {
        Ok(value) => json!({ "ok": value }),
        Err(e) => json!({ "error": to_oracle_error(&e).to_value() }),
    }
}

/// Replays one of [`handles`]'s ops.
pub fn exec(h: &Harness, class: &str, member: &str, inputs: &Inputs) -> Faulty<Dispatch> {
    let mut session = Session::new(h);
    match class {
        "Serializer" => serializer_op(&mut session, member, inputs),
        "Factory" => factory_op(&mut session, member, inputs),
        _ => instance_op(&mut session, class, member, inputs),
    }
}

/// A `factory` node's model manager, as a pool index.
fn factory_mm(session: &mut Session, node: &Value) -> Faulty<usize> {
    let mm = node
        .get("mm")
        .ok_or_else(|| Fault::Harness("a factory without a model manager".into()))?;
    session.mm_index(mm)
}

/// A `serializer` node: its model manager, and the `Serializer` rebuilt
/// with its recorded default options (`codec.js`: `new Serializer(factory,
/// mm, defaultOptions)`).
fn decode_serializer(session: &mut Session, node: &Value) -> Faulty<(usize, Serializer)> {
    let mm_node = node
        .get("mm")
        .ok_or_else(|| Fault::Harness("a serializer without a model manager".into()))?;
    let mm = session.mm_index(mm_node)?;
    let factory = node
        .get("factory")
        .ok_or_else(|| Fault::Harness("a serializer without a factory".into()))?;
    if factory_mm(session, factory)? != mm {
        return Err(Fault::Unsupported(
            "a serializer whose factory has another model manager".into(),
        ));
    }
    let options =
        js_value(node.get("defaultOptions").unwrap_or(&Value::Null)).map_err(Fault::Unsupported)?;
    let options = options_object(&options)?;
    let serializer = Serializer::new(true, true, options.as_ref())
        .map_err(|e| Fault::Harness(format!("the recorded serializer did not rebuild: {e:?}")))?;
    Ok((mm, serializer))
}

fn serializer_op(session: &mut Session, member: &str, inputs: &Inputs) -> Faulty<Dispatch> {
    if member == "new" {
        // `new Serializer(factory, modelManager, options)`: the two handles
        // are checked for truthiness, so a recorded one is replayed (a step
        // that diverges is a failure) and anything else is plain data.
        let truthy_handle = |session: &mut Session, v: Option<&Value>| -> Faulty<bool> {
            let Some(v) = v else { return Ok(false) };
            match v.get(M).and_then(Value::as_str) {
                Some("factory") => factory_mm(session, v).map(|_| true),
                Some("mm" | "mmref") => session.mm_index(v).map(|_| true),
                _ => Ok(js_value(v).map_err(Fault::Unsupported)?.is_truthy()),
            }
        };
        let factory = truthy_handle(session, inputs.args.first())?;
        let mm = truthy_handle(session, inputs.args.get(1))?;
        let options = match inputs.args.get(2) {
            Some(v) => options_object(&js_value(v).map_err(Fault::Unsupported)?)?,
            None => None,
        };
        return Ok(Dispatch::Ran(outcome(
            Serializer::new(factory, mm, options.as_ref())
                .map(|_| json!({ M: "object", "ctor": "Serializer" })),
        )));
    }
    let target = inputs
        .target
        .as_ref()
        .ok_or_else(|| Fault::Harness("a Serializer op without a receiver".into()))?;
    let (mm_index, serializer) = decode_serializer(session, target)?;
    let args = decode_args(session, inputs)?;
    let value = nth(&args, 0);
    let options = options_object(&nth(&args, 1))?;
    let mm = &session.pool[mm_index].mm;
    Ok(Dispatch::Ran(match member {
        "fromJSON" => outcome(
            serializer
                .from_json(mm, &value, options.as_ref(), &mut HarnessEnv)
                .map(|i| encode_instance(&i)),
        ),
        "toJSON" => outcome(
            serializer
                .to_json(mm, &value, options.as_ref())
                .map(|v| encode(&v)),
        ),
        _ => unreachable!("handles lists every Serializer member"),
    }))
}

/// A `Factory` argument that must be a string (a namespace or a type name).
fn string_arg(value: &JsValue) -> Faulty<String> {
    value
        .as_str()
        .map(str::to_string)
        .ok_or_else(|| Fault::Unsupported("a namespace or type name that is not a string".into()))
}

fn factory_op(session: &mut Session, member: &str, inputs: &Inputs) -> Faulty<Dispatch> {
    let target = inputs
        .target
        .as_ref()
        .ok_or_else(|| Fault::Harness("a Factory op without a receiver".into()))?;
    let mm_index = factory_mm(session, target)?;
    let args = decode_args(session, inputs)?;
    let (ns, type_name, id) = (nth(&args, 0), nth(&args, 1), nth(&args, 2));
    // `options = options || {}`, then `options.generate` and
    // `options.disableValidation`.
    let options = nth(&args, 3);
    let option = |key: &str| -> Faulty<JsValue> {
        match &options {
            v if !v.is_truthy() => Ok(JsValue::Undefined),
            JsValue::Object(map) => Ok(map.get(key).cloned().unwrap_or(JsValue::Undefined)),
            _ => Err(Fault::Unsupported(
                "factory options that are not a plain object".into(),
            )),
        }
    };
    if member != "newRelationship" && option("generate")?.is_truthy() {
        return Err(Fault::Blocked(
            "options.generate runs InstanceGenerator, which the ledger keeps in TS (D7)".into(),
            recipe::Blocker::Member("InstanceGenerator.visit".into()),
        ));
    }
    let disable_validation = option("disableValidation")?.is_truthy();
    let mm = &session.pool[mm_index].mm;
    let mut env = HarnessEnv;
    let result = match member {
        "newResource" | "newConcept" => factory::new_resource(
            mm,
            &string_arg(&ns)?,
            &string_arg(&type_name)?,
            id,
            disable_validation,
            &mut env,
        ),
        "newRelationship" => {
            factory::new_relationship(mm, &string_arg(&ns)?, &string_arg(&type_name)?, id)
        }
        "newTransaction" => {
            if ns.is_truthy() && type_name.is_truthy() {
                string_arg(&ns)?;
                string_arg(&type_name)?;
            }
            factory::new_transaction(mm, &ns, &type_name, id, disable_validation, &mut env)
        }
        "newEvent" => {
            if ns.is_truthy() && type_name.is_truthy() {
                string_arg(&ns)?;
                string_arg(&type_name)?;
            }
            factory::new_event(mm, &ns, &type_name, id, disable_validation, &mut env)
        }
        _ => unreachable!("handles lists every Factory member"),
    };
    Ok(Dispatch::Ran(outcome(result.map(|i| encode_instance(&i)))))
}

fn instance_op(
    session: &mut Session,
    class: &str,
    member: &str,
    inputs: &Inputs,
) -> Faulty<Dispatch> {
    let target = inputs
        .target
        .as_ref()
        .ok_or_else(|| Fault::Harness(format!("{class}.{member} without a receiver")))?;
    if target.get(M).and_then(Value::as_str) != Some("typed") {
        return Err(Fault::Unsupported(format!(
            "{class}.{member} with a receiver that is not a typed instance"
        )));
    }
    let mm_node = target
        .get("mm")
        .ok_or_else(|| Fault::Harness("typed value without mm".into()))?;
    let mm_index = session.mm_index(mm_node)?;
    let mut receiver = decode_instance(target).map_err(Fault::Unsupported)?;
    let args = decode_args(session, inputs)?;
    let r = &session.pool[mm_index];
    let mm = &r.mm;
    let with_effects = |result: Result<Value, ConcertoError>, receiver: &Instance| {
        let mut outcome = outcome(result);
        outcome["effects"] = json!({ "target": encode_instance(receiver) });
        Dispatch::Ran(outcome)
    };
    match (class, member) {
        ("Resource", "setPropertyValue" | "addArrayValue") => {
            let name = nth(&args, 0);
            let Some(name) = name.as_str().map(str::to_string) else {
                return Err(Fault::Unsupported(
                    "a property name that is not a string".into(),
                ));
            };
            let value = nth(&args, 1);
            let result = if member == "setPropertyValue" {
                resource::set_property_value(mm, &mut receiver, &name, value)
            } else {
                resource::add_array_value(mm, &mut receiver, &name, value)
            };
            Ok(with_effects(
                result.map(|()| recipe::undefined()),
                &receiver,
            ))
        }
        ("Identifiable", "setIdentifier") => {
            receiver.set_identifier(nth(&args, 0));
            Ok(with_effects(Ok(recipe::undefined()), &receiver))
        }
        ("Resource", "toJSON") => {
            // `this.getModelManager().getSerializer()`: the model manager's
            // own serializer, built with its options.
            let options = js_value(&r.options).map_err(Fault::Unsupported)?;
            let options = options_object(&options)?;
            let serializer = Serializer::new(true, true, options.as_ref())
                .map_err(|e| Fault::Harness(format!("the model manager's serializer: {e:?}")))?;
            Ok(Dispatch::Ran(outcome(
                resource::to_json(mm, &receiver, &serializer).map(|v| encode(&v)),
            )))
        }
        _ => unreachable!("handles lists every instance member"),
    }
}
