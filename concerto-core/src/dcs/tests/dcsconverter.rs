//! Ports `test/dcsconverter.js` (`describe('DCS Converter')`, 8 `it`s):
//! the golden `#jsonToYaml`/`#yamlToJson` round trip against
//! `test/data/decoratorcommands/possible-decorator-command-targets.{json,yaml}`
//! (vendored verbatim under `testdata/`, as `rootmodel.rs` vendors
//! `rootmodel.json`), plus its three inline `DecoratorTypeReference`
//! fixtures (resolved-but-unaliased, resolved-and-aliased, unresolved).
//! Every expected string here was checked byte for byte against the
//! reference (`yaml@2.8.3`).
use super::*;

const POSSIBLE_TARGETS_JSON: &str =
    include_str!("testdata/possible-decorator-command-targets.json");
const POSSIBLE_TARGETS_YAML: &str =
    include_str!("testdata/possible-decorator-command-targets.yaml");

#[test]
fn json_to_yaml_matches_the_golden_fixture_byte_for_byte() {
    let dcs_json: Value = serde_json::from_str(POSSIBLE_TARGETS_JSON).unwrap();
    let out = json_to_yaml(&dcs_json).unwrap();
    assert_eq!(out, POSSIBLE_TARGETS_YAML);
}

#[test]
fn yaml_to_json_matches_the_golden_fixture() {
    let dcs_json: Value = serde_json::from_str(POSSIBLE_TARGETS_JSON).unwrap();
    let out = yaml_to_json(POSSIBLE_TARGETS_YAML).unwrap();
    assert_eq!(out, dcs_json);
}

fn type_reference_dcs_json(namespace: Option<&str>, resolved_name: Option<&str>) -> Value {
    let mut type_obj = crate::json!({
        "$class": "concerto.metamodel@1.0.0.TypeIdentifier",
        "name": "Info",
    });
    if let Some(ns) = namespace {
        type_obj["namespace"] = Value::String(ns.to_string());
    }
    if let Some(rn) = resolved_name {
        type_obj["resolvedName"] = Value::String(rn.to_string());
    }
    crate::json!({
        "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
        "name": "exampleDCS",
        "version": "1.0.0",
        "commands": [{
            "$class": "org.accordproject.decoratorcommands@0.4.0.Command",
            "type": "UPSERT",
            "target": {
                "$class": "org.accordproject.decoratorcommands@0.4.0.CommandTarget",
                "namespace": "test@1.0.0"
            },
            "decorator": {
                "$class": "concerto.metamodel@1.0.0.Decorator",
                "name": "exampleDecorator",
                "arguments": [{
                    "$class": "concerto.metamodel@1.0.0.DecoratorTypeReference",
                    "type": type_obj,
                    "isArray": false
                }]
            }
        }]
    })
}

#[test]
fn json_to_yaml_handles_a_resolved_but_unaliased_type_reference() {
    let dcs_json = type_reference_dcs_json(Some("test@1.0.0"), None);
    let expected = "decoratorCommandsVersion: 0.4.0\n\
         name: exampleDCS\n\
         version: 1.0.0\n\
         commands:\n\
         \x20 - action: UPSERT\n\
         \x20   target:\n\
         \x20     namespace: test@1.0.0\n\
         \x20   decorator:\n\
         \x20     name: exampleDecorator\n\
         \x20     arguments:\n\
         \x20       - typeReference:\n\
         \x20           name: Info\n\
         \x20           namespace: test@1.0.0\n\
         \x20           isArray: false\n";
    assert_eq!(json_to_yaml(&dcs_json).unwrap(), expected);
}

#[test]
fn json_to_yaml_handles_a_resolved_and_aliased_type_reference() {
    let dcs_json = type_reference_dcs_json(Some("test@1.0.0"), Some("Data"));
    let expected = "decoratorCommandsVersion: 0.4.0\n\
         name: exampleDCS\n\
         version: 1.0.0\n\
         commands:\n\
         \x20 - action: UPSERT\n\
         \x20   target:\n\
         \x20     namespace: test@1.0.0\n\
         \x20   decorator:\n\
         \x20     name: exampleDecorator\n\
         \x20     arguments:\n\
         \x20       - typeReference:\n\
         \x20           name: Info\n\
         \x20           namespace: test@1.0.0\n\
         \x20           resolvedName: Data\n\
         \x20           isArray: false\n";
    assert_eq!(json_to_yaml(&dcs_json).unwrap(), expected);
}

#[test]
fn json_to_yaml_handles_an_unresolved_and_unaliased_type_reference() {
    let dcs_json = type_reference_dcs_json(None, None);
    let expected = "decoratorCommandsVersion: 0.4.0\n\
         name: exampleDCS\n\
         version: 1.0.0\n\
         commands:\n\
         \x20 - action: UPSERT\n\
         \x20   target:\n\
         \x20     namespace: test@1.0.0\n\
         \x20   decorator:\n\
         \x20     name: exampleDecorator\n\
         \x20     arguments:\n\
         \x20       - typeReference:\n\
         \x20           name: Info\n\
         \x20           isArray: false\n";
    assert_eq!(json_to_yaml(&dcs_json).unwrap(), expected);
}

#[test]
fn yaml_to_json_handles_a_resolved_but_unaliased_type_reference() {
    let input = "decoratorCommandsVersion: 0.4.0\n\
         name: exampleDCS\n\
         version: 1.0.0\n\
         commands:\n\
         \x20 - action: UPSERT\n\
         \x20   target:\n\
         \x20     namespace: test@1.0.0\n\
         \x20   decorator:\n\
         \x20     name: exampleDecorator\n\
         \x20     arguments:\n\
         \x20       - typeReference:\n\
         \x20           name: Info\n\
         \x20           namespace: test@1.0.0\n\
         \x20           isArray: false\n";
    let expected = type_reference_dcs_json(Some("test@1.0.0"), None);
    assert_eq!(yaml_to_json(input).unwrap(), expected);
}

#[test]
fn yaml_to_json_handles_a_resolved_and_aliased_type_reference() {
    let input = "decoratorCommandsVersion: 0.4.0\n\
         name: exampleDCS\n\
         version: 1.0.0\n\
         commands:\n\
         \x20 - action: UPSERT\n\
         \x20   target:\n\
         \x20     namespace: test@1.0.0\n\
         \x20   decorator:\n\
         \x20     name: exampleDecorator\n\
         \x20     arguments:\n\
         \x20       - typeReference:\n\
         \x20           name: Info\n\
         \x20           namespace: test@1.0.0\n\
         \x20           resolvedName: Data\n\
         \x20           isArray: false\n";
    let expected = type_reference_dcs_json(Some("test@1.0.0"), Some("Data"));
    assert_eq!(yaml_to_json(input).unwrap(), expected);
}

#[test]
fn yaml_to_json_handles_an_unresolved_and_unaliased_type_reference() {
    let input = "decoratorCommandsVersion: 0.4.0\n\
         name: exampleDCS\n\
         version: 1.0.0\n\
         commands:\n\
         \x20 - action: UPSERT\n\
         \x20   target:\n\
         \x20     namespace: test@1.0.0\n\
         \x20   decorator:\n\
         \x20     name: exampleDecorator\n\
         \x20     arguments:\n\
         \x20       - typeReference:\n\
         \x20           name: Info\n\
         \x20           isArray: false\n";
    let expected = type_reference_dcs_json(None, None);
    assert_eq!(yaml_to_json(input).unwrap(), expected);
}

#[test]
fn round_trips_a_command_with_string_number_and_boolean_arguments() {
    let dcs_json = crate::json!({
        "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
        "name": "argsDCS",
        "version": "1.0.0",
        "commands": [{
            "$class": "org.accordproject.decoratorcommands@0.4.0.Command",
            "type": "APPEND",
            "target": { "$class": "org.accordproject.decoratorcommands@0.4.0.CommandTarget", "namespace": "test@1.0.0" },
            "decorator": {
                "$class": "concerto.metamodel@1.0.0.Decorator",
                "name": "argumentsTest",
                "arguments": [
                    { "$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "inputString" },
                    { "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 2.5 },
                    { "$class": "concerto.metamodel@1.0.0.DecoratorBoolean", "value": true }
                ]
            }
        }]
    });
    let yaml = json_to_yaml(&dcs_json).unwrap();
    assert_eq!(
        yaml,
        "decoratorCommandsVersion: 0.4.0\n\
         name: argsDCS\n\
         version: 1.0.0\n\
         commands:\n\
         \x20 - action: APPEND\n\
         \x20   target:\n\
         \x20     namespace: test@1.0.0\n\
         \x20   decorator:\n\
         \x20     name: argumentsTest\n\
         \x20     arguments:\n\
         \x20       - type: String\n\
         \x20         value: inputString\n\
         \x20       - type: Number\n\
         \x20         value: 2.5\n\
         \x20       - type: Boolean\n\
         \x20         value: true\n"
    );
    // Round trip: yamlToJson(jsonToYaml(x)) restores x, including
    // `Number`/`Boolean` argument values as their JSON types (not the
    // stringified form the YAML carries them as in between).
    assert_eq!(yaml_to_json(&yaml).unwrap(), dcs_json);
}

/// A `DecoratorNumber` value is written as JS `String(value)` writes it —
/// TS 5.0.0 `jsonToYaml` gives `0.000001` and `1e+21`, where serde_json's
/// own `Display` gave `1e-6` and `1e21`.
#[test]
fn json_to_yaml_writes_a_number_argument_as_js_string_does() {
    let dcs_json = crate::json!({
        "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
        "name": "n",
        "version": "1.0.0",
        "commands": [{
            "$class": "org.accordproject.decoratorcommands@0.4.0.Command",
            "type": "APPEND",
            "target": { "$class": "org.accordproject.decoratorcommands@0.4.0.CommandTarget", "namespace": "test@1.0.0" },
            "decorator": {
                "$class": "concerto.metamodel@1.0.0.Decorator",
                "name": "d",
                "arguments": [
                    { "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 0.000001 },
                    { "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": 1e21 }
                ]
            }
        }]
    });
    assert_eq!(
        json_to_yaml(&dcs_json).unwrap(),
        "decoratorCommandsVersion: 0.4.0\n\
         name: n\n\
         version: 1.0.0\n\
         commands:\n\
         \x20 - action: APPEND\n\
         \x20   target:\n\
         \x20     namespace: test@1.0.0\n\
         \x20   decorator:\n\
         \x20     name: d\n\
         \x20     arguments:\n\
         \x20       - type: Number\n\
         \x20         value: 0.000001\n\
         \x20       - type: Number\n\
         \x20         value: 1e+21\n"
    );
}

#[test]
fn a_decorator_with_no_arguments_omits_the_arguments_key() {
    let decorator = crate::json!({ "name": "NoArgs", "arguments": [] });
    assert_eq!(
        handle_decorator(&decorator),
        Yaml::Map(vec![(
            "name".to_string(),
            Yaml::Scalar("NoArgs".to_string())
        )])
    );
}

#[test]
fn json_to_yaml_rejects_a_command_set_with_no_commands_array() {
    let dcs_json = crate::json!({
        "$class": "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet",
        "name": "x",
        "version": "1.0.0"
    });
    assert!(json_to_yaml(&dcs_json).is_err());
}

#[test]
fn yaml_to_json_rejects_malformed_yaml() {
    assert!(yaml_to_json("not: a: dcs\n  - broken").is_err());
}

/// `yamlToJson` itself does not check the command set: with no
/// `decoratorCommandsVersion`, TS builds the `$class` from `undefined`
/// and leaves the checking to `DecoratorManager.yamlToJson`.
#[test]
fn yaml_to_json_builds_the_class_from_an_absent_version_as_undefined() {
    let out = yaml_to_json("name: x\nversion: 1.0.0\ncommands: []").unwrap();
    assert_eq!(
        out["$class"],
        "org.accordproject.decoratorcommands@undefined.DecoratorCommandSet"
    );
    assert!(crate::dcs::validated_yaml_to_json("name: x\nversion: 1.0.0\ncommands: []").is_err());
}

/// `test/decoratormanager.js` "#jsonToYaml should throw error if input
/// is not valid DCS JSON", through `DecoratorManager.jsonToYaml`
/// ([`crate::dcs::validated_json_to_yaml`]).
#[test]
fn json_to_yaml_rejects_every_reference_invalid_input() {
    for invalid in [
        crate::json!({ "invalid": "dcsJson" }),
        crate::json!({ "version": "1.0.0", "commands": [] }),
        crate::json!({ "name": "test", "commands": [] }),
        crate::json!({ "name": "test", "version": "1.0.0" }),
    ] {
        assert!(
            crate::dcs::validated_json_to_yaml(&invalid).is_err(),
            "expected an error for {invalid}"
        );
    }
}

/// `test/decoratormanager.js` "#yamlToJson should throw error if input
/// is not valid DCS YAML", through `DecoratorManager.yamlToJson`
/// ([`crate::dcs::validated_yaml_to_json`]).
#[test]
fn yaml_to_json_rejects_every_reference_invalid_input() {
    for invalid in [
        "decoratorCommandsVersion: 0.4.0\ncommands: []\n",
        "decoratorCommandsVersion: 0.4.0\nname: test\ncommands: []\n",
        "name: test\nversion: 1.0.0\ncommands: []\n",
    ] {
        assert!(
            crate::dcs::validated_yaml_to_json(invalid).is_err(),
            "expected an error for {invalid:?}"
        );
    }
}
