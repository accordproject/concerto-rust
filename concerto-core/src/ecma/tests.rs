use crate::json;

use super::*;

#[test]
fn parse_int_and_parse_float_follow_ecmascript() {
    assert_eq!(parse_int("42"), 42.0);
    assert_eq!(parse_int("  -7.9abc"), -7.0);
    assert_eq!(parse_int("0x1F"), 31.0);
    assert_eq!(parse_int("1e+21"), 1.0);
    assert!(parse_int("abc").is_nan());
    assert_eq!(parse_float("3.25kg"), 3.25);
    assert_eq!(parse_float(" -.5e1x"), -5.0);
    assert_eq!(parse_float("Infinityx"), f64::INFINITY);
    assert!(parse_float("x1").is_nan());
}

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
fn to_number_follows_js() {
    assert_eq!(to_number(&json!(null)), 0.0);
    assert_eq!(to_number(&json!(true)), 1.0);
    assert_eq!(to_number(&json!(false)), 0.0);
    assert_eq!(to_number(&json!("42")), 42.0);
    assert!(to_number(&json!("abc")).is_nan());
    assert_eq!(to_number(&json!([10])), 10.0);
    assert_eq!(to_number(&json!([])), 0.0);
    assert!(to_number(&json!({"a": 1})).is_nan());
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
