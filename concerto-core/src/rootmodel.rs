//! Concerto's built-in system models.
//!
//! Every model manager starts with both system namespaces already loaded, in
//! the order TS's `BaseModelManager` constructor loads them:
//! `concerto.decorator@1.0.0` first, then `concerto@1.0.0`
//! ([`crate::model_manager::ModelManager::new`]). `concerto@1.0.0` holds the
//! five abstract base types every user type eventually extends: `Concept`,
//! `Asset`, `Participant`, `Transaction` and `Event`. `concerto.decorator@1.0.0`
//! holds the base `Decorator` concept and the built-in decorators such as
//! `DotNetNamespace`.
//!
//! Both ASTs are vendored unchanged, matching the copies `concerto-metamodel`
//! vendors, so the runtime preloads exactly what the reference preloads.
//! [`root_model`] and [`decorator_model`] deserialize each into
//! [`mm::Model`]; [`root_model_ast`] and [`decorator_model_ast`], which the
//! model manager loads, parse the same JSON as a `crate::json::Value`, in the
//! file's own key order.

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

/// The `concerto@1.0.0` root model JSON, as shipped with `concerto-core`.
const ROOT_MODEL_JSON: &str = include_str!("rootmodel.json");

/// The `concerto.decorator@1.0.0` model JSON, as shipped with `concerto-core`.
const DECORATOR_MODEL_JSON: &str = include_str!("decoratormodel.json");

/// The `concerto@1.0.0` system model, deserialized directly into the
/// metamodel crate's own [`mm::Model`].
pub fn root_model() -> mm::Model {
    serde_json::from_str(ROOT_MODEL_JSON).expect("Root model could not be parsed as `mm::Model`.")
}

/// The `concerto.decorator@1.0.0` model, deserialized directly into the
/// metamodel crate's own [`mm::Model`].
pub fn decorator_model() -> mm::Model {
    serde_json::from_str(DECORATOR_MODEL_JSON)
        .expect("Decorator model could not be parsed as `mm::Model`.")
}

/// The `concerto@1.0.0` system model, as a JSON AST: the vendored JSON, in
/// its own key order, as [`crate::model_manager::ModelManager::new`] loads
/// it.
pub fn root_model_ast() -> crate::json::Value {
    serde_json::from_str(ROOT_MODEL_JSON).expect("Root model could not be parsed as JSON.")
}

/// The `concerto.decorator@1.0.0` model, as a JSON AST: the vendored JSON,
/// in its own key order, as [`crate::model_manager::ModelManager::new`]
/// loads it.
pub fn decorator_model_ast() -> crate::json::Value {
    serde_json::from_str(DECORATOR_MODEL_JSON)
        .expect("Decorator model could not be parsed as JSON.")
}

/// The two system models' ASTs as compact JSON text, decorator model first,
/// each with the file name TS `addDecoratorModel`/`addRootModel` give it:
/// what JS `JSON.stringify` gives for concerto-core's own copies, which
/// concerto-wasm recognises so their load's verdict is computed once
/// (`systemModelFileHeader`).
#[cfg(feature = "js-compat")]
pub fn system_model_json_texts() -> [(&'static str, &'static str); 2] {
    static TEXTS: std::sync::OnceLock<[String; 2]> = std::sync::OnceLock::new();
    let [decorator, root] =
        TEXTS.get_or_init(|| [compact(DECORATOR_MODEL_JSON), compact(ROOT_MODEL_JSON)]);
    [
        ("concerto_decorator_1.0.0.cto", decorator.as_str()),
        ("concerto_1.0.0.cto", root.as_str()),
    ]
}

/// `json` (a vendored system model) without whitespace, in its own key order.
#[cfg_attr(not(feature = "js-compat"), allow(dead_code))]
fn compact(json: &str) -> String {
    let value: crate::json::Value =
        serde_json::from_str(json).expect("A system model could not be parsed as JSON.");
    serde_json::to_string(&value).expect("A system model could not be written as JSON.")
}

#[cfg(test)]
mod tests {
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
}
