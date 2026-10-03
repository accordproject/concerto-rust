// Host-side tests of the pure wire codec (no `js_sys` call is reached on
// these paths): `cargo test` from concerto-wasm/. A test may unwrap,
// index and panic: the crate's deny list guards the boundary path, where
// a panic poisons the object (P5-104: `clippy --all-targets` clean).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use super::*;

/// The reference the extract and decorate writers are checked against
/// (P5-104, D-11: test-only since the intermediate-`Value` fallbacks went):
/// a native `ModelManager`'s own models (the system ones included, in load
/// order) as `{ $class, models }` — the shape
/// `BaseModelManager.getAst`/`fromAst` (`src/basemodelmanager.ts`) use. The
/// view's own `fromAst` filters the system ones back out (`EXCLUDE_NS`)
/// exactly as it already does for the ts-mode `decorateModels`/`extract*`
/// bodies, so this need not filter them here.
fn model_manager_to_ast(mm: &ModelManager) -> Value {
    let models: Vec<Value> = mm.model_files().map(|mf| mf.ast().clone()).collect();
    json!({
        "$class": "concerto.metamodel@1.0.0.Models",
        "models": models,
    })
}

/// [`staged_header_from_parts`]'s header as a `Value` (P5-76: it is
/// serialized without one).
fn header_value(namespace: &str, imports: Option<&Value>) -> Option<Value> {
    staged_header_from_parts(namespace, imports)
        .map(|header| serde_json::to_value(header).expect("a header serializes"))
}

/// The JSON text of a full extract result (`{modelManager,
/// decoratorCommandSet, vocabularies}`, then the resident path's
/// `staged` and `validated` keys), written from the borrowed
/// [`dcs::extractor::ExtractResult`]: the text the extract
/// bindings wrote before the memo (P5-41, P5-57), kept as the test
/// oracle of [`DcsExtractKept::result_text`] (P5-103 removed the
/// bindings).
fn extract_result_text(
    result: &dcs::extractor::ExtractResult,
    staged: Option<&[Value]>,
) -> serde_json::Result<String> {
    let mut out = Vec::new();
    out.extend_from_slice(b"{\"modelManager\":");
    serde_json::to_writer(&mut out, &model_manager_to_ast(&result.model_manager))?;
    out.extend_from_slice(b",\"decoratorCommandSet\":");
    out.extend_from_slice(result.decorator_command_set.as_bytes());
    out.extend_from_slice(b",\"vocabularies\":");
    serde_json::to_writer(&mut out, &result.vocabularies)?;
    if let Some(staged) = staged {
        out.extend_from_slice(b",\"staged\":");
        serde_json::to_writer(&mut out, staged)?;
        out.extend_from_slice(b",\"validated\":true");
    }
    out.push(b'}');
    Ok(String::from_utf8(out).unwrap())
}

/// P5-56 (T2, F-A2): a repeated extract through the memo writes, byte
/// for byte, the text a full extract writes (result AST, command sets,
/// vocabularies, staged ids and headers), for every action and locale,
/// with `removeDecoratorsFromModel` false and (P5-77) true, and stages
/// files whose ASTs equal the full extract's; moving the epoch drops the
/// memo.
#[test]
fn the_extract_memo_writes_what_a_full_extract_writes() {
    let mm = |v: &str| json!([{"$class": "concerto.metamodel@1.0.0.DecoratorString", "value": v}]);
    let dec = |name: &str, args: Value| json!({"$class": "concerto.metamodel@1.0.0.Decorator", "name": name, "arguments": args});
    let mut handle = ModelManagerHandle::new().unwrap();
    handle
        .manager
        .add_model_ast(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.memo@1.0.0",
                "decorators": [dec("Term", mm("Memo")), dec("Tag", mm("m"))],
                "declarations": [{
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Person", "isAbstract": false,
                    "decorators": [dec("Term", mm("A person")), dec("Flag", json!([]))],
                    "properties": [{
                        "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "name", "isArray": false, "isOptional": false,
                        "decorators": [dec("Term_description", mm("The name")), dec("Tag", mm("n"))]
                    }]
                }]
            }),
            None,
        )
        .unwrap();
    for (action, remove) in [
        dcs::extractor::Action::ExtractAll,
        dcs::extractor::Action::ExtractVocab,
        dcs::extractor::Action::ExtractNonVocab,
    ]
    .into_iter()
    .flat_map(|action| [(action, false), (action, true)])
    {
        let fill = dcs::ExtractOptions {
            remove_decorators_from_model: remove,
            ..dcs::ExtractOptions::default()
        };
        let result = dcs::extract(&handle.manager, &fill, action, true).unwrap();
        let kept = DcsExtractKept::new(result.source_models.unwrap(), result.model_manager)
            .unwrap_or_else(|_| panic!("the result compacts"));
        for locale in ["en", "fr"] {
            let opts = dcs::ExtractOptions {
                remove_decorators_from_model: remove,
                locale: locale.to_string(),
            };
            let full = dcs::extract(&handle.manager, &opts, action, false).unwrap();
            let mut t1 = ModelManagerHandle::new().unwrap();
            let mut t2 = ModelManagerHandle::new().unwrap();
            let staged = stage_result(&mut t1, &full.model_manager);
            let expected = extract_result_text(&full, Some(&staged)).unwrap();
            let (sets, vocabularies) =
                dcs::encode_extract_source(&kept.source, &opts, action).unwrap();
            let staged = kept.stage(&mut t2);
            let got = kept.result_text(&sets, &vocabularies, &staged).unwrap();
            assert_eq!(got, expected, "{action:?} {remove} {locale}");
            assert_eq!(t2.staged.files.len(), t1.staged.files.len());
            assert!(!t2.staged.files.is_empty());
            for (a, b) in t1.staged.files.values().zip(t2.staged.files.values()) {
                assert_eq!(a.namespace(), b.namespace());
                assert_eq!(a.ast(), b.ast(), "{action:?} {remove} {locale}");
            }
        }
    }
    *handle.dcs_memo.get_mut() = Some(DcsExtractMemo {
        key: (handle.epoch, true, None),
        kept: None,
    });
    handle.bump_epoch();
    assert!(handle.dcs_memo.get_mut().is_none());
}

/// P5-94: the flat staging result, read back the way the TS side reads
/// it (the implicit import's short names appended for a non-system
/// file), is the `{"id", "header"}` result the removed text staging
/// bindings returned (P5-103), for a canonical file, a
/// system file, an unversioned system file and a file with no header.
#[test]
fn flat_staged_text_reads_back_as_the_staged_result() {
    fn read_back(text: &str) -> Value {
        let flat: Vec<Value> = serde_json::from_str(text).unwrap();
        if flat.len() == 1 {
            return json!({"id": flat[0], "header": null});
        }
        let system = flat[3].as_bool().unwrap();
        let n = flat[4].as_u64().unwrap() as usize;
        let mut short_names: Vec<Value> = (0..n)
            .map(|i| json!([flat[5 + 2 * i], flat[6 + 2 * i]]))
            .collect();
        if !system {
            short_names.extend(
                IMPLICIT_IMPORT_SHORT_NAMES
                    .iter()
                    .map(|(key, name)| json!([key, name])),
            );
        }
        let uri_map: Vec<Value> = flat[5 + 2 * n..]
            .chunks(2)
            .map(|pair| json!([pair[0], pair[1]]))
            .collect();
        json!({"id": flat[0], "header": {
            "namespace": flat[1], "version": flat[2], "system": system,
            "shortNames": short_names, "uriMap": uri_map,
        }})
    }
    let imports = json!([
        {"$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "org.a@1.0.0", "name": "A", "uri": "https://a"},
        {"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "org.b@2.0.0", "types": ["B", "C"],
         "aliasedTypes": [{"$class": "concerto.metamodel@1.0.0.AliasedType", "name": "C", "aliasedName": "D"}],
         "uri": "https://b"},
    ]);
    let system_imports = json!([
        {"$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "concerto.decorator@1.0.0", "name": "DotNetNamespace"},
    ]);
    let cases: [(&str, Option<&Value>); 5] = [
        ("org.x@1.0.0", Some(&imports)),
        ("org.y@1.0.0", None),
        ("concerto@1.0.0", Some(&system_imports)),
        ("concerto", None),
        ("org.unversioned", None),
    ];
    for (id, (namespace, imports)) in cases.into_iter().enumerate() {
        let id = id as u32;
        let header = staged_header_from_parts(namespace, imports);
        let object = json!({"id": id, "header": header_value(namespace, imports)});
        let flat = flat_staged_text(id, header.as_ref()).unwrap();
        assert_eq!(read_back(&flat), object, "{namespace}");
    }
    assert_eq!(flat_staged_text(7, None).unwrap(), "[7]");
}

/// P5-28: [`staged_header_from_parts`] of a canonical file gives what
/// `modelFileFromAstHeader` sets: the version, the short names in
/// order (an alias in place of its type's name, the implicit system
/// import last) and the URI map keyed by each import's first name.
#[test]
fn staged_header_reads_a_canonical_file() {
    let imports = json!([
        {"$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "org.a@1.0.0", "name": "A", "uri": "https://a"},
        {"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "org.b@2.0.0", "types": ["B", "C"],
         "aliasedTypes": [{"$class": "concerto.metamodel@1.0.0.AliasedType", "name": "C", "aliasedName": "D"}],
         "uri": "https://b"},
    ]);
    let header = header_value("org.x@1.0.0", Some(&imports)).unwrap();
    assert_eq!(
        header,
        json!({
            "namespace": "org.x@1.0.0",
            "version": "1.0.0",
            "system": false,
            "shortNames": [
                ["A", "org.a@1.0.0.A"],
                ["B", "org.b@2.0.0.B"],
                ["D", "org.b@2.0.0.C"],
                ["Concept", "concerto@1.0.0.Concept"],
                ["Asset", "concerto@1.0.0.Asset"],
                ["Transaction", "concerto@1.0.0.Transaction"],
                ["Participant", "concerto@1.0.0.Participant"],
                ["Event", "concerto@1.0.0.Event"],
            ],
            "uriMap": [["org.a@1.0.0.A", "https://a"], ["org.b@2.0.0.B", "https://b"]],
        })
    );
}

/// P5-28: a system file has no implicit import, and an unversioned
/// system namespace gives a `null` version; no `imports` node is none.
#[test]
fn staged_header_reads_a_system_file() {
    // The unversioned `concerto` parses as a name only, so it gets no
    // header and the view reads it through `modelFileFromAstHeader`, as
    // for any other unversioned namespace (P5-101: this test expected
    // a header, which `staged_header_from_parts` has never given; it
    // failed on the integration head before this change).
    assert_eq!(header_value("concerto", None), None);
    let header = header_value("concerto@1.0.0", Some(&Value::Null)).unwrap();
    assert_eq!(header["version"], json!("1.0.0"));
    assert_eq!(header["system"], json!(true));
    assert_eq!(header["shortNames"], json!([]));
}

/// P5-28: anything `modelFileFromAstHeader` would throw for, or read
/// from a shape other than the canonical one, gives no header, so the
/// view calls that binding over the JS values as before.
#[test]
fn staged_header_declines_what_the_binding_would_not_simply_set() {
    let one = |imp: Value| header_value("org.x@1.0.0", Some(&json!([imp])));
    assert!(
        header_value("org.x", None).is_none(),
        "unversioned namespace"
    );
    assert!(
        header_value("org.1x@1.0.0", None).is_none(),
        "invalid namespace part"
    );
    assert!(
        header_value("org.x@1.0.0", Some(&json!({}))).is_none(),
        "non-array imports"
    );
    assert!(
        one(json!({"$class": "concerto.metamodel@1.0.0.ImportAll", "namespace": "org.a@1.0.0"}))
            .is_none()
    );
    assert!(one(json!({"$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "org.a", "name": "A"})).is_none());
    assert!(
        one(json!({"$class": "ImportType", "namespace": "org.a@1.0.0", "name": "A"})).is_none()
    );
    assert!(one(json!({"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "org.a@1.0.0", "types": ["A", 1]})).is_none());
    assert!(
        one(json!({"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "org.a@1.0.0"}))
            .is_none()
    );
    assert!(one(json!({"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "org.a@1.0.0", "types": [], "uri": "u"})).is_none());
    assert!(one(json!({"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "org.a@1.0.0", "types": ["A"],
        "aliasedTypes": [{"name": "A", "aliasedName": "String"}]})).is_none());
    assert!(one(json!({"$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "org.a@1.0.0", "name": "A", "uri": 1})).is_none());
    assert!(one(json!({"$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "org.a@1.0.0", "name": "A", "uri": ""})).is_some());
}

/// `decode_wire`, which must succeed (`Error` has no `Debug`).
fn decoded(value: &Value) -> CoreValue {
    match decode_wire(value) {
        Ok(v) => v,
        Err(_) => panic!("decode_wire failed on {value}"),
    }
}

/// The number `decode_wire` reads from `text`, the JSON a view's
/// `JSON.stringify` wrote.
fn wire_number(text: &str) -> f64 {
    let value: Value = serde_json::from_str(text).unwrap();
    match decoded(&value) {
        CoreValue::Number(n) => n,
        other => panic!("expected a number, got {other:?}"),
    }
}

/// P4-10 review: without serde_json's `float_roundtrip` feature these
/// two doubles came back 1 ULP off (…888 and …2917).
#[test]
fn decode_wire_keeps_doubles_exact() {
    for n in [989.9951327998887_f64, 477.95269883162916_f64] {
        let text = serde_json::to_string(&n).unwrap();
        assert_eq!(wire_number(&text).to_bits(), n.to_bits(), "{text}");
    }
    assert_eq!(
        wire_number("989.9951327998887").to_bits(),
        989.9951327998887_f64.to_bits()
    );
    assert_eq!(
        wire_number("477.95269883162916").to_bits(),
        477.95269883162916_f64.to_bits()
    );
}

/// A deterministic pseudo-random sample of finite doubles (xorshift64
/// over raw bit patterns, so every exponent range is hit): each one
/// written in shortest round-trip form (what JS `JSON.stringify` writes,
/// and what `serde_json`/ryu writes too) decodes to the same bits, and
/// encodes back to the same text.
#[test]
fn decode_wire_round_trips_a_random_sample_of_doubles() {
    let mut state: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut checked = 0;
    while checked < 200_000 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let n = f64::from_bits(state);
        if !n.is_finite() {
            continue;
        }
        let text = serde_json::to_string(&n).unwrap();
        let back = wire_number(&text);
        if n == 0.0 {
            // `-0` crosses as a tagged wire number, never plain JSON.
            assert_eq!(back, 0.0);
        } else {
            assert_eq!(back.to_bits(), n.to_bits(), "{text}");
            assert_eq!(
                serde_json::to_string(&encode_wire(&CoreValue::Number(back))).unwrap(),
                text
            );
        }
        checked += 1;
    }
}

/// The same doubles inside a document, as `serializerFromJsonCompact` and the
/// per-field bindings decode it.
#[test]
fn decode_wire_keeps_nested_doubles_exact() {
    let value: Value = serde_json::from_str(
        r#"{"$class":"org.x@1.0.0.T","d":989.9951327998887,"ds":[477.95269883162916]}"#,
    )
    .unwrap();
    let CoreValue::Object(map) = decoded(&value) else {
        panic!("expected an object");
    };
    assert_eq!(map.get("d"), Some(&CoreValue::Number(989.9951327998887)));
    assert_eq!(
        map.get("ds"),
        Some(&CoreValue::Array(vec![CoreValue::Number(
            477.95269883162916
        )]))
    );
}

/// A wire `bigint` round-trips through `decode_wire`/`encode_wire`
/// (task P2-11b-U6): the digit string crosses unchanged in both
/// directions.
#[test]
fn wire_bigint_round_trips() {
    let value: Value = serde_json::from_str(r#"{"@@oracle":"bigint","value":"10"}"#).unwrap();
    assert_eq!(decoded(&value), CoreValue::BigInt("10".to_string()));
    assert_eq!(encode_wire(&CoreValue::BigInt("10".to_string())), value);
}

/// P5-02 review (accordproject/concerto-rust#73): a lone (unpaired)
/// UTF-16 surrogate escape is replaced with the `�` escape, which
/// `serde_json` accepts, while every other field and a genuine
/// surrogate *pair* survive untouched.
#[test]
fn sanitize_lone_surrogate_escapes_replaces_only_unpaired_ones() {
    let fffd_escape = "\\uFFFD"; // literal 6-char JSON escape, not U+FFFD itself
    // Lone high surrogate: replaced.
    assert_eq!(
        sanitize_lone_surrogate_escapes(r#"{"name":"\ud800"}"#),
        format!(r#"{{"name":"{fffd_escape}"}}"#)
    );
    // Lone low surrogate: replaced.
    assert_eq!(
        sanitize_lone_surrogate_escapes(r#"{"name":"\udc00"}"#),
        format!(r#"{{"name":"{fffd_escape}"}}"#)
    );
    // A valid surrogate pair (U+1F389, a real astral character): left
    // exactly as-is.
    assert_eq!(
        sanitize_lone_surrogate_escapes(r#"{"name":"🎉"}"#),
        r#"{"name":"🎉"}"#
    );
    // High surrogate followed by a non-surrogate escape: replaced, and
    // the following escape is unaffected.
    assert_eq!(
        sanitize_lone_surrogate_escapes(r#"{"name":"\ud800\n"}"#),
        format!(r#"{{"name":"{fffd_escape}\n"}}"#)
    );
    // No escapes at all: unchanged.
    assert_eq!(
        sanitize_lone_surrogate_escapes(r#"{"a":"b","n":1}"#),
        r#"{"a":"b","n":1}"#
    );
}

/// The sanitized text always reparses, and a lone surrogate's field
/// becomes the literal U+FFFD character rather than vanishing.
#[test]
fn sanitize_lone_surrogate_escapes_output_is_valid_json() {
    let sanitized = sanitize_lone_surrogate_escapes(r#"{"name":"\ud800","ok":true}"#);
    let value: Value = serde_json::from_str(&sanitized).expect("sanitized text must parse");
    assert_eq!(
        value["name"],
        json!(char::from_u32(0xFFFD).unwrap().to_string())
    );
    assert_eq!(value["ok"], json!(true));
}

/// The wire documents `parse_wire`/`WireOut` are checked on: every
/// kind, nested, plus plain JSON, a tag that is not a string, and a
/// duplicate key.
const WIRE_SAMPLES: &[&str] = &[
    r#"null"#,
    r#"true"#,
    r#"[1,-2,3.5,18446744073709551615,1e300,-0.0,0]"#,
    r#""a \"string\" with é and 🎉""#,
    r#"{"b":1,"a":[{"x":null}],"b":2}"#,
    r#"{"@@oracle":1,"x":"y"}"#,
    r#"{"@@oracle":"undefined"}"#,
    r#"[{"@@oracle":"number","value":"NaN"},{"@@oracle":"number","value":"-Infinity"},{"@@oracle":"number","value":"Infinity"},{"@@oracle":"number","value":"-0"}]"#,
    r#"{"@@oracle":"bigint","value":"123456789012345678901234567890"}"#,
    r#"{"@@oracle":"map","entries":[["k",{"@@oracle":"undefined"}],[{"@@oracle":"number","value":"NaN"},[1,2]]]}"#,
    r#"[{"@@oracle":"dayjs","valid":false},{"@@oracle":"dayjs","valid":true,"ms":1700000000123,"utcOffset":120},{"@@oracle":"dayjs","valid":true,"ms":0}]"#,
    r#"{"@@oracle":"typed","ctor":"ValidatedResource","fqn":"org.acme@1.0.0.Item","fields":{"$namespace":"org.acme@1.0.0","$type":"Item","$identifierFieldName":"id","$identifier":"i1","id":"i1","$timestamp":null,"n":{"@@oracle":"number","value":"NaN"},"child":{"@@oracle":"typed","ctor":"Relationship","fqn":"org.acme@1.0.0.Other","fields":{"$class":"org.acme@1.0.0.Other"}},"labels":["a","b"]}}"#,
    r#"{"@@oracle":"typed","ctor":"Resource","fqn":"org.acme@1.0.0.Item","fields":{}}"#,
];

/// P5-16: `parse_wire` reads every sample as `decode_wire` does, and
/// `WireOut` writes each decoded value as `encode_wire` plus `snapshot`
/// do, byte for byte.
#[test]
fn parse_wire_and_wire_out_match_the_value_route() {
    for text in WIRE_SAMPLES {
        let value: Value = serde_json::from_str(text).unwrap();
        let expected = decoded(&value);
        let parsed = match parse_wire(text) {
            Ok(v) => v,
            Err(_) => panic!("parse_wire failed on {text}"),
        };
        let via_value = serde_json::to_string(&encode_wire(&expected)).unwrap();
        assert_eq!(
            serde_json::to_string(&encode_wire(&parsed)).unwrap(),
            via_value,
            "{text}"
        );
        assert_eq!(
            serde_json::to_string(&WireOut::<false>(&parsed)).unwrap(),
            via_value,
            "{text}"
        );
        if let CoreValue::Instance(instance) = &parsed {
            assert_eq!(
                serde_json::to_string(&WireInstanceOut::<false>(instance)).unwrap(),
                serde_json::to_string(&encode_wire_instance(instance)).unwrap(),
                "{text}"
            );
        }
    }
}

/// P5-16: a wire shape the codec does not recognise fails `parse_wire`
/// as it fails `decode_wire`, anywhere in the document.
#[test]
fn parse_wire_rejects_what_decode_wire_rejects() {
    for text in [
        r#"{"@@oracle":"bogus"}"#,
        r#"[1,{"@@oracle":"number","value":"1"}]"#,
        r#"{"@@oracle":"number"}"#,
        r#"{"@@oracle":"bigint","value":1}"#,
        r#"{"@@oracle":"map"}"#,
        r#"{"@@oracle":"map","entries":[1]}"#,
        r#"{"@@oracle":"map","entries":[[]]}"#,
        r#"{"@@oracle":"map","entries":[["k"]]}"#,
        r#"{"@@oracle":"dayjs","valid":true}"#,
        r#"{"@@oracle":"typed","ctor":"Other","fqn":"a.B","fields":{}}"#,
        r#"{"@@oracle":"typed","ctor":"Resource","fields":{}}"#,
        r#"{"@@oracle":"typed","ctor":"Resource","fqn":"a.B"}"#,
        r#"{"x":{"a":[{"@@oracle":"typed","ctor":"Resource","fqn":"a.B","fields":{"y":{"@@oracle":"nope"}}}]}}"#,
    ] {
        let value: Value = serde_json::from_str(text).unwrap();
        assert!(decode_wire(&value).is_err(), "decode_wire accepted {text}");
        assert!(parse_wire(text).is_err(), "parse_wire accepted {text}");
    }
}

/// `value` in concerto-core's compact layout, as the TS writer
/// (src/engine/wire.ts) writes it: an `i32` for a number that fits one,
/// a double otherwise.
fn compact_bytes(value: &Value, out: &mut Vec<u8>) {
    let raw_str = |s: &str, out: &mut Vec<u8>| {
        out.extend_from_slice(&u32::try_from(s.len()).unwrap().to_le_bytes());
        out.extend_from_slice(s.as_bytes());
    };
    match value {
        Value::Null => out.push(0),
        Value::Bool(b) => out.push(if *b { 2 } else { 1 }),
        Value::Number(n) => match n.as_i64().and_then(|i| i32::try_from(i).ok()) {
            Some(i) => {
                out.push(4);
                out.extend_from_slice(&i.to_le_bytes());
            }
            None => {
                out.push(3);
                out.extend_from_slice(&n.as_f64().unwrap().to_le_bytes());
            }
        },
        Value::String(s) => {
            out.push(5);
            raw_str(s, out);
        }
        Value::Array(items) => {
            out.push(6);
            out.extend_from_slice(&u32::try_from(items.len()).unwrap().to_le_bytes());
            for item in items {
                compact_bytes(item, out);
            }
        }
        Value::Object(map) => {
            out.push(7);
            out.extend_from_slice(&u32::try_from(map.len()).unwrap().to_le_bytes());
            for (key, item) in map {
                raw_str(key, out);
                compact_bytes(item, out);
            }
        }
    }
}

/// P5-101 (E-7): `parse_wire_bytes` reads every sample, written in the
/// compact layout, as `parse_wire` reads its text, and rejects what
/// `parse_wire` rejects; bytes not in the layout are rejected too.
#[test]
fn parse_wire_bytes_matches_parse_wire() {
    // A `-0.0` literal is not text `JSON.stringify` writes (it writes
    // `0`, and the TS writers send `-0` tagged), and the layout reads a
    // double as that text reads (`-0` as `0`), so such a sample is left
    // out.
    for text in WIRE_SAMPLES.iter().filter(|text| !text.contains("-0.0")) {
        let value: Value = serde_json::from_str(text).unwrap();
        let mut bytes = Vec::new();
        compact_bytes(&value, &mut bytes);
        let (Ok(from_text), Ok(from_bytes)) = (parse_wire(text), parse_wire_bytes(&bytes)) else {
            panic!("a sample not read: {text}");
        };
        assert_eq!(
            serde_json::to_string(&WireOut::<false>(&from_bytes)).unwrap(),
            serde_json::to_string(&WireOut::<false>(&from_text)).unwrap(),
            "{text}"
        );
        assert_eq!(
            concerto_core::introspect::compact_value(&bytes).unwrap(),
            value,
            "{text}"
        );
    }
    for text in [
        r#"{"@@oracle":"bogus"}"#,
        r#"[1,{"@@oracle":"number","value":"1"}]"#,
        r#"{"@@oracle":"dayjs","valid":true}"#,
        r#"{"x":{"a":[{"@@oracle":"typed","ctor":"Resource","fqn":"a.B","fields":{"y":{"@@oracle":"nope"}}}]}}"#,
    ] {
        let mut bytes = Vec::new();
        compact_bytes(&serde_json::from_str(text).unwrap(), &mut bytes);
        assert!(parse_wire_bytes(&bytes).is_err(), "{text}");
    }
    for bytes in [&[][..], &[9][..], &[0, 0][..], &[5, 1, 0, 0, 0, 0xff][..]] {
        assert!(parse_wire_bytes(bytes).is_err(), "{bytes:?}");
    }
}

/// P5-16: `CompactInstanceOut` writes the header values by position and
/// the other fields in order, less the ones the view skips.
#[test]
fn compact_instance_out_moves_the_header_out_of_the_fields() {
    let text = r#"{"@@oracle":"typed","ctor":"ValidatedResource","fqn":"org.acme@1.0.0.Item","fields":{"$namespace":"org.acme@1.0.0","$type":"Item","$identifierFieldName":"id","$identifier":"i1","id":"i1","$timestamp":{"@@oracle":"dayjs","valid":true,"ms":5,"utcOffset":0},"n":{"@@oracle":"number","value":"NaN"},"$class":"x","child":{"@@oracle":"typed","ctor":"Relationship","fqn":"org.acme@1.0.0.Other","fields":{"$class":"org.acme@1.0.0.Other"}},"labels":["a","b"]}}"#;
    let CoreValue::Instance(instance) = parse_wire(text).ok().unwrap() else {
        panic!("not an instance");
    };
    assert_eq!(
        serde_json::to_string(&CompactInstanceOut(&instance)).unwrap(),
        r#"["ValidatedResource","org.acme@1.0.0.Item","org.acme@1.0.0","Item","id","i1",{"@@oracle":"dayjs","valid":true,"ms":5.0,"utcOffset":0.0},{"n":{"@@oracle":"number","value":"NaN"},"child":{"@@oracle":"typed","ctor":"Relationship","fqn":"org.acme@1.0.0.Other","fields":{"$class":"org.acme@1.0.0.Other"}},"labels":["a","b"]}]"#
    );
    // A missing header value is written as `undefined`, and with no
    // string `$identifierFieldName` no other key is dropped.
    let text = r#"{"@@oracle":"typed","ctor":"Resource","fqn":"a.B","fields":{"$identifierFieldName":1,"1":true}}"#;
    let CoreValue::Instance(instance) = parse_wire(text).ok().unwrap() else {
        panic!("not an instance");
    };
    let undefined = r#"{"@@oracle":"undefined"}"#;
    assert_eq!(
        serde_json::to_string(&CompactInstanceOut(&instance)).unwrap(),
        format!(
            r#"["Resource","a.B",{undefined},{undefined},1,{undefined},{undefined},{{"1":true}}]"#
        )
    );
}

/// P5-16: `WireOut::<true>` writes an integral number as an integer
/// literal, which reads back as the same double, and every other number
/// exactly as `WireOut::<false>` does.
#[test]
fn wire_out_ints_reads_back_the_same_numbers() {
    for n in [
        0.0,
        1.0,
        -1.0,
        42.0,
        1.5,
        -3.25,
        9_007_199_254_740_991.0,
        -9_007_199_254_740_991.0,
        9_007_199_254_740_992.0,
        1e21,
        1e-7,
        5e-324,
        f64::MAX,
    ] {
        let value = CoreValue::Number(n);
        let ints = serde_json::to_string(&WireOut::<true>(&value)).unwrap();
        let plain = serde_json::to_string(&WireOut::<false>(&value)).unwrap();
        let back: f64 = serde_json::from_str(&ints).unwrap();
        assert_eq!(back.to_bits(), n.to_bits(), "{ints}");
        if n.fract() == 0.0 && n.abs() < 9_007_199_254_740_992.0 {
            assert!(!ints.contains('.') && !ints.contains('e'), "{ints}");
        } else {
            assert_eq!(ints, plain);
        }
    }
}

/// P5-101 (D-3, E-7): the merged options read once per options text
/// ([`SerializerOptionsEntry`]) give `validateInstance`'s walk the same
/// reading from `fromJSON`'s wire encoding (`-0` tagged, an `undefined`
/// option kept as a tag) as from `JSON.stringify`'s text of the same
/// options, and `from_json` the serializer's merged defaults.
#[test]
fn serializer_options_read_wire_and_plain_text_alike() {
    let wire = SerializerOptionsEntry::new(
        r#"{"validate":true,"utcOffset":{"@@oracle":"number","value":"-0"},"strictQualifiedDateTimes":{"@@oracle":"undefined"},"rejectUnknownKeys":true}"#,
    )
    .unwrap_or_else(|_| panic!("wire options read"));
    let plain =
        SerializerOptionsEntry::new(r#"{"validate":true,"utcOffset":0,"rejectUnknownKeys":true}"#)
            .unwrap_or_else(|_| panic!("plain options read"));
    assert_eq!(wire.native, plain.native);
    assert!(wire.native.validate && wire.native.reject_unknown_keys);
    assert_eq!(
        wire.from_json,
        populator::from_json_options(&wire.serializer.default_options)
    );
    let none = SerializerOptionsEntry::new("null").unwrap_or_else(|_| panic!("null reads"));
    assert_eq!(
        none.native,
        native_from_json_options(&Value::Object(Default::default()))
    );
    assert!(SerializerOptionsEntry::new("[1]").is_err());
}

/// A model AST with these imports.
fn header_model(namespace: &str, imports: Value) -> Value {
    json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": namespace,
        "imports": imports,
        "declarations": [],
    })
}

/// P5-101 (D-4): the staged entry [`stage_shared`] writes for a model
/// file of a DecoratorManager result with this AST, as a value: its
/// header ([`staged_header_from_parts`]) in the flat layout, under the
/// stage id 0. `None` when there is no header.
fn dcs_entry(ast: &Value) -> Option<Value> {
    let namespace = ast.get("namespace")?.as_str()?;
    let header = staged_header_from_parts(namespace, ast.get("imports"))?;
    Some(
        serde_json::to_value(FlatStaged {
            id: 0,
            header: Some(&header),
        })
        .unwrap(),
    )
}

#[test]
fn a_dcs_staged_entry_has_the_version_and_leaves_out_the_implicit_import() {
    let entry = dcs_entry(&header_model("org.acme@1.2.3", json!([]))).unwrap();
    assert_eq!(entry, json!([0, "org.acme@1.2.3", "1.2.3", false, 0]));
    // An absent `imports` is read as none, as `ast.imports.concat` is skipped.
    let mut ast = header_model("org.acme@1.2.3", json!(null));
    ast.as_object_mut().unwrap().remove("imports");
    assert_eq!(dcs_entry(&ast).unwrap(), entry);
}

#[test]
fn a_dcs_staged_entry_maps_imports_in_order_with_aliases_and_uris() {
    let imports = json!([
        {
            "$class": "concerto.metamodel@1.0.0.ImportTypes",
            "namespace": "org.other@1.0.0",
            "types": ["A", "B"],
            "aliasedTypes": [
                {"$class": "concerto.metamodel@1.0.0.AliasedType", "name": "B", "aliasedName": "Bee"}
            ],
            "uri": "https://example.com/other.cto",
        },
        {
            "$class": "concerto.metamodel@1.0.0.ImportType",
            "namespace": "org.third@2.0.0",
            "name": "C",
            "uri": "https://example.com/third.cto",
        },
        {
            "$class": "concerto.metamodel@1.0.0.ImportTypes",
            "namespace": "org.fourth@1.0.0",
            "types": ["D"],
            "aliasedTypes": [],
            "uri": "",
        },
    ]);
    let entry = dcs_entry(&header_model("org.acme@1.0.0", imports)).unwrap();
    assert_eq!(
        entry,
        json!([
            0,
            "org.acme@1.0.0",
            "1.0.0",
            false,
            4,
            "A",
            "org.other@1.0.0.A",
            "Bee",
            "org.other@1.0.0.B",
            "C",
            "org.third@2.0.0.C",
            "D",
            "org.fourth@1.0.0.D",
            "org.other@1.0.0.A",
            "https://example.com/other.cto",
            "org.third@2.0.0.C",
            "https://example.com/third.cto",
        ])
    );
}

/// P5-101 (D-4): a system namespace has a header (no implicit import),
/// as for any other staged file; every other case the binding would
/// treat in its own way has none.
#[test]
fn a_dcs_staged_entry_leaves_every_other_case_to_the_binding() {
    assert_eq!(
        dcs_entry(&header_model("concerto@1.0.0", json!([]))).unwrap(),
        json!([0, "concerto@1.0.0", "1.0.0", true, 0])
    );
    let import = |value: Value| header_model("org.acme@1.0.0", json!([value]));
    let cases = [
        // Unversioned or invalid namespaces.
        header_model("org.acme", json!([])),
        header_model("org.1acme@1.0.0", json!([])),
        header_model("org.acme@x", json!([])),
        // Imports the binding rejects.
        import(json!({
            "$class": "concerto.metamodel@1.0.0.ImportType",
            "namespace": "org.other",
            "name": "A",
        })),
        import(json!({
            "$class": "concerto.metamodel@1.0.0.ImportAll",
            "namespace": "org.other@1.0.0",
        })),
        import(json!({
            "$class": "concerto.metamodel@1.0.0.ImportTypes",
            "namespace": "org.other@1.0.0",
            "types": ["A"],
            "aliasedTypes": [{"name": "A", "aliasedName": "String"}],
        })),
        // Values of another JSON type than the plain case reads.
        header_model("org.acme@1.0.0", json!({})),
        import(json!({
            "$class": "concerto.metamodel@1.0.0.ImportTypes",
            "namespace": "org.other@1.0.0",
            "types": [1],
        })),
        import(json!({
            "$class": "concerto.metamodel@1.0.0.ImportType",
            "namespace": "org.other@1.0.0",
            "name": "A",
            "uri": 1,
        })),
        import(json!({
            "$class": "concerto.metamodel@1.0.0.ImportTypes",
            "namespace": "org.other@1.0.0",
            "types": ["A"],
            "aliasedTypes": [{"name": "A", "aliasedName": null}],
        })),
    ];
    for ast in cases {
        assert_eq!(dcs_entry(&ast), None, "{ast}");
    }
}

/// P5-73 (accordproject/concerto-rust#414): the two fixed system model
/// texts get the header a load of them gives, and any other text, even the same model with other whitespace, key
/// order or a malformed node, gets none, so it is loaded and checked.
#[test]
fn system_model_header_is_only_for_the_exact_system_texts() {
    let texts = concerto_core::rootmodel::system_model_json_texts();
    for (file_name, text) in texts {
        let (file, imports) =
            ModelFile::from_json_text_with_imports(text, None, Some(file_name.into()))
                .unwrap()
                .unwrap();
        // P5-101 (D-4): the flat layout, its stage id 0.
        let expected = flat_staged_text(
            0,
            staged_header_from_parts(file.namespace(), imports.as_ref()).as_ref(),
        )
        .unwrap();
        assert!(expected.len() > "[0]".len(), "{expected}");
        let header = system_model_header(text).unwrap();
        assert_eq!(header, expected);

        let mut value: Value = serde_json::from_str(text).unwrap();
        let pretty = serde_json::to_string_pretty(&value).unwrap();
        assert_eq!(system_model_header(&pretty), None);
        value["decorators"] = json!("x");
        assert_eq!(system_model_header(&value.to_string()), None);
    }
    assert_eq!(system_model_header(""), None);
    assert_eq!(system_model_header(&format!("{} ", texts[1].1)), None);
}
