//! The shared traits, implemented by the introspection types.

use concerto_core::{
    Declaration, DeclarationKind, Decorated, HasValidators, ModelManager, Named, Property, Typed,
    Validate,
};
use serde_json::{Value, json};

const MM: &str = "concerto.metamodel@1.0.0";

fn declaration(class: &str, body: Value) -> Declaration {
    let mut node = json!({ "$class": format!("{MM}.{class}") });
    for (key, value) in body.as_object().unwrap() {
        node[key] = value.clone();
    }
    Declaration::try_from(&node).expect("valid declaration")
}

fn decorator(name: &str) -> Value {
    json!({ "$class": format!("{MM}.Decorator"), "name": name })
}

#[test]
fn every_declaration_is_named_and_knows_its_kind() {
    let cases = [
        (
            "AssetDeclaration",
            json!({ "name": "Car", "isAbstract": false, "properties": [] }),
        ),
        (
            "EnumDeclaration",
            json!({ "name": "Colour", "properties": [] }),
        ),
        ("LongScalar", json!({ "name": "Count" })),
        (
            "MapDeclaration",
            json!({
                "name": "Dictionary",
                "key": { "$class": format!("{MM}.StringMapKeyType") },
                "value": { "$class": format!("{MM}.StringMapValueType") }
            }),
        ),
    ];
    for (class, body) in cases {
        let name = body["name"].as_str().unwrap().to_string();
        let declaration = declaration(class, body);
        assert_eq!(declaration.name(), name);
        assert_eq!(declaration.declaration_kind(), class);
    }

    let class = declaration(
        "EventDeclaration",
        json!({ "name": "Happened", "isAbstract": false, "properties": [] }),
    );
    let class = class.as_class().unwrap();
    assert_eq!(class.name(), "Happened");
    assert_eq!(class.declaration_kind(), "EventDeclaration");
    assert_eq!(class.kind().declaration_kind(), "EventDeclaration");
}

#[test]
fn only_a_scalar_declaration_has_a_type() {
    let scalar = declaration("StringScalar", json!({ "name": "Email" }));
    assert_eq!(scalar.type_name(), Some("String"));
    assert_eq!(scalar.as_scalar().unwrap().type_name(), Some("String"));

    let concept = declaration(
        "ConceptDeclaration",
        json!({ "name": "Person", "isAbstract": false, "properties": [] }),
    );
    assert_eq!(concept.type_name(), None);
    let enumeration = declaration("EnumDeclaration", json!({ "name": "E", "properties": [] }));
    assert_eq!(enumeration.type_name(), None);
}

#[test]
fn class_declarations_and_properties_carry_their_decorators() {
    let concept = declaration(
        "ConceptDeclaration",
        json!({
            "name": "Person",
            "isAbstract": false,
            "decorators": [decorator("Term"), decorator("Hidden")],
            "properties": [{
                "$class": format!("{MM}.StringProperty"),
                "name": "email", "isArray": false, "isOptional": false,
                "decorators": [decorator("PII")]
            }, {
                "$class": format!("{MM}.EnumProperty"),
                "name": "RED",
                "decorators": [decorator("Colour")]
            }]
        }),
    );
    let class = concept.as_class().unwrap();
    let names = |decorated: &dyn Decorated| -> Vec<String> {
        decorated
            .decorators()
            .iter()
            .map(|d| d.name.clone())
            .collect()
    };
    assert_eq!(names(class), ["Term", "Hidden"]);
    assert_eq!(names(&class.own_properties()[0]), ["PII"]);
    assert_eq!(names(&class.own_properties()[1]), ["Colour"]);

    let bare = declaration(
        "ConceptDeclaration",
        json!({ "name": "Bare", "isAbstract": false, "properties": [] }),
    );
    assert!(bare.as_class().unwrap().decorators().is_empty());
}

#[test]
fn a_loaded_element_passes_its_validator_checks() {
    let property = Property::try_from(&json!({
        "$class": format!("{MM}.IntegerProperty"),
        "name": "age", "isArray": false, "isOptional": false,
        "validator": { "$class": format!("{MM}.IntegerDomainValidator"), "lower": 0, "upper": 150 }
    }))
    .unwrap();
    assert!(property.check_validators().is_ok());
    assert_eq!(property.type_name(), Some("Integer"));

    let scalar = declaration(
        "StringScalar",
        json!({
            "name": "Code",
            "validator": { "$class": format!("{MM}.StringRegexValidator"), "pattern": "^[A-Z]+$", "flags": "" },
            "lengthValidator": { "$class": format!("{MM}.StringLengthValidator"), "minLength": 1, "maxLength": 4 }
        }),
    );
    assert!(scalar.as_scalar().unwrap().check_validators().is_ok());
}

#[test]
fn a_string_scalar_with_a_bad_validator_is_rejected_at_load() {
    let length = Declaration::try_from(&json!({
        "$class": format!("{MM}.StringScalar"),
        "name": "Code",
        "lengthValidator": { "$class": format!("{MM}.StringLengthValidator"), "minLength": 5, "maxLength": 4 }
    }));
    assert_eq!(
        length.unwrap_err().to_string(),
        "illegal model: minLength must be less than or equal to maxLength on Code"
    );
    let pattern = Declaration::try_from(&json!({
        "$class": format!("{MM}.StringScalar"),
        "name": "Code",
        "validator": { "$class": format!("{MM}.StringRegexValidator"), "pattern": "(", "flags": "" }
    }));
    assert!(
        pattern
            .unwrap_err()
            .to_string()
            .starts_with("illegal model: Invalid regular expression on Code")
    );
}

#[test]
fn a_declaration_validates_against_the_loaded_models() {
    let mut manager = ModelManager::new().unwrap();
    manager
        .add_model(
            &json!({
                "$class": format!("{MM}.Model"),
                "namespace": "org.example@1.0.0",
                "declarations": [{
                    "$class": format!("{MM}.ConceptDeclaration"),
                    "name": "Child", "isAbstract": false, "properties": [],
                    "superType": { "$class": format!("{MM}.TypeIdentifier"), "name": "Missing" }
                }, {
                    "$class": format!("{MM}.EnumDeclaration"),
                    "name": "Colour", "properties": []
                }]
            }),
            None,
        )
        .unwrap();
    let model_file = manager.model_file("org.example@1.0.0").unwrap();
    let [child, colour] = model_file.declarations() else {
        panic!("two declarations");
    };
    assert_eq!(
        child
            .validate(&manager, "org.example@1.0.0")
            .unwrap_err()
            .to_string(),
        "Could not find super type Missing for Child"
    );
    assert!(colour.validate(&manager, "org.example@1.0.0").is_ok());
}
