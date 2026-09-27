//! A JSON AST can be deserialised and inspected through its `Debug` form
//! (accordproject/concerto-rust#24).

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

#[test]
fn a_json_ast_prints_its_debug_form() {
    let ast = serde_json::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.example@1.0.0",
        "declarations": [{
            "$class": "concerto.metamodel@1.0.0.AssetDeclaration",
            "name": "Vehicle",
            "isAbstract": false,
            "identified": {
                "$class": "concerto.metamodel@1.0.0.IdentifiedBy",
                "name": "vin"
            },
            "properties": [{
                "$class": "concerto.metamodel@1.0.0.StringProperty",
                "name": "vin",
                "isArray": false,
                "isOptional": false
            }]
        }]
    });

    let model: mm::Model = serde_json::from_value(ast).unwrap();
    let debug = format!("{model:#?}");
    println!("{debug}");

    assert!(debug.contains("AssetDeclaration("));
    assert!(debug.contains("IdentifiedBy("));
    assert!(debug.contains("StringProperty("));
    assert!(debug.contains("\"vin\""));
}
