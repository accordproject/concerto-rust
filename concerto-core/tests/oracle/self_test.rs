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
use super::fixture;
use super::ops;

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
            "outcome": { "error": { "class": "Error", "message": "FQN is invalid." } }
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
        let dispatch = ops::exec(&fx.op, &fx.inputs);
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
    let verdict = compare::judge(fx, ops::exec(&fx.op, &fx.inputs));
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
            "outcome": { "error": { "class": "TypeNotFoundException", "message": "FQN is invalid." } }
        }),
    );
    let (fixtures, _) = fixture::load_all(&dir);
    let fx = &fixtures[0];
    let verdict = compare::judge(fx, ops::exec(&fx.op, &fx.inputs));
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
    let verdict = compare::judge(fx, ops::exec(&fx.op, &fx.inputs));
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
    let verdict = compare::judge(fx, ops::exec(&fx.op, &fx.inputs));
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
    let verdict = compare::judge(fx, ops::exec(&fx.op, &fx.inputs));
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
    let verdict = compare::judge(fx, ops::exec(&fx.op, &fx.inputs));
    assert!(
        matches!(verdict, Verdict::Pass),
        "expected Pass, got {verdict:?}"
    );
    let _ = fs::remove_dir_all(&dir);
}
