//! Concerto's built-in system model.
//!
//! Every model manager starts with the `concerto@1.0.0` namespace already
//! loaded. It holds the five abstract base types every user type eventually
//! extends: `Concept`, `Asset`, `Participant`, `Transaction` and `Event`.
//! The AST is the `rootmodel.json` that ships with `concerto-core`, vendored
//! unchanged, so the runtime preloads exactly what the reference
//! implementation preloads.

/// The `concerto@1.0.0` root model JSON, as shipped with `concerto-core`.
const ROOT_MODEL_JSON: &str = include_str!("rootmodel.json");

/// The `concerto@1.0.0` system model, as a JSON AST.
pub fn root_model_ast() -> serde_json::Value {
    serde_json::from_str(ROOT_MODEL_JSON).expect("the vendored root model is valid JSON")
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
}
