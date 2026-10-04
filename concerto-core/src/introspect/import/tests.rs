use super::*;
use crate::model_util::qualify;

/// The fully-qualified name `short` resolves to through this import alone,
/// as TS `importShortNames` registers it: by local name (an alias only
/// under its alias), a later entry replacing an earlier one.
fn resolve(imp: &Import, short: &str) -> Option<String> {
    imp.local_names()
        .into_iter()
        .zip(imp.imported_names())
        .rfind(|(local, _)| *local == short)
        .map(|(_, name)| qualify(imp.namespace(), name))
}

#[test]
fn resolves_named_import() {
    let imp = Import::try_from(&crate::json!({
        "$class": "concerto.metamodel@1.0.0.ImportType",
        "namespace": "org.acme@1.0.0",
        "name": "Person"
    }))
    .unwrap();
    assert_eq!(imp.namespace(), "org.acme@1.0.0");
    assert_eq!(
        resolve(&imp, "Person").as_deref(),
        Some("org.acme@1.0.0.Person")
    );
    assert_eq!(resolve(&imp, "Other"), None);
}

#[test]
fn resolves_multi_import_with_alias() {
    let imp = Import::try_from(&crate::json!({
        "$class": "concerto.metamodel@1.0.0.ImportTypes",
        "namespace": "org.acme@1.0.0",
        "types": ["A", "B"],
        "aliasedTypes": [
            { "$class": "concerto.metamodel@1.0.0.AliasedType", "name": "B", "aliasedName": "Bee" }
        ]
    }))
    .unwrap();
    assert_eq!(resolve(&imp, "A").as_deref(), Some("org.acme@1.0.0.A"));
    assert_eq!(resolve(&imp, "Bee").as_deref(), Some("org.acme@1.0.0.B"));
    assert_eq!(resolve(&imp, "C"), None);
}

#[test]
fn local_names_use_the_alias_where_one_is_given() {
    let imp = Import::try_from(&crate::json!({
        "$class": "concerto.metamodel@1.0.0.ImportTypes",
        "namespace": "org.acme@1.0.0",
        "types": ["A", "B"],
        "aliasedTypes": [
            { "$class": "concerto.metamodel@1.0.0.AliasedType", "name": "B", "aliasedName": "Bee" }
        ]
    }))
    .unwrap();
    assert_eq!(imp.local_names(), ["A", "Bee"]);

    let single = Import::try_from(&crate::json!({
        "$class": "concerto.metamodel@1.0.0.ImportType",
        "namespace": "org.acme@1.0.0",
        "name": "Person"
    }))
    .unwrap();
    assert_eq!(single.local_names(), ["Person"]);
}

#[test]
fn wildcard_import_is_rejected() {
    let err = Import::try_from(&crate::json!({
        "$class": "concerto.metamodel@1.0.0.ImportAll",
        "namespace": "org.acme@1.0.0"
    }));
    // TS: `ModelFile.fromAst` throws a plain `Error` with this exact,
    // hardcoded message (not an `IllegalModelException`).
    assert_eq!(
        err.unwrap_err().to_string(),
        "Wildcard Imports are not permitted."
    );
}

#[test]
fn missing_class_is_rejected() {
    let err = Import::try_from(&crate::json!({ "namespace": "org.acme@1.0.0" }));
    assert!(err.unwrap_err().to_string().contains("$class"));
}

#[test]
fn missing_class_is_reported_verbatim() {
    let err = Import::try_from(&crate::json!({ "namespace": "org.acme@1.0.0" }));
    assert_eq!(
        err.unwrap_err().to_string(),
        "import node is missing its $class"
    );
}

#[test]
fn unknown_import_kind_errors() {
    let err = Import::try_from(&crate::json!({
        "$class": "concerto.metamodel@1.0.0.MysteryImport",
        "namespace": "org.acme@1.0.0"
    }));
    assert_eq!(
        err.unwrap_err().to_string(),
        "unknown import type: MysteryImport"
    );
}

#[test]
fn an_import_class_may_be_given_as_the_short_name() {
    let imp = Import::try_from(&crate::json!({
        "$class": "ImportType",
        "namespace": "org.acme@1.0.0",
        "name": "Person"
    }))
    .unwrap();
    assert_eq!(
        resolve(&imp, "Person").as_deref(),
        Some("org.acme@1.0.0.Person")
    );
}

#[test]
fn import_types_with_no_types_array_is_empty() {
    let imp = Import::try_from(&crate::json!({
        "$class": "concerto.metamodel@1.0.0.ImportTypes",
        "namespace": "org.acme@1.0.0"
    }))
    .unwrap();
    assert!(imp.imported_names().is_empty());
    assert!(imp.local_names().is_empty());
}

#[test]
fn an_alias_with_no_class_is_still_an_alias() {
    let imp = Import::try_from(&crate::json!({
        "$class": "concerto.metamodel@1.0.0.ImportTypes",
        "namespace": "org.acme@1.0.0",
        "types": ["A", "B"],
        "aliasedTypes": [{ "name": "B", "aliasedName": "Bee" }]
    }))
    .unwrap();
    assert_eq!(resolve(&imp, "Bee").as_deref(), Some("org.acme@1.0.0.B"));
    assert_eq!(imp.local_names(), ["A", "Bee"]);
}

#[test]
fn non_string_types_and_malformed_aliases_are_skipped() {
    let imp = Import::try_from(&crate::json!({
        "$class": "concerto.metamodel@1.0.0.ImportTypes",
        "namespace": "org.acme@1.0.0",
        "types": ["A", 3, { "name": "C" }],
        "aliasedTypes": [{ "name": "A" }, 7, { "name": "A", "aliasedName": "Ay" }]
    }))
    .unwrap();
    assert_eq!(imp.imported_names(), ["A"]);
    assert_eq!(resolve(&imp, "Ay").as_deref(), Some("org.acme@1.0.0.A"));
}

#[test]
fn types_or_aliases_that_are_not_arrays_are_empty() {
    let imp = Import::try_from(&crate::json!({
        "$class": "concerto.metamodel@1.0.0.ImportTypes",
        "namespace": "org.acme@1.0.0",
        "types": "A",
        "aliasedTypes": {}
    }))
    .unwrap();
    assert!(imp.imported_names().is_empty());
    assert_eq!(resolve(&imp, "A"), None);
}

#[test]
fn an_aliased_type_no_longer_resolves_under_its_declared_name() {
    let imp = Import::try_from(&crate::json!({
        "$class": "concerto.metamodel@1.0.0.ImportTypes",
        "namespace": "org.acme@1.0.0",
        "types": ["A", "B"],
        "aliasedTypes": [
            { "$class": "concerto.metamodel@1.0.0.AliasedType", "name": "B", "aliasedName": "Bee" }
        ]
    }))
    .unwrap();
    assert_eq!(resolve(&imp, "Bee").as_deref(), Some("org.acme@1.0.0.B"));
    // "B" itself is no longer a visible local name once aliased to "Bee".
    assert_eq!(resolve(&imp, "B"), None);
    // The unaliased sibling still resolves under its own name.
    assert_eq!(resolve(&imp, "A").as_deref(), Some("org.acme@1.0.0.A"));
}

#[test]
fn a_non_string_uri_is_ignored() {
    let imp = Import::try_from(&crate::json!({
        "$class": "concerto.metamodel@1.0.0.ImportType",
        "namespace": "org.acme@1.0.0",
        "name": "Person",
        "uri": 5
    }))
    .unwrap();
    assert_eq!(
        resolve(&imp, "Person").as_deref(),
        Some("org.acme@1.0.0.Person")
    );
}

#[test]
fn the_last_alias_of_a_name_wins() {
    // TS `fromAst` builds the aliases with `Map.set`: `Foo` is visible as
    // `Baz` only.
    let imp = Import::try_from(&crate::json!({
        "$class": "concerto.metamodel@1.0.0.ImportTypes",
        "namespace": "b@1.0.0",
        "types": ["Foo", "Foo"],
        "aliasedTypes": [
            { "name": "Foo", "aliasedName": "Bar" },
            { "name": "Foo", "aliasedName": "Baz" }
        ]
    }))
    .unwrap();
    assert_eq!(imp.local_names(), ["Baz", "Baz"]);
    assert_eq!(resolve(&imp, "Baz").as_deref(), Some("b@1.0.0.Foo"));
    assert_eq!(resolve(&imp, "Bar"), None);
}
