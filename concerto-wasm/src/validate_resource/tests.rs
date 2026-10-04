// Host-side tests of the decoder alone (no `js_sys` call is reached).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::*;

fn str_bytes(out: &mut Vec<u8>, s: &str) {
    out.extend_from_slice(&(s.len() as u32).to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

#[test]
fn decodes_every_tag() {
    let mut b = vec![7];
    b.extend_from_slice(&7u32.to_le_bytes());
    str_bytes(&mut b, "n");
    b.push(0);
    str_bytes(&mut b, "f");
    b.push(1);
    str_bytes(&mut b, "t");
    b.push(2);
    str_bytes(&mut b, "d");
    b.push(3);
    b.extend_from_slice(&1.5f64.to_le_bytes());
    str_bytes(&mut b, "i");
    b.push(4);
    b.extend_from_slice(&(-7i32).to_le_bytes());
    str_bytes(&mut b, "s");
    b.push(5);
    str_bytes(&mut b, "é");
    str_bytes(&mut b, "a");
    b.push(6);
    b.extend_from_slice(&2u32.to_le_bytes());
    b.push(3);
    b.extend_from_slice(&2.0f64.to_le_bytes());
    b.push(0);
    let v = decode(&b).ok().unwrap();
    assert_eq!(
        v,
        concerto_core::json!({"n": null, "f": false, "t": true, "d": 1.5, "i": -7, "s": "é", "a": [2, null]})
    );
    // An integral double reads as an integer, as `js_number` spells it.
    assert!(v["a"][0].is_i64());
}

#[test]
fn repeated_key_keeps_first_position_and_last_value() {
    let mut b = vec![7];
    b.extend_from_slice(&3u32.to_le_bytes());
    for (k, v) in [("a", 1u8), ("b", 2), ("a", 1)] {
        str_bytes(&mut b, k);
        b.push(v);
    }
    let v = decode(&b).ok().unwrap();
    let keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["a", "b"]);
    assert_eq!(v["a"], Value::Bool(false));
}

/// How deep a value may nest before the transport gives up on it (the
/// TS visitor then runs instead): concerto-core's reader's limit.
const MAX_DEPTH: u32 = 512;

#[test]
fn rejects_malformed_input() {
    // Truncated, unknown tag, trailing bytes, bad UTF-8, a huge count.
    for b in [
        vec![],
        vec![3, 0, 0],
        vec![9],
        vec![0, 0],
        vec![5, 1, 0, 0, 0, 0xff],
        vec![6, 0xff, 0xff, 0xff, 0xff],
    ] {
        assert!(decode(&b).is_err(), "{b:?}");
    }
    // Too deep.
    let mut deep = Vec::new();
    for _ in 0..=MAX_DEPTH + 1 {
        deep.push(6);
        deep.extend_from_slice(&1u32.to_le_bytes());
    }
    deep.push(0);
    assert!(decode(&deep).is_err());
}

#[test]
fn codes() {
    assert_eq!(code_of(Ok(Ok(()))), CODE_VALID);
    let validation: Error = concerto_core::error::ContractError::pre_port(
        ErrorKind::Validation,
        "bad".to_string(),
        None,
    )
    .into();
    assert_eq!(code_of(Ok(Err(validation))), CODE_VALIDATION);
    // A validator's `Validation` error (BC-39) keeps its `errorType`
    // through the full `throw` path.
    let mut validator = concerto_core::error::ContractError::pre_port(
        ErrorKind::Validation,
        "too long".to_string(),
        None,
    );
    validator.validator = Some(concerto_core::error::ValidatorReport {
        id: "null".to_string(),
        fqn: "org.acme@1.0.0.C.s".to_string(),
        error_type: "DefaultValidatorException",
    });
    assert_eq!(code_of(Ok(Err(validator.into()))), CODE_ERROR);
    let other: Error = concerto_core::error::ContractError::pre_port(
        ErrorKind::TypeNotFound,
        "missing".to_string(),
        None,
    )
    .into();
    assert_eq!(code_of(Ok(Err(other))), CODE_ERROR);
    // An unsupported value keeps no error, and empties the slot of any
    // error an earlier call left in it, so TS makes no call to clear it.
    assert_eq!(code_of(Err(Unsupported)), CODE_UNSUPPORTED);
    assert_eq!(validate_error_message(), "");
}
