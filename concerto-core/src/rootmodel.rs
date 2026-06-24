//! Concerto's built-in system model.
//!
//! Every model manager starts with the `concerto@1.0.0` namespace already
//! loaded. It holds the five abstract base types every user type eventually
//! extends: `Concept`, `Asset`, `Participant`, `Transaction` and `Event`. We
//! build the AST in code rather than ship a JSON file, so the two can't drift
//! out of sync.

const MM: &str = "concerto.metamodel@1.0.0";

fn base_concept(name: &str, identified: bool) -> serde_json::Value {
    let mut decl = serde_json::json!({
        "$class": format!("{MM}.ConceptDeclaration"),
        "name": name,
        "isAbstract": true,
        "properties": []
    });
    if identified {
        decl["identified"] = serde_json::json!({ "$class": format!("{MM}.Identified") });
    }
    decl
}

/// The `concerto@1.0.0` system model, as a JSON AST.
pub fn root_model_ast() -> serde_json::Value {
    serde_json::json!({
        "$class": format!("{MM}.Model"),
        "namespace": "concerto@1.0.0",
        "imports": [],
        "declarations": [
            base_concept("Concept", false),
            base_concept("Asset", true),
            base_concept("Participant", true),
            base_concept("Transaction", false),
            base_concept("Event", false),
        ]
    })
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
