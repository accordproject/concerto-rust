use serde_json::Value;

use super::{ARRAY, F64, FALSE, I32, NULL, OBJECT, STR, TRUE, to_value};

/// `value` in the layout, as the TS writer (concerto-core
/// `src/engine/ast-codec.ts`) writes it: an `i32` for a number that
/// fits one, a double otherwise.
pub(crate) fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    write(value, &mut out);
    out
}

fn write_str(s: &str, out: &mut Vec<u8>) {
    out.extend_from_slice(&u32::try_from(s.len()).unwrap().to_le_bytes());
    out.extend_from_slice(s.as_bytes());
}

fn write(value: &Value, out: &mut Vec<u8>) {
    match value {
        Value::Null => out.push(NULL),
        Value::Bool(b) => out.push(if *b { TRUE } else { FALSE }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64().and_then(|i| i32::try_from(i).ok()) {
                out.push(I32);
                out.extend_from_slice(&i.to_le_bytes());
            } else {
                out.push(F64);
                out.extend_from_slice(&n.as_f64().unwrap().to_le_bytes());
            }
        }
        Value::String(s) => {
            out.push(STR);
            write_str(s, out);
        }
        Value::Array(items) => {
            out.push(ARRAY);
            out.extend_from_slice(&u32::try_from(items.len()).unwrap().to_le_bytes());
            for item in items {
                write(item, out);
            }
        }
        Value::Object(map) => {
            out.push(OBJECT);
            out.extend_from_slice(&u32::try_from(map.len()).unwrap().to_le_bytes());
            for (key, item) in map {
                write_str(key, out);
                write(item, out);
            }
        }
    }
}

/// The validator's reading is [`to_value`]'s, but for a double, spelt
/// as `instance::validate::js_number` spells a finite JS number: an
/// integer below `2^53` in magnitude, itself otherwise.
#[test]
fn to_validator_value_spells_doubles_as_the_validator_does() {
    use super::to_validator_value;
    use crate::instance::validate::js_number;
    for v in [
        -0.0,
        0.5,
        3.0e9,
        -3.0e9,
        2f64.powi(53) - 1.0,
        2f64.powi(53),
        2f64.powi(60),
        1e21,
        -2.5e-7,
    ] {
        let mut bytes = vec![F64];
        bytes.extend_from_slice(&v.to_le_bytes());
        assert_eq!(to_validator_value(&bytes).unwrap(), js_number(v), "{v}");
    }
    let value: Value =
        serde_json::from_str(r#"{"a":1,"b":[true,false,null,"x"],"c":{"d":-2}}"#).unwrap();
    assert_eq!(to_validator_value(&encode(&value)).unwrap(), value);
    assert!(to_validator_value(&[9]).is_err());
    assert!(to_validator_value(&[NULL, NULL]).is_err());
}

#[test]
fn decodes_the_document_json_text_gives() {
    for text in [
        r#"{"a":1,"b":[true,false,null],"c":"xé😀","d":{"e":-2,"f":1.5}}"#,
        r#"[0,-1,2147483647,-2147483648,2147483648,-2147483649,9007199254740992,-9007199254740992,1e21,1.7976931348623157e308,5e-324,0.1,-2.5e-7]"#,
        r#"{"z":1,"a":2,"$class":"x"}"#,
        "[]",
        "{}",
        r#""""#,
    ] {
        let value: Value = serde_json::from_str(text).unwrap();
        assert_eq!(to_value(&encode(&value)).unwrap(), value, "{text}");
    }
}

#[test]
fn integral_doubles_read_as_json_text_reads_them() {
    // What `JSON.stringify` writes for each double, read by serde_json.
    for (v, text) in [
        (-0.0, "0"),
        (3.0e9, "3000000000"),
        (-3.0e9, "-3000000000"),
        (2f64.powi(53), "9007199254740992"),
        (2f64.powi(53) + 2.0, "9007199254740994"),
        (2f64.powi(60), "1152921504606847000"),
        (-(2f64.powi(60)), "-1152921504606847000"),
        (2f64.powi(64), "18446744073709552000"),
        (-(2f64.powi(63)), "-9223372036854776000"),
        (1e20, "100000000000000000000"),
        (1e21, "1e+21"),
        (-1e21, "-1e+21"),
        (0.1, "0.1"),
        (1e-7, "1e-7"),
    ] {
        let mut bytes = vec![F64];
        bytes.extend_from_slice(&f64::to_le_bytes(v));
        let expected: Value = serde_json::from_str(text).unwrap();
        let got = to_value(&bytes).unwrap();
        assert_eq!(got, expected, "{v}");
        assert_eq!(got.to_string(), expected.to_string(), "{v}");
    }
}

#[test]
fn rejects_bytes_not_in_the_layout() {
    for bytes in [
        &[][..],
        &[9][..],
        &[STR, 2, 0, 0, 0, b'a'][..],
        &[STR, 1, 0, 0, 0, 0xff][..],
        &[NULL, NULL][..],
        &[ARRAY, 2, 0, 0, 0, NULL][..],
        &[OBJECT, 1, 0, 0, 0, 1, 0, 0, 0, b'a'][..],
        &[F64, 0, 0, 0, 0, 0, 0, 0xf0, 0x7f][..],
    ] {
        assert!(to_value(bytes).is_err(), "{bytes:?}");
    }
    let mut deep = [ARRAY, 1, 0, 0, 0].repeat(600);
    deep.push(NULL);
    assert!(to_value(&deep).is_err());
}

/// A skipped value (`deserialize_ignored_any`, which no generated
/// struct reaches: the typed read refuses an unknown key before its
/// value) is checked as a read one is, a double's finiteness included,
/// so that bytes the typed read accepts are bytes [`to_value`] accepts.
#[test]
fn a_skipped_value_is_checked_as_a_read_one() {
    use serde::Deserialize;
    use serde::de::IgnoredAny;

    use super::Compact;

    let skip = |bytes: &[u8]| {
        let mut compact = Compact::new(bytes);
        IgnoredAny::deserialize(&mut compact).and_then(|_| compact.end())
    };
    for v in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        let mut bytes = vec![ARRAY, 2, 0, 0, 0, I32, 1, 0, 0, 0, F64];
        bytes.extend_from_slice(&v.to_le_bytes());
        assert!(skip(&bytes).is_err(), "{v}");
        assert!(to_value(&bytes).is_err(), "{v}");
    }
    let mut bytes = vec![OBJECT, 1, 0, 0, 0, 1, 0, 0, 0, b'k', F64];
    bytes.extend_from_slice(&1.5f64.to_le_bytes());
    assert!(skip(&bytes).is_ok());
    for bytes in [
        &[F64, 0, 0][..],
        &[9][..],
        &[STR, 0xff, 0xff, 0xff, 0xff][..],
        &[ARRAY, 0xff, 0xff, 0xff, 0xff, NULL][..],
        &[OBJECT, 1, 0, 0, 0, 0xff, 0xff, 0xff, 0xff][..],
    ] {
        assert!(skip(bytes).is_err(), "{bytes:?}");
    }
}

/// An array's or an object's size hint is its count bounded by the
/// bytes left, so a visitor that reserves it (`kept::KeptSeed`) never
/// reserves for a count the bytes cannot hold (in WASM, a count of
/// `u32::MAX` times a `Kept` overflowed the reservation: a trap).
#[test]
fn a_size_hint_is_bounded_by_the_bytes_left() {
    use serde::de::{Deserializer, MapAccess, SeqAccess, Visitor};

    use super::Compact;

    // The hint the visitor is given (the read then fails: it leaves
    // the items unread).
    struct Hint<'a>(&'a std::cell::Cell<Option<usize>>);
    impl<'de> Visitor<'de> for Hint<'_> {
        type Value = ();
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a container")
        }
        fn visit_seq<A: SeqAccess<'de>>(self, seq: A) -> Result<(), A::Error> {
            self.0.set(seq.size_hint());
            Ok(())
        }
        fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<(), A::Error> {
            self.0.set(map.size_hint());
            Ok(())
        }
    }
    let hint = |bytes: &[u8]| {
        let given = std::cell::Cell::new(None);
        let _ = (&mut Compact::new(bytes)).deserialize_any(Hint(&given));
        given.get()
    };
    assert_eq!(hint(&[ARRAY, 0xff, 0xff, 0xff, 0xff, NULL, NULL]), Some(2));
    assert_eq!(hint(&[ARRAY, 2, 0, 0, 0, NULL, NULL]), Some(2));
    assert_eq!(hint(&[ARRAY, 2, 0, 0, 0, NULL, NULL, NULL]), Some(2));
    let mut entries = vec![OBJECT, 0xff, 0xff, 0xff, 0xff];
    entries.extend_from_slice(&[0; 11]);
    assert_eq!(hint(&entries), Some(2));
    assert_eq!(hint(&[OBJECT, 0xff, 0xff, 0xff, 0xff]), Some(0));
}

/// Bytes not in the layout, written by hand, with each malformed value
/// at each place a model reads one (the whole AST, its namespace, an
/// unknown key, a decorator argument's value, a declaration's kept
/// `location`). Each is the outer error of both staging paths (a
/// `TypeError` at the JS boundary), never a load and never a panic. An
/// oversized array count in a `location` is not reserved up front.
#[test]
fn malformed_bytes_are_an_error_at_every_place() {
    use crate::introspect::ModelFile;

    let u32le = |n: u32| n.to_le_bytes().to_vec();
    let key = |k: &str| {
        [
            u32le(u32::try_from(k.len()).unwrap()),
            k.as_bytes().to_vec(),
        ]
        .concat()
    };
    let string = |v: &str| [vec![STR], key(v)].concat();
    let object = |entries: &[(&str, Vec<u8>)]| {
        let mut out = vec![OBJECT];
        out.extend(u32le(u32::try_from(entries.len()).unwrap()));
        for (k, v) in entries {
            out.extend(key(k));
            out.extend(v.iter().copied());
        }
        out
    };
    let array = |items: &[Vec<u8>]| {
        let mut out = vec![ARRAY];
        out.extend(u32le(u32::try_from(items.len()).unwrap()));
        for item in items {
            out.extend(item.iter().copied());
        }
        out
    };
    let double = |v: f64| [vec![F64], v.to_le_bytes().to_vec()].concat();
    let mm = |name: &str| string(&format!("concerto.metamodel@1.0.0.{name}"));

    // The model, with `inject` at `place`.
    let model = |place: &str, inject: &[u8]| -> Vec<u8> {
        if place == "ast" {
            return inject.to_vec();
        }
        let at = |p: &str, valid: Vec<u8>| if place == p { inject.to_vec() } else { valid };
        let argument = object(&[
            ("$class", mm("DecoratorNumber")),
            ("value", at("argument", double(1.5))),
        ]);
        let decorator = object(&[
            ("$class", mm("Decorator")),
            ("name", string("d")),
            ("arguments", array(&[argument])),
        ]);
        let declaration = object(&[
            ("$class", mm("ConceptDeclaration")),
            ("name", string("C")),
            ("isAbstract", vec![FALSE]),
            ("properties", array(&[])),
            ("location", at("location", vec![NULL])),
        ]);
        let mut entries = vec![
            ("$class", mm("Model")),
            ("namespace", at("namespace", string("org.malformed@1.0.0"))),
            ("imports", array(&[])),
            ("decorators", array(&[decorator])),
            ("declarations", array(&[declaration])),
        ];
        if place == "unknownKey" {
            entries.push(("unknownKey", inject.to_vec()));
        }
        object(&entries)
    };
    let load = |bytes: &[u8], checked: bool| {
        if checked {
            ModelFile::from_compact_checked_with_imports(bytes, None, None)
        } else {
            ModelFile::from_compact_with_imports(bytes, None, None)
        }
    };

    // Each place is read: a valid value there loads, or, for an
    // unknown key, is the typed read's data error, not the layout one.
    for (place, valid) in [
        ("ast", model("", &[])),
        ("namespace", string("org.valid@1.0.0")),
        (
            "unknownKey",
            array(&[double(0.5), object(&[("k", vec![TRUE])])]),
        ),
        ("argument", double(-2.5)),
        ("location", vec![NULL]),
    ] {
        let bytes = model(place, &valid);
        let loaded = load(&bytes, false);
        if place == "unknownKey" {
            assert!(matches!(loaded, Ok(Err(_))), "{place}");
        } else {
            assert!(matches!(loaded, Ok(Ok(_))), "{place}");
        }
    }

    let mut deep = [ARRAY, 1, 0, 0, 0].repeat(600);
    deep.push(NULL);
    let malformed: Vec<(&str, Vec<u8>)> = vec![
        ("NaN", double(f64::NAN)),
        ("+Inf", double(f64::INFINITY)),
        ("-Inf", double(f64::NEG_INFINITY)),
        ("a NaN item", array(&[double(f64::NAN)])),
        ("a truncated double", vec![F64, 0, 0, 0]),
        ("a truncated i32", vec![I32, 0]),
        ("a missing value", vec![]),
        ("an unknown tag", vec![8]),
        ("an unknown tag 0xff", vec![0xff]),
        ("an unknown tag in an array", array(&[vec![9]])),
        (
            "an oversized string length",
            [vec![STR], u32le(u32::MAX), b"x".to_vec()].concat(),
        ),
        (
            "an oversized array count",
            [vec![ARRAY], u32le(u32::MAX), vec![NULL]].concat(),
        ),
        (
            "an oversized object count",
            [vec![OBJECT], u32le(u32::MAX)].concat(),
        ),
        (
            "an oversized key length",
            [vec![OBJECT], u32le(1), u32le(u32::MAX)].concat(),
        ),
        (
            "a string that is not UTF-8",
            [vec![STR], u32le(2), vec![0xc3, 0x28]].concat(),
        ),
        (
            "a key that is not UTF-8",
            [vec![OBJECT], u32le(1), u32le(1), vec![0xff, NULL]].concat(),
        ),
        ("nested too deeply", deep),
    ];
    for place in ["ast", "namespace", "unknownKey", "argument", "location"] {
        for (what, inject) in &malformed {
            let bytes = model(place, inject);
            for checked in [false, true] {
                assert!(
                    load(&bytes, checked).is_err(),
                    "{what} at {place} (checked: {checked}) is not the layout error"
                );
            }
        }
    }

    // Trailing bytes, and every proper prefix of a valid model.
    let valid = model("", &[]);
    let mut trailing = valid.clone();
    trailing.push(NULL);
    for checked in [false, true] {
        assert!(load(&trailing, checked).is_err(), "trailing bytes");
        for len in 0..valid.len() {
            assert!(load(&valid[..len], checked).is_err(), "truncated to {len}");
        }
    }

    // Each byte of a valid model replaced by a tag, a count's or a
    // double's byte, or an unknown tag: never a panic, and a load gives
    // the `ast()` the bytes hold.
    for at in 0..valid.len() {
        for byte in [NULL, F64, OBJECT, 8, 0x7f, 0xf0, 0xff] {
            let mut bytes = valid.clone();
            bytes[at] = byte;
            for checked in [false, true] {
                if let Ok(Ok((model_file, _))) = load(&bytes, checked) {
                    assert_eq!(model_file.ast(), &to_value(&bytes).unwrap(), "{at} {byte}");
                }
            }
        }
    }
}

/// Property test: the same ASTs through both staging paths,
/// [`ModelFile::from_json_text_with_imports`] over `JSON.stringify`'s
/// text and [`ModelFile::from_compact_with_imports`] over the bytes the
/// TS writer writes, with and without BC-19's shape check, give the same
/// result: an equal model file (its typed load and its `ast()`) and
/// `imports` node, or the same error (an unreadable AST's message
/// quotes `serde_json`, whose text errors carry a position, so only its
/// kind and code are compared). The ASTs are the system models, the
/// metamodel, and, when `CONCERTO_ORACLE_FIXTURES` is set, every AST of
/// the CTO cache next to the oracle corpus, each as it is and after
/// seeded random mutations (a key dropped, `$class` moved last, an
/// unknown key, a value replaced, an array item doubled).
#[test]
fn text_and_compact_paths_agree() {
    use crate::introspect::ModelFile;

    let mut bases: Vec<Value> = crate::rootmodel::system_model_json_texts()
        .iter()
        .map(|(_, text)| serde_json::from_str(text).unwrap())
        .collect();
    bases.push(serde_json::from_str(include_str!("../../metamodel.json")).unwrap());
    if let Ok(fixtures) = std::env::var("CONCERTO_ORACLE_FIXTURES") {
        let cache = std::path::Path::new(&fixtures).join("../cto-cache");
        let mut files = Vec::new();
        for dir in std::fs::read_dir(&cache).into_iter().flatten().flatten() {
            for file in std::fs::read_dir(dir.path())
                .into_iter()
                .flatten()
                .flatten()
            {
                files.push(file.path());
            }
        }
        files.sort();
        for file in files {
            let entry: Value =
                serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
            if let Some(ast) = entry.get("ast").filter(|ast| ast.is_object()) {
                bases.push(ast.clone());
            }
        }
    }

    let mut rng = 0x9e37_79b9_7f4a_7c15_u64;
    let mut cases = 0;
    let mut loaded = 0;
    for base in &bases {
        for round in 0..8 {
            let mut ast = base.clone();
            for _ in 0..round.min(3) {
                mutate(&mut ast, &mut rng);
            }
            let text = js_stringify(&ast);
            let bytes = encode(&ast);
            for checked in [false, true] {
                let (from_text, from_compact) = if checked {
                    (
                        ModelFile::from_json_text_checked_with_imports(&text, None, None),
                        ModelFile::from_compact_checked_with_imports(&bytes, None, None),
                    )
                } else {
                    (
                        ModelFile::from_json_text_with_imports(&text, None, None),
                        ModelFile::from_compact_with_imports(&bytes, None, None),
                    )
                };
                cases += 1;
                match (from_text.unwrap(), from_compact.unwrap()) {
                    (Ok((a, ai)), Ok((b, bi))) => {
                        loaded += 1;
                        // Both ASTs parsed first: `Debug` prints a lazily kept
                        // AST's source, not its value, until then.
                        assert_eq!(a.ast(), b.ast(), "{text}");
                        assert_eq!(format!("{a:?}"), format!("{b:?}"), "{text}");
                        assert_eq!(ai, bi, "{text}");
                    }
                    (Err(a), Err(b)) => {
                        assert_eq!((a.kind(), a.code()), (b.kind(), b.code()), "{text}");
                        if a.code() != "modelfile-load-unreadable" {
                            assert_eq!(a, b, "{text}");
                        }
                    }
                    (a, b) => panic!(
                        "the paths disagree (checked: {checked}) on {text}: text {:?}, compact {:?}",
                        a.err(),
                        b.err()
                    ),
                }
            }
        }
    }
    assert!(loaded > 0 && loaded < cases, "{loaded} of {cases} loaded");
}

/// `JSON.stringify`'s text of `value`, as a JS object (every number a
/// double, written by `Number::toString`).
fn js_stringify(value: &Value) -> String {
    match value {
        Value::Number(n) => {
            let mut buffer = ryu_js::Buffer::new();
            buffer.format_finite(n.as_f64().unwrap()).to_string()
        }
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(js_stringify).collect();
            format!("[{}]", items.join(","))
        }
        Value::Object(map) => {
            let entries: Vec<String> = map
                .iter()
                .map(|(key, item)| format!("{}:{}", Value::String(key.clone()), js_stringify(item)))
                .collect();
            format!("{{{}}}", entries.join(","))
        }
        other => other.to_string(),
    }
}

fn next(rng: &mut u64) -> u64 {
    *rng ^= *rng << 13;
    *rng ^= *rng >> 7;
    *rng ^= *rng << 17;
    *rng
}

fn pick<T: Clone>(items: &[T], rng: &mut u64) -> T {
    items[usize::try_from(next(rng) % items.len() as u64).unwrap()].clone()
}

/// The JSON pointers of every node of `value`.
fn pointers(value: &Value, at: &str, out: &mut Vec<String>) {
    out.push(at.to_string());
    match value {
        Value::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                pointers(item, &format!("{at}/{i}"), out);
            }
        }
        Value::Object(map) => {
            for (key, item) in map {
                let key = key.replace('~', "~0").replace('/', "~1");
                pointers(item, &format!("{at}/{key}"), out);
            }
        }
        _ => {}
    }
}

/// One random change to a random node of `ast`.
fn mutate(ast: &mut Value, rng: &mut u64) {
    let mut all = Vec::new();
    pointers(ast, "", &mut all);
    let at = pick(&all, rng);
    let replacements = [
        Value::Null,
        Value::Bool(true),
        Value::Bool(false),
        serde_json::json!(0),
        serde_json::json!(-1),
        serde_json::json!(1.5),
        serde_json::json!(2_147_483_648_i64),
        serde_json::json!(1_152_921_504_606_847_000_f64),
        serde_json::json!(1e21),
        serde_json::json!(""),
        serde_json::json!("x"),
        serde_json::json!("concerto.metamodel@1.0.0.StringProperty"),
        serde_json::json!("concerto.metamodel@1.0.0.ConceptDeclaration"),
        serde_json::json!([]),
        serde_json::json!({}),
        serde_json::json!({"$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "C"}),
    ];
    let node = ast.pointer_mut(&at).unwrap();
    match (next(rng) % 5, node) {
        (0, Value::Object(map)) if !map.is_empty() => {
            let keys: Vec<String> = map.keys().cloned().collect();
            map.shift_remove(&pick(&keys, rng));
        }
        (1, Value::Object(map)) => {
            if let Some(class) = map.shift_remove("$class") {
                map.insert("$class".to_string(), class);
            }
        }
        (2, Value::Object(map)) => {
            map.insert("unknownKey".to_string(), pick(&replacements, rng));
        }
        (3, Value::Array(items)) if !items.is_empty() => {
            let item = pick(items, rng);
            items.push(item);
        }
        (_, node) => *node = pick(&replacements, rng),
    }
}
