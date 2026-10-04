use super::*;

#[test]
fn id_regex_compiles() {
    assert!(regress::Regex::with_flags(ID_PATTERN, "u").is_ok());
}

#[test]
fn is_valid_identifier_fast_path_agrees_with_the_regex() {
    // Every answer the ASCII fast path gives is the regex's own.
    let alphabet: Vec<char> = (0x20u8..0x7f).map(char::from).collect();
    let mut names = vec![String::new()];
    for first in &alphabet {
        names.push(first.to_string());
        for second in ['a', 'Z', '0', '9', '$', '_', '-', ' ', '.', '\\', '\u{e9}'] {
            names.push(format!("{first}{second}"));
            names.push(format!("{first}{second}x"));
        }
    }
    names.extend([
        r"\u0041bc".to_string(),
        "a\u{0663}".to_string(),
        "\u{e9}t\u{e9}".to_string(),
    ]);
    for name in names {
        assert_eq!(
            is_valid_identifier(&name),
            ID_REGEX.find(&name).is_some(),
            "{name:?}"
        );
    }
}

/// The recording `tests/semver/record.mjs` made of node-semver 7.6.3's
/// `parse`: `{semver, full, cases: [{input, parsed}]}`.
fn node_semver_recording() -> Value {
    serde_json::from_str(include_str!("../../tests/semver/node-semver-7.6.3.json"))
        .unwrap_or_else(|e| panic!("node-semver-7.6.3.json: {e}"))
}

/// [`split_namespace`] accepts and rejects exactly what
/// `parse_namespace_with(_, false)` does, with the same error, and gives
/// back its `name` and `version`; over every recorded semver input as a
/// namespace version, and the other namespace shapes. The one exception
/// (BC-02): an unversioned namespace, which `split_namespace` gives back
/// with no version for its callers to reject.
#[test]
fn split_namespace_matches_parse_namespace() {
    let recording = node_semver_recording();
    let mut namespaces: Vec<String> = recording["cases"]
        .as_array()
        .unwrap_or_else(|| unreachable!())
        .iter()
        .map(|case| {
            let input = case["input"].as_str().unwrap_or_else(|| unreachable!());
            format!("org.acme@{input}")
        })
        .collect();
    namespaces.extend(
        [
            "",
            "org.acme",
            "org.acme@",
            "@1.0.0",
            "org.acme@1.0.0@2.0.0",
            "a@b@c",
            "concerto@1.0.0",
            "concerto",
            "org.acme@v1.0.0",
            "org.acme@ 1.0.0 ",
            "org.acme@1.0.0-beta.1+build.2",
            "org.acme@01.0.0",
        ]
        .map(str::to_string),
    );
    for ns in &namespaces {
        let split = split_namespace(ns);
        if !ns.is_empty() && !ns.contains('@') {
            assert!(parse_namespace_with(Some(ns), false).is_err(), "{ns:?}");
            assert_eq!(split.ok(), Some((ns.as_str(), None)), "{ns:?}");
            continue;
        }
        match parse_namespace_with(Some(ns), false) {
            Ok(ParsedNamespace::Full { name, version, .. }) => {
                let (n, v) = split.unwrap_or_else(|e| panic!("{ns:?}: {e}"));
                assert_eq!((n, v), (name.as_str(), version.as_deref()), "{ns:?}");
            }
            Ok(other) => panic!("{ns:?}: {other:?}"),
            Err(expected) => {
                let actual = split.err().unwrap_or_else(|| panic!("{ns:?} accepted"));
                assert_eq!(actual.to_string(), expected.to_string(), "{ns:?}");
            }
        }
    }
}

#[test]
fn semver_recording_is_node_semver_7_6_3() {
    let recording = node_semver_recording();
    assert_eq!(recording["semver"], "7.6.3");
}

/// Whether a strict SemVer 2.0.0 version is beyond node-semver 7.6.3's
/// own limits (BC-41): more than `MAX_LENGTH` (256) UTF-16 units, or a
/// component above `Number.MAX_SAFE_INTEGER`. (Its regex's `{0,256}` and
/// `{0,250}` identifier bounds cannot bind within 256 units.)
fn beyond_node_semver_limits(input: &str) -> bool {
    let Ok(v) = semver::Version::parse(input) else {
        return false;
    };
    input.encode_utf16().count() > 256
        || [v.major, v.minor, v.patch]
            .iter()
            .any(|n| *n > 9_007_199_254_740_991)
}

#[test]
fn semver_parse_matches_node_semver() {
    // The differential test against node-semver's own `parse`, over
    // prerelease and build metadata, leading zeros, whitespace, `v`
    // prefixes, numeric limits, very long input and random near-versions
    // (tests/semver/record.mjs). Since BC-41 `semver_parse` is
    // node-semver's result for an input with no surrounding whitespace
    // and no leading `v`, and nothing else; and a namespace version is
    // accepted ([`is_strict_semver`]) exactly when it gets a result or
    // only node-semver's own limits reject it (then `versionParsed` is
    // null, as in TS).
    let recording = node_semver_recording();
    let cases = recording["cases"]
        .as_array()
        .unwrap_or_else(|| unreachable!());
    assert!(cases.len() > 3_000, "{}", cases.len());
    let (mut accepted, mut lenient, mut beyond) = (0usize, 0usize, 0usize);
    for case in cases {
        let input = case["input"].as_str().unwrap_or_else(|| unreachable!());
        let actual = semver_parse(input).map(|v| {
            let prerelease: Vec<Value> = v
                .prerelease
                .iter()
                .map(|id| match id {
                    PrereleaseIdentifier::Number(n) => crate::json!(n),
                    PrereleaseIdentifier::String(s) => crate::json!(s),
                })
                .collect();
            crate::json!({
                "raw": v.raw,
                "major": v.major,
                "minor": v.minor,
                "patch": v.patch,
                "prerelease": prerelease,
                "build": v.build,
                "version": v.version,
            })
        });
        let expected = match &case["parsed"] {
            Value::Null => None,
            _ if ecma::js_trim(input) != input || input.starts_with('v') => {
                // node-semver's trimming and `v` prefix: rejected (BC-41).
                lenient += 1;
                None
            }
            parsed => {
                accepted += 1;
                // JSON has one number type: compare the numbers as f64.
                let mut parsed = parsed.clone();
                for key in ["major", "minor", "patch"] {
                    parsed[key] = crate::json!(parsed[key].as_f64());
                }
                if let Some(ids) = parsed["prerelease"].as_array_mut() {
                    for id in ids.iter_mut() {
                        if let Some(n) = id.as_f64() {
                            *id = crate::json!(n);
                        }
                    }
                }
                Some(parsed)
            }
        };
        assert_eq!(actual, expected, "{input:?}");
        let beyond_limits = beyond_node_semver_limits(input);
        assert_eq!(
            is_strict_semver(input),
            (expected.is_some() && ecma::js_trim(input) == input && !input.starts_with('v'))
                || beyond_limits,
            "{input:?}"
        );
        if beyond_limits {
            assert!(expected.is_none(), "{input:?}");
            beyond += 1;
        }
    }
    assert!(accepted > 1_000, "{accepted}");
    assert!(lenient > 10, "{lenient}");
    assert!(beyond > 0, "{beyond}");
}

#[test]
fn id_regex_follows_the_ts_classes() {
    // Nd continues but does not start; Mn, Mc, Pc, ZWNJ and ZWJ continue;
    // a literal backslash-u escape is part of the name.
    assert!(is_valid_identifier("a\u{0663}"));
    assert!(!is_valid_identifier("\u{0663}a"));
    assert!(is_valid_identifier("a\u{0301}\u{200C}\u{200D}_"));
    assert!(is_valid_identifier(r"Abc"));
    assert!(!is_valid_identifier(""));
    assert!(!is_valid_identifier("with space"));
    // The *string* "undefined" is a valid identifier; a JS `undefined`
    // never reaches this function (BC-01).
    assert!(is_valid_identifier("undefined"));
}

#[test]
fn semver_parse_is_strict_semver_2_0_0() {
    // BC-41: strict SemVer 2.0.0, no node-compat leniency.
    assert!(semver_parse("1.0.0").is_some());
    assert!(semver_parse("1.2.3-alpha.1+build.5").is_some());
    assert!(semver_parse(" v1.2.3-alpha.1+build.5 ").is_none());
    assert!(semver_parse("v1.0.0").is_none());
    assert!(semver_parse(" 1.0.0").is_none());
    assert!(semver_parse("1.0.0 ").is_none());
    assert!(semver_parse("1.1.2+.123").is_none());
    assert!(semver_parse("1.0").is_none());
    assert!(semver_parse("01.0.0").is_none());
    // Components go up to 2^64-1 (is_strict_semver), but beyond
    // node-semver's Number.MAX_SAFE_INTEGER there is no SemVer (TS's
    // `versionParsed` is null): see
    // `parse_namespace_beyond_node_semver_limits_has_no_version_parsed`.
    assert!(is_strict_semver("9007199254740992.0.0"));
    assert!(is_strict_semver(
        "18446744073709551615.18446744073709551615.18446744073709551615"
    ));
    assert!(!is_strict_semver("18446744073709551616.0.0"));
    assert!(!is_strict_semver("v1.0.0"));
    assert!(!is_strict_semver(" 1.0.0"));
    assert!(semver_parse("9007199254740992.0.0").is_none());
    assert!(semver_parse("18446744073709551616.0.0").is_none());
    let v = semver_parse("1.2.3-rc.10").unwrap_or_else(|| unreachable!());
    assert_eq!(v.version, "1.2.3-rc.10");
    assert_eq!(
        v.prerelease,
        vec![
            PrereleaseIdentifier::String("rc".into()),
            PrereleaseIdentifier::Number(10.0)
        ]
    );
    // No whitespace of any kind is trimmed.
    assert!(semver_parse("\u{0085}1.0.0").is_none());
    assert!(semver_parse("\u{FEFF}1.0.0").is_none());
}

/// The `versionParsed` of `org.acme@<version>`, which must be accepted.
fn version_parsed_of(version: &str) -> Option<SemVer> {
    let ns = format!("org.acme@{version}");
    match parse_namespace_with(Some(&ns), false) {
        Ok(ParsedNamespace::Full {
            version: Some(v),
            version_parsed,
            ..
        }) => {
            assert_eq!(v, version);
            version_parsed
        }
        other => panic!("{ns}: {other:?}"),
    }
}

#[test]
fn parse_namespace_beyond_node_semver_limits_has_no_version_parsed() {
    // Maintainer decision 2026-09-29: Rust matches TS. A strict version
    // beyond node-semver's limits is accepted (BC-41), but its
    // `versionParsed` is null, as `semver.parse` gives in TS.
    const SAFE: &str = "9007199254740991"; // 2^53-1
    const UNSAFE: &str = "9007199254740992"; // 2^53
    for (major, minor, patch) in [(SAFE, "0", "0"), ("0", SAFE, "0"), ("0", "0", SAFE)] {
        let v = version_parsed_of(&format!("{major}.{minor}.{patch}"))
            .unwrap_or_else(|| panic!("{major}.{minor}.{patch}"));
        assert_eq!(v.major + v.minor + v.patch, MAX_SAFE_INTEGER);
    }
    for version in [
        format!("{UNSAFE}.0.0"),
        format!("0.{UNSAFE}.0"),
        format!("0.0.{UNSAFE}"),
        "9007199254740993.0.0".to_string(),
        "18446744073709551615.18446744073709551615.18446744073709551615".to_string(),
    ] {
        assert_eq!(version_parsed_of(&version), None, "{version}");
    }
    // MAX_LENGTH: 256 UTF-16 units gives a SemVer, 257 none.
    let at_limit = format!("1.0.0-{}", "a".repeat(250));
    assert_eq!(at_limit.len(), 256);
    assert!(version_parsed_of(&at_limit).is_some());
    let over = format!("1.0.0-{}", "a".repeat(251));
    assert_eq!(version_parsed_of(&over), None);
    // Still rejected: not strict SemVer 2.0.0, or above 2^64-1.
    for ns in ["org.acme@v1.0.0", "org.acme@18446744073709551616.0.0"] {
        assert!(parse_namespace_with(Some(ns), false).is_err(), "{ns}");
    }
}

#[test]
fn capitalize_first_letter_counts_utf16_units() {
    assert_eq!(capitalize_first_letter(""), "");
    assert_eq!(capitalize_first_letter("ßa"), "SSa");
    // U+10428 DESERET SMALL LETTER LONG I has an upper case, but JS
    // upper-cases only the lone high surrogate.
    assert_eq!(capitalize_first_letter("\u{10428}a"), "\u{10428}a");
}

#[test]
fn import_fully_qualified_names_covers_every_import_kind() {
    let names = |imp: Value| import_fully_qualified_names(Some(&imp)).map_err(|e| e.to_string());
    assert_eq!(
        names(
            crate::json!({"$class": "concerto.metamodel@1.0.0.ImportAll", "namespace": "a@1.0.0"})
        ),
        Ok(vec!["a@1.0.0.*".to_string()])
    );
    assert_eq!(
        names(
            crate::json!({"$class": "concerto.metamodel@1.0.0.ImportType", "namespace": "a@1.0.0", "name": "B"})
        ),
        Ok(vec!["a@1.0.0.B".to_string()])
    );
    assert_eq!(
        names(
            crate::json!({"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "a@1.0.0", "types": ["B", "C"]})
        ),
        Ok(vec!["a@1.0.0.B".to_string(), "a@1.0.0.C".to_string()])
    );
    assert_eq!(
        names(
            crate::json!({"$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "a@1.0.0"})
        ),
        Err("Cannot read properties of undefined (reading 'forEach')".to_string())
    );
    assert_eq!(
        names(crate::json!({"$class": "ImportAll"})),
        Err("Unrecognized imports ImportAll".to_string())
    );
}

/// One test function per `it()` in `test/modelutil.js`,
/// named after its `describe`/`it` titles, so `grep` finds the port of
/// each assertion (PORTING.md 10.11). The 6 `#isAssignableTo` cases are
/// tagged `W` (they stub `ModelFile`/`Property`/`ModelManager` with
/// sinon): `ModelUtil.isAssignableTo` is exercised instead against a real
/// arena-backed `ModelManager` in `model_manager::tests::
/// ported_members_run_on_the_arena` and `model_manager::tests`'
/// `is_assignable_to` cases.
mod ts_modelutil_js {
    use super::*;

    // #isPrimitiveType > check isPrimitiveType
    #[test]
    fn is_primitive_type_check_is_primitive_type() {
        assert!(!is_primitive_type("org.acme.baz@1.0.0.Foo"));
        assert!(is_primitive_type("Boolean"));
        assert!(is_primitive_type("Integer"));
        assert!(is_primitive_type("Long"));
        assert!(is_primitive_type("DateTime"));
        assert!(is_primitive_type("String"));
    }

    // #getShortName > should handle a name with a namespace
    #[test]
    fn get_short_name_should_handle_a_name_with_a_namespace() {
        assert_eq!(short_name("org.acme.baz@1.0.0.Foo"), "Foo");
    }

    // #getShortName > should handle a name without a namespace
    #[test]
    fn get_short_name_should_handle_a_name_without_a_namespace() {
        assert_eq!(short_name("Foo"), "Foo");
    }

    // #getNamespace > check getNamespace
    #[test]
    fn get_namespace_check_get_namespace() {
        assert_eq!(
            get_namespace(Some("org.acme.baz@1.0.0.Foo")).unwrap(),
            "org.acme.baz@1.0.0"
        );
        assert_eq!(get_namespace(Some("Foo")).unwrap(), "");
    }

    // #capitalizeFirstLetter > should handle a single lower case letter
    #[test]
    fn capitalize_first_letter_should_handle_a_single_lower_case_letter() {
        assert_eq!(capitalize_first_letter("a"), "A");
    }

    // #capitalizeFirstLetter > should handle a single upper case letter
    #[test]
    fn capitalize_first_letter_should_handle_a_single_upper_case_letter() {
        assert_eq!(capitalize_first_letter("A"), "A");
    }

    // #capitalizeFirstLetter > should handle a string of lower case letters
    #[test]
    fn capitalize_first_letter_should_handle_a_string_of_lower_case_letters() {
        assert_eq!(capitalize_first_letter("abcdef"), "Abcdef");
    }

    // #capitalizeFirstLetter > should handle a string of mixed case letters
    #[test]
    fn capitalize_first_letter_should_handle_a_string_of_mixed_case_letters() {
        assert_eq!(capitalize_first_letter("aBcDeF"), "ABcDeF");
    }

    // #getFullyQualifiedName > valid inputs
    #[test]
    fn get_fully_qualified_name_valid_inputs() {
        assert_eq!(qualify("a.namespace", "type"), "a.namespace.type");
    }

    // #getFullyQualifiedName > empty namespace should return the type with no leading dot
    #[test]
    fn get_fully_qualified_name_empty_namespace_should_return_the_type_with_no_leading_dot() {
        assert_eq!(qualify("", "type"), "type");
    }

    // #removeNamespaceVersionFromFullyQualifiedName > valid inputs
    #[test]
    fn remove_namespace_version_from_fully_qualified_name_valid_inputs() {
        assert_eq!(
            remove_namespace_version_from_fully_qualified_name(Some("org.acme@1.0.0.Person"))
                .unwrap(),
            "org.acme.Person"
        );
    }

    // #removeNamespaceVersionFromFullyQualifiedName > primtive type [sic]
    #[test]
    fn remove_namespace_version_from_fully_qualified_name_primitive_type() {
        assert_eq!(
            remove_namespace_version_from_fully_qualified_name(Some("String")).unwrap(),
            "String"
        );
    }

    // #parseNamespace > valid, with version
    #[test]
    fn parse_namespace_valid_with_version() {
        let ParsedNamespace::Full {
            name,
            escaped_namespace,
            version,
            version_parsed,
        } = parse_namespace_with(Some("org.acme@1.0.0"), false).unwrap()
        else {
            unreachable!("version parsing is not disabled")
        };
        assert_eq!(name, "org.acme");
        assert_eq!(escaped_namespace, "org.acme_1.0.0");
        assert_eq!(version.as_deref(), Some("1.0.0"));
        assert_eq!(version_parsed.unwrap().major, 1.0);
    }

    // #parseNamespace > valid, with version validation disabled
    #[test]
    fn parse_namespace_valid_with_version_validation_disabled() {
        // TS calls `parseNamespace('org.acme@1.0.x', {
        // disableVersionParsing: true })`; `1.0.x` is never validated as a
        // semver, and the result carries `name` only (no
        // `escapedNamespace`/`version`/`versionParsed` properties).
        let ParsedNamespace::NameOnly { name } =
            parse_namespace_with(Some("org.acme@1.0.x"), true).unwrap()
        else {
            unreachable!("version parsing is disabled")
        };
        assert_eq!(name, "org.acme");
    }

    // #parseNamespace > invalid (null)
    #[test]
    fn parse_namespace_invalid_null() {
        let err = parse_namespace_with(None, false).unwrap_err();
        assert!(err.to_string().contains("Namespace is null"), "{err}");
    }

    // #parseNamespace > invalid (org.acme@1.0.0@2.3)
    #[test]
    fn parse_namespace_invalid_two_at_signs() {
        let err = parse_namespace_with(Some("org.acme@1.0.0@2.3"), false).unwrap_err();
        assert!(err.to_string().contains("Invalid namespace"), "{err}");
    }

    // #parseNamespace > invalid version
    #[test]
    fn parse_namespace_invalid_version() {
        let err = parse_namespace_with(Some("org.acme@1.1.2+.123"), false).unwrap_err();
        assert!(err.to_string().contains("Invalid namespace"), "{err}");
    }

    // BC-02: an unversioned namespace is rejected,
    // with the error an invalid one gets, whether or not the version
    // is parsed.
    #[test]
    fn parse_namespace_rejects_an_unversioned_namespace() {
        for disable in [false, true] {
            for ns in ["org.acme", "concerto", "a"] {
                let err = parse_namespace_with(Some(ns), disable).unwrap_err();
                assert_eq!(err.contract().kind, ErrorKind::InvalidArgument, "{ns}");
                assert!(err.to_string().contains("Invalid namespace"), "{ns}: {err}");
            }
        }
        assert!(parse_namespace("org.acme").is_err());
    }
}
