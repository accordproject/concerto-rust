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
//! Both ASTs are vendored unchanged under `concerto-core`, matching the copies
//! `concerto-metamodel` vendors for code generation, so the runtime preloads
//! exactly what the reference implementation preloads. Nothing here is built
//! by hand: each JSON deserializes directly into the metamodel crate's own
//! [`mm::Model`], and [`root_model_ast`] and [`decorator_model_ast`] - kept so
//! the rest of the crate can go on loading a model from a `serde_json::Value`
//! - are those same typed models serialized straight back out.

// TODO: Load concerto metamodel together with decorator and vocab metamodels from
//       metamodel repo in build time.
use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

/// The `concerto@1.0.0` root model JSON, as shipped with `concerto-core`.
const ROOT_MODEL_JSON: &str = include_str!("rootmodel.json");

/// The `concerto.decorator@1.0.0` model JSON, as shipped with `concerto-core`.
const DECORATOR_MODEL_JSON: &str = include_str!("decoratormodel.json");

/// The `concerto@1.0.0` system model, deserialized directly into the
/// metamodel crate's own [`mm::Model`].
pub fn root_model() -> mm::Model {
    #[cfg(feature = "alt-value-only")]
    return crate::introspect::typed_ast::lenient_from_value(
        &serde_json::from_str(ROOT_MODEL_JSON).expect("JSON"),
    )
    .expect("Root model could not be parsed as `mm::Model`.");
    #[allow(unreachable_code)]
    crate::introspect::typed_ast::lenient_from_str(ROOT_MODEL_JSON)
        .expect("Root model could not be parsed as `mm::Model`.")
}

/// The `concerto.decorator@1.0.0` model, deserialized directly into the
/// metamodel crate's own [`mm::Model`].
pub fn decorator_model() -> mm::Model {
    #[cfg(feature = "alt-value-only")]
    return crate::introspect::typed_ast::lenient_from_value(
        &serde_json::from_str(DECORATOR_MODEL_JSON).expect("JSON"),
    )
    .expect("Decorator model could not be parsed as `mm::Model`.");
    #[allow(unreachable_code)]
    crate::introspect::typed_ast::lenient_from_str(DECORATOR_MODEL_JSON)
        .expect("Decorator model could not be parsed as `mm::Model`.")
}

/// The `concerto@1.0.0` system model, as a JSON AST: [`root_model`] typed and
/// serialized straight back out, so callers that load a model from a
/// `serde_json::Value` (every one of them, [`crate::model_manager::ModelManager::new`]
/// included) can go on doing so.
pub fn root_model_ast() -> serde_json::Value {
    serde_json::to_value(root_model()).expect("Root model could not be converted to JSON.")
}

/// The `concerto.decorator@1.0.0` model, as a JSON AST: [`decorator_model`]
/// typed and serialized straight back out, so callers that load a model from
/// a `serde_json::Value` (every one of them,
/// [`crate::model_manager::ModelManager::new`] included) can go on doing so.
pub fn decorator_model_ast() -> serde_json::Value {
    serde_json::to_value(decorator_model())
        .expect("Decorator model could not be converted to JSON.")
}

js_compat_pub! {
    /// P5-73 (accordproject/concerto-rust#414): the two system models' ASTs
    /// as compact JSON text, decorator model first, each with the file name
    /// TS `addDecoratorModel`/`addRootModel` give it. Each text is the
    /// vendored JSON written out again without whitespace, in its own key
    /// order, which is what JS `JSON.stringify` gives for concerto-core's own
    /// copies of the same files (`src/decoratormodelhelper.ts`,
    /// `src/rootmodelhelper.ts`). concerto-wasm recognises exactly these
    /// texts, so the verdict of their load is computed once
    /// (`systemModelFileHeader`). Computed on first use.
    pub fn system_model_json_texts() -> [(&'static str, &'static str); 2] {
        static TEXTS: std::sync::OnceLock<[String; 2]> = std::sync::OnceLock::new();
        let [decorator, root] = TEXTS
            .get_or_init(|| [compact(DECORATOR_MODEL_JSON), compact(ROOT_MODEL_JSON)]);
        [
            ("concerto_decorator_1.0.0.cto", decorator.as_str()),
            ("concerto_1.0.0.cto", root.as_str()),
        ]
    }
}

/// `json` (a vendored system model) without whitespace, in its own key order.
#[cfg_attr(not(feature = "js-compat"), allow(dead_code))]
fn compact(json: &str) -> String {
    let value: serde_json::Value =
        serde_json::from_str(json).expect("A system model could not be parsed as JSON.");
    serde_json::to_string(&value).expect("A system model could not be written as JSON.")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// P5-73: the compact texts are the vendored models, unchanged but for
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
                serde_json::from_str::<serde_json::Value>(text).unwrap(),
                serde_json::from_str::<serde_json::Value>(json).unwrap()
            );
        }
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(decorator).unwrap()["namespace"],
            "concerto.decorator@1.0.0"
        );
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(root).unwrap()["namespace"],
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

    /// [`decorator_model_ast`] is the same JSON AST view [`decorator_model`]
    /// (typed) gives, added for [`crate::model_manager::ModelManager::new`]
    /// (P1-07b), the same way [`root_model_ast`] already backs the root model.
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
