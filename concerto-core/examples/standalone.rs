//! Standalone Rust acceptance example (P6-02, accordproject/concerto-rust#84).
//!
//! Uses `concerto-core` the way a native Rust caller would: no TypeScript,
//! no WASM, no `js-compat` feature. It walks the whole D11 in-scope surface
//! (accordproject/concerto-rust#29, docs/public-api.md section 1):
//!
//! 1. load a model set from JSON ASTs (no CTO parsing — that is out of
//!    scope, docs/public-api.md section 1);
//! 2. run semantic validation over the loaded model set;
//! 3. introspect the loaded declarations and properties;
//! 4. validate instances, first-error style;
//! 5. validate instances, collect-all style, and read the diagnostics
//!    (accordproject/concerto#1239);
//! 6. turn on the accordproject/concerto#1273 strict options.
//!
//! See `docs/native-guide.md` for the walkthrough this example implements.
//!
//! Run it directly with:
//!
//! ```sh
//! cargo run --example standalone -p accordproject-concerto-core
//! ```
//!
//! or, so it is checked in CI, under `cargo test` (the logic below runs
//! from both `main` and the `#[test]` at the bottom of this file):
//!
//! ```sh
//! cargo test --example standalone -p accordproject-concerto-core
//! ```

use concerto_core::instance::{DiagnosticCode, ValidationOptions};
use concerto_core::json;
use concerto_core::{ClassKind, ErrorKind, ModelManager};

/// The `Person` model, as a JSON AST (`concerto.metamodel@1.0.0.Model`).
///
/// This is what a CTO-to-AST parser would hand a native caller today (the
/// CTO front end itself, `concerto-tree-sitter`, is a named follow-up —
/// docs/public-api.md section 1); the AST is the stable input to
/// `add_model_ast`.
fn person_model_ast() -> concerto_core::json::Value {
    json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.acme.hr@1.0.0",
        "declarations": [
            {
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Person",
                "isAbstract": false,
                "identified": {
                    "$class": "concerto.metamodel@1.0.0.IdentifiedBy",
                    "name": "email"
                },
                "properties": [
                    {
                        "$class": "concerto.metamodel@1.0.0.StringProperty",
                        "name": "email",
                        "isArray": false,
                        "isOptional": false
                    },
                    {
                        "$class": "concerto.metamodel@1.0.0.StringProperty",
                        "name": "name",
                        "isArray": false,
                        "isOptional": false
                    },
                    {
                        "$class": "concerto.metamodel@1.0.0.IntegerProperty",
                        "name": "age",
                        "isArray": false,
                        "isOptional": true
                    }
                ]
            }
        ]
    })
}

/// The example's body, called from both `main` (`cargo run`) and the
/// `#[test]` below (`cargo test`), so both exercise the same walkthrough.
fn run() {
    // ---- 1. Load a model set from JSON ASTs --------------------------
    //
    // `ModelManager::new` loads the Concerto system models (the root and
    // decorator models) and is otherwise empty.
    let mut models = ModelManager::new().expect("the vendored system models always load");

    // `add_model_ast` loads one file's AST, checked for structure only, and
    // returns a handle to it.
    models
        .add_model_ast(&person_model_ast(), Some("hr.cto"))
        .expect("the Person AST is well-formed");

    // ---- 2. Semantic validation of the loaded model set ----------------
    //
    // Structural checks already ran on load; `validate_models` runs the
    // cross-model checks (unresolved imports, duplicate identifiers, and so
    // on) now that every file is loaded.
    models
        .validate_models()
        .expect("the loaded model set is semantically valid");

    // ---- 3. Introspection ------------------------------------------------
    let person = models
        .get_declaration("org.acme.hr@1.0.0.Person")
        .expect("Person was just loaded");
    assert_eq!(person.name(), "Person");
    assert_eq!(person.declaration_kind(), "ConceptDeclaration");

    let person_class = person
        .as_class()
        .expect("Person is a concept, so it is a ClassDeclaration");
    assert_eq!(person_class.kind(), ClassKind::Concept);
    println!(
        "org.acme.hr@1.0.0.Person is a {:?} with {} own properties",
        person_class.kind(),
        person_class.own_properties().len(),
    );

    for (owner, property) in models
        .properties("org.acme.hr@1.0.0.Person")
        .expect("Person exists")
    {
        println!(
            "  {owner}.{name}: {ty} (optional: {opt})",
            name = property.name(),
            ty = property.type_name().unwrap_or("?"),
            opt = property.is_optional(),
        );
    }

    // ---- 4. Instance validation, first error ------------------------------
    let ada = json!({
        "$class": "org.acme.hr@1.0.0.Person",
        "email": "ada@example.com",
        "name": "Ada Lovelace",
        "age": 36,
    });
    models
        .validate_instance(&ada, &ValidationOptions::default())
        .expect("ada is a valid Person");
    println!("ada@example.com validates cleanly");

    let malformed = json!({
        "$class": "org.acme.hr@1.0.0.Person",
        "email": "grace@example.com",
        // The wrong type for `name`: a required `String` property holding a
        // number.
        "name": 42,
        "age": 36,
    });
    let err = models
        .validate_instance(&malformed, &ValidationOptions::default())
        .expect_err("name is the wrong type");
    println!("first error: {err} (kind {:?})", err.kind());
    assert_eq!(err.kind(), ErrorKind::Validation);

    // ---- 5. Instance validation, every diagnostic (#1239) ------------------
    //
    // `check_instance` reports the same violation(s) `validate_instance`
    // stops at the first of, but as a `ValidationReport`: a `Vec<Diagnostic>`
    // to keep processing with, each pinpointed by a JSON Pointer, rather than
    // an `Err` to propagate with `?`.
    let report = models.check_instance(&malformed, &ValidationOptions::default());
    assert!(!report.is_valid());
    for diagnostic in report.diagnostics() {
        println!(
            "  {pointer} [{code}] {message}",
            pointer = diagnostic.pointer,
            code = diagnostic.code,
            message = diagnostic.message,
        );
    }
    assert!(
        report
            .diagnostics()
            .iter()
            .any(|d| d.code == DiagnosticCode::TypeViolation),
        "the bad `name` value is reported as a type violation",
    );

    // ---- 6. Strict mode (#1273): reject unknown keys -----------------------
    let with_typo = json!({
        "$class": "org.acme.hr@1.0.0.Person",
        "email": "grace@example.com",
        "name": "Grace Hopper",
        // A typo for `age`. `concerto.metamodel@1.0.0.Person` has no such
        // property, so under `ValidationOptions::STRICT` this is rejected
        // outright, rather than silently ignored.
        "adge": 85,
    });
    let strict_err = models
        .validate_instance(&with_typo, &ValidationOptions::STRICT)
        .expect_err("`adge` is not a declared property");
    println!("STRICT rejects the typo: {strict_err}");
}

fn main() {
    run();
}

#[test]
fn native_acceptance_walkthrough_runs() {
    run();
}
