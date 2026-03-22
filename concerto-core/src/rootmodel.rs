//! Programmatic construction of the Concerto root model.
//!
//! The root model defines the five abstract base types in the `concerto@1.0.0`
//! namespace: `Concept`, `Asset`, `Participant`, `Transaction`, and `Event`.
//!
//! Rather than embedding a static `rootmodel.json` file that could drift from
//! the metamodel, this module builds the equivalent AST JSON from code.

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

const MM: &str = "concerto.metamodel@1.0.0";

fn class(suffix: &str) -> String {
    format!("{MM}.{suffix}")
}

fn concept_declaration(name: &str, identified: Option<mm::Identified>) -> mm::ConceptDeclaration {
    mm::ConceptDeclaration {
        _class: class("ConceptDeclaration"),
        name: name.to_string(),
        is_abstract: true,
        identified,
        super_type: None,
        properties: vec![],
        decorators: None,
        location: None,
    }
}

/// Returns the root model AST as a `serde_json::Value`.
///
/// This is equivalent to the `rootmodel.json` file in the JavaScript
/// implementation and defines the five abstract base types.
pub fn root_model_ast() -> serde_json::Value {
    let identified = mm::Identified {
        _class: class("Identified"),
    };

    let import = mm::ImportType {
        _class: class("ImportType"),
        name: "DotNetNamespace".to_string(),
        namespace: "concerto.decorator@1.0.0".to_string(),
        uri: None,
    };

    let declarations = vec![
        concept_declaration("Concept", None),
        concept_declaration("Asset", Some(identified.clone())),
        concept_declaration("Participant", Some(identified)),
        concept_declaration("Transaction", None),
        concept_declaration("Event", None),
    ];

    // Serialize each declaration individually so they keep their full
    // ConceptDeclaration fields ($class, isAbstract, properties, etc.)
    // rather than being truncated to the base Declaration struct.
    let decl_values: Vec<serde_json::Value> = declarations
        .iter()
        .map(|d| serde_json::to_value(d).expect("root model declaration serialization"))
        .collect();

    let import_value = serde_json::to_value(&import).expect("root model import serialization");

    // The mm::DecoratorLiteral struct doesn't carry the `value` field
    // (it's on the subtype DecoratorString), so we build manually.
    let decorator_value = serde_json::json!({
        "$class": format!("{MM}.Decorator"),
        "name": "DotNetNamespace",
        "arguments": [
            {
                "$class": format!("{MM}.DecoratorString"),
                "value": "AccordProject.Concerto"
            }
        ]
    });

    serde_json::json!({
        "$class": format!("{MM}.Model"),
        "namespace": "concerto@1.0.0",
        "imports": [import_value],
        "declarations": decl_values,
        "decorators": [decorator_value]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_model_has_five_declarations() {
        let ast = root_model_ast();
        let decls = ast["declarations"].as_array().unwrap();
        assert_eq!(decls.len(), 5);
        assert_eq!(decls[0]["name"], "Concept");
        assert_eq!(decls[1]["name"], "Asset");
        assert_eq!(decls[2]["name"], "Participant");
        assert_eq!(decls[3]["name"], "Transaction");
        assert_eq!(decls[4]["name"], "Event");
    }

    #[test]
    fn root_model_namespace() {
        let ast = root_model_ast();
        assert_eq!(ast["namespace"], "concerto@1.0.0");
    }

    #[test]
    fn asset_and_participant_are_identified() {
        let ast = root_model_ast();
        let decls = ast["declarations"].as_array().unwrap();
        // Asset (index 1) and Participant (index 2) should have identified
        assert!(decls[1].get("identified").is_some());
        assert!(decls[2].get("identified").is_some());
        // Concept, Transaction, Event should not
        assert!(decls[0].get("identified").is_none());
        assert!(decls[3].get("identified").is_none());
        assert!(decls[4].get("identified").is_none());
    }
}
