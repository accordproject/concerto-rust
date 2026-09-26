//! Ports of `test/decoratormanager.js` (`accordproject/concerto`,
//! `packages/concerto-core`): the `#decorateModels` tests that target a
//! `MapDeclaration`, and the `#extractDecorators` group.
//!
//! Every model is the one the TS test loads with `addCTOModel`: CTO stays in
//! JS (PORTING.md OD-9), so `testdata/decoratorcommands/<name>.ast.json` is
//! the AST concerto-cto 5.0.0 parses `test/data/decoratorcommands/<name>.cto`
//! to, taken from the oracle corpus's CTO -> AST cache (P1-07a), under the
//! same file name and `skipLocationNodes` option the test passes
//! (`.skiplocation.` in the name when it passes `true`). The command-set and
//! expected-output JSON files are copied verbatim from
//! `test/data/decoratorcommands/`.
//!
//! Where TS reads a map's key or value decorators through
//! `MapDeclaration.key`/`.value` (`MapKeyType`/`MapValueType`, which have no
//! decorator accessors in this crate yet), these tests read the same
//! decorators from the decorated model file's AST, which is what those
//! elements are built from.
//!
//! Not ported, from the `#extractDecorators` group (21 `it`s, 19 ported):
//!
//! - "should ensure that extraction and re-application of decorators and
//!   vocabs from a model is an identity operation", and the same "including
//!   namespace terms": both re-apply the extracted vocabularies with
//!   concerto-vocabulary's `VocabularyManager.generateDecoratorCommands`,
//!   which has no Rust port, and compare the models as CTO text printed by
//!   concerto-cto's `Printer.toCTO`, which stays in JS.
use serde_json::{Value, json};
use yaml_rust2::{Yaml, YamlLoader};

use super::*;
use crate::introspect::Decorated;

const TEST_CTO_SKIP_LOCATION: &str =
    include_str!("testdata/decoratorcommands/test.skiplocation.ast.json");
const MAP_DECLARATION_DCS: &str = include_str!("testdata/decoratorcommands/map-declaration.json");
const INCOMPATIBLE_VERSION_DCS: &str =
    include_str!("testdata/decoratorcommands/incompatible_version_dcs.json");

/// `new ModelManager(…)` then `addCTOModel(<the model>, file_name)`.
fn model_manager_with(ast_json: &str, file_name: &str) -> ModelManager {
    let ast: Value = serde_json::from_str(ast_json).expect("test AST is JSON");
    let mut model_manager = ModelManager::new().unwrap();
    model_manager
        .add_models([(&ast, Some(file_name.to_string()))])
        .unwrap();
    model_manager
}

fn json_file(text: &str) -> Value {
    serde_json::from_str(text).expect("test data is JSON")
}

// ---------------------------------------------------------------------------
// #decorateModels, MapDeclaration targets
// ---------------------------------------------------------------------------

/// `DecoratorManager.decorateModels(testModelManager, JSON.parse(dcs),
/// options)` over `test.cto`, loaded with `{ skipLocationNodes: true }`.
fn decorate_test_cto(dcs: &str, options: DecorateOptions) -> ModelManager {
    let model_manager = model_manager_with(TEST_CTO_SKIP_LOCATION, "test.cto");
    let mut command_set = json_file(dcs);
    let mut options = options;
    decorate_models(
        &model_manager,
        std::slice::from_mut(&mut command_set),
        &mut options,
    )
    .unwrap()
}

fn validate_and_validate_commands() -> DecorateOptions {
    DecorateOptions {
        validate: true,
        validate_commands: true,
        ..Default::default()
    }
}

/// `decoratedModelManager.getType(fqn)`'s map declaration node, from its
/// model file's AST.
fn map_declaration_ast<'a>(model_manager: &'a ModelManager, name: &str) -> &'a Value {
    model_manager
        .model_file("test@1.0.0")
        .unwrap()
        .ast()
        .get("declarations")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .find(|d| d.get("name").and_then(Value::as_str) == Some(name))
        .unwrap_or_else(|| panic!("no declaration {name}"))
}

/// `map.key.getDecorator(name)`/`map.value.getDecorator(name)` is not null.
fn has_element_decorator(map: &Value, element: &str, name: &str) -> bool {
    map.get(element)
        .and_then(|e| e.get("decorators"))
        .and_then(Value::as_array)
        .is_some_and(|ds| {
            ds.iter()
                .any(|d| d.get("name").and_then(Value::as_str) == Some(name))
        })
}

// "should decorate the specified MapDeclaration"
#[test]
fn decorates_the_specified_map_declaration() {
    let decorated = decorate_test_cto(MAP_DECLARATION_DCS, validate_and_validate_commands());
    let dictionary = decorated.get_declaration("test@1.0.0.Dictionary").unwrap();
    assert!(
        dictionary
            .get_decorator("MapDeclarationDecorator")
            .is_some()
    );
}

// "should decorate the specified element on the specified Map Declaration (Map Key)"
#[test]
fn decorates_the_specified_element_on_the_specified_map_declaration_map_key() {
    let decorated = decorate_test_cto(MAP_DECLARATION_DCS, validate_and_validate_commands());
    let dictionary = map_declaration_ast(&decorated, "Dictionary");
    assert!(has_element_decorator(dictionary, "key", "Foo"));
    assert!(has_element_decorator(dictionary, "key", "Qux"));
}

// "should auto upgrade decoratorcommands $class minor version if it is below
// DCS_VERSION (asserts decorators are correctly applied)"
#[test]
fn auto_upgrades_an_older_minor_dcs_version_and_applies_its_decorators() {
    let decorated = decorate_test_cto(
        INCOMPATIBLE_VERSION_DCS,
        DecorateOptions {
            migrate: true,
            ..validate_and_validate_commands()
        },
    );
    let dictionary = map_declaration_ast(&decorated, "Dictionary");
    assert!(has_element_decorator(dictionary, "key", "Foo"));
    assert!(has_element_decorator(dictionary, "key", "Qux"));
}

// "should auto upgrade decoratorcommands $class minor version if it is below
// DCS_VERSION (asserts correct upgrade on DCS $class properties)"
#[test]
fn migrate_to_upgrades_every_dcs_class_and_leaves_metamodel_classes() {
    let mut dcs = json_file(INCOMPATIBLE_VERSION_DCS);
    migrate_to(&mut dcs).unwrap();
    assert_eq!(
        dcs["$class"],
        "org.accordproject.decoratorcommands@0.4.0.DecoratorCommandSet"
    );
    assert_eq!(
        dcs["commands"][0]["$class"],
        "org.accordproject.decoratorcommands@0.4.0.Command"
    );
    assert_eq!(
        dcs["commands"][0]["target"]["$class"],
        "org.accordproject.decoratorcommands@0.4.0.CommandTarget"
    );
    // concerto metamodel $class does not change
    assert_eq!(
        dcs["commands"][0]["target"]["type"],
        "concerto.metamodel@1.0.0.StringMapKeyType"
    );
    assert_eq!(
        dcs["commands"][0]["decorator"]["$class"],
        "concerto.metamodel@1.0.0.Decorator"
    );
}

// "should decorate the specified type on the specified Map Declaration (Map Key)"
#[test]
fn decorates_the_specified_type_on_the_specified_map_declaration_map_key() {
    let decorated = decorate_test_cto(MAP_DECLARATION_DCS, validate_and_validate_commands());
    let dictionary = map_declaration_ast(&decorated, "Dictionary");
    assert!(has_element_decorator(
        dictionary,
        "key",
        "DecoratesKeyByType"
    ));
}

// "should decorate the specified element on the specified Map Declaration (Map Value)"
#[test]
fn decorates_the_specified_element_on_the_specified_map_declaration_map_value() {
    let decorated = decorate_test_cto(MAP_DECLARATION_DCS, validate_and_validate_commands());
    let dictionary = map_declaration_ast(&decorated, "Dictionary");
    assert!(has_element_decorator(dictionary, "value", "Bar"));
    assert!(has_element_decorator(dictionary, "value", "Quux"));
}

// "should decorate the specified type on the specified Map Declaration (Map Value)"
#[test]
fn decorates_the_specified_type_on_the_specified_map_declaration_map_value() {
    let decorated = decorate_test_cto(MAP_DECLARATION_DCS, validate_and_validate_commands());
    let dictionary = map_declaration_ast(&decorated, "Dictionary");
    assert!(has_element_decorator(
        dictionary,
        "value",
        "DecoratesValueByType"
    ));
}

// "should decorate Declaration, Key and Value elements on the specified Map Declaration"
#[test]
fn decorates_declaration_key_and_value_elements_on_the_specified_map_declaration() {
    let decorated = decorate_test_cto(MAP_DECLARATION_DCS, validate_and_validate_commands());
    let dictionary = decorated.get_declaration("test@1.0.0.Dictionary").unwrap();
    assert!(
        dictionary
            .get_decorator("MapDeclarationDecorator")
            .is_some()
    );
    let dictionary = map_declaration_ast(&decorated, "Dictionary");
    assert!(has_element_decorator(dictionary, "key", "Baz"));
    assert!(has_element_decorator(dictionary, "value", "Baz"));
}

// "should decorate a Key and Value element on an unspecified Map Declaration
// when a type is specified (type takes precedence over element value
// KEY_VALUE)"
#[test]
fn a_target_type_takes_precedence_over_a_key_value_map_element() {
    let decorated = decorate_test_cto(MAP_DECLARATION_DCS, validate_and_validate_commands());
    let dictionary = map_declaration_ast(&decorated, "Dictionary");
    assert!(has_element_decorator(dictionary, "key", "Bazola"));
    assert!(has_element_decorator(dictionary, "value", "Bongo"));
}

// "should decorate all Map Declaration Key and Value elements on the model
// when a declaration is not specified"
#[test]
fn decorates_every_map_key_and_value_when_no_declaration_is_specified() {
    let decorated = decorate_test_cto(MAP_DECLARATION_DCS, validate_and_validate_commands());
    for name in ["Dictionary", "Rolodex"] {
        let map = map_declaration_ast(&decorated, name);
        assert!(has_element_decorator(map, "key", "DecoratesAllMapKeys"));
        assert!(has_element_decorator(map, "value", "DecoratesAllMapValues"));
    }
}

// ---------------------------------------------------------------------------
// #extractDecorators
// ---------------------------------------------------------------------------

const EXTRACT_TEST: &str = include_str!("testdata/decoratorcommands/extract-test.ast.json");

/// `{ removeDecoratorsFromModel: true, locale: 'en' }`.
fn remove_decorators_en() -> ExtractOptions {
    ExtractOptions {
        remove_decorators_from_model: true,
        locale: "en".to_string(),
    }
}

/// `DecoratorManager.extractVocabularies(testModelManager, { removeDecoratorsFromModel:
/// true, locale: 'en' })` over one model.
fn extract_vocabularies_of(ast_json: &str, file_name: &str) -> Result<Vec<String>> {
    let model_manager = model_manager_with(ast_json, file_name);
    extract_vocabularies(&model_manager, &remove_decorators_en()).map(|r| r.vocabularies)
}

/// `YAML.parse(text)` (the `yaml` npm package, YAML 1.2 core schema) as JSON.
fn yaml_parse(text: &str) -> Value {
    fn to_json(y: &Yaml) -> Value {
        match y {
            Yaml::String(s) => Value::String(s.clone()),
            Yaml::Integer(i) => json!(i),
            Yaml::Real(r) => json!(r.parse::<f64>().expect("a YAML real is a number")),
            Yaml::Boolean(b) => Value::Bool(*b),
            Yaml::Null => Value::Null,
            Yaml::Array(items) => Value::Array(items.iter().map(to_json).collect()),
            Yaml::Hash(map) => Value::Object(
                map.iter()
                    .map(|(k, v)| {
                        let key = match k {
                            Yaml::String(s) => s.clone(),
                            other => panic!("non-string YAML key {other:?}"),
                        };
                        (key, to_json(v))
                    })
                    .collect(),
            ),
            other => panic!("unexpected YAML node {other:?}"),
        }
    }
    let docs = YamlLoader::load_from_str(text)
        .unwrap_or_else(|e| panic!("vocabulary is not valid YAML ({e}):\n{text}"));
    assert_eq!(docs.len(), 1, "one YAML document:\n{text}");
    to_json(&docs[0])
}

/// The single vocabulary `extractVocabularies` returns for a model, parsed.
fn single_vocabulary(ast_json: &str, file_name: &str) -> Value {
    let vocab = extract_vocabularies_of(ast_json, file_name).unwrap();
    assert_eq!(vocab.len(), 1);
    yaml_parse(&vocab[0])
}

// "should be able to extract decorators and vocabs from a model without options"
#[test]
fn extracts_decorators_and_vocabs_without_options() {
    let model_manager = model_manager_with(EXTRACT_TEST, "test.cto");
    let resp = extract_decorators(&model_manager, &ExtractOptions::default());
    // `resp.decoratorCommandSet.should.not.be.null`
    assert!(resp.is_ok());
}

// "should give proper response in there is no vocabulary on any model"
#[test]
fn gives_no_vocabularies_for_a_model_without_vocabulary() {
    let model_manager = model_manager_with(
        include_str!("testdata/decoratorcommands/model-without-vocab.ast.json"),
        "test.cto",
    );
    let resp = extract_decorators(&model_manager, &ExtractOptions::default()).unwrap();
    assert_eq!(resp.vocabularies, Vec::<String>::new());
}

/// The `#extractVocabularies` tests that compare against an expected JSON
/// array of vocabulary strings, and check `vocab[0]` has no `custom`.
fn assert_vocabularies(ast_json: &str, expected_json: &str) {
    let vocab = extract_vocabularies_of(ast_json, "test.cto").unwrap();
    let expected: Vec<String> = serde_json::from_str(expected_json).unwrap();
    assert_eq!(vocab, expected);
    assert!(!vocab[0].contains("custom"));
}

// "should be able to extract vocabs from a model"
#[test]
fn extracts_vocabs_from_a_model() {
    assert_vocabularies(
        EXTRACT_TEST,
        include_str!("testdata/decoratorcommands/extract-test-vocab.json"),
    );
}

// "should be able to extract vocabs from a model without Declaration Term "
#[test]
fn extracts_vocabs_from_a_model_without_declaration_term() {
    assert_vocabularies(
        include_str!("testdata/decoratorcommands/extract-test-without-declaration-term.ast.json"),
        include_str!("testdata/decoratorcommands/extract-test-vocab-without-declaration-term.json"),
    );
}

// "should be able to extract vocabs from a model with terms for namespace"
#[test]
fn extracts_vocabs_from_a_model_with_terms_for_namespace() {
    assert_vocabularies(
        include_str!("testdata/decoratorcommands/extract-test-with-namespace-term.ast.json"),
        include_str!("testdata/decoratorcommands/extract-test-vocab-2.json"),
    );
}

// "should be able to extract vocabs from a model with only terms for namespace"
#[test]
fn extracts_vocabs_from_a_model_with_only_terms_for_namespace() {
    assert_vocabularies(
        include_str!("testdata/decoratorcommands/extract-test-with-only-namespace-term.ast.json"),
        include_str!("testdata/decoratorcommands/extract-test-vocab-3.json"),
    );
}

fn assert_invalid_vocabulary_key(ast_json: &str) {
    let err = extract_vocabularies_of(ast_json, "test.cto").unwrap_err();
    assert!(
        err.to_string().contains("Invalid vocabulary key"),
        "unexpected error: {err}"
    );
}

// "should throw error if namespace level reserved terms found in a model"
#[test]
fn rejects_namespace_level_reserved_terms() {
    assert_invalid_vocabulary_key(include_str!(
        "testdata/decoratorcommands/extract-test-with-namespace-invalid-term.ast.json"
    ));
}

// "should throw error if declaration level reserved terms found in a model"
#[test]
fn rejects_declaration_level_reserved_terms() {
    assert_invalid_vocabulary_key(include_str!(
        "testdata/decoratorcommands/extract-test-with-declaration-invalid-term.ast.json"
    ));
}

// "should throw error if property level reserved terms found in a model"
#[test]
fn rejects_property_level_reserved_terms() {
    assert_invalid_vocabulary_key(include_str!(
        "testdata/decoratorcommands/extract-test-with-property-invalid-term.ast.json"
    ));
}

// "should be able to extract non-vocab decorators from a model"
#[test]
fn extracts_non_vocab_decorators_from_a_model() {
    let model_manager = model_manager_with(EXTRACT_TEST, "test.cto");
    let options = ExtractOptions {
        locale: "en".to_string(),
        ..Default::default()
    };
    let dcs = extract_non_vocab_decorators(&model_manager, &options)
        .unwrap()
        .decorator_command_set;
    let expected = json_file(include_str!(
        "testdata/decoratorcommands/extract-test-dcs.json"
    ));
    assert_eq!(Value::Array(dcs.clone()), expected);
    assert!(Value::Array(dcs).to_string().contains("term_desc"));
}

// "should preserve type reference arguments when extracting decorators"
// (the type references carry the `namespace` only
// `BaseModelManager.resolveMetaModel` adds: `extractDecorators` reads
// `getAst(true, true)`)
#[test]
fn preserves_type_reference_arguments_when_extracting_decorators() {
    let model_manager = model_manager_with(
        include_str!("testdata/decoratorcommands/extract-test-type-reference.ast.json"),
        "test.cto",
    );
    let resp = extract_decorators(&model_manager, &remove_decorators_en()).unwrap();
    let command_set = resp
        .decorator_command_set
        .iter()
        .find(|dcs| dcs.get("name").and_then(Value::as_str) == Some("test"))
        .unwrap()
        .clone();
    let args: Vec<Value> = command_set["commands"]
        .as_array()
        .unwrap()
        .iter()
        .map(|command| command["decorator"]["arguments"].clone())
        .collect();
    let type_ref = |is_array: bool| {
        json!({
            "$class": "concerto.metamodel@1.0.0.DecoratorTypeReference",
            "type": {
                "$class": "concerto.metamodel@1.0.0.TypeIdentifier",
                "name": "Address",
                "namespace": "test@1.0.0"
            },
            "isArray": is_array
        })
    };
    assert_eq!(
        args,
        vec![
            json!([type_ref(false)]),
            json!([
                type_ref(true),
                { "$class": "concerto.metamodel@1.0.0.DecoratorString", "value": "text" }
            ]),
        ]
    );
    validate(&command_set, None).unwrap();
    let decorated = decorate_models(
        &resp.model_manager,
        &mut [command_set],
        &mut DecorateOptions::default(),
    )
    .unwrap();
    let person = decorated.get_declaration("test@1.0.0.Person").unwrap();
    let form = person.get_decorator("Form").unwrap();
    assert!(matches!(
        form.arguments().first(),
        Some(crate::introspect::DecoratorArgument::TypeReference(t)) if t.name == "Address"
    ));
}

// "should correctly quote all YAML hazard categories and round-trip values intact"
#[test]
fn quotes_every_yaml_hazard_category_and_round_trips_the_values() {
    let parsed = single_vocabulary(
        include_str!("testdata/decoratorcommands/extract-test-yaml-edge-cases.ast.json"),
        "edge.cto",
    );
    let props = &parsed["declarations"][0]["properties"];
    // empty string must survive as "" not null
    assert_eq!(props[0]["emptyTerm"], "");
    // YAML_INLINE_SPECIAL: tab, double-quote, backslash, literal \n (2-char, not newline)
    assert_eq!(props[1]["tabInValue"], "col1\tcol2");
    assert_eq!(props[2]["doubleQuoteInValue"], "say \"hello\"");
    assert_eq!(props[3]["backslashInValue"], "path\\to\\file");
    assert_eq!(props[4]["literalBackslashN"], "literal\\n value");
    // YAML_BLOCK_INDICATORS: first-char triggers block/flow/tag/anchor/alias/directive
    assert_eq!(props[5]["gtChar"], "> folded block");
    assert_eq!(props[6]["pipeChar"], "| literal block");
    assert_eq!(props[7]["exclamationChar"], "!tag handle");
    assert_eq!(props[8]["ampersandChar"], "&anchor ref");
    assert_eq!(props[9]["asteriskChar"], "*alias ref");
    assert_eq!(props[10]["percentChar"], "%directive");
    // YAML_RESERVED_WORDS: uppercase variants that /i flag must catch
    assert_eq!(props[11]["uppercaseTrue"], "TRUE");
    assert_eq!(props[12]["uppercaseYes"], "YES");
    assert_eq!(props[13]["uppercaseNull"], "NULL");
    assert_eq!(props[14]["yamlNull"], "~");
    // YAML_NUMERIC: leading-dot decimal, negative, scientific notation
    assert_eq!(props[15]["leadingDotDecimal"], ".5");
    assert_eq!(props[16]["negativeDecimal"], "-1.5");
    assert_eq!(props[17]["scientificNotation"], "1e10");
    // YAML_INLINE_SPECIAL: carriage return
    assert_eq!(props[18]["carriageReturn"], "foo\rbar");
    // YAML_BLOCK_INDICATORS: dash-space and question-space trigger sequence/mapping
    assert_eq!(props[19]["dashSpaceValue"], "- foo");
    assert_eq!(props[20]["questionSpaceValue"], "? key");
    // YAML_NUMERIC: YAML 1.1 hex/octal/binary forms coerced to numbers without quotes
    assert_eq!(props[21]["hexValue"], "0x1A");
    assert_eq!(props[22]["octalValue"], "0o10");
    assert_eq!(props[23]["binaryValue"], "0b11");
}

// "should correctly quote complex YAML-like string values embedded in Term decorators"
#[test]
fn quotes_complex_yaml_like_term_values() {
    let parsed = single_vocabulary(
        include_str!("testdata/decoratorcommands/extract-test-yaml-complex.ast.json"),
        "test.cto",
    );
    let props = &parsed["declarations"][0]["properties"];
    // newlines, colon, single-quote all round-trip intact
    assert_eq!(
        props[0]["name"],
        "name: Martin D'vloper\nage: 26\nhobbies:\n  - painting\n  - playing_music"
    );
    // hash comment char round-trips intact
    assert_eq!(props[1]["status"], "status: active # reviewed");
    // leading/trailing spaces round-trip intact
    assert_eq!(props[2]["label"], " leading and trailing ");
    // YAML flow-sequence syntax round-trips intact
    assert_eq!(props[3]["tags"], "[primary, secondary]");
}

// "should handle all parseVocabularies code paths — namespace, declaration,
// property, mapElement with YAML-special strings and non-string Term_ types"
#[test]
fn handles_every_parse_vocabularies_path_with_non_string_term_extensions() {
    let parsed = single_vocabulary(
        include_str!("testdata/decoratorcommands/extract-test-allpaths-nonstring.ast.json"),
        "allpaths.cto",
    );
    // namespace-level Term with colon (must be quoted) and Term_* with Number and Boolean
    assert_eq!(parsed["term"], "My Namespace: Title");
    assert_eq!(parsed["version"], 3);
    assert_eq!(parsed["active"], true);

    let decls = &parsed["declarations"];
    // declaration-level Term with colon and Term_* with Number and Boolean
    assert_eq!(decls[0]["Product"], "Product: Details");
    assert_eq!(decls[0]["sortOrder"], 1);
    assert_eq!(decls[0]["featured"], false);

    // property-level Term with colon and Term_* with Number and Boolean
    let props = &decls[0]["properties"];
    assert_eq!(props[0]["fieldName"], "Field: Label");
    assert_eq!(props[0]["rank"], 2);
    assert_eq!(props[0]["visible"], true);

    // mapElement-level Term with colon (must be quoted) and Term_* with Number and Boolean
    let map_props = &decls[1]["properties"];
    assert_eq!(map_props[0]["KEY"], "Key: Identifier");
    assert_eq!(map_props[0]["weight"], 5);
    assert_eq!(map_props[1]["VALUE"], "Value: Data");
    assert_eq!(map_props[1]["required"], false);
}

// "should round-trip Term_ extension keys whose name ends with _type"
#[test]
fn round_trips_term_extension_keys_ending_in_type() {
    let parsed = single_vocabulary(
        include_str!("testdata/decoratorcommands/extract-test-type-suffix.ast.json"),
        "test.cto",
    );
    // namespace-level: my_type key must appear (not filtered)
    assert_eq!(parsed["my_type"], "namespace type value");
    // declaration-level: my_type survives
    let decl = &parsed["declarations"][0];
    assert_eq!(decl["my_type"], "concept type value");
    // property-level: my_type survives
    assert_eq!(decl["properties"][0]["my_type"], "field type value");
}

// "should preserve falsy numeric and boolean Term values at property level"
#[test]
fn preserves_falsy_numeric_and_boolean_property_terms() {
    let parsed = single_vocabulary(
        include_str!("testdata/decoratorcommands/extract-test-falsy-nonstring-prop-term.ast.json"),
        "test.cto",
    );
    let props = &parsed["declarations"][0]["properties"];
    // @Term(0) must emit 0, not be coerced to '' by || fallback
    assert_eq!(props[0]["price"], 0);
    // @Term(false) must emit false, not be coerced to '' by || fallback
    assert_eq!(props[1]["active"], false);
}

// "should preserve falsy empty-string Term values at namespace, declaration,
// and property level"
#[test]
fn preserves_falsy_empty_string_terms_at_every_level() {
    let parsed = single_vocabulary(
        include_str!("testdata/decoratorcommands/extract-test-falsy-term.ast.json"),
        "test.cto",
    );
    // namespace-level empty-string term must be preserved, not skipped
    assert_eq!(parsed["term"], "");
    let decls = &parsed["declarations"];
    // declaration with only @Term("") must emit "" not be replaced by fallback label
    assert_eq!(decls[0]["EmptyTermConcept"], "");
    // property with @Term("") must round-trip as ""
    assert_eq!(decls[0]["properties"][0]["myField"], "");
    // declaration with @Term("") + extension: term must be "", extension must
    // appear, no fallback label
    assert_eq!(decls[1]["EmptyTermWithExtension"], "");
    assert_eq!(decls[1]["desc"], "has extension too");
}

// "should correctly quote strings that are syntactically invalid YAML when unquoted"
#[test]
fn quotes_strings_that_are_invalid_yaml_unquoted() {
    let parsed = single_vocabulary(
        include_str!("testdata/decoratorcommands/extract-test-yaml-invalid.ast.json"),
        "invalid.cto",
    );
    let props = &parsed["declarations"][0]["properties"];
    // unclosed flow collections — syntax errors without quoting
    assert_eq!(props[0]["unclosedBracket"], "[unclosed bracket");
    assert_eq!(props[1]["unclosedBrace"], "{key: val");
    // YAML document markers — would terminate/start document without quoting
    assert_eq!(props[2]["docStartMarker"], "---");
    assert_eq!(props[3]["docEndMarker"], "...");
    // actual embedded newline — block scalar without quoting breaks inline context
    assert_eq!(props[4]["embeddedNewline"], "line1\nline2");
    // first-char reserved indicators
    assert_eq!(props[5]["backtick"], "`template`");
    assert_eq!(props[6]["atSign"], "@decorated");
    // colon at end — creates a mapping key without quoting
    assert_eq!(props[7]["colonAtEnd"], "foo:");
    // single quote mid-string — valid plain scalar, must NOT be over-quoted
    assert_eq!(props[8]["singleQuote"], "it's here");
}

// "should fall back to property name for term when only Term_ extension exists
// with no Term"
#[test]
fn falls_back_to_the_property_name_when_only_a_term_extension_exists() {
    let parsed = single_vocabulary(
        include_str!("testdata/decoratorcommands/extract-test-prop-extension-no-term.ast.json"),
        "test.cto",
    );
    let prop_entry = &parsed["declarations"][0]["properties"][0];
    // term must fall back to property name (consistent with declaration
    // fallback), not null or ""
    assert_eq!(prop_entry["myField"], "myField");
    // Term_ extension key must be present alongside it
    assert_eq!(prop_entry["desc"], "my desc");
}
