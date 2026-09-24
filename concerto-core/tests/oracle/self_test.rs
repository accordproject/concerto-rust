//! Validates the harness itself against hand-authored fixtures in the
//! oracle's exact JSON schema (README "Fixture schema").
//!
//! This checkout has no generated corpus to run against (`oracle.rs`'s
//! module doc: the corpus is generated separately, into a sibling
//! `concerto` checkout, and is not part of this repository). These tests
//! are the harness's own regression tests instead: one real fixture per
//! decode/compare path the current op set (`ops.rs`) covers, proving that
//! loading, dispatch and judging genuinely work end to end, independently
//! of whether a corpus happens to be present when `cargo test` runs.

use std::fs;
use std::path::PathBuf;

use serde_json::json;

use super::compare::{self, Verdict};
use super::cto_cache::CtoCache;
use super::fixture;
use super::ledger::Ledger;
use super::ops;
use super::{Harness, report};

/// A harness with no CTO cache and no ledger.
fn bare() -> Harness {
    Harness {
        cache: None,
        ledger: Ledger::default(),
    }
}

/// Writes one fixture JSON file, merging `body` (`inputs`, `outcome`, and
/// any override) over a set of sensible defaults.
fn write_fixture(dir: &std::path::Path, op: &str, id: &str, body: serde_json::Value) {
    let mut full = json!({
        "id": id,
        "source": "self-test",
        "op": op,
        "env": { "random": false },
        "occurrences": 1,
    });
    for (k, v) in body.as_object().expect("fixture body must be an object") {
        full[k.as_str()] = v.clone();
    }
    fs::write(
        dir.join(format!("{id}.json")),
        serde_json::to_string_pretty(&full).unwrap(),
    )
    .unwrap();
}

fn scratch_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "concerto-oracle-self-test-{name}-{}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn passes_a_correct_fixture_for_each_supported_op() {
    let dir = scratch_dir("pass");
    write_fixture(
        &dir,
        "ModelUtil.getShortName",
        "short-name",
        json!({ "inputs": { "args": ["org.acme.Foo"] }, "outcome": { "ok": "Foo" } }),
    );
    write_fixture(
        &dir,
        "ModelUtil.getNamespace",
        "namespace",
        json!({ "inputs": { "args": ["org.acme.Foo"] }, "outcome": { "ok": "org.acme" } }),
    );
    write_fixture(
        &dir,
        "ModelUtil.getNamespace",
        "namespace-nullish-error",
        json!({
            "inputs": { "args": [{ "@@oracle": "undefined" }] },
            "outcome": { "error": {
                "class": "Error", "message": "FQN is invalid.", "location": null, "component": null
            } }
        }),
    );
    write_fixture(
        &dir,
        "ModelUtil.parseNamespace",
        "parse-namespace-name-only",
        json!({
            "inputs": { "args": ["org.acme@1.0.0", { "disableVersionParsing": true }] },
            "outcome": { "ok": { "name": "org.acme" } }
        }),
    );
    // A real recorded fixture (task P1-07, `packages/concerto-core/test/modelutil.js`
    // "parseNamespace valid, with version"): `versionParsed` is a node-semver
    // `SemVer` instance, which codec.js encodes as the generic outcome-only
    // object summary, not its fields.
    write_fixture(
        &dir,
        "ModelUtil.parseNamespace",
        "parse-namespace-with-version",
        json!({
            "inputs": { "args": ["org.acme@1.0.0"] },
            "outcome": { "ok": {
                "name": "org.acme",
                "escapedNamespace": "org.acme_1.0.0",
                "version": "1.0.0",
                "versionParsed": { "@@oracle": "object", "ctor": "SemVer" }
            } }
        }),
    );
    write_fixture(
        &dir,
        "ModelUtil.isPrimitiveType",
        "is-primitive",
        json!({ "inputs": { "args": ["String"] }, "outcome": { "ok": true } }),
    );
    write_fixture(
        &dir,
        "ModelUtil.capitalizeFirstLetter",
        "capitalize",
        json!({ "inputs": { "args": ["acme"] }, "outcome": { "ok": "Acme" } }),
    );
    write_fixture(
        &dir,
        "ModelUtil.isValidIdentifier",
        "valid-identifier",
        json!({ "inputs": { "args": ["suchName"] }, "outcome": { "ok": true } }),
    );
    write_fixture(
        &dir,
        "ModelUtil.getFullyQualifiedName",
        "fqn",
        json!({ "inputs": { "args": ["org.acme", "Foo"] }, "outcome": { "ok": "org.acme.Foo" } }),
    );
    write_fixture(
        &dir,
        "ModelUtil.removeNamespaceVersionFromFullyQualifiedName",
        "remove-version",
        json!({
            "inputs": { "args": ["org.acme@1.0.0.Person"] },
            "outcome": { "ok": "org.acme.Person" }
        }),
    );
    write_fixture(
        &dir,
        "ModelUtil.isSystemProperty",
        "system-prop",
        json!({ "inputs": { "args": ["$class"] }, "outcome": { "ok": true } }),
    );
    write_fixture(
        &dir,
        "ModelUtil.isPrivateSystemProperty",
        "private-system-prop",
        json!({ "inputs": { "args": ["$class"] }, "outcome": { "ok": false } }),
    );
    write_fixture(
        &dir,
        "ModelUtil.isValidMapKey",
        "map-key",
        json!({
            "inputs": { "args": [{ "$class": "concerto.metamodel@1.0.0.StringMapKeyType" }] },
            "outcome": { "ok": true }
        }),
    );
    write_fixture(
        &dir,
        "ModelUtil.isValidMapValue",
        "map-value",
        json!({
            "inputs": { "args": [{ "$class": "concerto.metamodel@1.0.0.BooleanMapValueType" }] },
            "outcome": { "ok": true }
        }),
    );
    write_fixture(
        &dir,
        "ModelUtil.importFullyQualifiedNames",
        "import-all",
        json!({
            "inputs": { "args": [{
                "$class": "concerto.metamodel@1.0.0.ImportAll",
                "namespace": "org.acme"
            }] },
            "outcome": { "ok": ["org.acme.*"] }
        }),
    );
    // Real recorded fixtures (task P1-07 review; `TypeNotFoundException
    // #constructor with N arguments`, and the JSONPopulator fixture that
    // hits the same default-message path from inside a real port): this op
    // needs no `ModelManager` reconstruction at all, unlike every other
    // non-`ModelUtil` op (see `ops.rs`'s module doc).
    write_fixture(
        &dir,
        "TypeNotFoundException.new",
        "type-not-found-default-message",
        json!({
            "inputs": { "args": ["namespace.TypeName"] },
            "outcome": { "ok": { "@@oracle": "error", "error": {
                "class": "TypeNotFoundException",
                "message": "Type \"namespace.TypeName\" not found.",
                "location": null,
                "component": "@accordproject/concerto-core"
            } } }
        }),
    );
    write_fixture(
        &dir,
        "TypeNotFoundException.new",
        "type-not-found-custom-message-and-component",
        json!({
            "inputs": { "args": ["namespace.TypeName", "MESSAGE_TEXT", "foo"] },
            "outcome": { "ok": { "@@oracle": "error", "error": {
                "class": "TypeNotFoundException",
                "message": "MESSAGE_TEXT",
                "location": null,
                "component": "foo"
            } } }
        }),
    );

    let (fixtures, load_errors) = fixture::load_all(&dir);
    assert!(
        load_errors.is_empty(),
        "unexpected load errors: {load_errors:?}"
    );
    assert_eq!(
        fixtures.len(),
        17,
        "expected one fixture per write_fixture call"
    );

    for fx in &fixtures {
        let dispatch = ops::exec(&bare(), &fx.op, &fx.inputs);
        let verdict = compare::judge(fx, dispatch);
        assert!(
            matches!(verdict, Verdict::Pass),
            "fixture {} ({}) did not pass: {verdict:?}",
            fx.id,
            fx.op
        );
    }

    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn reports_a_real_mismatch_as_fail_not_pass() {
    let dir = scratch_dir("fail");
    write_fixture(
        &dir,
        "ModelUtil.getShortName",
        "wrong",
        json!({ "inputs": { "args": ["org.acme.Foo"] }, "outcome": { "ok": "NotFoo" } }),
    );
    let (fixtures, _) = fixture::load_all(&dir);
    let fx = &fixtures[0];
    let verdict = compare::judge(fx, ops::exec(&bare(), &fx.op, &fx.inputs));
    assert!(
        matches!(verdict, Verdict::Fail { .. }),
        "expected Fail, got {verdict:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn reports_a_wrong_error_class_as_fail_not_pass() {
    let dir = scratch_dir("fail-error");
    write_fixture(
        &dir,
        "ModelUtil.getNamespace",
        "wrong-class",
        json!({
            "inputs": { "args": [{ "@@oracle": "undefined" }] },
            "outcome": { "error": {
                "class": "TypeNotFoundException", "message": "FQN is invalid.",
                "location": null, "component": null
            } }
        }),
    );
    let (fixtures, _) = fixture::load_all(&dir);
    let fx = &fixtures[0];
    let verdict = compare::judge(fx, ops::exec(&bare(), &fx.op, &fx.inputs));
    assert!(
        matches!(verdict, Verdict::Fail { .. }),
        "expected Fail, got {verdict:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn reports_an_unimplemented_op_as_unsupported_not_fail() {
    let dir = scratch_dir("unsupported");
    write_fixture(
        &dir,
        "ModelManager.isAssignableTo",
        "needs-a-model-manager",
        json!({ "inputs": { "args": [] }, "outcome": { "ok": true } }),
    );
    let (fixtures, _) = fixture::load_all(&dir);
    let fx = &fixtures[0];
    let verdict = compare::judge(fx, ops::exec(&bare(), &fx.op, &fx.inputs));
    assert!(
        matches!(verdict, Verdict::Unsupported { .. }),
        "expected Unsupported, got {verdict:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn reports_an_undecodable_argument_as_unsupported_not_fail() {
    let dir = scratch_dir("unsupported-arg");
    write_fixture(
        &dir,
        "ModelUtil.getShortName",
        "mm-argument",
        json!({
            "inputs": { "args": [{ "@@oracle": "mmref", "id": "m1" }] },
            "outcome": { "ok": "Foo" }
        }),
    );
    let (fixtures, _) = fixture::load_all(&dir);
    let fx = &fixtures[0];
    let verdict = compare::judge(fx, ops::exec(&bare(), &fx.op, &fx.inputs));
    assert!(
        matches!(verdict, Verdict::Unsupported { .. }),
        "expected Unsupported, got {verdict:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn a_random_fixture_is_unsupported_regardless_of_its_op() {
    let dir = scratch_dir("random");
    write_fixture(
        &dir,
        "ModelUtil.getShortName",
        "random-flag",
        json!({
            "inputs": { "args": ["org.acme.Foo"] },
            "outcome": { "ok": "Foo" },
            "env": { "random": true }
        }),
    );
    let (fixtures, _) = fixture::load_all(&dir);
    let fx = &fixtures[0];
    let verdict = compare::judge(fx, ops::exec(&bare(), &fx.op, &fx.inputs));
    assert!(
        matches!(verdict, Verdict::Unsupported { .. }),
        "expected Unsupported, got {verdict:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn resolves_a_blob_referenced_argument() {
    let dir = scratch_dir("blob");
    let sha = "abcd1234deadbeef";
    let blob_dir = dir.join("blobs").join(&sha[..2]);
    fs::create_dir_all(&blob_dir).unwrap();
    fs::write(
        blob_dir.join(format!("{sha}.json")),
        serde_json::to_string(&json!("org.acme.Foo")).unwrap(),
    )
    .unwrap();
    write_fixture(
        &dir,
        "ModelUtil.getShortName",
        "blob-arg",
        json!({
            "inputs": { "args": [{ "@@oracle": "blob", "sha256": sha }] },
            "outcome": { "ok": "Foo" }
        }),
    );
    let (fixtures, load_errors) = fixture::load_all(&dir);
    assert!(load_errors.is_empty(), "{load_errors:?}");
    let fx = &fixtures[0];
    let verdict = compare::judge(fx, ops::exec(&bare(), &fx.op, &fx.inputs));
    assert!(
        matches!(verdict, Verdict::Pass),
        "expected Pass, got {verdict:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Model-manager recipes and the CTO cache
// ---------------------------------------------------------------------------

/// The CTO text the recipe tests "parse", and the AST a cache entry maps it
/// to: an enum and a concept with a field of that enum type.
const CTO: &str = "namespace test@1.0.0\nenum Colour { o RED }\nconcept Car { o Colour colour }\n";

fn test_ast() -> serde_json::Value {
    json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "test@1.0.0",
        "imports": [],
        "declarations": [
            {
                "$class": "concerto.metamodel@1.0.0.EnumDeclaration",
                "name": "Colour",
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "RED" }
                ]
            },
            {
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Car",
                "isAbstract": false,
                "properties": [
                    {
                        "$class": "concerto.metamodel@1.0.0.ObjectProperty",
                        "name": "colour",
                        "isArray": false,
                        "isOptional": false,
                        "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Colour" }
                    }
                ]
            }
        ]
    })
}

/// Writes one cache entry the way `build-cto-cache.js` lays it out.
fn write_cache_entry(
    cache: &std::path::Path,
    cto: &str,
    file_name: Option<&str>,
    entry: serde_json::Value,
) {
    let key = CtoCache::key(cto, file_name, &serde_json::Value::Null);
    let dir = cache.join(&key[..2]);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(format!("{key}.json")), entry.to_string()).unwrap();
}

fn with_cache(dir: &std::path::Path) -> Harness {
    Harness {
        cache: Some(CtoCache::at(dir.to_path_buf())),
        ledger: Ledger::default(),
    }
}

/// A model manager recipe with one `addCTOModel` step.
fn recipe_with_cto(file_name: &str, disable_validation: bool, status: &str) -> serde_json::Value {
    json!({
        "@@oracle": "mm", "id": 0, "kind": "ModelManager",
        "steps": [{
            "method": "addCTOModel",
            "args": [CTO, file_name, disable_validation],
            "status": status,
            "errorClass": if status == "ok" { serde_json::Value::Null } else { json!("IllegalModelException") }
        }]
    })
}

fn judge_one(h: &Harness, dir: &std::path::Path) -> Verdict {
    let (fixtures, load_errors) = fixture::load_all(dir);
    assert!(load_errors.is_empty(), "{load_errors:?}");
    assert_eq!(fixtures.len(), 1);
    let fx = &fixtures[0];
    compare::judge(fx, ops::exec(h, &fx.op, &fx.inputs))
}

/// The cache key must be the SHA-256 of `JSON.stringify([cto, fileName,
/// skipLocationNodes])`. The expected digests were computed by Node
/// (`crypto.createHash('sha256').update(JSON.stringify(t))`), not by Rust,
/// over strings that exercise `JSON.stringify`'s escaping.
#[test]
fn cto_cache_keys_match_build_cto_cache_js() {
    assert_eq!(
        CtoCache::key(
            "namespace test@1.0.0\n\"quoted\" \u{1} é",
            Some("a.cto"),
            &serde_json::Value::Null
        ),
        "4e1a627892a106d26c4c2e971cb58245d7fa11e9d369934486a2836235ad15a7"
    );
    assert_eq!(
        CtoCache::key("x", None, &json!(true)),
        "5ac340e4a35422a3307590db2ff89ef701781889501f710aed55c15291e9ec47"
    );
}

#[test]
fn replays_add_cto_model_through_the_cache() {
    let dir = scratch_dir("add-cto");
    let cache = scratch_dir("add-cto-cache");
    write_cache_entry(&cache, CTO, Some("test.cto"), json!({ "ast": test_ast() }));
    write_fixture(
        &dir,
        "ModelManager.addCTOModel",
        "add-cto",
        json!({
            "inputs": {
                "target": { "@@oracle": "mm", "id": 0, "kind": "ModelManager", "steps": [] },
                "args": [CTO, "test.cto"]
            },
            "outcome": { "ok": {
                "@@oracle": "ModelFile",
                "namespace": "test@1.0.0",
                "name": "test.cto",
                "ast": test_ast()
            } }
        }),
    );
    let verdict = judge_one(&with_cache(&cache), &dir);
    assert!(
        matches!(verdict, Verdict::Pass),
        "expected Pass, got {verdict:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn a_cto_text_missing_from_the_cache_is_a_harness_error() {
    let dir = scratch_dir("missing-cto");
    let cache = scratch_dir("missing-cto-cache");
    write_fixture(
        &dir,
        "ModelManager.addCTOModel",
        "missing-cto",
        json!({
            "inputs": {
                "target": { "@@oracle": "mm", "id": 0, "kind": "ModelManager", "steps": [] },
                "args": [CTO, "test.cto"]
            },
            "outcome": { "ok": null }
        }),
    );
    let verdict = judge_one(&with_cache(&cache), &dir);
    assert!(
        matches!(verdict, Verdict::HarnessError { .. }),
        "expected HarnessError, got {verdict:?}"
    );
    // No cache at all is a harness error too, never a skip.
    let verdict = judge_one(&bare(), &dir);
    assert!(
        matches!(verdict, Verdict::HarnessError { .. }),
        "expected HarnessError, got {verdict:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn replays_a_cached_parse_exception_as_the_outcome() {
    let dir = scratch_dir("parse-error");
    let cache = scratch_dir("parse-error-cache");
    let error = json!({
        "class": "ParseException",
        "message": "Expected \"concept\" but \"x\" found. File bad.cto line 1 column 1",
        "location": { "start": { "line": 1, "column": 1, "offset": 0 }, "end": { "line": 1, "column": 1, "offset": 0 } }
    });
    write_cache_entry(&cache, "x", Some("bad.cto"), json!({ "error": error }));
    let mut expected = error.clone();
    expected["component"] = json!("@accordproject/concerto-util");
    // Recorded ParseException locations carry peggy's `source: undefined`,
    // which the cache's JSON drops (ops.rs, `restore_location_source`).
    expected["location"]["source"] = json!({ "@@oracle": "undefined" });
    write_fixture(
        &dir,
        "ModelManager.addCTOModel",
        "parse-error",
        json!({
            "inputs": {
                "target": { "@@oracle": "mm", "id": 0, "kind": "ModelManager", "steps": [] },
                "args": ["x", "bad.cto"]
            },
            "outcome": { "error": expected }
        }),
    );
    let verdict = judge_one(&with_cache(&cache), &dir);
    assert!(
        matches!(verdict, Verdict::Pass),
        "expected Pass, got {verdict:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn a_step_that_replays_with_another_status_is_a_failure() {
    let dir = scratch_dir("divergence");
    let cache = scratch_dir("divergence-cache");
    write_cache_entry(&cache, CTO, Some("test.cto"), json!({ "ast": test_ast() }));
    // Recorded as an error; the Rust engine loads it fine.
    write_fixture(
        &dir,
        "ModelManager.getNamespaces",
        "divergence",
        json!({
            "inputs": { "target": recipe_with_cto("test.cto", true, "error"), "args": [] },
            "outcome": { "ok": ["concerto.decorator@1.0.0", "concerto@1.0.0"] }
        }),
    );
    let verdict = judge_one(&with_cache(&cache), &dir);
    match verdict {
        Verdict::Fail { kind, detail } => {
            assert_eq!(kind, compare::FailKind::StateDivergence);
            assert!(detail.contains("state divergence"), "{detail}");
        }
        other => panic!("expected Fail, got {other:?}"),
    }
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn runs_a_model_util_collaborator_op_over_a_replayed_model_manager() {
    let dir = scratch_dir("is-enum");
    let cache = scratch_dir("is-enum-cache");
    write_cache_entry(&cache, CTO, Some("test.cto"), json!({ "ast": test_ast() }));
    let mm = recipe_with_cto("test.cto", false, "ok");
    write_fixture(
        &dir,
        "ModelUtil.isEnum",
        "is-enum",
        json!({
            "inputs": { "args": [{
                "@@oracle": "propref",
                "decl": {
                    "@@oracle": "declref",
                    "mf": { "@@oracle": "mfref", "mm": mm, "ns": "test@1.0.0" },
                    "index": 1,
                    "name": "Car"
                },
                "index": 0,
                "name": "colour"
            }] },
            "outcome": { "ok": true }
        }),
    );
    let verdict = judge_one(&with_cache(&cache), &dir);
    assert!(
        matches!(verdict, Verdict::Pass),
        "expected Pass, got {verdict:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

#[test]
fn a_dangling_model_manager_reference_is_a_harness_error() {
    let dir = scratch_dir("dangling");
    write_fixture(
        &dir,
        "ModelManager.getNamespaces",
        "dangling",
        json!({
            "inputs": { "target": { "@@oracle": "mmref", "id": 7 }, "args": [] },
            "outcome": { "ok": [] }
        }),
    );
    let verdict = judge_one(&bare(), &dir);
    assert!(
        matches!(verdict, Verdict::HarnessError { .. }),
        "expected HarnessError, got {verdict:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// A validating add after a file that was added without validation cannot
/// be replayed faithfully while single-file validation is not ported
/// (`recipe.rs`, "Validation on add"): unsupported, not a guess.
#[test]
fn a_validating_add_after_an_unvalidated_one_is_unsupported() {
    let dir = scratch_dir("validate-after-unvalidated");
    let cache = scratch_dir("validate-after-unvalidated-cache");
    write_cache_entry(&cache, CTO, Some("test.cto"), json!({ "ast": test_ast() }));
    let other = "namespace other@1.0.0\n";
    write_cache_entry(
        &cache,
        other,
        Some("other.cto"),
        json!({ "ast": {
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "other@1.0.0",
            "imports": [],
            "declarations": []
        } }),
    );
    write_fixture(
        &dir,
        "ModelManager.addCTOModel",
        "validate-after-unvalidated",
        json!({
            "inputs": { "target": recipe_with_cto("test.cto", true, "ok"), "args": [other, "other.cto"] },
            "outcome": { "ok": null }
        }),
    );
    let verdict = judge_one(&with_cache(&cache), &dir);
    assert!(
        matches!(verdict, Verdict::Unsupported { .. }),
        "expected Unsupported, got {verdict:?}"
    );
    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&cache);
}

/// Runs a directory of fixtures through the recorder against `baseline`.
fn report_against(dir: &std::path::Path, baseline: &report::Baseline) -> report::Report {
    let (fixtures, load_errors) = fixture::load_all(dir);
    assert!(load_errors.is_empty(), "{load_errors:?}");
    let mut recorder = report::Recorder::new(dir.to_path_buf(), None, Vec::new());
    for fx in &fixtures {
        recorder.record(
            fx,
            compare::judge(fx, ops::exec(&bare(), &fx.op, &fx.inputs)),
            &Ledger::default(),
        );
    }
    recorder.finish(baseline)
}

/// [`report_against`] as a run with `ORACLE_OP=ModelUtil.` would record it.
fn report_filtered(dir: &std::path::Path, baseline: &report::Baseline) -> report::Report {
    let (fixtures, _) = fixture::load_all(dir);
    let mut recorder =
        report::Recorder::new(dir.to_path_buf(), Some("ModelUtil.".into()), Vec::new());
    for fx in &fixtures {
        recorder.record(
            fx,
            compare::judge(fx, ops::exec(&bare(), &fx.op, &fx.inputs)),
            &Ledger::default(),
        );
    }
    recorder.finish(baseline)
}

fn regresses(report: &report::Report) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        report.assert_no_regressions()
    }))
    .is_err()
}

fn entry(id: &str, status: report::Status) -> ((String, String), report::Status) {
    (
        ("ModelUtil.getShortName".to_string(), id.to_string()),
        status,
    )
}

/// A run that matches the baseline passes. A baselined failure that passes
/// now, a pass the baseline does not list, and a baselined fixture gone
/// from the corpus are listed, and each fails a full run until the baseline
/// is regenerated; with `ORACLE_OP` they are only reported.
#[test]
fn a_run_passes_only_when_it_matches_the_baseline_exactly() {
    use compare::FailKind::ValueMismatch;
    use report::Status::{Fail, Pass};

    let dir = scratch_dir("baseline-exact");
    write_fixture(
        &dir,
        "ModelUtil.getShortName",
        "known",
        json!({ "inputs": { "args": ["org.acme.Foo"] }, "outcome": { "ok": "NotFoo" } }),
    );
    write_fixture(
        &dir,
        "ModelUtil.getShortName",
        "passes",
        json!({ "inputs": { "args": ["org.acme.Bar"] }, "outcome": { "ok": "Bar" } }),
    );
    let exact: report::Baseline =
        [entry("known", Fail(ValueMismatch)), entry("passes", Pass)].into();
    report_against(&dir, &exact).assert_no_regressions();

    // Fixed: the baseline still says it fails.
    let fixed: report::Baseline = [
        entry("known", Fail(ValueMismatch)),
        entry("passes", Fail(ValueMismatch)),
    ]
    .into();
    let report = report_against(&dir, &fixed);
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["fixed"], json!(["ModelUtil.getShortName\tpasses"]));
    assert!(
        regresses(&report),
        "a fixed fixture must fail a full run until recorded"
    );
    report_filtered(&dir, &fixed).assert_no_regressions();

    // A new pass: the baseline does not list it.
    let new_pass: report::Baseline = [entry("known", Fail(ValueMismatch))].into();
    let report = report_against(&dir, &new_pass);
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(
        json["new_passes"],
        json!(["ModelUtil.getShortName\tpasses"])
    );
    assert!(
        regresses(&report),
        "an unrecorded pass must fail a full run"
    );
    report_filtered(&dir, &new_pass).assert_no_regressions();

    // Missing: a baselined fixture that is not in the corpus.
    let mut stale = exact.clone();
    stale.extend([entry("gone", Pass)]);
    let report = report_against(&dir, &stale);
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["missing"], json!(["ModelUtil.getShortName\tgone"]));
    assert!(
        regresses(&report),
        "a stale baseline entry must fail a full run"
    );
    report_filtered(&dir, &stale).assert_no_regressions();
    let _ = fs::remove_dir_all(&dir);
}

/// A baseline that lists one fixture twice is rejected.
#[test]
fn a_baseline_with_a_duplicate_entry_is_rejected() {
    let dir = scratch_dir("baseline-duplicate");
    let path = dir.join("baseline.tsv");
    fs::write(
        &path,
        "ModelUtil.getShortName\tx\tpass\nModelUtil.getShortName\tx\tfail:value-mismatch\n",
    )
    .unwrap();
    let caught = std::panic::catch_unwind(|| report::read_baseline(&path));
    assert!(
        caught.is_err(),
        "a duplicate baseline entry must be rejected"
    );
    let _ = fs::remove_dir_all(&dir);
}

/// Each regression the baseline guards against fails the run: a new
/// failure, a known failure of another kind, a pass that fails, and a
/// baselined fixture (passing or failing) that becomes unsupported.
#[test]
fn every_kind_of_regression_against_the_baseline_fails_the_run() {
    use compare::FailKind::{MessageMismatch, ValueMismatch};
    use report::Status::{Fail, Pass};

    let dir = scratch_dir("baseline-regressions");
    // Fails with a value mismatch.
    write_fixture(
        &dir,
        "ModelUtil.getShortName",
        "value",
        json!({ "inputs": { "args": ["org.acme.Foo"] }, "outcome": { "ok": "NotFoo" } }),
    );
    // Unsupported (the env.random guard).
    write_fixture(
        &dir,
        "ModelUtil.getShortName",
        "random",
        json!({
            "inputs": { "args": ["org.acme.Foo"] },
            "outcome": { "ok": "Foo" },
            "env": { "random": true }
        }),
    );
    let cases: [(&str, report::Baseline); 5] = [
        ("a failure not in the baseline", [].into()),
        (
            "a known failure of another kind",
            [entry("value", Fail(MessageMismatch))].into(),
        ),
        ("a pass that now fails", [entry("value", Pass)].into()),
        (
            "a pass that is now unsupported",
            [entry("value", Fail(ValueMismatch)), entry("random", Pass)].into(),
        ),
        (
            "a known failure that is now unsupported",
            [
                entry("value", Fail(ValueMismatch)),
                entry("random", Fail(ValueMismatch)),
            ]
            .into(),
        ),
    ];
    for (what, baseline) in cases {
        assert!(
            regresses(&report_against(&dir, &baseline)),
            "{what} must fail the run"
        );
    }
    // And the same corpus against its own baseline is clean.
    let own: report::Baseline = [entry("value", Fail(ValueMismatch))].into();
    assert!(!regresses(&report_against(&dir, &own)));
    let _ = fs::remove_dir_all(&dir);
}

/// Only a `ParseException` gets concerto-util's default component and its
/// location's `source: undefined` back; any other cached error keeps what
/// the cache recorded, and an entry without a class is a harness error.
#[test]
fn only_a_cached_parse_exception_gets_its_defaults_restored() {
    let location = json!({ "start": { "line": 1, "column": 1, "offset": 0 }, "end": { "line": 1, "column": 2, "offset": 1 } });
    let parse = ops::OracleError::from_cached_error(
        &json!({ "class": "ParseException", "message": "m", "location": location }),
    )
    .unwrap();
    assert_eq!(
        parse.component.as_deref(),
        Some("@accordproject/concerto-util")
    );
    assert_eq!(
        parse.location.unwrap()["source"],
        json!({ "@@oracle": "undefined" })
    );

    let other = ops::OracleError::from_cached_error(
        &json!({ "class": "TypeError", "message": "m", "location": location }),
    )
    .unwrap();
    assert_eq!(other.component, None);
    assert_eq!(other.location, Some(location));

    assert!(ops::OracleError::from_cached_error(&json!({ "message": "m" })).is_err());
}

/// A run with a harness error (here, a CTO text with no cache entry) must
/// not regenerate the baseline.
#[test]
fn a_run_with_harness_errors_cannot_write_the_baseline() {
    let dir = scratch_dir("regenerate-invalid");
    write_fixture(
        &dir,
        "ModelManager.addCTOModel",
        "no-cache",
        json!({
            "inputs": {
                "target": { "@@oracle": "mm", "id": 0, "kind": "ModelManager", "steps": [] },
                "args": [CTO, "test.cto"]
            },
            "outcome": { "ok": null }
        }),
    );
    let report = report_against(&dir, &report::Baseline::new());
    let out = dir.join("baseline.tsv");
    let caught =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| report.write_baseline(&out)));
    assert!(
        caught.is_err(),
        "a run with a harness error must not write the baseline"
    );
    assert!(!out.exists(), "nothing may be written");
    let _ = fs::remove_dir_all(&dir);
}

/// Owners: the ledger by member, then by class, then PORTING's op families,
/// and `unowned` spelled out when none applies.
#[test]
fn owners_fall_back_to_the_porting_op_families_and_unowned() {
    let ledger = Ledger::default();
    assert_eq!(ledger.owner("Factory.newResource"), "P3-01");
    assert_eq!(ledger.owner("ModelManager.deleteModelFile"), "P2-08");
    assert_eq!(ledger.owner("DecoratorManager.decorateModels"), "P2-12");
    assert_eq!(ledger.owner("Declaration.getName"), "unowned");
}

/// A plan-owner override beats the ledger: `Factory`'s ledger rows are TS
/// with no task, yet its fixtures belong to P3-01. Other stays-TS classes
/// are unaffected.
#[test]
fn plan_owner_overrides_take_precedence_over_the_ledger() {
    let root = std::env::temp_dir().join(format!(
        "concerto-oracle-self-test-owner-override-{}",
        std::process::id()
    ));
    let fixtures = root.join("migration").join("oracle").join("fixtures");
    let ledger_dir = root.join("migration").join("ledger");
    fs::create_dir_all(&fixtures).unwrap();
    fs::create_dir_all(&ledger_dir).unwrap();
    fs::write(
        ledger_dir.join("SEAM_LEDGER.tsv"),
        "file\tclass\tmember\tclassification\tplanned_task\n\
         src/factory.ts\tFactory\tnewResource\tTS\t-\n\
         src/modelloader.ts\tModelLoader\tloadModelManager\tTS\t-\n",
    )
    .unwrap();
    let ledger = Ledger::load(&fixtures);
    assert_eq!(ledger.owner("Factory.newResource"), "P3-01");
    assert_eq!(ledger.owner("Factory.new"), "P3-01");
    assert_eq!(ledger.owner("ModelLoader.loadModelManager"), "stays-ts");
    let _ = fs::remove_dir_all(&root);
}
