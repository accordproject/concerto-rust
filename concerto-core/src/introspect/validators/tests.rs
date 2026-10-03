//! Ported from `test/introspect/stringvalidator.js` and
//! `test/introspect/collectionsizevalidator.js` (P2-02): the constructor,
//! `validate` and `compatibleWith` cases, run here directly over the
//! Rust types rather than through `sinon.createStubInstance(Field)`. Not
//! ported: cases that assert on a live JS `RegExp` object (`getRegex()`
//! returning something with a settable `lastIndex`) — `regress` is
//! stateless, so [`CompiledRegex::matches`] has no `lastIndex` to leak in
//! the first place, which is the property those cases check for.

use super::*;
use crate::error::{Error, Result};
use crate::introspect::FullyQualified;

/// A minimal `ValidatedElement`, standing in for `sinon.createStubInstance(Field)`.
struct TestField {
    fqn: &'static str,
    name: &'static str,
    default_value: Option<Value>,
}

impl TestField {
    fn new(fqn: &'static str, name: &'static str) -> Self {
        Self {
            fqn,
            name,
            default_value: None,
        }
    }

    fn with_default(mut self, value: Value) -> Self {
        self.default_value = Some(value);
        self
    }
}

impl FullyQualified for TestField {
    type Error = Error;

    fn fully_qualified_name(&self) -> Result<String> {
        Ok(self.fqn.to_string())
    }
}

impl ValidatedElement for TestField {
    fn default_value(&self) -> Result<Option<Value>> {
        Ok(self.default_value.clone())
    }

    fn name(&self) -> Result<String> {
        Ok(self.name.to_string())
    }
}

fn field() -> TestField {
    TestField::new("org.acme.myField", "myField")
}

fn regex_ast(pattern: &str, flags: &str) -> mm::StringRegexValidator {
    serde_json::from_value(serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.StringRegexValidator",
        "pattern": pattern,
        "flags": flags,
    }))
    .expect("valid StringRegexValidator AST")
}

fn length_ast(min: Option<f64>, max: Option<f64>) -> mm::StringLengthValidator {
    let mut ast = serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringLengthValidator" });
    if let Some(min) = min {
        ast["minLength"] = min.into();
    }
    if let Some(max) = max {
        ast["maxLength"] = max.into();
    }
    serde_json::from_value(ast).expect("valid StringLengthValidator AST")
}

fn size_ast(min: Option<f64>, max: Option<f64>) -> mm::CollectionSizeValidator {
    let mut ast =
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator" });
    if let Some(min) = min {
        ast["minSize"] = min.into();
    }
    if let Some(max) = max {
        ast["maxSize"] = max.into();
    }
    serde_json::from_value(ast).expect("valid CollectionSizeValidator AST")
}

fn string_validator(
    pattern: Option<(&str, &str)>,
    length: Option<(Option<f64>, Option<f64>)>,
) -> Result<StringValidator> {
    let regex = pattern.map(|(p, f)| regex_ast(p, f));
    let length = length.map(|(min, max)| length_ast(min, max));
    StringValidator::new(&field(), regex.as_ref(), length.as_ref(), None)
}

// ---- StringValidator: #constructor ----

#[test]
fn string_validator_rejects_an_invalid_regex() {
    let err = string_validator(Some(("^[A-z", "")), None).unwrap_err();
    assert!(err.to_string().contains("Validator error for field"));
}

/// `v8_regex_reason`'s specific mapping: `regress`'s "Unbalanced
/// parenthesis" is translated to V8's own wording, "Unterminated
/// group" (OD-4).
#[test]
fn string_validator_maps_unbalanced_parenthesis_to_v8_wording() {
    let err = string_validator(Some(("(", "")), None).unwrap_err();
    assert!(
        err.to_string()
            .contains("Invalid regular expression: /(/: Unterminated group"),
        "{err}"
    );
}

/// `v8_regex_reason`'s fallback arm: a reason not in the table is
/// `regress`'s own text, unchanged.
#[test]
fn string_validator_passes_through_an_unmapped_regex_error_reason() {
    let err = string_validator(Some(("a{2,1}", "")), None).unwrap_err();
    assert!(
        err.to_string()
            .contains("Invalid regular expression: /a{2,1}/: Invalid quantifier"),
        "{err}"
    );
}

#[test]
fn string_validator_rejects_invalid_regex_flags() {
    for flags in ["x", "gg", "uv", "vu", "iI"] {
        let err = string_validator(Some(("foo", flags)), None).unwrap_err();
        assert!(
            err.to_string().contains(&format!(
                "Invalid flags supplied to RegExp constructor '{flags}'"
            )),
            "flags {flags:?} should be rejected: {err}"
        );
    }
}

#[test]
fn string_validator_accepts_every_valid_regex_flag_once() {
    assert!(string_validator(Some(("foo", "dgimsy")), None).is_ok());
    assert!(string_validator(Some(("foo", "u")), None).is_ok());
    assert!(string_validator(Some(("foo", "v")), None).is_ok());
}

#[test]
fn string_validator_rejects_length_with_no_bounds() {
    let err = string_validator(Some(("^[A-z]", "")), Some((None, None))).unwrap_err();
    assert!(
        err.to_string()
            .contains("Invalid string length, minLength and-or maxLength must be specified")
    );
}

/// BC-40 (R1): a length validator whose bounds are both *absent*
/// (`length=[,]`), or not an object at all, is rejected like one whose
/// bounds are both `null`, as an `IllegalModel` error (BC-39) with the
/// `DefaultValidatorException` error type. TS 5.0.0 accepted it.
#[test]
fn string_validator_rejects_length_with_absent_bounds() {
    for ast in [
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringLengthValidator" }),
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringLengthValidator", "minLength": null }),
        serde_json::json!(true),
    ] {
        let length = length_validator_from_ast(Some(&ast));
        let err = StringValidator::new(&field(), None, length.as_ref(), Some(&ast))
            .expect_err(&format!("{ast} should be rejected"));
        let contract = err.into_ported().expect("a contract error");
        assert_eq!(contract.kind, ErrorKind::IllegalModel, "{ast}");
        assert_eq!(contract.code, "stringvalidator-constructor-invalidlength");
        assert_eq!(
            contract.validator.as_ref().map(|report| report.error_type),
            Some(DEFAULT_VALIDATOR_EXCEPTION)
        );
    }
    // One bound is enough.
    let min_only = serde_json::json!({ "minLength": 1 });
    let length = length_validator_from_ast(Some(&min_only));
    assert!(StringValidator::new(&field(), None, length.as_ref(), Some(&min_only)).is_ok());
}

#[test]
fn string_validator_rejects_min_length_above_max_length() {
    let err = string_validator(Some(("^[A-z]", "")), Some((Some(200.0), Some(100.0)))).unwrap_err();
    assert!(
        err.to_string()
            .contains("minLength must be less than or equal to maxLength")
    );
}

#[test]
fn string_validator_rejects_negative_lengths() {
    for (min, max) in [
        (Some(-2.0), None),
        (None, Some(-100.0)),
        (Some(-1.0), Some(-100.0)),
    ] {
        let err = string_validator(None, Some((min, max))).unwrap_err();
        assert!(
            err.to_string()
                .contains("minLength and-or maxLength must be positive integers"),
            "{min:?}/{max:?} should be rejected"
        );
    }
}

#[test]
fn string_validator_rejects_a_default_value_shorter_than_min_length() {
    let f = field().with_default(serde_json::json!("abc"));
    let err =
        StringValidator::new(&f, None, Some(&length_ast(Some(5.0), Some(10.0))), None).unwrap_err();
    assert!(
        err.to_string()
            .contains("The string length of 'abc' should be at least 5 characters.")
    );
}

#[test]
fn string_validator_rejects_a_default_value_longer_than_max_length() {
    let f = field().with_default(serde_json::json!("abcdefgh"));
    let err =
        StringValidator::new(&f, None, Some(&length_ast(Some(2.0), Some(5.0))), None).unwrap_err();
    assert!(
        err.to_string()
            .contains("The string length of 'abcdefgh' should not exceed 5 characters.")
    );
}

#[test]
fn string_validator_accepts_a_default_value_matching_length_and_pattern() {
    let f = field().with_default(serde_json::json!("ABC"));
    assert!(
        StringValidator::new(
            &f,
            Some(&regex_ast("^[A-Z]{3,5}$", "")),
            Some(&length_ast(Some(3.0), Some(5.0))),
            None
        )
        .is_ok()
    );
}

// ---- StringValidator: #validate ----

#[test]
fn string_validator_ignores_a_null_string() {
    let v = string_validator(Some(("^[A-z][A-z][0-9]{7}", "")), None).unwrap();
    assert!(v.validate(&field(), Some("id"), None).is_ok());
}

#[test]
fn string_validator_validates_a_matching_string() {
    let v = string_validator(Some(("^[A-z][A-z][0-9]{7}", "")), None).unwrap();
    assert!(v.validate(&field(), Some("id"), Some("AB1234567")).is_ok());
}

#[test]
fn string_validator_detects_a_mismatched_string() {
    let v = string_validator(Some(("^[A-z][A-z][0-9]{7}", "")), None).unwrap();
    let err = v.validate(&field(), Some("id"), Some("xyz")).unwrap_err();
    assert!(
        err.to_string()
            .contains("Validator error for field `id`. org.acme.myField")
    );
}

#[test]
fn string_validator_repeatedly_validates_a_matching_string_with_a_global_regex() {
    let v = string_validator(Some(("^[A-z][A-z][0-9]{7}", "g")), None).unwrap();
    for _ in 0..3 {
        assert!(v.validate(&field(), Some("id"), Some("AB1234567")).is_ok());
    }
}

#[test]
fn string_validator_repeatedly_rejects_a_mismatched_string_with_a_global_regex() {
    let v = string_validator(Some(("^[A-z][A-z][0-9]{7}", "g")), None).unwrap();
    for _ in 0..2 {
        assert!(v.validate(&field(), Some("id"), Some("xyz")).is_err());
    }
}

#[test]
fn string_validator_repeatedly_validates_a_matching_string_with_a_sticky_regex() {
    let v = string_validator(Some(("^[A-z][A-z][0-9]{7}", "y")), None).unwrap();
    for _ in 0..2 {
        assert!(v.validate(&field(), Some("id"), Some("AB1234567")).is_ok());
    }
}

#[test]
fn string_validator_a_sticky_regex_only_matches_at_the_start() {
    // Not in the TS suite directly, but is exactly what `y` means: a
    // match that does not start at offset 0 must be rejected even though
    // the same pattern (without `y`) would find it further along.
    let v = string_validator(Some(("[0-9]+", "y")), None).unwrap();
    assert!(v.validate(&field(), Some("id"), Some("abc123")).is_err());
    assert!(v.validate(&field(), Some("id"), Some("123abc")).is_ok());
}

#[test]
fn string_validator_validates_escaped_characters() {
    let v = string_validator(Some((r"^[\\]*\n$", "")), None).unwrap();
    assert!(v.validate(&field(), Some("id"), Some("\\\\\n")).is_ok());
    assert!(v.validate(&field(), Some("id"), Some("\\hi!\n")).is_err());
}

#[test]
fn string_validator_validates_a_unicode_string() {
    let pattern = r"^(\p{Lu}|\p{Ll}|\p{Lt}|\p{Lm}|\p{Lo}|\p{Nl}|\$|_|\\u[0-9A-Fa-f]{4})(?:\p{Lu}|\p{Ll}|\p{Lt}|\p{Lm}|\p{Lo}|\p{Nl}|\$|_|\\u[0-9A-Fa-f]{4}|\p{Mn}|\p{Mc}|\p{Nd}|\p{Pc}|\u200C|\u200D)*$";
    let v = string_validator(Some((pattern, "u")), None).unwrap();
    assert!(v.validate(&field(), Some("id"), Some("AB1234567")).is_ok());
    assert!(v.validate(&field(), Some("id"), Some("1FOO")).is_err());
}

#[test]
fn string_validator_length_only_bounds() {
    let min_only = string_validator(None, Some((Some(2.0), None))).unwrap();
    assert!(
        min_only
            .validate(&field(), Some("id"), Some("AB1234567455455455"))
            .is_ok()
    );
    let err = min_only
        .validate(&field(), Some("id"), Some("w"))
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("The string length of 'w' should be at least 2 characters.")
    );
    let err = min_only
        .validate(&field(), Some("id"), Some(""))
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("The string length of '' should be at least 2 characters.")
    );

    let max_only = string_validator(None, Some((None, Some(10.0)))).unwrap();
    assert!(
        max_only
            .validate(&field(), Some("id"), Some("ABCD123456"))
            .is_ok()
    );
    assert!(max_only.validate(&field(), Some("id"), Some("")).is_ok());
    let err = max_only
        .validate(&field(), Some("id"), Some("ABCD1234567"))
        .unwrap_err();
    assert!(err.to_string().contains("should not exceed 10 characters."));
}

#[test]
fn string_validator_length_takes_precedence_over_regex() {
    let v = string_validator(Some(("^[A-z]{1,100}$", "")), Some((Some(1.0), Some(10.0)))).unwrap();
    let err = v
        .validate(&field(), Some("id"), Some("AbCdefghijklmksadada"))
        .unwrap_err();
    assert!(err.to_string().contains("should not exceed 10 characters."));
}

// ---- StringValidator: #compatibleWith ----

#[test]
fn string_validator_is_incompatible_with_a_number_validator() {
    let other =
        NumberValidator::new(&field(), &serde_json::json!({"lower": -1, "upper": 1})).unwrap();
    let v = string_validator(Some(("foo", "")), Some((Some(1.0), Some(100.0)))).unwrap();
    assert!(!v.compatible_with(Some(&Validator::Number(other))));
}

#[test]
fn string_validator_compatible_with_same_pattern_and_flags() {
    let other = string_validator(Some(("foo", "")), None).unwrap();
    let v = string_validator(Some(("foo", "")), None).unwrap();
    assert!(v.compatible_with(Some(&Validator::String(other))));
}

#[test]
fn string_validator_incompatible_with_a_changed_pattern() {
    let other = string_validator(Some(("bar", "")), None).unwrap();
    let v = string_validator(Some(("foo", "")), None).unwrap();
    assert!(!v.compatible_with(Some(&Validator::String(other))));
}

#[test]
fn string_validator_incompatible_with_changed_flags() {
    let other = string_validator(Some(("foo", "i")), None).unwrap();
    let v = string_validator(Some(("foo", "g")), None).unwrap();
    assert!(!v.compatible_with(Some(&Validator::String(other))));
}

#[test]
fn string_validator_length_compatibility() {
    let wide = || string_validator(None, Some((Some(1.0), Some(100.0)))).unwrap();
    assert!(wide().compatible_with(Some(&Validator::String(wide()))));

    let narrow = string_validator(None, Some((None, Some(10.0)))).unwrap();
    assert!(!wide().compatible_with(Some(&Validator::String(narrow))));

    let this_tighter_min = string_validator(None, Some((Some(1.0), Some(100.0)))).unwrap();
    let other_tighter_min = string_validator(None, Some((Some(10.0), Some(100.0)))).unwrap();
    assert!(!this_tighter_min.compatible_with(Some(&Validator::String(other_tighter_min))));

    let this_wider_max = string_validator(None, Some((Some(1.0), Some(100.0)))).unwrap();
    let other_tighter_max = string_validator(None, Some((Some(1.0), Some(10.0)))).unwrap();
    assert!(!this_wider_max.compatible_with(Some(&Validator::String(other_tighter_max))));

    let this_no_max = string_validator(None, Some((Some(1.0), None))).unwrap();
    let other_has_max = string_validator(None, Some((Some(1.0), Some(10.0)))).unwrap();
    assert!(!this_no_max.compatible_with(Some(&Validator::String(other_has_max))));

    // The symmetric case for min_length: this has no lower bound (so
    // accepts shorter strings than `other` allows), so it is not
    // compatible with an `other` that does have one.
    let this_no_min = string_validator(None, Some((None, Some(100.0)))).unwrap();
    let other_has_min = string_validator(None, Some((Some(1.0), Some(100.0)))).unwrap();
    assert!(!this_no_min.compatible_with(Some(&Validator::String(other_has_min))));
}

/// [`string_validator_length_compatibility`], but for an *absent*
/// `minLength`/`maxLength` key specifically, built the way a real
/// fixture reaches `StringValidator::new` — through
/// `validators::length_validator_from_ast` (`Property::try_from`'s own
/// call site) — rather than `length_ast`'s straight `serde` decode. A
/// regression here (P5-05-T2a review) let a bound read from an absent
/// key (then a `Some(NaN)` sentinel, `None` since BC-40) silently compare
/// as `false` against any real bound in `compatible_with`'s old
/// `(Some(this), Some(other)) if this < other` arm, wrongly treating "no
/// bound at all" as compatible with a narrower one, instead of taking the
/// `isNull` branch this validator's own explicit-`null` (`length_ast`)
/// case above already covers.
#[test]
fn string_validator_length_compatibility_with_an_absent_bound_matches_an_explicit_null_one() {
    fn length_validator_via_ast(min: Option<f64>, max: Option<f64>) -> StringValidator {
        let mut ast =
            serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringLengthValidator" });
        if let Some(min) = min {
            ast["minLength"] = min.into();
        }
        if let Some(max) = max {
            ast["maxLength"] = max.into();
        }
        let length = length_validator_from_ast(Some(&ast));
        StringValidator::new(&field(), None, length.as_ref(), None).unwrap()
    }

    // `this` has no lower bound at all (the `minLength` key is simply
    // absent, not explicitly `null`); `other` has a real one. `this`
    // accepts shorter strings than `other` allows, so it must not be
    // compatible with it — the same verdict the explicit-`null` case
    // (`string_validator_length_compatibility`'s `this_no_min`) already
    // gets.
    let this_no_min = length_validator_via_ast(None, Some(100.0));
    let other_has_min = length_validator_via_ast(Some(1.0), Some(100.0));
    assert!(!this_no_min.compatible_with(Some(&Validator::String(other_has_min))));

    // The symmetric case for maxLength.
    let this_no_max = length_validator_via_ast(Some(1.0), None);
    let other_has_max = length_validator_via_ast(Some(1.0), Some(10.0));
    assert!(!this_no_max.compatible_with(Some(&Validator::String(other_has_max))));

    // Both sides have the *same* absent bound: compatible, matching the
    // "no constraint on either side" case `compatible_with` already
    // covers for two explicit `None`s.
    let both_no_min = length_validator_via_ast(None, Some(100.0));
    assert!(this_no_min.compatible_with(Some(&Validator::String(both_no_min))));
}

// ---- StringValidator: accessors ----

#[test]
fn string_validator_min_length_and_max_length_accessors() {
    let both = string_validator(None, Some((Some(2.0), Some(8.0)))).unwrap();
    assert_eq!(both.min_length(), Some(2.0));
    assert_eq!(both.max_length(), Some(8.0));

    let min_only = string_validator(None, Some((Some(3.0), None))).unwrap();
    assert_eq!(min_only.min_length(), Some(3.0));
    assert_eq!(min_only.max_length(), None);

    let max_only = string_validator(None, Some((None, Some(9.0)))).unwrap();
    assert_eq!(max_only.min_length(), None);
    assert_eq!(max_only.max_length(), Some(9.0));
}

// ---- CompiledRegex: #eq ----

/// `StringValidator` derives `PartialEq`, which for its `regex` field
/// goes through `CompiledRegex`'s own manual `PartialEq` (pattern and
/// flags only — `regress::Regex` itself has none).
#[test]
fn string_validator_equality_compares_pattern_and_flags() {
    let a = string_validator(Some(("foo", "i")), None).unwrap();
    let b = string_validator(Some(("foo", "i")), None).unwrap();
    assert_eq!(a, b);

    let different_pattern = string_validator(Some(("bar", "i")), None).unwrap();
    assert_ne!(a, different_pattern);

    let different_flags = string_validator(Some(("foo", "g")), None).unwrap();
    assert_ne!(a, different_flags);
}

// ---- CollectionSizeValidator: #constructor ----

#[test]
fn collection_size_validator_reads_both_bounds() {
    let v = CollectionSizeValidator::new(&field(), &size_ast(Some(1.0), Some(10.0)), None).unwrap();
    assert_eq!(v.min_size(), Some(1.0));
    assert_eq!(v.max_size(), Some(10.0));
}

#[test]
fn collection_size_validator_min_only() {
    let v = CollectionSizeValidator::new(&field(), &size_ast(Some(3.0), None), None).unwrap();
    assert_eq!(v.min_size(), Some(3.0));
    assert_eq!(v.max_size(), None);
}

#[test]
fn collection_size_validator_rejects_no_bounds() {
    let err = CollectionSizeValidator::new(&field(), &size_ast(None, None), None).unwrap_err();
    assert!(
        err.to_string()
            .contains("minSize and/or maxSize must be specified")
    );
}

#[test]
fn collection_size_validator_rejects_negative_bounds() {
    let err =
        CollectionSizeValidator::new(&field(), &size_ast(Some(-1.0), None), None).unwrap_err();
    assert!(err.to_string().contains("positive integers"));
    let err =
        CollectionSizeValidator::new(&field(), &size_ast(None, Some(-2.0)), None).unwrap_err();
    assert!(err.to_string().contains("positive integers"));
}

#[test]
fn collection_size_validator_rejects_min_above_max() {
    let err =
        CollectionSizeValidator::new(&field(), &size_ast(Some(5.0), Some(2.0)), None).unwrap_err();
    assert!(
        err.to_string()
            .contains("minSize must be less than or equal to maxSize")
    );
}

#[test]
fn collection_size_validator_allows_min_equal_max_and_zero() {
    let v = CollectionSizeValidator::new(&field(), &size_ast(Some(3.0), Some(3.0)), None).unwrap();
    assert_eq!(v.min_size(), Some(3.0));
    let v = CollectionSizeValidator::new(&field(), &size_ast(Some(0.0), Some(5.0)), None).unwrap();
    assert_eq!(v.min_size(), Some(0.0));
}

// ---- CollectionSizeValidator: #validate ----

#[test]
fn collection_size_validator_validate() {
    let v = CollectionSizeValidator::new(&field(), &size_ast(Some(2.0), Some(5.0)), None).unwrap();
    assert!(v.validate(&field(), Some("id"), 3.0).is_ok());
    let err = v.validate(&field(), Some("id"), 1.0).unwrap_err();
    assert!(err.to_string().contains("at least 2 elements"));
    let err = v.validate(&field(), Some("id"), 6.0).unwrap_err();
    assert!(err.to_string().contains("no more than 5 elements"));
}

/// Both bounds are inclusive: a value exactly at `minSize` or `maxSize`
/// is accepted, not rejected.
#[test]
fn collection_size_validator_validate_is_inclusive_at_both_bounds() {
    let v = CollectionSizeValidator::new(&field(), &size_ast(Some(2.0), Some(5.0)), None).unwrap();
    assert!(v.validate(&field(), Some("id"), 2.0).is_ok());
    assert!(v.validate(&field(), Some("id"), 5.0).is_ok());
}

// ---- CollectionSizeValidator: #compatibleWith ----

#[test]
fn collection_size_validator_compatible_with() {
    let v = |min: Option<f64>, max: Option<f64>| {
        CollectionSizeValidator::new(&field(), &size_ast(min, max), None).unwrap()
    };

    assert!(!v(Some(1.0), None).compatible_with(None));
    assert!(
        v(Some(2.0), Some(5.0))
            .compatible_with(Some(&Validator::CollectionSize(v(Some(1.0), Some(6.0)))))
    );
    assert!(
        !v(Some(1.0), Some(5.0))
            .compatible_with(Some(&Validator::CollectionSize(v(Some(3.0), Some(5.0)))))
    );
    assert!(
        !v(Some(1.0), Some(5.0))
            .compatible_with(Some(&Validator::CollectionSize(v(Some(1.0), Some(3.0)))))
    );
    assert!(
        !v(None, Some(10.0))
            .compatible_with(Some(&Validator::CollectionSize(v(Some(1.0), Some(10.0)))))
    );
    assert!(
        !v(Some(1.0), None)
            .compatible_with(Some(&Validator::CollectionSize(v(Some(1.0), Some(10.0)))))
    );
    assert!(
        v(Some(1.0), Some(5.0))
            .compatible_with(Some(&Validator::CollectionSize(v(Some(1.0), None))))
    );
    assert!(
        v(Some(1.0), Some(5.0))
            .compatible_with(Some(&Validator::CollectionSize(v(None, Some(5.0)))))
    );
    assert!(
        v(Some(2.0), Some(8.0))
            .compatible_with(Some(&Validator::CollectionSize(v(Some(2.0), Some(8.0)))))
    );
}

// ---- NumberValidator: #constructor / accessors ----

#[test]
fn number_validator_reads_both_bounds() {
    let v = NumberValidator::new(&field(), &number_ast(Some(1.0), Some(10.0))).unwrap();
    assert_eq!(v.lower_bound(), Some(&Value::from(1.0)));
    assert_eq!(v.upper_bound(), Some(&Value::from(10.0)));
}

#[test]
fn number_validator_lower_only() {
    let v = NumberValidator::new(&field(), &number_ast(Some(3.0), None)).unwrap();
    assert_eq!(v.lower_bound(), Some(&Value::from(3.0)));
    assert_eq!(v.upper_bound(), None);
}

// ---- NumberValidator: #toString ----

#[test]
fn number_validator_to_string() {
    let v = NumberValidator::new(&field(), &number_ast(Some(1.0), Some(10.0))).unwrap();
    assert_eq!(v.to_string(), "NumberValidator lower: 1 upper: 10");

    let lower_only = NumberValidator::new(&field(), &number_ast(Some(2.0), None)).unwrap();
    assert_eq!(
        lower_only.to_string(),
        "NumberValidator lower: 2 upper: null"
    );
}

// ---- NumberValidator: #compatibleWith ----

fn number_ast(lower: Option<f64>, upper: Option<f64>) -> Value {
    let mut ast = serde_json::json!({});
    if let Some(lower) = lower {
        ast["lower"] = lower.into();
    }
    if let Some(upper) = upper {
        ast["upper"] = upper.into();
    }
    ast
}

#[test]
fn number_validator_incompatible_with_no_other_or_a_non_number_validator() {
    let v = NumberValidator::new(&field(), &number_ast(Some(1.0), Some(5.0))).unwrap();
    assert!(!v.compatible_with(None));
    let other = string_validator(Some(("foo", "")), None).unwrap();
    assert!(!v.compatible_with(Some(&Validator::String(other))));
}

#[test]
fn number_validator_compatible_with() {
    let v = |lower: Option<f64>, upper: Option<f64>| {
        NumberValidator::new(&field(), &number_ast(lower, upper)).unwrap()
    };

    assert!(
        v(Some(2.0), Some(5.0)).compatible_with(Some(&Validator::Number(v(Some(1.0), Some(6.0)))))
    );
    assert!(
        !v(Some(1.0), Some(5.0)).compatible_with(Some(&Validator::Number(v(Some(3.0), Some(5.0)))))
    );
    assert!(
        !v(Some(1.0), Some(5.0)).compatible_with(Some(&Validator::Number(v(Some(1.0), Some(3.0)))))
    );
    assert!(
        !v(None, Some(10.0)).compatible_with(Some(&Validator::Number(v(Some(1.0), Some(10.0)))))
    );
    assert!(
        !v(Some(1.0), None).compatible_with(Some(&Validator::Number(v(Some(1.0), Some(10.0)))))
    );
    assert!(v(Some(1.0), Some(5.0)).compatible_with(Some(&Validator::Number(v(Some(1.0), None)))));
    assert!(v(Some(1.0), Some(5.0)).compatible_with(Some(&Validator::Number(v(None, Some(5.0))))));
    assert!(
        v(Some(2.0), Some(8.0)).compatible_with(Some(&Validator::Number(v(Some(2.0), Some(8.0)))))
    );
}

// ---- Validator: enum-level #compatibleWith dispatch ----

/// `Validator::compatible_with` (the enum wrapper, not each variant's own
/// method) just dispatches to the variant's `compatible_with` — every
/// test above calls the variant's method directly, so it never actually
/// goes through this dispatcher. Confirm each arm forwards both a
/// compatible and an incompatible answer through the wrapper.
#[test]
fn validator_compatible_with_dispatches_to_each_variant() {
    let number = |lower: Option<f64>, upper: Option<f64>| {
        Validator::Number(NumberValidator::new(&field(), &number_ast(lower, upper)).unwrap())
    };
    let string =
        |pattern: &str| Validator::String(string_validator(Some((pattern, "")), None).unwrap());
    let collection = |min: Option<f64>, max: Option<f64>| {
        Validator::CollectionSize(
            CollectionSizeValidator::new(&field(), &size_ast(min, max), None).unwrap(),
        )
    };

    assert!(number(Some(1.0), Some(5.0)).compatible_with(Some(&number(Some(1.0), Some(5.0)))));
    assert!(!number(Some(1.0), Some(5.0)).compatible_with(Some(&number(Some(2.0), Some(5.0)))));

    assert!(string("foo").compatible_with(Some(&string("foo"))));
    assert!(!string("foo").compatible_with(Some(&string("bar"))));

    assert!(
        collection(Some(1.0), Some(5.0)).compatible_with(Some(&collection(Some(1.0), Some(5.0))))
    );
    assert!(
        !collection(Some(1.0), Some(5.0)).compatible_with(Some(&collection(Some(2.0), Some(5.0))))
    );
}

// ---- BC-39: the error class of each validator error ----

/// The kind and `errorType` of a validator error.
fn kind_and_type(err: Error) -> (ErrorKind, &'static str) {
    let contract = err.into_ported().expect("a contract error");
    let error_type = contract
        .validator
        .as_ref()
        .expect("a validator report")
        .error_type;
    (contract.kind, error_type)
}

/// BC-39 (R1): a validator error found while the model loads (a bad
/// bound, an invalid regex, a default value outside the validator) is an
/// `IllegalModel` error, and an instance value that fails a validator is a
/// `Validation` error. Both keep their `errorType`. TS 5.0.0 threw a
/// `BaseException` for all of them.
#[test]
fn validator_errors_are_illegal_model_at_load_and_validation_for_instances() {
    const MODEL: ErrorKind = ErrorKind::IllegalModel;
    const INSTANCE: ErrorKind = ErrorKind::Validation;
    const DEFAULT: &str = DEFAULT_VALIDATOR_EXCEPTION;

    // Load time.
    let no_bounds = NumberValidator::new(&field(), &number_ast(None, None)).unwrap_err();
    assert_eq!(kind_and_type(no_bounds), (MODEL, DEFAULT));
    let swapped = NumberValidator::new(&field(), &number_ast(Some(5.0), Some(1.0))).unwrap_err();
    assert_eq!(kind_and_type(swapped), (MODEL, DEFAULT));
    let number_default = field().with_default(serde_json::json!(50));
    let outside =
        NumberValidator::new(&number_default, &number_ast(Some(1.0), Some(10.0))).unwrap_err();
    assert_eq!(kind_and_type(outside), (MODEL, DEFAULT));
    let no_size = CollectionSizeValidator::new(&field(), &size_ast(None, None), None).unwrap_err();
    assert_eq!(kind_and_type(no_size), (MODEL, DEFAULT));
    let bad_regex = string_validator(Some(("^[A-z", "")), None).unwrap_err();
    assert_eq!(kind_and_type(bad_regex), (MODEL, REGEX_VALIDATOR_EXCEPTION));
    let negative = string_validator(None, Some((Some(-1.0), None))).unwrap_err();
    assert_eq!(kind_and_type(negative), (MODEL, DEFAULT));
    let string_default = field().with_default(serde_json::json!("abc"));
    let too_short = StringValidator::new(
        &string_default,
        None,
        Some(&length_ast(Some(5.0), None)),
        None,
    )
    .unwrap_err();
    assert_eq!(kind_and_type(too_short), (MODEL, DEFAULT));

    // Instance validation.
    let number = NumberValidator::new(&field(), &number_ast(Some(1.0), Some(10.0))).unwrap();
    let above = number
        .validate(&field(), Some("id"), Some(11.0))
        .unwrap_err();
    assert_eq!(kind_and_type(above), (INSTANCE, DEFAULT));
    let size = CollectionSizeValidator::new(&field(), &size_ast(Some(1.0), None), None).unwrap();
    let empty = size.validate(&field(), Some("id"), 0.0).unwrap_err();
    assert_eq!(kind_and_type(empty), (INSTANCE, DEFAULT));
    let string = string_validator(Some(("^a", "")), Some((None, Some(3.0)))).unwrap();
    let long = string
        .validate(&field(), Some("id"), Some("abcd"))
        .unwrap_err();
    assert_eq!(kind_and_type(long), (INSTANCE, DEFAULT));
    let mismatch = string
        .validate(&field(), Some("id"), Some("b"))
        .unwrap_err();
    assert_eq!(kind_and_type(mismatch), (INSTANCE, DEFAULT));
}
