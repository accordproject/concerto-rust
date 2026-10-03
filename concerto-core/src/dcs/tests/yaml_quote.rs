use super::*;

fn quote(value: &str) -> String {
    quote_string_value(value, Some(DECORATOR_STRING_TYPE))
}

#[test]
fn non_string_argument_types_pass_through_unchanged() {
    assert_eq!(
        quote_string_value("42", Some("concerto.metamodel@1.0.0.DecoratorNumber")),
        "42"
    );
    assert_eq!(quote_string_value("42", None), "42");
}

#[test]
fn a_plain_word_is_left_unquoted() {
    assert_eq!(quote("hello"), "hello");
    assert_eq!(quote("Hello, World"), "Hello, World");
    assert_eq!(quote("a-simple-term"), "a-simple-term");
}

#[test]
fn an_empty_string_is_quoted_as_it_would_parse_back_as_null() {
    assert_eq!(quote(""), "\"\"");
}

#[test]
fn strings_that_look_like_other_scalar_types_are_quoted() {
    for s in [
        "true", "True", "TRUE", "false", "null", "Null", "NULL", "~", "42", "-7", "+3", "0x1F",
        "0o17", "3.14", "-0.5", "1e10", "1.5e-3", ".inf", "-.inf", ".nan",
    ] {
        assert_eq!(quote(s), json_quote(s), "expected {s:?} to be quoted");
    }
}

#[test]
fn a_string_that_is_not_a_number_stays_plain() {
    // Not a full match for the int/float tags (trailing non-digit).
    assert_eq!(quote("42a"), "42a");
    assert_eq!(quote("v1.2.3"), "v1.2.3");
}

#[test]
fn leading_indicator_characters_are_quoted() {
    for s in [
        "- item", "-", "?", "? key", "#comment", "*anchor", "&anchor", "!tag", "|lit", ">fold",
        "'q'", "\"q\"", "%tag", "@at", "`tick`", ",comma", "[seq", "{map",
    ] {
        assert_eq!(quote(s), json_quote(s), "expected {s:?} to be quoted");
    }
}

#[test]
fn a_colon_space_anywhere_forces_quoting() {
    assert_eq!(quote("key: value"), json_quote("key: value"));
    assert_eq!(quote("a:b"), "a:b");
}

#[test]
fn trailing_whitespace_or_colon_forces_quoting() {
    assert_eq!(quote("trailing "), json_quote("trailing "));
    assert_eq!(quote("trailing:"), json_quote("trailing:"));
}

#[test]
fn an_embedded_hash_preceded_by_space_forces_quoting() {
    assert_eq!(quote("a #comment"), json_quote("a #comment"));
    // Not preceded by whitespace: allowed plain.
    assert_eq!(quote("a#b"), "a#b");
}

#[test]
fn a_newline_always_forces_quoting() {
    assert_eq!(
        quote("line one\nline two"),
        json_quote("line one\nline two")
    );
}

#[test]
fn a_document_marker_line_forces_quoting() {
    assert_eq!(quote("---"), json_quote("---"));
    assert_eq!(
        quote("--- not a marker really"),
        json_quote("--- not a marker really")
    );
    assert_eq!(quote("..."), json_quote("..."));
    assert_eq!(quote("%YAML 1.2"), json_quote("%YAML 1.2"));
}

#[test]
fn control_characters_force_quoting() {
    assert_eq!(quote("a\tb\u{0007}c"), json_quote("a\tb\u{0007}c"));
}

#[test]
fn non_ascii_text_is_left_plain() {
    assert_eq!(quote("café"), "café");
    assert_eq!(quote("日本語"), "日本語");
}

#[test]
fn a_short_string_under_the_line_width_is_never_folded() {
    let s = "a ".repeat(30) + "z"; // 61 chars, well under 80
    assert_eq!(quote(&s), s);
}

#[test]
fn a_long_string_past_the_line_width_is_folded_and_so_quoted() {
    let s = "word ".repeat(30); // 150 chars, folds past width 80
    assert_eq!(quote(&s), json_quote(&s));
}

#[test]
fn a_long_string_with_no_fold_point_is_left_plain() {
    // One giant run with no space after column 80 to split on.
    let s = "x".repeat(200);
    assert_eq!(quote(&s), s);
}
