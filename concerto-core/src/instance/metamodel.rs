//! `BaseModelManager.validateAst` (`src/basemodelmanager.ts`; task P3-04,
//! `accordproject/concerto-rust#59`; `SEAM_LEDGER.tsv` row
//! `src/basemodelmanager.ts BaseModelManager validateAst`, planned task
//! `P3-04+P4-08`): checking a Concerto AST document against the metamodel
//! itself, rebuilt on [`super::validate`] (P3-01, the instance validator
//! that folded in `concerto-validate-rs`'s structural check, plan decision
//! D3) and [`super::deserialize::STRICT_VALIDATE_OPTIONS`] (P3-02,
//! accordproject/concerto#1273's strictness preset — the doc comment on
//! [`super::deserialize`] names this module as its intended P3-04 caller).
//!
//! **Scope (this task only).** The issue's plan gave `concerto-validate-rs`
//! (D3) as the exit condition — "validate-rs tests pass on the new core" —
//! but the maintainer's later comment on the issue supersedes that: leave
//! `concerto-validate-rs` untouched (it is reference-only, to be archived),
//! and instead port its test cases as tests of the function this module
//! adds (below). [`validate_metamodel`] is that function: the standalone
//! structural check `concerto-validate-rs::validate_metamodel` provided,
//! rebuilt on the instance validator with the strict preset instead of
//! `concerto-validate-rs`'s own bug-ridden hand-rolled one (plan §1.3).
//! [`validate_ast`] adds `validateAst`'s version check in front of it.
//! Wiring either into a caller's own [`ModelManager`] — TS's
//! `options.metamodelValidation`, and the temporary add/remove of
//! `this.metamodelModelFile` so `getType` resolves it — is `SEAM_LEDGER.tsv`'s
//! other half of this row. Task P4-08b (accordproject/concerto-rust#174)
//! added it as [`ModelManager::validate_ast`] (with
//! [`ModelManager::set_metamodel_validation`]), built on this module's
//! [`check_version`], [`metamodel_model_file`] and [`deserialize_ast`].

use serde_json::Value;

use super::deserialize::STRICT_VALIDATE_OPTIONS;
use super::factory::InstanceEnv;
use super::serializer::Serializer;
use super::value::JsValue;
use crate::error::{ConcertoError, ContractError, ErrorKind, Result};
use crate::introspect::model_file::ModelFile;
use crate::model_manager::ModelManager;
use crate::model_util::{self, ParsedNamespace};

/// `MetaModelNamespace` (`@accordproject/concerto-metamodel`), as
/// `basemodelmanager.ts` imports it.
pub const METAMODEL_NAMESPACE: &str = "concerto.metamodel@1.0.0";

/// The metamodel's own AST: the same vendored copy
/// `crate::dcs` includes (`concerto-core/src/dcs/metamodel.json`, identical
/// byte for byte to `concerto-metamodel/vendor/concerto.metamodel@1.0.0.json`
/// — `MetaModelUtil.metaModelAst`, the document `new ModelManager({
/// addMetamodel: true })` adds), so this module vendors no copy of its own.
const METAMODEL_AST_JSON: &str = include_str!("../dcs/metamodel.json");

/// A fixed identifier and clock. None of the metamodel's own declarations
/// (`Model`, `ConceptDeclaration`, `StringProperty`, and so on) are
/// system-identified or timestamped, so [`validate_metamodel`] never reads
/// either; this exists only because [`Serializer::from_json`] takes an
/// [`InstanceEnv`] (D7: identifiers and the clock come from the caller, not
/// the model).
struct FixedEnv;

impl InstanceEnv for FixedEnv {
    fn new_id(&mut self) -> String {
        "00000000-0000-4000-8000-000000000000".into()
    }
    fn now_ms(&mut self) -> f64 {
        0.0
    }
}

/// A fresh [`ModelManager`] with the metamodel model itself loaded. Built
/// new on every call, the same way TS's `validateAst` adds
/// `this.metamodelModelFile` only for the duration of the check (and only
/// when a metamodel is not already present) rather than caching it — see
/// the module doc on why the caller's own model manager is out of scope
/// here.
fn metamodel_model_manager() -> Result<ModelManager> {
    let mut mm = ModelManager::new()?;
    let metamodel: Value =
        serde_json::from_str(METAMODEL_AST_JSON).expect("the vendored metamodel AST is JSON");
    mm.add_models([(&metamodel, Some(format!("{METAMODEL_NAMESPACE}.cto")))])?;
    Ok(mm)
}

/// The text a TS `catch (err)` would see on `err.message`: the exception's
/// own, already-constructed message. For a [`ConcertoError::Contract`] that
/// is [`ContractError::final_message`] (the same text the native oracle
/// harness compares, per its own doc comment); the other two variants
/// predate the contract shape and are given the same fallback text
/// `concerto-wasm`'s `From<ConcertoError> for Error` uses for them.
fn ts_message(err: &ConcertoError) -> String {
    match err {
        ConcertoError::Contract(err) => err.final_message(),
        ConcertoError::IllegalModel { message, .. } => message.clone(),
        ConcertoError::TypeNotFound { type_name } => format!("type not found: {type_name}"),
    }
}

/// `BaseModelManager.validateAst`'s structural check:
/// `this.getSerializer().fromJSON(modelFile.getAst())`
/// (`src/basemodelmanager.ts`), run with accordproject/concerto#1273's
/// `STRICT_VALIDATE_OPTIONS` preset (task P3-02) so an unknown property or a
/// required property explicitly set to `null` is rejected too — the
/// strictness `concerto-validate-rs` never had (plan §1.3's confirmed bugs:
/// no `Long`/`DateTime`/relationship/enum support, only the direct super
/// type's properties merged, abstract and nested `$class` values unchecked).
/// Any failure — no `$class`, an unresolvable type, a structural mismatch —
/// is re-thrown as `MetamodelException(error.message)`, exactly as TS's
/// `catch` block does.
pub fn validate_metamodel(ast: &Value) -> Result<()> {
    let mm = metamodel_model_manager()?;
    let serializer = Serializer::new(true, true, None)?;
    let options = STRICT_VALIDATE_OPTIONS.serializer_options();
    let mut env = FixedEnv;
    serializer
        .from_json(&mm, &JsValue::from_json(ast), Some(&options), &mut env)
        .map(|_resource| ())
        .map_err(|err| wrapped(&err))
}

/// `throw new MetamodelException(error.message)`: `validateAst`'s `catch`
/// block.
fn wrapped(err: &ConcertoError) -> ConcertoError {
    ContractError::new(
        ErrorKind::Metamodel,
        "basemodelmanager-validateast-wrapped",
        vec![("message", ts_message(err))],
    )
    .into()
}

/// `BaseModelManager`'s cached `this.metamodelModelFile`: `new
/// ModelFile(this, MetaModelUtil.metaModelAst, undefined,
/// MetaModelNamespace)` (`src/basemodelmanager.ts`'s constructor), so its
/// file name is the namespace itself and it has no CTO definitions.
pub(crate) fn metamodel_model_file() -> Result<ModelFile> {
    let metamodel: Value =
        serde_json::from_str(METAMODEL_AST_JSON).expect("the vendored metamodel AST is JSON");
    ModelFile::from_json(&metamodel, Some(METAMODEL_NAMESPACE.to_string()))
}

/// `validateAst`'s structural check against the caller's own model manager
/// `mm` (which must already hold the metamodel):
/// `this.getSerializer().fromJSON(modelFile.getAst())`, with the manager's
/// serializer default options (`baseDefaultOptions`, `{validate: true,
/// utcOffset}`), not [`validate_metamodel`]'s strict preset — concerto-core
/// 5.0.0 passes no options here. Any failure is re-thrown as
/// `MetamodelException(error.message)`.
///
/// TS's `Serializer` merges the manager's own constructor options into its
/// defaults too. None of the keys `Serializer.fromJSON` reads
/// (`acceptResourcesForRelationships`, `utcOffset`,
/// `strictQualifiedDateTimes`) can change a metamodel document's outcome —
/// the metamodel declares no relationship and no `DateTime` property — so
/// only a manager option `validate: false` would, and no
/// `ModelManager` in this port carries a serializer option bag.
pub(crate) fn deserialize_ast(mm: &ModelManager, ast: &Value) -> Result<()> {
    let serializer = Serializer::new(true, true, None)?;
    let mut env = FixedEnv;
    serializer
        .from_json(mm, &JsValue::from_json(ast), None, &mut env)
        .map(|_resource| ())
        .map_err(|err| wrapped(&err))
}

/// `BaseModelManager.validateAst(modelFile)` (`src/basemodelmanager.ts`):
/// the version check, then the structural check ([`validate_metamodel`]).
///
/// Unlike the TS reference, this takes the AST directly rather than a
/// `ModelFile` handle — task P3-04's scope is the standalone check
/// `concerto-validate-rs` provided (module doc), not the `ModelFile`/
/// `ModelManager` view integration (task P4-08). When `ast`'s `$class` is
/// missing or not a string the version check is skipped — the TS reference
/// never reaches it either, since by the time `validateAst` runs,
/// `modelFile.getAst().$class` is always the metamodel's own `Model` class
/// — and [`validate_metamodel`] reports the malformed document on its own.
pub fn validate_ast(ast: &Value) -> Result<()> {
    check_version(ast)?;
    validate_metamodel(ast)
}

/// `validateAst`'s version check: the version of the namespace of `ast`'s
/// `$class` must be the metamodel's own
/// (`basemodelmanager-validateast-versionmismatch`). Skipped when `$class`
/// is missing or not a string ([`validate_ast`]'s doc).
pub(crate) fn check_version(ast: &Value) -> Result<()> {
    if let Some(class_name) = ast.get("$class").and_then(Value::as_str) {
        let ns = model_util::get_namespace(Some(class_name))?;
        let model_file_version = namespace_version(ns)?;
        let metamodel_version = namespace_version(METAMODEL_NAMESPACE)?;
        if model_file_version != metamodel_version {
            return Err(ContractError::new(
                ErrorKind::Metamodel,
                "basemodelmanager-validateast-versionmismatch",
                vec![
                    ("modelFileVersion", model_file_version.unwrap_or_default()),
                    ("metamodelVersion", metamodel_version.unwrap_or_default()),
                ],
            )
            .into());
        }
    }
    Ok(())
}

/// `ModelUtil.parseNamespace(ns).version`.
fn namespace_version(ns: &str) -> Result<Option<String>> {
    let ParsedNamespace::Full { version, .. } = model_util::parse_namespace(Some(ns), false)?
    else {
        unreachable!("parse_namespace(_, false) always returns ParsedNamespace::Full")
    };
    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ---- ported from concerto-validate-rs's src/lib.rs tests (issue
    //      accordproject/concerto-rust#59's exit condition, as narrowed by
    //      the maintainer's issue comment: validate-rs's own tests, ported
    //      as tests of this module, citing the source test) ----

    /// concerto-validate-rs `tests::test_valid_metamodel_validation`: the
    /// vendored metamodel document, validated against itself. Unlike the
    /// source test, this reads the same vendored copy this module already
    /// includes rather than a repo-root `metamodel.json` (that file exists
    /// only in `concerto-validate-rs`, out of scope here — see the module
    /// doc), but it is byte-for-byte the same document (both copies are
    /// `MetaModelUtil.metaModelAst`).
    #[test]
    fn valid_metamodel_validation() {
        let metamodel: Value =
            serde_json::from_str(METAMODEL_AST_JSON).expect("the vendored AST is JSON");
        let result = validate_metamodel(&metamodel);
        assert!(
            result.is_ok(),
            "metamodel validation should succeed: {result:?}"
        );
    }

    /// concerto-validate-rs `tests::test_invalid_json`: malformed JSON text
    /// fails validation. This module's API boundary differs deliberately
    /// from `concerto-validate-rs`'s: `validate_metamodel` takes an already
    /// parsed [`Value`], not a `&str` — JSON parsing is a step upstream of
    /// this module in concerto-rust (`serde_json::from_str`, as every other
    /// entry point in this crate does), not something this port repeats. A
    /// document with no usable `$class` at all is the nearest equivalent
    /// this module's own boundary can express; it still fails, through
    /// `Serializer::from_json`'s own "no `$class`" check.
    #[test]
    fn invalid_json_has_no_usable_class() {
        let result = validate_metamodel(&json!({}));
        assert!(
            result.is_err(),
            "a $class-less document should fail: {result:?}"
        );
    }

    /// concerto-validate-rs `tests::test_invalid_namespace`: a `namespace`
    /// of the wrong JSON type fails validation. The source fixture omits
    /// `imports`/`declarations`, which `STRICT_VALIDATE_OPTIONS` would also
    /// reject as missing required properties; either way the document must
    /// fail, so this keeps both defects to stay close to the source.
    #[test]
    fn invalid_namespace_type() {
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": 123
        });
        let result = validate_metamodel(&ast);
        assert!(
            result.is_err(),
            "a non-string namespace should fail: {result:?}"
        );
    }

    /// concerto-validate-rs `tests::test_missing_class_property`: a document
    /// with no `$class` at all fails validation.
    #[test]
    fn missing_class_property() {
        let ast = json!({
            "namespace": "test.namespace",
            "imports": [],
            "declarations": []
        });
        let result = validate_metamodel(&ast);
        assert!(
            result.is_err(),
            "a $class-less document should fail: {result:?}"
        );
    }

    /// concerto-validate-rs `tests::test_simple_model_validation`: a small,
    /// well-formed model passes validation.
    #[test]
    fn simple_model_validation() {
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "test.namespace@1.0.0",
            "imports": [],
            "declarations": [
                {
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "TestConcept",
                    "isAbstract": false,
                    "properties": [
                        {
                            "$class": "concerto.metamodel@1.0.0.StringProperty",
                            "name": "testField",
                            "isArray": false,
                            "isOptional": false
                        }
                    ]
                }
            ]
        });
        let result = validate_metamodel(&ast);
        assert!(
            result.is_ok(),
            "a simple valid model should pass validation: {result:?}"
        );
    }

    /// concerto-validate-rs `tests::test_extra_properties`: an undeclared
    /// property anywhere in the document fails validation.
    /// `concerto-validate-rs`'s own hand-rolled structural check caught
    /// this only by accident of its shape checks; here it is
    /// `populator::validate_properties` (called unconditionally by
    /// `visitClassDeclaration`, in both TS and this port) that rejects it
    /// — *not* `STRICT_VALIDATE_OPTIONS`. This
    /// document's two extra keys (`isOptional` on the declaration,
    /// `propertyType` on the property) are both non-null, and a non-null
    /// unknown key is rejected by `Serializer.fromJSON`'s default
    /// (non-strict) behaviour too, in TS and in this port alike — see
    /// `super::deserialize`'s own divergence table ("Unknown field =
    /// non-null" is an error under the default). The strict preset's own,
    /// real effect on unknown keys is exercised below by
    /// `null_extra_property_rejected_only_under_the_strict_preset`.
    #[test]
    fn extra_properties_rejected_under_the_strict_preset() {
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "test.namespace@1.0.0",
            "imports": [],
            "declarations": [
                {
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "TestConcept",
                    "isAbstract": false,
                    "isOptional": false,
                    "properties": [
                        {
                            "$class": "concerto.metamodel@1.0.0.StringProperty",
                            "name": "testField",
                            "isArray": false,
                            "isOptional": false,
                            "propertyType": "String"
                        }
                    ]
                }
            ]
        });
        let result = validate_metamodel(&ast);
        assert!(
            result.is_err(),
            "extra properties should fail validation under the strict preset: {result:?}"
        );
    }

    /// The strict preset's actual, provable effect on unknown keys, and the
    /// test that would fail if `validate_metamodel` stopped applying
    /// `STRICT_VALIDATE_OPTIONS` (e.g. `Some(&options)` dropped to `None`).
    /// Per `super::deserialize`'s own divergence table, an unknown property
    /// set to `null` is *ignored* by default (it never reaches
    /// `populator::validate_properties`, since `get_assignable_properties`
    /// drops nullish values before that check runs) and rejected only when
    /// `STRICT_VALIDATE_OPTIONS.reject_unknown_keys` is set — unlike a
    /// non-null unknown property, which `extra_properties_rejected_under_
    /// the_strict_preset` above shows fails either way, strict or not.
    ///
    /// The first assertion calls `validate_metamodel` itself (which always
    /// applies the strict preset) and expects it to reject the document.
    /// The second calls the underlying serializer directly with the
    /// default (non-strict) options, on the identical document, and expects
    /// it to accept it — establishing that the first assertion's failure is
    /// really due to the strict preset, not some other check.
    #[test]
    fn null_extra_property_rejected_only_under_the_strict_preset() {
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "test.namespace@1.0.0",
            "imports": [],
            "declarations": [
                {
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "TestConcept",
                    "isAbstract": false,
                    "unknownKey": null,
                    "properties": []
                }
            ]
        });

        let strict_result = validate_metamodel(&ast);
        assert!(
            strict_result.is_err(),
            "a null unknown property should be rejected by validate_metamodel, \
             which applies the strict preset: {strict_result:?}"
        );

        let mm = metamodel_model_manager().expect("metamodel model manager");
        let serializer = Serializer::new(true, true, None).expect("serializer");
        let mut env = FixedEnv;
        let default_result = serializer.from_json(&mm, &JsValue::from_json(&ast), None, &mut env);
        assert!(
            default_result.is_ok(),
            "the same null unknown property should be ignored under the \
             (non-strict) default options: {default_result:?}"
        );
    }

    // ---- validateAst's own behaviour beyond validate_metamodel (not in
    //      concerto-validate-rs, which has no version check at all) ----

    #[test]
    fn validate_ast_accepts_a_well_formed_model() {
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": []
        });
        assert!(validate_ast(&ast).is_ok());
    }

    #[test]
    fn validate_ast_rejects_an_unknown_metamodel_version() {
        let ast = json!({
            "$class": "concerto.metamodel@99.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "declarations": []
        });
        let err = validate_ast(&ast).expect_err("an unknown metamodel version should fail");
        let ConcertoError::Contract(contract) = err else {
            panic!("expected a Contract error, got {err:?}");
        };
        assert_eq!(contract.kind, ErrorKind::Metamodel);
        assert_eq!(
            contract.message(),
            "Model file version 99.0.0 does not match metamodel version 1.0.0"
        );
    }

    #[test]
    fn validate_ast_rejects_a_bad_metamodel_ast_with_an_undeclared_property() {
        // TS: modelmanager.js "#addModel > should throw for a bad metamodel
        // AST".
        let ast = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "undeclared": []
        });
        assert!(validate_ast(&ast).is_err());
    }

    // ---- ModelManager::validate_ast: `validateAst` on a caller's own
    //      model manager, the `metamodelValidation` option (task P4-08b) ----

    fn namespaces(mm: &ModelManager) -> Vec<String> {
        mm.model_files()
            .map(|mf| mf.namespace().to_string())
            .collect()
    }

    fn model_file(ast: &Value) -> ModelFile {
        ModelFile::from_json(ast, Some("test.cto".into())).expect("a well-formed model file")
    }

    #[test]
    fn metamodel_validation_is_off_by_default_and_carried_by_scratch_copies() {
        let mut mm = ModelManager::new().unwrap();
        assert!(!mm.metamodel_validation());
        mm.set_metamodel_validation(true);
        assert!(mm.metamodel_validation());
        mm.add_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "imports": [],
                "declarations": []
            }),
            None,
        )
        .unwrap();
        assert!(
            mm.delete_model_file("org.acme@1.0.0")
                .unwrap()
                .metamodel_validation()
        );
    }

    #[test]
    fn manager_validate_ast_accepts_a_well_formed_model_and_removes_the_metamodel() {
        let mut mm = ModelManager::new().unwrap();
        let before = namespaces(&mm);
        let mf = model_file(&json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Person",
                "isAbstract": false,
                "properties": [{
                    "$class": "concerto.metamodel@1.0.0.StringProperty",
                    "name": "name",
                    "isArray": false,
                    "isOptional": false
                }]
            }]
        }));
        mm.validate_ast(&mf).unwrap();
        assert_eq!(namespaces(&mm), before);
        assert!(
            mm.declaration_id("concerto.metamodel@1.0.0.Model")
                .is_none()
        );
    }

    #[test]
    fn manager_validate_ast_failure_leaves_the_metamodel_registered() {
        // TS: `validateAst`'s `deleteModelFile(MetaModelNamespace)` follows
        // the `try`/`catch` that re-throws, so a failed check never reaches it.
        let mut mm = ModelManager::new().unwrap();
        let mf = model_file(&json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": [],
            "undeclared": []
        }));
        let err = mm
            .validate_ast(&mf)
            .expect_err("an undeclared property is invalid");
        let ConcertoError::Contract(contract) = err else {
            panic!("expected a Contract error, got {err:?}");
        };
        assert_eq!(contract.kind, ErrorKind::Metamodel);
        let mut expected: Vec<String> =
            ModelManager::new().map(|fresh| namespaces(&fresh)).unwrap();
        expected.push(METAMODEL_NAMESPACE.to_string());
        assert_eq!(namespaces(&mm), expected);
        assert_eq!(
            mm.model_file(METAMODEL_NAMESPACE).unwrap().file_name(),
            Some(METAMODEL_NAMESPACE)
        );
        // A later check finds it already there and keeps it.
        let valid = model_file(&json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.other@1.0.0",
            "imports": [],
            "declarations": []
        }));
        mm.validate_ast(&valid).unwrap();
        assert_eq!(namespaces(&mm), expected);
    }

    #[test]
    fn manager_validate_ast_version_mismatch_adds_nothing() {
        let mut mm = ModelManager::new().unwrap();
        let before = namespaces(&mm);
        let mf = model_file(&json!({
            "$class": "concerto.metamodel@99.0.0.Model",
            "namespace": "org.acme@1.0.0",
            "imports": [],
            "declarations": []
        }));
        let err = mm
            .validate_ast(&mf)
            .expect_err("an unknown metamodel version");
        let ConcertoError::Contract(contract) = err else {
            panic!("expected a Contract error, got {err:?}");
        };
        assert_eq!(
            contract.message(),
            "Model file version 99.0.0 does not match metamodel version 1.0.0"
        );
        assert_eq!(namespaces(&mm), before);
    }
}
