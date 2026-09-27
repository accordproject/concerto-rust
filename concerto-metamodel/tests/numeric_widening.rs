//! OD-3: Integer and Long AST fields must accept every number TS accepts.
//!
//! TS reads these fields as plain JS numbers (f64 semantics), so a Long
//! bound above `i64::MAX`, an Integer bound outside `i32`'s range, and a
//! float in a location field all load. Before this change the generated
//! types used `i32`/`i64`, so these failed to deserialise even though the
//! TS reference accepts them (accordproject/concerto-rust#90).
//!
//! Checked against the frozen TS 5.0.0 reference in
//! `migration/oracle/reference` (the workspace at `/home/user/concerto`):
//! `ModelManager.fromAst` (the same code path `AstModelManager` uses to
//! load an AST model, not the stricter `Serializer.fromJSON` instance
//! validation) accepts each AST below and returns a `NumberValidator`/
//! `Position` carrying the exact same value asserted here — `upper:
//! 1e19` stays `10000000000000000000`, `upper: (i32::MAX as i64) + 1`
//! stays `2147483648`, and `lower: 0.5, upper: 10.75` load unchanged.

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

/// A Long domain bound above `i64::MAX`. JS rounds `1e19` to the nearest
/// representable f64, which is what the TS reference sees and validates
/// against; the Rust type must keep the same value.
#[test]
fn int_dom_big_a_long_bound_above_i64_max_loads() {
    let ast = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.LongDomainValidator",
        "lower": 0,
        "upper": 1e19
    });
    let validator: mm::LongDomainValidator = serde_json::from_value(ast).unwrap();
    assert_eq!(validator.upper, Some(1e19));
    assert!(1e19_f64 > i64::MAX as f64, "the bound must exceed i64::MAX");
}

/// An Integer domain bound that overflows `i32`.
#[test]
fn int_dom_big_an_integer_bound_above_i32_max_loads() {
    let ast = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.IntegerDomainValidator",
        "lower": 0,
        "upper": (i32::MAX as i64) + 1
    });
    let validator: mm::IntegerDomainValidator = serde_json::from_value(ast).unwrap();
    assert_eq!(validator.upper, Some((i32::MAX as f64) + 1.0));
}

/// An Integer domain bound given as a float, as TS would accept from any
/// JS number.
#[test]
fn int_dom_float_an_integer_bound_given_as_a_float_loads() {
    let ast = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.IntegerDomainValidator",
        "lower": 0.5,
        "upper": 10.75
    });
    let validator: mm::IntegerDomainValidator = serde_json::from_value(ast).unwrap();
    assert_eq!(validator.lower, Some(0.5));
    assert_eq!(validator.upper, Some(10.75));
}

/// A float in a `Position` location field (`line`, `column`, `offset` are
/// all Integer fields in the metamodel).
#[test]
fn loc_float_a_float_in_a_position_field_loads() {
    let ast = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.Position",
        "line": 1.5,
        "column": 2.25,
        "offset": 3.75
    });
    let position: mm::Position = serde_json::from_value(ast).unwrap();
    assert_eq!(position.line, 1.5);
    assert_eq!(position.column, 2.25);
    assert_eq!(position.offset, 3.75);
}

/// Round-tripping a widened value keeps it exactly, as JS numbers would.
#[test]
fn a_widened_long_bound_round_trips_exactly() {
    let ast = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.LongDomainValidator",
        "lower": 0,
        "upper": 1e19
    });
    let validator: mm::LongDomainValidator = serde_json::from_value(ast.clone()).unwrap();
    let back = serde_json::to_value(&validator).unwrap();
    assert_eq!(back["upper"].as_f64(), Some(1e19));
}
