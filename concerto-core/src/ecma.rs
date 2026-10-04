//! Crate-private helpers for the ECMAScript semantics the TypeScript
//! reference relies on (PORTING.md section 3.1).
//!
//! The model ASTs reach Rust as `crate::json::Value`, so these helpers take
//! JSON values. A JSON value can hold every JS value a model AST holds,
//! except `undefined` (an absent key, modelled as `Option::None` by callers)
//! and the non-finite numbers (which neither the CTO parser nor `JSON.parse`
//! produce).

use std::cmp::Ordering;
use std::sync::LazyLock;

use crate::json::Value;

/// ECMAScript `Number::toString` (radix 10): `1` not `1.0`, `1e+21`, `NaN`,
/// `Infinity`, and `-0` gives `"0"`.
pub(crate) fn number_to_string(n: f64) -> String {
    let mut buffer = ryu_js::Buffer::new();
    buffer.format(n).to_string()
}

/// ECMAScript `ToString` of a JSON value, as a template literal or string
/// concatenation applies it: strings as they are, numbers through
/// [`number_to_string`], `null` as `"null"`, arrays joined with `,` (a `null`
/// element gives the empty string) and plain objects as `"[object Object]"`.
pub(crate) fn to_js_string(value: &Value) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => number_to_string(json_number(n)),
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::Null => String::new(),
                other => to_js_string(other),
            })
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_string(),
    }
}

/// A JSON number as the JS number `JSON.parse` would have produced. serde_json
/// keeps integers above 2^53 exactly; JS has already rounded them, and
/// `as_f64` rounds the same way.
fn json_number(n: &serde_json::Number) -> f64 {
    n.as_f64().unwrap_or(f64::NAN)
}

/// The primitive a relational comparison sees after `ToPrimitive` (hint
/// number). A JSON array or object has no `valueOf` override, so it becomes
/// its `ToString` text.
enum Primitive {
    Number(f64),
    String(String),
}

impl Primitive {
    fn of(value: &Value) -> Self {
        match value {
            Value::Null => Self::Number(0.0),
            Value::Bool(b) => Self::Number(if *b { 1.0 } else { 0.0 }),
            Value::Number(n) => Self::Number(json_number(n)),
            Value::String(s) => Self::String(s.clone()),
            Value::Array(_) | Value::Object(_) => Self::String(to_js_string(value)),
        }
    }

    fn to_number(&self) -> f64 {
        match self {
            Self::Number(n) => *n,
            Self::String(s) => string_to_number(s),
        }
    }
}

/// ECMAScript `ToNumber` of a JSON value: a number as itself, `null` as `0`,
/// a boolean as `1`/`0`, a string through [`string_to_number`], and an array
/// or object through its `ToString` text. For a caller that needs the number
/// itself, such as an unchecked numeric validator bound (DV-002).
#[cfg(feature = "js-compat")]
pub(crate) fn to_number(value: &Value) -> f64 {
    Primitive::of(value).to_number()
}

/// ECMAScript `a < b` (IsLessThan, left first). Two strings compare by UTF-16
/// code units; otherwise both sides go through `ToNumber`, and a `NaN` on
/// either side makes the comparison false.
pub(crate) fn less_than(a: &Value, b: &Value) -> bool {
    compare(Primitive::of(a), Primitive::of(b)) == Some(Ordering::Less)
}

/// ECMAScript `a > b`, which is IsLessThan with the operands swapped.
pub(crate) fn greater_than(a: &Value, b: &Value) -> bool {
    compare(Primitive::of(a), Primitive::of(b)) == Some(Ordering::Greater)
}

/// `a < b` where `a` is a JS number (an instance value) and `b` a JSON value.
pub(crate) fn number_less_than(a: f64, b: &Value) -> bool {
    compare(Primitive::Number(a), Primitive::of(b)) == Some(Ordering::Less)
}

/// `a > b` where `a` is a JS number (an instance value) and `b` a JSON value.
pub(crate) fn number_greater_than(a: f64, b: &Value) -> bool {
    compare(Primitive::Number(a), Primitive::of(b)) == Some(Ordering::Greater)
}

fn compare(a: Primitive, b: Primitive) -> Option<Ordering> {
    match (&a, &b) {
        (Primitive::String(x), Primitive::String(y)) => {
            Some(x.encode_utf16().cmp(y.encode_utf16()))
        }
        _ => a.to_number().partial_cmp(&b.to_number()),
    }
}

/// The characters JS `String.prototype.trim` removes: WhiteSpace and
/// LineTerminator. This differs from Rust's `char::is_whitespace` in two code
/// points: U+FEFF is JS whitespace, and U+0085 is not.
pub(crate) fn is_js_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{0009}'
            | '\u{000A}'
            | '\u{000B}'
            | '\u{000C}'
            | '\u{000D}'
            | '\u{0020}'
            | '\u{00A0}'
            | '\u{1680}'
            | '\u{2000}'
            ..='\u{200A}'
                | '\u{2028}'
                | '\u{2029}'
                | '\u{202F}'
                | '\u{205F}'
                | '\u{3000}'
                | '\u{FEFF}'
    )
}

/// JS `String.prototype.trim`.
pub(crate) fn js_trim(s: &str) -> &str {
    s.trim_matches(is_js_whitespace)
}

/// ECMAScript `StringToNumber`: surrounding JS whitespace is ignored, the
/// empty string is `0`, `0x`/`0o`/`0b` prefixes are unsigned integers,
/// `Infinity` may carry a sign, and anything else must be a complete decimal
/// literal or the result is `NaN`.
pub(crate) fn string_to_number(s: &str) -> f64 {
    let s = js_trim(s);
    if s.is_empty() {
        return 0.0;
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = s.strip_prefix(prefix) {
            if digits.is_empty() || !digits.chars().all(|c| c.is_digit(radix)) {
                return f64::NAN;
            }
            // Accumulate in f64, as JS does for integers beyond 2^53.
            return digits.chars().fold(0.0, |acc, c| {
                acc * f64::from(radix) + f64::from(c.to_digit(radix).unwrap_or(0))
            });
        }
    }
    let unsigned = s.strip_prefix(['+', '-']).unwrap_or(s);
    if unsigned == "Infinity" {
        return if s.starts_with('-') {
            f64::NEG_INFINITY
        } else {
            f64::INFINITY
        };
    }
    // Rust's float grammar also accepts `inf`, `nan` and `infinity`; the JS
    // StrDecimalLiteral only has digits, one `.`, and an exponent.
    let decimal = unsigned
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '+' | '-'));
    if !decimal || !unsigned.starts_with(|c: char| c.is_ascii_digit() || c == '.') {
        return f64::NAN;
    }
    s.parse::<f64>().unwrap_or(f64::NAN)
}

/// ECMAScript `parseInt(string)` (no radix): leading JS whitespace, an
/// optional sign, an optional `0x`/`0X` (radix 16), then the longest run of
/// digits; `NaN` when there are none.
pub(crate) fn parse_int(s: &str) -> f64 {
    let s = s.trim_start_matches(is_js_whitespace);
    let (sign, rest) = match s.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, s.strip_prefix('+').unwrap_or(s)),
    };
    let (radix, digits) = match rest.strip_prefix("0x").or_else(|| rest.strip_prefix("0X")) {
        Some(hex) => (16, hex),
        None => (10, rest),
    };
    let run: Vec<u32> = digits.chars().map_while(|c| c.to_digit(radix)).collect();
    if run.is_empty() {
        return f64::NAN;
    }
    let magnitude = if radix == 10 {
        let text: String = digits.chars().take(run.len()).collect();
        text.parse::<f64>().unwrap_or(f64::NAN)
    } else {
        run.iter()
            .fold(0.0, |acc, d| acc * f64::from(radix) + f64::from(*d))
    };
    sign * magnitude
}

/// ECMAScript `parseFloat(string)`: leading JS whitespace, then the longest
/// prefix that is a `StrDecimalLiteral` (`Infinity`, digits with an optional
/// fraction and exponent); `NaN` when there is none.
pub(crate) fn parse_float(s: &str) -> f64 {
    // Compiled once, not per call.
    static STR_DECIMAL_LITERAL: LazyLock<regress::Regex> = LazyLock::new(|| {
        regress::Regex::new(r"^[+-]?(?:Infinity|(?:\d+\.?\d*|\.\d+)(?:[eE][+-]?\d+)?)")
            .expect("static pattern")
    });
    let s = s.trim_start_matches(is_js_whitespace);
    match STR_DECIMAL_LITERAL.find(s) {
        Some(m) => string_to_number(&s[m.range]),
        None => f64::NAN,
    }
}

/// JS truthiness of a JSON value (`!!value`).
pub(crate) fn is_truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => {
            let n = json_number(n);
            n != 0.0 && !n.is_nan()
        }
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

#[cfg(test)]
mod tests;
