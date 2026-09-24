//! Crate-private helpers for the ECMAScript semantics the TypeScript
//! reference relies on (PORTING.md section 3.1).
//!
//! The model ASTs reach Rust as `serde_json::Value` (OD-3), so these helpers
//! take JSON values. A JSON value can hold every JS value a model AST holds,
//! except `undefined` (an absent key, modelled as `Option::None` by callers)
//! and the non-finite numbers (which neither the CTO parser nor `JSON.parse`
//! produce).

use std::cmp::Ordering;

use serde_json::Value;

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
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn numbers_format_the_js_way() {
        assert_eq!(number_to_string(1.0), "1");
        assert_eq!(number_to_string(-0.0), "0");
        assert_eq!(number_to_string(0.1), "0.1");
        assert_eq!(number_to_string(1e21), "1e+21");
        assert_eq!(number_to_string(f64::NAN), "NaN");
        assert_eq!(number_to_string(f64::INFINITY), "Infinity");
    }

    #[test]
    fn to_string_follows_js() {
        assert_eq!(to_js_string(&json!(null)), "null");
        assert_eq!(to_js_string(&json!(5)), "5");
        assert_eq!(to_js_string(&json!([1, null, "a"])), "1,,a");
        assert_eq!(to_js_string(&json!({"a": 1})), "[object Object]");
    }

    #[test]
    fn relational_comparison_follows_js() {
        // null is 0, strings compare as strings, NaN never compares.
        assert!(less_than(&json!(null), &json!(10)));
        assert!(less_than(&json!("10"), &json!("9")));
        assert!(!less_than(&json!("x"), &json!(10)));
        assert!(!greater_than(&json!("x"), &json!(10)));
        assert!(greater_than(&json!("11"), &json!(10)));
        assert!(number_less_than(-1.0, &json!(0)));
        assert!(number_greater_than(101.0, &json!(100)));
    }

    #[test]
    fn string_to_number_follows_js() {
        assert_eq!(string_to_number(" 12 "), 12.0);
        assert_eq!(string_to_number(""), 0.0);
        assert_eq!(string_to_number("0x10"), 16.0);
        assert_eq!(string_to_number("-Infinity"), f64::NEG_INFINITY);
        assert_eq!(string_to_number(".5"), 0.5);
        assert!(string_to_number("inf").is_nan());
        assert!(string_to_number("-0x10").is_nan());
        assert!(string_to_number("1_0").is_nan());
    }

    #[test]
    fn trim_uses_the_js_whitespace_set() {
        assert_eq!(js_trim("\u{FEFF} a \u{3000}"), "a");
        assert_eq!(js_trim("\u{0085}a"), "\u{0085}a");
    }

    #[test]
    fn truthiness_follows_js() {
        assert!(!is_truthy(&json!(0)));
        assert!(!is_truthy(&json!("")));
        assert!(is_truthy(&json!([])));
        assert!(is_truthy(&json!("0")));
    }
}
