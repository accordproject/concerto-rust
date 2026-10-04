use super::*;

/// The compact texts are the vendored models, unchanged but for
/// whitespace, and keep the files' key order (`$class` first).
#[test]
fn system_model_json_texts_are_the_vendored_models_without_whitespace() {
    let [(decorator_file, decorator), (root_file, root)] = system_model_json_texts();
    assert_eq!(decorator_file, "concerto_decorator_1.0.0.cto");
    assert_eq!(root_file, "concerto_1.0.0.cto");
    for (text, json) in [(decorator, DECORATOR_MODEL_JSON), (root, ROOT_MODEL_JSON)] {
        assert!(text.starts_with(r#"{"$class":"concerto.metamodel@1.0.0.Model","#));
        assert!(!text.contains('\n') && !text.contains(": "));
        assert_eq!(
            serde_json::from_str::<crate::json::Value>(text).unwrap(),
            serde_json::from_str::<crate::json::Value>(json).unwrap()
        );
    }
    assert_eq!(
        serde_json::from_str::<crate::json::Value>(decorator).unwrap()["namespace"],
        "concerto.decorator@1.0.0"
    );
    assert_eq!(
        serde_json::from_str::<crate::json::Value>(root).unwrap()["namespace"],
        "concerto@1.0.0"
    );
}

#[test]
fn root_model_defines_five_base_types() {
    let ast = root_model_ast();
    assert_eq!(ast["namespace"], "concerto@1.0.0");
    let decls = ast["declarations"].as_array().unwrap();
    let names: Vec<&str> = decls.iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        ["Concept", "Asset", "Participant", "Transaction", "Event"]
    );
}

#[test]
fn asset_and_participant_are_identified() {
    let ast = root_model_ast();
    let decls = ast["declarations"].as_array().unwrap();
    assert!(decls[1].get("identified").is_some()); // Asset
    assert!(decls[2].get("identified").is_some()); // Participant
    assert!(decls[0].get("identified").is_none()); // Concept
}

/// [`decorator_model_ast`] is the JSON AST view of [`decorator_model`].
#[test]
fn decorator_model_ast_matches_the_typed_model() {
    let ast = decorator_model_ast();
    assert_eq!(ast["namespace"], "concerto.decorator@1.0.0");
    let decls = ast["declarations"].as_array().unwrap();
    let names: Vec<&str> = decls.iter().map(|d| d["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["Decorator", "DotNetNamespace"]);
}

#[test]
fn decorator_model_declares_dot_net_namespace() {
    let model = decorator_model();
    assert_eq!(model.namespace, "concerto.decorator@1.0.0");
    let names: Vec<&str> = model
        .declarations
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|d| match d {
            mm::Declaration::ConceptDeclaration(c) => c.name.as_str(),
            other => panic!("unexpected declaration in the decorator model: {other:?}"),
        })
        .collect();
    assert_eq!(names, ["Decorator", "DotNetNamespace"]);
}
