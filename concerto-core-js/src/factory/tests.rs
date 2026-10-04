use super::*;

struct Env;

impl InstanceEnv for Env {
    fn new_id(&mut self) -> String {
        "00000000-0000-4000-8000-000000000000".into()
    }
    fn now_ms(&mut self) -> f64 {
        0.0
    }
}

fn manager(cto_ast: concerto_core::json::Value) -> ModelManager {
    let mut mm = ModelManager::new().expect("a model manager");
    mm.add_model_with_definitions(&cto_ast, None, Some("test.cto".into()))
        .expect("the model loads");
    mm
}

fn model() -> ModelManager {
    manager(concerto_core::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.acme@1.0.0",
        "imports": [],
        "declarations": [
            {
                "$class": "concerto.metamodel@1.0.0.AssetDeclaration",
                "name": "Car",
                "isAbstract": false,
                "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "vin" },
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "vin",
                      "isArray": false, "isOptional": false,
                      "validator": { "$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": "^[A-Z]+$", "flags": "" } },
                    { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "wheels",
                      "isArray": false, "isOptional": false, "defaultValue": 4 }
                ]
            },
            {
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Shape",
                "isAbstract": true,
                "properties": []
            },
            {
                "$class": "concerto.metamodel@1.0.0.TransactionDeclaration",
                "name": "Tx",
                "isAbstract": false,
                "properties": []
            }
        ]
    }))
}

#[test]
fn new_resource_builds_a_validated_resource_with_defaults() {
    let mm = model();
    let car = new_resource(
        &mm,
        "org.acme@1.0.0",
        "Car",
        JsValue::String("ABC".into()),
        false,
        &mut Env,
    )
    .expect("a car");
    assert_eq!(car.kind, InstanceKind::ValidatedResource);
    assert_eq!(
        keys(&car.props),
        [
            "$namespace",
            "$type",
            "$identifierFieldName",
            "$identifier",
            "vin",
            "$timestamp",
            "wheels"
        ]
    );
    assert_eq!(car.get("wheels"), &JsValue::Number(4.0));
    assert_eq!(car.get("$timestamp"), &JsValue::Null);
}

/// BC-45: instance creation applies every default, so a `DateTime`
/// default that is not strict throws a `ValidationException` there; the
/// model itself loads, and a strict default is set.
#[test]
fn new_resource_rejects_a_non_strict_date_time_default() {
    let model_with = |default: &str| {
        manager(concerto_core::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.dates@1.0.0",
            "imports": [],
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "C",
                "isAbstract": false,
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.DateTimeProperty", "name": "at",
                      "isArray": false, "isOptional": true, "defaultValue": default }
                ]
            }]
        }))
    };
    let create = |mm: &ModelManager| {
        new_resource(
            mm,
            "org.dates@1.0.0",
            "C",
            JsValue::Undefined,
            false,
            &mut Env,
        )
    };
    let ok = create(&model_with("2008-09-15T15:53:00Z")).expect("a strict default");
    assert!(matches!(ok.get("at"), JsValue::DateTime(d) if d.is_valid()));
    for bad in [
        "2008-09-15T15:53:00",
        "2022-11-18",
        "",
        "FOO",
        "2024-02-30T00:00:00Z",
    ] {
        let err = create(&model_with(bad)).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Validation, "{err}");
        assert_eq!(err.code(), "typed-assignfielddefaults-datetime", "{bad}");
    }
}

#[test]
fn new_resource_checks_in_ts_order() {
    let mm = model();
    let message = |r: Result<Instance>| match r.map_err(Error::into_contract) {
        Err(e) => e.message(),
        other => panic!("expected an error, got {other:?}"),
    };
    assert_eq!(
        message(new_resource(
            &mm,
            "org.acme@1.0.0",
            "Shape",
            JsValue::Undefined,
            false,
            &mut Env
        )),
        "Cannot instantiate the abstract type \"Shape\" in the \"org.acme@1.0.0\" namespace."
    );
    assert_eq!(
        message(new_resource(
            &mm,
            "org.acme@1.0.0",
            "Car",
            JsValue::Number(1.0),
            false,
            &mut Env
        )),
        "Invalid or missing identifier for Type \"Car\" in namespace \"org.acme@1.0.0\"."
    );
    assert_eq!(
        message(new_resource(
            &mm,
            "org.acme@1.0.0",
            "Car",
            JsValue::String(" ".into()),
            false,
            &mut Env
        )),
        "Missing identifier for Type \"Car\" in namespace \"org.acme@1.0.0\"."
    );
    assert_eq!(
        message(new_resource(
            &mm,
            "org.acme@1.0.0",
            "Car",
            JsValue::String("abc".into()),
            false,
            &mut Env
        )),
        "Provided id does not match regex: /^[A-Z]+$/"
    );
    assert_eq!(
        message(new_resource(
            &mm,
            "org.acme@1.0.0",
            "Tx",
            JsValue::String("1".into()),
            false,
            &mut Env
        )),
        "Type is not identifiable org.acme@1.0.0.Tx"
    );
}

#[test]
fn transactions_get_a_timestamp() {
    let mm = model();
    let tx = new_transaction(
        &mm,
        &JsValue::String("org.acme@1.0.0".into()),
        &JsValue::String("Tx".into()),
        JsValue::Undefined,
        false,
        &mut Env,
    )
    .expect("a transaction");
    assert!(matches!(tx.get("$timestamp"), JsValue::DateTime(_)));
    let message = match new_transaction(
        &mm,
        &JsValue::Undefined,
        &JsValue::String("Tx".into()),
        JsValue::Undefined,
        false,
        &mut Env,
    )
    .map_err(Error::into_contract)
    {
        Err(e) => e.message(),
        other => panic!("expected an error, got {other:?}"),
    };
    assert_eq!(message, "ns not specified");
}
