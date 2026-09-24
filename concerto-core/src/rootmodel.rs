//! Concerto's built-in system models.
//!
//! Every model manager starts with the `concerto@1.0.0` namespace already
//! loaded. It holds the five abstract base types every user type eventually
//! extends: `Concept`, `Asset`, `Participant`, `Transaction` and `Event`. The
//! `concerto.decorator@1.0.0` namespace, holding the base `Decorator` concept
//! and the built-in decorators such as `DotNetNamespace`, ships alongside it
//! but is not preloaded into a fresh [`crate::model_manager::ModelManager`].
//!
//! Both ASTs are vendored unchanged under `concerto-core`, matching the copies
//! `concerto-metamodel` vendors for code generation, so the runtime preloads
//! exactly what the reference implementation preloads. Nothing here is built
//! by hand: each JSON deserializes directly into the metamodel crate's own
//! [`mm::Model`], and [`root_model_ast`] - kept so the rest of the crate can
//! go on loading a model from a `serde_json::Value` - is that same typed
//! model serialized straight back out.

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
    serde_json::from_str(ROOT_MODEL_JSON).expect("Root model could not be parsed as `mm::Model`.")
}

/// The `concerto.decorator@1.0.0` model, deserialized directly into the
/// metamodel crate's own [`mm::Model`].
pub fn decorator_model() -> mm::Model {
    serde_json::from_str(DECORATOR_MODEL_JSON)
        .expect("Decorator model could not be parsed as `mm::Model`.")
}

/// The `concerto@1.0.0` system model, as a JSON AST: [`root_model`] typed and
/// serialized straight back out, so callers that load a model from a
/// `serde_json::Value` (every one of them, [`crate::model_manager::ModelManager::new`]
/// included) can go on doing so.
pub fn root_model_ast() -> serde_json::Value {
    serde_json::to_value(root_model()).expect("Root model could not be converted to JSON.")
}

#[cfg(test)]
mod tests {
    use super::*;

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
