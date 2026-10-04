//! A stale plan is never used. Each test builds plans by
//! validating, changes the model, then validates an instance whose
//! answer depends on the change. And a cached build error is the error a
//! fresh build raises.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use crate::json;
use crate::json::Value;

use super::class_plan;
use crate::error::ErrorKind;
use crate::instance::validate::{ValidateOptions, validate_instance};
use crate::introspect::model_file::ModelFile;
use crate::model_manager::{ModelFileSource, ModelManager};

const NS: &str = "org.acme@1.0.0";

/// The number of plans `mm` holds, and of their properties.
fn stats(mm: &ModelManager) -> (usize, usize) {
    mm.plan_cache_stats()
}

/// `concept P { o String name [regex] [o Integer age] }`.
fn p_model(regex: Option<&str>, with_age: bool) -> Value {
    let mut name = json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": "name", "isArray": false, "isOptional": false
    });
    if let Some(pattern) = regex {
        name["validator"] = json!({
            "$class": "concerto.metamodel@1.0.0.StringRegexValidator",
            "pattern": pattern, "flags": ""
        });
    }
    let mut properties = vec![name];
    if with_age {
        properties.push(json!({
            "$class": "concerto.metamodel@1.0.0.IntegerProperty",
            "name": "age", "isArray": false, "isOptional": false
        }));
    }
    json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": NS,
        "declarations": [{
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "P", "isAbstract": false, "properties": properties
        }]
    })
}

fn manager(model: &Value) -> ModelManager {
    let mut mm = ModelManager::new().unwrap();
    mm.load_model(model, Some("p.cto".into())).unwrap();
    mm
}

fn check(mm: &ModelManager, v: Value) -> crate::error::Result<()> {
    validate_instance(mm, &v, &ValidateOptions::default())
}

fn x() -> Value {
    json!({ "$class": "org.acme@1.0.0.P", "name": "x" })
}

/// A declaration handle no declaration holds (the
/// JS binding's `validatePropertyById` takes one from JS) fails, and is
/// not cached: caching it would size the cache by the handle.
#[test]
fn an_unknown_declaration_handle_fails_uncached() {
    let mm = manager(&p_model(None, false));
    let before = stats(&mm);
    let unknown = crate::model_manager::DeclId::from_index(u32::MAX);
    assert!(class_plan(&mm, unknown).is_err());
    assert_eq!(stats(&mm), before);
}

#[test]
fn update_model_file_validates_against_the_new_regex() {
    let mm = manager(&p_model(None, false));
    check(&mm, x()).unwrap();
    assert!(stats(&mm).0 > 0);
    let mf = ModelFile::from_json(&p_model(Some("^y"), false), Some("p.cto".into())).unwrap();
    let updated = mm.update_model_file(mf, true).unwrap();
    let err = check(&updated, x()).unwrap_err();
    assert_eq!(err.kind(), ErrorKind::Validation);
    check(
        &updated,
        json!({ "$class": "org.acme@1.0.0.P", "name": "y" }),
    )
    .unwrap();
    // The manager it was updated from keeps its own answer.
    check(&mm, x()).unwrap();
}

#[test]
fn update_model_file_validates_against_a_new_property() {
    let mm = manager(&p_model(None, false));
    check(&mm, x()).unwrap();
    let mf = ModelFile::from_json(&p_model(None, true), Some("p.cto".into())).unwrap();
    let updated = mm.update_model_file(mf, true).unwrap();
    // `age` is now required...
    assert!(check(&updated, x()).is_err());
    // ...and declared.
    check(
        &updated,
        json!({ "$class": "org.acme@1.0.0.P", "name": "x", "age": 3 }),
    )
    .unwrap();
    assert!(
        check(
            &mm,
            json!({ "$class": "org.acme@1.0.0.P", "name": "x", "age": 3 })
        )
        .is_err()
    );
}

#[test]
fn delete_then_add_validates_against_the_new_model() {
    let mm = manager(&p_model(None, false));
    check(&mm, x()).unwrap();
    let mut deleted = mm.delete_model_file(NS).unwrap();
    assert_eq!(stats(&deleted), (0, 0));
    assert!(check(&deleted, x()).is_err());
    deleted
        .load_model(&p_model(Some("^y"), false), None)
        .unwrap();
    assert!(check(&deleted, x()).is_err());
}

#[test]
fn update_external_models_validates_against_the_downloaded_model() {
    let mut mm = manager(&p_model(None, false));
    check(&mm, x()).unwrap();
    mm.update_external_models([ModelFileSource {
        ast: p_model(Some("^y"), false),
        definitions: None,
        file_name: Some("@external/p.cto".into()),
    }])
    .unwrap();
    assert_eq!(stats(&mm), (0, 0));
    assert_eq!(check(&mm, x()).unwrap_err().kind(), ErrorKind::Validation);
}

/// `concept A { o org.other@1.0.0.B b }`, importing `B` from a namespace
/// that may not be loaded.
fn a_model() -> Value {
    json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": NS,
        "imports": [{
            "$class": "concerto.metamodel@1.0.0.ImportType",
            "namespace": "org.other@1.0.0", "name": "B"
        }],
        "declarations": [{
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "A", "isAbstract": false,
            "properties": [{
                "$class": "concerto.metamodel@1.0.0.ObjectProperty",
                "name": "b", "isArray": false, "isOptional": false,
                "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "B" }
            }]
        }]
    })
}

#[test]
fn adding_a_model_in_place_drops_a_plan_that_could_not_resolve_a_type() {
    // `A` loaded before `B`'s namespace: the plan records `b` as
    // unresolved, and validating a `b` raises the error resolving it.
    let b = json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.other@1.0.0",
        "declarations": [{
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "B", "isAbstract": false,
            "properties": [{
                "$class": "concerto.metamodel@1.0.0.StringProperty",
                "name": "s", "isArray": false, "isOptional": false
            }]
        }]
    });
    let instance = json!({
        "$class": "org.acme@1.0.0.A",
        "b": { "$class": "org.other@1.0.0.B", "s": "x" }
    });
    let mut mm = manager(&a_model());
    assert!(check(&mm, instance.clone()).is_err());
    assert!(stats(&mm).0 > 0);
    mm.load_model(&b, None).unwrap();
    assert_eq!(stats(&mm), (0, 0));
    check(&mm, instance).unwrap();
}

/// A plan whose property type does not resolve raises, from the cache,
/// the very error a fresh build records; and a declaration whose chain
/// does not resolve caches that error in its plan.
#[test]
fn a_cached_build_error_is_the_fresh_build_s_error() {
    let mm = manager(&a_model());
    let instance = json!({
        "$class": "org.acme@1.0.0.A",
        "b": { "$class": "org.other@1.0.0.B", "s": "x" }
    });
    let fresh = check(&mm, instance.clone()).unwrap_err();
    let cached = check(&mm, instance).unwrap_err();
    assert_eq!(fresh, cached);
    let a = mm.declaration_id("org.acme@1.0.0.A").unwrap();
    let plan = class_plan(&mm, a).unwrap();
    assert_eq!(plan.field(&mm, 0).unwrap_err(), fresh);

    // `concept C extends Missing {}`: no plan, and the chain's own error.
    let mut mm = ModelManager::new().unwrap();
    mm.load_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": NS,
                "declarations": [{
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "C", "isAbstract": false,
                    "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Missing" },
                    "properties": []
                }]
            }),
            None,
        )
        .ok();
    if let Some(c) = mm.declaration_id("org.acme@1.0.0.C") {
        let expected = mm.properties("org.acme@1.0.0.C").unwrap_err();
        assert_eq!(class_plan(&mm, c).unwrap_err(), expected);
        assert_eq!(class_plan(&mm, c).unwrap_err(), expected);
    }
}
