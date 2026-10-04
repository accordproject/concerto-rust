//! The stable surface of docs/public-api.md section 5, as a native caller
//! uses it (task P6-01, accordproject/concerto-rust#83).

use concerto_core::json;
use concerto_core::json::Value;
use concerto_core::model_manager::AstOptions;
use concerto_core::model_util::{ParsedNamespace, parse_namespace, qualify, short_name};
use concerto_core::{ClassKind, Declaration, ErrorKind, ModelManager};

const MM: &str = "concerto.metamodel@1.0.0";

fn concept(name: &str, super_type: Option<&str>, properties: Value) -> Value {
    let mut node = json!({
        "$class": format!("{MM}.ConceptDeclaration"),
        "name": name,
        "isAbstract": false,
        "properties": properties,
    });
    if let Some(super_type) = super_type {
        node["superType"] = json!({ "$class": format!("{MM}.TypeIdentifier"), "name": super_type });
    }
    node
}

fn string_field(name: &str, optional: bool) -> Value {
    json!({ "$class": format!("{MM}.StringProperty"), "name": name, "isArray": false, "isOptional": optional })
}

fn object_field(name: &str, type_name: &str) -> Value {
    json!({
        "$class": format!("{MM}.ObjectProperty"),
        "name": name,
        "isArray": false,
        "isOptional": false,
        "type": { "$class": format!("{MM}.TypeIdentifier"), "name": type_name }
    })
}

/// `org.acme@1.0.0`: `Address`, `Person` (identified by `email`, with an
/// `address`), `Employee extends Person`, an asset, and an enum.
fn model() -> Value {
    json!({
        "$class": format!("{MM}.Model"),
        "namespace": "org.acme@1.0.0",
        "declarations": [
            concept("Address", None, json!([string_field("city", false)])),
            {
                "$class": format!("{MM}.ConceptDeclaration"),
                "name": "Person",
                "isAbstract": false,
                "identified": { "$class": format!("{MM}.IdentifiedBy"), "name": "email" },
                "properties": [string_field("email", false), object_field("address", "Address")]
            },
            concept("Employee", Some("Person"), json!([string_field("team", true)])),
            {
                "$class": format!("{MM}.AssetDeclaration"),
                "name": "Car",
                "isAbstract": false,
                "properties": []
            },
            {
                "$class": format!("{MM}.EnumDeclaration"),
                "name": "Colour",
                "properties": [{ "$class": format!("{MM}.EnumProperty"), "name": "RED" }]
            }
        ]
    })
}

fn loaded() -> ModelManager {
    let mut manager = ModelManager::new().unwrap();
    manager.add_model_ast(&model(), Some("acme.cto")).unwrap();
    manager.validate_models().unwrap();
    manager
}

fn names<T>(found: Vec<(String, T)>) -> Vec<String> {
    found.into_iter().map(|(name, _)| name).collect()
}

#[test]
fn loads_from_an_ast_or_its_text() {
    let mut manager = ModelManager::builder()
        .metamodel_validation(true)
        .build()
        .unwrap();
    assert!(manager.metamodel_validation());
    let text = model().to_string();
    let id = manager.add_model_ast_text(&text, Some("acme.cto")).unwrap();
    assert_eq!(
        manager.file(id).map(|mf| mf.namespace()),
        Some("org.acme@1.0.0")
    );
    manager.validate_models().unwrap();

    // Loading the same namespace twice is an error.
    let err = manager.add_model_ast(&model(), None).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidArgument);

    // Text that is not JSON is an `IllegalModel` error naming the file.
    let err = manager
        .add_model_ast_text("{", Some("broken.cto"))
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::IllegalModel);
    assert_eq!(err.file_name(), Some("broken.cto"));
}

#[test]
fn a_failed_batch_load_changes_nothing() {
    let mut manager = ModelManager::new().unwrap();
    let before = manager.model_files().count();
    let orphan = json!({
        "$class": format!("{MM}.Model"),
        "namespace": "org.orphan@1.0.0",
        "declarations": [concept("Child", Some("Missing"), json!([]))]
    });
    assert!(manager.add_model_asts([(&orphan, None)]).is_err());
    assert_eq!(manager.model_files().count(), before);

    let ids = manager
        .add_model_asts([(&model(), Some("acme.cto"))])
        .unwrap();
    assert_eq!(ids.len(), 1);
}

#[test]
fn updates_and_removes_a_model_in_place() {
    let mut manager = loaded();
    let mut updated = model();
    updated["declarations"] = json!([concept("Address", None, json!([]))]);
    manager
        .update_model_ast(&updated, Some("acme.cto"))
        .unwrap();
    assert!(manager.get_declaration("org.acme@1.0.0.Person").is_err());
    assert!(manager.get_declaration("org.acme@1.0.0.Address").is_ok());

    manager.remove_model("org.acme@1.0.0").unwrap();
    assert!(manager.model_file("org.acme@1.0.0").is_none());
    assert!(manager.remove_model("org.acme@1.0.0").is_err());

    let err = manager.update_model_ast(&model(), None).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
}

#[test]
fn walks_the_declarations_and_the_inheritance() {
    let manager = loaded();
    assert!(
        manager
            .declarations()
            .any(|(fqn, _)| fqn == "org.acme@1.0.0.Colour")
    );
    // The system models are included.
    assert!(
        manager
            .declarations()
            .any(|(fqn, _)| fqn == "concerto@1.0.0.Concept")
    );
    let assets: Vec<String> = manager
        .class_declarations_of_kind(ClassKind::Asset)
        .map(|(fqn, class)| {
            assert_eq!(class.kind(), ClassKind::Asset);
            fqn
        })
        .collect();
    assert_eq!(assets, ["org.acme@1.0.0.Car"]);
    let enums: Vec<String> = manager.enum_declarations().map(|(fqn, _)| fqn).collect();
    assert_eq!(enums, ["org.acme@1.0.0.Colour"]);

    let (super_fqn, super_decl) = manager
        .super_type("org.acme@1.0.0.Employee")
        .unwrap()
        .unwrap();
    assert_eq!(super_fqn, "org.acme@1.0.0.Person");
    assert_eq!(super_decl.name(), "Person");
    assert_eq!(
        names(manager.super_types("org.acme@1.0.0.Employee").unwrap()),
        ["org.acme@1.0.0.Person", "concerto@1.0.0.Concept"]
    );
    assert_eq!(
        names(manager.subclasses("org.acme@1.0.0.Person").unwrap()),
        ["org.acme@1.0.0.Employee"]
    );
    assert_eq!(
        names(manager.assignable_types("org.acme@1.0.0.Person").unwrap()),
        ["org.acme@1.0.0.Person", "org.acme@1.0.0.Employee"]
    );
    assert!(
        manager
            .super_type("concerto@1.0.0.Concept")
            .unwrap()
            .is_none()
    );
    assert!(
        manager
            .is_assignable_to("org.acme@1.0.0.Employee", "org.acme@1.0.0.Person")
            .unwrap()
    );
}

#[test]
fn reads_properties_own_inherited_and_nested() {
    let manager = loaded();
    let employee = "org.acme@1.0.0.Employee";
    let own: Vec<&str> = manager
        .own_properties(employee)
        .unwrap()
        .iter()
        .map(|p| p.name())
        .collect();
    assert_eq!(own, ["team"]);

    let all: Vec<(String, &str)> = manager
        .properties(employee)
        .unwrap()
        .into_iter()
        .map(|(owner, p)| (owner, p.name()))
        .collect();
    assert_eq!(
        all,
        [
            ("org.acme@1.0.0.Employee".to_string(), "team"),
            ("org.acme@1.0.0.Person".to_string(), "email"),
            ("org.acme@1.0.0.Person".to_string(), "address"),
        ]
    );
    // `main`'s signature: the properties alone, in the same order.
    let plain: Vec<&str> = manager
        .get_all_properties(employee)
        .unwrap()
        .into_iter()
        .map(|p| p.name())
        .collect();
    assert_eq!(plain, ["team", "email", "address"]);

    let (owner, email) = manager.property(employee, "email").unwrap().unwrap();
    assert_eq!(
        (owner.as_str(), email.name()),
        ("org.acme@1.0.0.Person", "email")
    );
    assert!(manager.property(employee, "nope").unwrap().is_none());

    let (owner, city) = manager.property_path(employee, "address.city").unwrap();
    assert_eq!(
        (owner.as_str(), city.name()),
        ("org.acme@1.0.0.Address", "city")
    );
    assert_eq!(city.type_name(), Some("String"));

    assert_eq!(manager.identifier_field(employee).unwrap(), Some("email"));
    assert!(manager.is_identified(employee).unwrap());
    assert_eq!(
        manager.identifier_field("org.acme@1.0.0.Address").unwrap(),
        None
    );
}

#[test]
fn gives_the_ast_and_filters() {
    let manager = loaded();
    let ast = manager.ast(AstOptions::default()).unwrap();
    assert_eq!(ast["models"].as_array().map(Vec::len), Some(1));
    let with_system = manager
        .ast(AstOptions {
            resolve: true,
            include_system_models: true,
        })
        .unwrap();
    assert_eq!(with_system["models"].as_array().map(Vec::len), Some(3));

    let kept = manager
        .filter(|fqn, declaration| {
            fqn.starts_with("org.acme@1.0.0.") && !matches!(declaration, Declaration::Enum(_))
        })
        .unwrap();
    assert!(kept.get_declaration("org.acme@1.0.0.Person").is_ok());
    assert!(kept.get_declaration("org.acme@1.0.0.Colour").is_err());
}

#[test]
fn a_model_file_answers_by_short_or_qualified_name() {
    let manager = loaded();
    let mf = manager.model_file("org.acme@1.0.0").unwrap();
    assert!(mf.local_type("Person").is_some());
    assert!(mf.local_type("org.acme@1.0.0.Person").is_some());
    assert_eq!(
        mf.fully_qualified_type_name("Person").as_deref(),
        Some("org.acme@1.0.0.Person")
    );
    assert_eq!(mf.asset_declarations().count(), 1);
    assert!(mf.asset_declaration("Car").is_some());
    assert_eq!(mf.enum_declarations().count(), 1);
    assert_eq!(mf.class_declarations().count(), 5);
    assert_eq!(mf.scalar_declarations().count(), 0);
    assert!(
        mf.imported_type_names()
            .iter()
            .any(|name| name == "concerto@1.0.0.Concept")
    );
    assert!(mf.external_imports().is_empty());
}

#[test]
fn errors_are_read_through_accessors() {
    let located = json!({
        "$class": format!("{MM}.Model"),
        "namespace": "org.bad@1.0.0",
        "declarations": [{
            "$class": format!("{MM}.ConceptDeclaration"),
            "name": "Child",
            "isAbstract": false,
            "properties": [],
            "superType": { "$class": format!("{MM}.TypeIdentifier"), "name": "Missing" },
            "location": {
                "$class": format!("{MM}.Range"),
                "start": { "$class": format!("{MM}.Position"), "line": 2, "column": 1, "offset": 10 },
                "end": { "$class": format!("{MM}.Position"), "line": 4, "column": 2, "offset": 40 }
            }
        }]
    });
    let mut manager = ModelManager::new().unwrap();
    manager.add_model_ast(&located, Some("bad.cto")).unwrap();
    let err = manager.validate_models().unwrap_err();
    assert_eq!(err.kind(), ErrorKind::IllegalModel);
    assert!(!err.code().is_empty());
    assert!(!err.to_string().is_empty());
    let location = err.location().expect("the class's location");
    assert_eq!((location.start.line, location.start.column), (2, 1));
    assert_eq!((location.end.line, location.end.offset), (4, 40));
    assert!(err.details().is_empty());
    let source: &dyn std::error::Error = &err;
    assert!(source.source().is_none());

    let err = manager.get_declaration("org.bad@1.0.0.Nope").unwrap_err();
    assert_eq!(err.kind(), ErrorKind::TypeNotFound);
    assert_eq!(err.code(), "pre-port");
}

#[test]
fn names_split_and_join() {
    assert_eq!(short_name("org.acme@1.0.0.Person"), "Person");
    assert_eq!(qualify("org.acme@1.0.0", "Person"), "org.acme@1.0.0.Person");
    let ParsedNamespace::Full { name, version, .. } = parse_namespace("org.acme@1.0.0").unwrap()
    else {
        panic!("a full parse");
    };
    assert_eq!(
        (name.as_str(), version.as_deref()),
        ("org.acme", Some("1.0.0"))
    );
    assert!(parse_namespace("").is_err());
}

#[test]
fn the_metamodel_check_is_reachable_without_the_instance_module() {
    assert_eq!(concerto_core::metamodel::NAMESPACE, MM);
    concerto_core::metamodel::validate_ast(&model()).unwrap();
    let err = concerto_core::metamodel::validate_structure(&json!({
        "$class": format!("{MM}.Model"),
        "namespace": "org.acme@1.0.0",
        "declarations": [{ "$class": format!("{MM}.ConceptDeclaration") }]
    }))
    .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Metamodel);
}

#[test]
fn validates_a_plain_json_instance_first_error_or_collect_all() {
    use concerto_core::error::DetailCode;
    use concerto_core::instance::{DiagnosticCode, ValidationOptions};

    let manager = loaded();
    let options = ValidationOptions::default();
    let person = json!({
        "$class": "org.acme@1.0.0.Person",
        "email": "a@example.com",
        "address": { "$class": "org.acme@1.0.0.Address", "city": "Paris" }
    });
    manager.validate_instance(&person, &options).unwrap();
    assert!(manager.check_instance(&person, &options).is_valid());

    // A nested object with no `$class` is read as its declared type.
    let employee = json!({ "email": "b@example.com", "address": { "city": "Rome" } });
    manager
        .validate_instance_as("org.acme@1.0.0.Employee", &employee, &options)
        .unwrap();
    assert!(
        manager
            .check_instance_as("org.acme@1.0.0.Person", &person, &options)
            .into_result()
            .is_ok()
    );

    // First error, as `Serializer.fromJSON` throws it.
    let missing = json!({ "$class": "org.acme@1.0.0.Person", "email": "c@example.com" });
    let err = manager.validate_instance(&missing, &options).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Validation);
    let report = manager.check_instance(&missing, &options);
    assert_eq!(report.diagnostics().len(), 1);
    assert_eq!(
        report.diagnostics()[0].code,
        DiagnosticCode::MissingRequiredProperty
    );
    assert_eq!(report.diagnostics()[0].pointer, "/address");

    // The #1273 options, with their details.
    let unknown = json!({
        "$class": "org.acme@1.0.0.Person",
        "email": "d@example.com",
        "address": { "$class": "org.acme@1.0.0.Address", "city": "Oslo", "zip": null }
    });
    manager.validate_instance(&unknown, &options).unwrap();
    let err = manager
        .validate_instance(&unknown, &ValidationOptions::STRICT)
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Validation);
    assert_eq!(err.details()[0].code, DetailCode::UnknownProperty);
    assert_eq!(err.details()[0].path, "$.address.zip");
    let report = manager.check_instance(&unknown, &ValidationOptions::STRICT);
    let diagnostics: Vec<_> = report.into_iter().collect();
    assert_eq!(diagnostics[0].code, DiagnosticCode::UndeclaredField);
    assert_eq!(diagnostics[0].pointer, "/address/zip");

    // An instance of a type that is not assignable to the named one.
    let car = json!({ "$class": "org.acme@1.0.0.Car" });
    let report = manager.check_instance_as("org.acme@1.0.0.Person", &car, &options);
    assert_eq!(report.diagnostics()[0].code, DiagnosticCode::NotAssignable);
}
