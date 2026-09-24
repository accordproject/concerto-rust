//! Decodes fixture argument values (`migration/oracle/lib/codec.js`'s value
//! encoding, README "Value encoding") into the plain Rust values an op
//! implementation takes.
//!
//! Only the encodings the ops in `ops.rs` actually need are decoded here.
//! Every other `"@@oracle"` kind (`mm`, `mmref`, `mfref`, `mfnew`, `typed`,
//! a `declref`/`propref`/… and so on — the receiver-and-collaborator shapes
//! a `ModelManager`/introspection op needs) is reported [`Unsupported`]
//! rather than guessed at: a later task that ports those op families adds
//! their decoding here.

use serde_json::Value;

/// One decoded fixture value: a plain JSON value, or the JS `undefined` the
/// oracle cannot represent as JSON (README: `{"@@oracle":"undefined"}`).
#[derive(Debug, Clone)]
pub enum Decoded {
    Undefined,
    Value(Value),
}

/// A value this harness does not (yet) know how to decode or an op does not
/// (yet) know how to consume. Never a verdict on the Rust engine itself —
/// see `report.rs`, which counts these as `unsupported`, distinct from
/// `fail`.
#[derive(Debug, Clone)]
pub struct Unsupported(pub String);

/// Decodes one fixture value.
pub fn decode(v: &Value) -> Result<Decoded, Unsupported> {
    if let Value::Object(map) = v
        && let Some(Value::String(kind)) = map.get("@@oracle")
    {
        return match kind.as_str() {
            "undefined" => Ok(Decoded::Undefined),
            other => Err(Unsupported(format!("@@oracle:{other} not decoded"))),
        };
    }
    Ok(Decoded::Value(v.clone()))
}

/// Decodes every element of `inputs.args`. Fails on the first value this
/// harness cannot decode at all (as opposed to a value a particular op
/// cannot use — see [`as_str`] and friends, called after this).
pub fn decode_args(args: &[Value]) -> Result<Vec<Decoded>, Unsupported> {
    args.iter().map(decode).collect()
}

/// The argument at `index`, or JS `undefined` when the fixture recorded
/// fewer arguments than the op signature has (an omitted optional
/// argument).
pub fn arg(args: &[Decoded], index: usize) -> Decoded {
    args.get(index).cloned().unwrap_or(Decoded::Undefined)
}

/// A required, non-nullable string argument.
pub fn as_str(d: &Decoded) -> Result<&str, Unsupported> {
    match d {
        Decoded::Value(Value::String(s)) => Ok(s),
        _ => Err(Unsupported("expected a string argument".into())),
    }
}

/// A `string | null | undefined` argument, both nullish spellings mapped to
/// `None` as the Rust ports of `ModelUtil` do (their doc comments: "`None`
/// (JS `undefined` or `null`)").
pub fn as_nullable_str(d: &Decoded) -> Result<Option<&str>, Unsupported> {
    match d {
        Decoded::Undefined => Ok(None),
        Decoded::Value(Value::Null) => Ok(None),
        Decoded::Value(Value::String(s)) => Ok(Some(s.as_str())),
        _ => Err(Unsupported("expected a nullable string argument".into())),
    }
}

/// A plain-object-or-array-or-primitive argument, nullish values mapped to
/// `None` (an AST node argument such as `ModelUtil.isValidMapKey`'s `key`).
pub fn as_value(d: &Decoded) -> Option<&Value> {
    match d {
        Decoded::Undefined => None,
        Decoded::Value(Value::Null) => None,
        Decoded::Value(v) => Some(v),
    }
}

/// `ModelUtil.parseNamespace`'s second argument,
/// `options?: {disableVersionParsing?: boolean}`.
pub fn disable_version_parsing(d: Option<&Decoded>) -> Result<bool, Unsupported> {
    match d {
        None | Some(Decoded::Undefined) => Ok(false),
        Some(Decoded::Value(Value::Null)) => Ok(false),
        Some(Decoded::Value(Value::Object(m))) => Ok(m
            .get("disableVersionParsing")
            .and_then(Value::as_bool)
            .unwrap_or(false)),
        _ => Err(Unsupported(
            "expected a parseNamespace options object".into(),
        )),
    }
}
