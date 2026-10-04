use super::*;

/// `org.example@1.0.0` with Person ← Employee ← Manager and an enum.
fn manager() -> ModelManager {
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.example@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Person", "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "name", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Employee", "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.DoubleProperty", "name": "salary", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Manager", "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Employee" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "title", "isArray": false, "isOptional": true }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.EnumDeclaration", "name": "Color",
                      "properties": [ { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "RED" } ] }
                ]
            }),
            None,
        )
        .unwrap();
    mgr
}

/// A file registered shared in a second manager is the same file,
/// with the same duplicate namespace error as `add_model_file`;
/// `compact_model_asts` returns each file's AST text, compacts a
/// file only this manager holds and leaves a shared one as it is,
/// and every AST reads back equal.
#[test]
fn shared_model_files_and_compact_model_asts() {
    let mut source = manager();
    let before: Vec<Value> = source.model_files().map(|mf| mf.ast().clone()).collect();
    let mut other = ModelManager::new().unwrap();
    let shared = source
        .shared_model_files()
        .find(|mf| mf.namespace() == "org.example@1.0.0")
        .cloned()
        .unwrap();
    other.add_shared_model_file(Arc::clone(&shared)).unwrap();
    let held = other
        .shared_model_files()
        .find(|mf| mf.namespace() == "org.example@1.0.0")
        .unwrap();
    assert!(Arc::ptr_eq(held, &shared));
    let dup = other
        .add_shared_model_file(Arc::clone(&shared))
        .unwrap_err();
    let dup_owned = other.add_model_file((*shared).clone()).unwrap_err();
    assert_eq!(dup.to_string(), dup_owned.to_string());
    drop(shared);

    let texts = source.compact_model_asts().unwrap();
    assert_eq!(texts.len(), before.len());
    for ((text, mf), ast) in texts.iter().zip(source.model_files()).zip(&before) {
        assert_eq!(&**text, serde_json::to_string(ast).unwrap());
        assert_eq!(mf.ast(), ast);
    }
    let mut alone = manager();
    let texts = alone.compact_model_asts().unwrap();
    for (text, ast) in texts.iter().zip(&before) {
        assert_eq!(&**text, serde_json::to_string(ast).unwrap());
    }
    assert_eq!(
        alone
            .model_files()
            .map(|mf| mf.ast().clone())
            .collect::<Vec<_>>(),
        before
    );
}

/// TS: `Field.getDefaultValue` (src/introspect/field.ts) reads
/// `this.ast.defaultValue` straight off the raw AST, for every field kind
/// — including `DateTimeProperty`, which the official metamodel does not
/// declare a `defaultValue` field on at all (module doc on
/// [`ModelManager::property_default_value`]), so a typed per-variant read
/// would silently drop one. Covers a present string default, an absent
/// one, one explicitly `null` in the AST (also `None`, the same as
/// absent: `Field.getDefaultValue`'s own doc says falsy-but-not-`false`
/// values are still returned, but TS's `null` and `undefined` are
/// indistinguishable through a plain property read, and `filter(!is_null)`
/// is this port's chosen way to collapse the two), and a `DateTime`
/// field's, which is the case this method exists for.
#[test]
fn property_default_value_reads_the_raw_ast_including_datetime() {
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.example@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Order", "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "status",
                          "isArray": false, "isOptional": true, "defaultValue": "OPEN" },
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "note",
                          "isArray": false, "isOptional": true },
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "nulled",
                          "isArray": false, "isOptional": true, "defaultValue": null },
                        { "$class": "concerto.metamodel@1.0.0.DateTimeProperty", "name": "placedAt",
                          "isArray": false, "isOptional": true, "defaultValue": "2020-01-01T00:00:00.000Z" }
                      ] }
                ]
            }),
            None,
        )
        .unwrap();

    let prop = |name: &str| {
        mgr.find_property_id("org.example@1.0.0.Order", name)
            .unwrap()
            .unwrap_or_else(|| panic!("{name} not found"))
    };

    assert_eq!(
        mgr.property_default_value(prop("status")),
        Some(&crate::json!("OPEN"))
    );
    assert_eq!(mgr.property_default_value(prop("note")), None);
    assert_eq!(mgr.property_default_value(prop("nulled")), None);
    assert_eq!(
        mgr.property_default_value(prop("placedAt")),
        Some(&crate::json!("2020-01-01T00:00:00.000Z"))
    );
    assert!(
        mgr.property_default_value(PropId::from_index(u32::MAX))
            .is_none()
    );
}

/// TS: `getDirectSubclasses` builds its population from
/// `Introspector.getClassDeclarations()`, which reads
/// `modelManager.getModelFiles()` with no argument and so leaves out
/// every namespace in `EXCLUDE_NS` (src/basemodelmanager.ts). A fresh
/// manager has only those, so nothing directly extends the system root.
#[test]
#[allow(deprecated)]
fn direct_subclasses_of_a_fresh_manager_leave_out_the_system_models() {
    let mgr = ModelManager::new().unwrap();
    assert!(
        mgr.get_direct_subclasses("concerto@1.0.0.Concept")
            .unwrap()
            .is_empty()
    );
    assert!(
        mgr.get_direct_subclasses("concerto@1.0.0.Asset")
            .unwrap()
            .is_empty()
    );
}

/// Only the user declarations extend the system root, in registration
/// order: `Person` implicitly, and the enum `Color` implicitly too.
/// `Asset`, `Participant`, `Transaction`, `Event` and the decorator
/// model's own declarations are not in the population.
#[test]
#[allow(deprecated)]
fn direct_subclasses_are_only_user_declarations() {
    let mgr = manager();
    assert_eq!(
        mgr.get_direct_subclasses("concerto@1.0.0.Concept").unwrap(),
        ["org.example@1.0.0.Person", "org.example@1.0.0.Color"]
    );
    assert_eq!(
        mgr.get_direct_subclasses("org.example@1.0.0.Person")
            .unwrap(),
        ["org.example@1.0.0.Employee"]
    );
}

/// TS `collectSubclasses([this])` always adds the receiver itself, so a
/// fresh manager's `Concept` is assignable only from itself.
#[test]
#[allow(deprecated)]
fn assignable_class_declarations_of_a_fresh_manager_leave_out_the_system_models() {
    let mgr = ModelManager::new().unwrap();
    assert_eq!(
        mgr.get_assignable_class_declarations("concerto@1.0.0.Concept")
            .unwrap(),
        ["concerto@1.0.0.Concept"]
    );
}

#[test]
#[allow(deprecated)]
fn assignable_class_declarations_are_the_receiver_and_user_declarations() {
    let mgr = manager();
    assert_eq!(
        mgr.get_assignable_class_declarations("concerto@1.0.0.Concept")
            .unwrap(),
        [
            "concerto@1.0.0.Concept",
            "org.example@1.0.0.Person",
            "org.example@1.0.0.Employee",
            "org.example@1.0.0.Manager",
            "org.example@1.0.0.Color",
        ]
    );
}

/// A user asset that implicitly extends the system `Asset` is found; the
/// system root declarations themselves are not.
#[test]
#[allow(deprecated)]
fn a_user_asset_is_the_only_direct_subclass_of_asset() {
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "Car", "isAbstract": false,
                      "identified": { "$class": "concerto.metamodel@1.0.0.Identified" },
                      "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();
    assert_eq!(
        mgr.get_direct_subclasses("concerto@1.0.0.Asset").unwrap(),
        ["org.acme@1.0.0.Car"]
    );
    assert_eq!(
        mgr.get_assignable_class_declarations("concerto@1.0.0.Asset")
            .unwrap(),
        ["concerto@1.0.0.Asset", "org.acme@1.0.0.Car"]
    );
    assert!(
        mgr.get_direct_subclasses("concerto@1.0.0.Concept")
            .unwrap()
            .is_empty()
    );
}

/// BC-52: the subclass queries by handle answer from the cached
/// subclass map, and a model change (here an appended file) drops
/// it.
#[test]
fn subclass_queries_by_handle_follow_an_appended_file() {
    let mut mgr = manager();
    let person = mgr.declaration_id("org.example@1.0.0.Person").unwrap();
    let fqns = |mgr: &ModelManager, ids: &[DeclId]| -> Vec<String> {
        ids.iter()
            .map(|id| mgr.decl_fqn(*id).unwrap().to_string())
            .collect()
    };
    assert_eq!(
        fqns(&mgr, &mgr.assignable_ids(person).unwrap()),
        [
            "org.example@1.0.0.Person",
            "org.example@1.0.0.Employee",
            "org.example@1.0.0.Manager",
        ]
    );
    // Answered again from the cache.
    assert_eq!(
        fqns(&mgr, &mgr.direct_subclasses_of(person).unwrap()),
        ["org.example@1.0.0.Employee"]
    );
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.other@1.0.0",
                "imports": [{ "$class": "concerto.metamodel@1.0.0.ImportType",
                    "namespace": "org.example@1.0.0", "name": "Person" }],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Student",
                      "isAbstract": false, "properties": [],
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" } }
                ]
            }),
            None,
        )
        .unwrap();
    assert_eq!(
        fqns(&mgr, &mgr.direct_subclasses_of(person).unwrap()),
        ["org.example@1.0.0.Employee", "org.other@1.0.0.Student"]
    );
    assert_eq!(
        fqns(&mgr, &mgr.assignable_ids(person).unwrap()),
        [
            "org.example@1.0.0.Person",
            "org.example@1.0.0.Employee",
            "org.example@1.0.0.Manager",
            "org.other@1.0.0.Student",
        ]
    );
    #[allow(deprecated)]
    let by_name = mgr
        .get_assignable_class_declarations("org.example@1.0.0.Person")
        .unwrap();
    assert_eq!(by_name.len(), 4);
    assert!(mgr.assignable_ids(DeclId::from_index(100_000)).is_err());
    assert!(
        mgr.direct_subclasses_of(DeclId::from_index(100_000))
            .is_err()
    );
}

/// BC-52, BC-11: a cyclic chain below a declaration is the
/// `IllegalModelException` naming the cycle; its direct subclasses are
/// still found.
#[test]
fn assignable_ids_below_a_cyclic_chain_is_an_illegal_model_error() {
    let concept = |name: &str, sup: &str| {
        crate::json!({ "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": name, "isAbstract": false, "properties": [],
                "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": sup } })
    };
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
        &crate::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.cycle@1.0.0",
            "declarations": [concept("A", "C"), concept("B", "A"), concept("C", "B")]
        }),
        None,
    )
    .unwrap();
    let a = mgr.declaration_id("org.cycle@1.0.0.A").unwrap();
    let err = mgr.assignable_ids(a).unwrap_err();
    let c = err.contract();
    assert_eq!(c.kind, ErrorKind::IllegalModel);
    assert_eq!(c.code, "classdeclaration-circularinheritance");
    assert_eq!(
        c.message(),
        "The super type chain of \"org.cycle@1.0.0.A\" is circular: org.cycle@1.0.0.A -> org.cycle@1.0.0.C -> org.cycle@1.0.0.B -> org.cycle@1.0.0.A."
    );
    let direct = mgr.direct_subclasses_of(a).unwrap();
    assert_eq!(
        direct
            .iter()
            .map(|id| mgr.decl_fqn(*id).unwrap())
            .collect::<Vec<_>>(),
        ["org.cycle@1.0.0.B"]
    );
    assert!(mgr.assignable_types("org.cycle@1.0.0.A").is_err());
}

#[test]
fn preloads_system_model() {
    let mgr = ModelManager::new().unwrap();
    assert!(mgr.get_declaration("concerto@1.0.0.Concept").is_ok());
    assert!(mgr.get_declaration("concerto@1.0.0.Asset").is_ok());
}

/// TS: `ModelFile.fromAst` (src/introspect/modelfile.ts) defaults a
/// `superType`-less `AssetDeclaration` to `Asset` itself, not the generic
/// `Concept` `ClassDeclaration.process`'s own fallback gives a
/// `ConceptDeclaration` — so an asset with no explicit `extends` still
/// inherits `Asset`'s own `$identifier` even though it names its own
/// explicit identifier too (TS allows the redeclaration-looking overlap
/// here specifically because the two are never simultaneously in
/// `getProperties()`'s own duplicate-name check only when both are
/// literally named `$identifier`, which an explicit `identified by`
/// field never is).
#[test]
#[allow(deprecated)]
fn an_asset_with_no_extends_implicitly_extends_asset_itself() {
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme.defaults@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "DefaultAsset", "isAbstract": false,
                      "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "assetId" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "assetId", "isArray": false, "isOptional": false },
                        { "$class": "concerto.metamodel@1.0.0.DoubleProperty", "name": "value", "isArray": false, "isOptional": false }
                      ] }
                ]
            }),
            None,
        )
        .unwrap();
    assert_eq!(
        mgr.get_super_type("org.acme.defaults@1.0.0.DefaultAsset")
            .unwrap()
            .as_deref(),
        Some("concerto@1.0.0.Asset")
    );
    let props = mgr
        .properties("org.acme.defaults@1.0.0.DefaultAsset")
        .unwrap();
    let names: Vec<(&str, &str)> = props
        .iter()
        .map(|(owner, p)| (owner.as_str(), p.name()))
        .collect();
    assert_eq!(
        names,
        [
            ("org.acme.defaults@1.0.0.DefaultAsset", "assetId"),
            ("org.acme.defaults@1.0.0.DefaultAsset", "value"),
            ("concerto@1.0.0.Asset", "$identifier"),
        ]
    );
}

/// A fresh manager preloads `concerto.decorator@1.0.0` as
/// well as `concerto@1.0.0`, decorator model first,
/// matching TS's `addDecoratorModel(); addRootModel();`.
#[test]
fn preloads_decorator_model_before_root_model() {
    let mgr = ModelManager::new().unwrap();
    assert!(mgr.model_file("concerto.decorator@1.0.0").is_some());
    assert!(mgr.model_file("concerto@1.0.0").is_some());
    let namespaces: Vec<&str> = mgr.model_files().map(ModelFile::namespace).collect();
    assert_eq!(namespaces, ["concerto.decorator@1.0.0", "concerto@1.0.0"]);
}

/// `concerto.decorator@1.0.0.Decorator` and
/// `DotNetNamespace` resolve on a fresh manager.
#[test]
fn decorator_and_dot_net_namespace_resolve() {
    let mgr = ModelManager::new().unwrap();
    let decorator = mgr
        .get_declaration("concerto.decorator@1.0.0.Decorator")
        .unwrap();
    assert_eq!(decorator.name(), "Decorator");
    let dot_net_namespace = mgr
        .get_declaration("concerto.decorator@1.0.0.DotNetNamespace")
        .unwrap();
    assert_eq!(dot_net_namespace.name(), "DotNetNamespace");
}

/// A user model that imports
/// `concerto.decorator@1.0.0.Decorator` and extends it loads and
/// validates against the preloaded decorator model.
#[test]
fn user_model_extending_decorator_loads_and_validates() {
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "concerto.decorator@1.0.0", "name": "Decorator" }
                ],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "CustomDecorator",
                      "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Decorator" },
                      "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();

    assert!(
        mgr.get_declaration("org.acme@1.0.0.CustomDecorator")
            .is_ok()
    );
    assert!(
        mgr.is_assignable_to(
            "org.acme@1.0.0.CustomDecorator",
            "concerto.decorator@1.0.0.Decorator"
        )
        .unwrap()
    );
    assert!(mgr.validate_models().is_ok());
}

#[test]
fn duplicate_namespace_rejected() {
    let mut mgr = ModelManager::new().unwrap();
    let model = crate::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.x@1.0.0", "declarations": []
    });
    mgr.load_model(&model, None).unwrap();
    assert!(mgr.load_model(&model, None).is_err());
}

/// TS `_throwAlreadyExists`: a plain `Error`, never the `IllegalModel`
/// this port raised before — with both files' names in the message
/// when both have one.
#[test]
fn duplicate_namespace_names_both_files() {
    let mut mgr = ModelManager::new().unwrap();
    let model = crate::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.x@1.0.0", "declarations": []
    });
    mgr.load_model(&model, Some("old.cto".into())).unwrap();
    let err = mgr.load_model(&model, Some("new.cto".into())).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Namespace org.x@1.0.0 specified in file new.cto is already declared in file old.cto"
    );
}

/// Neither file has a name: both optional clauses drop out.
#[test]
fn duplicate_namespace_without_file_names() {
    let mut mgr = ModelManager::new().unwrap();
    let model = crate::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.x@1.0.0", "declarations": []
    });
    mgr.load_model(&model, None).unwrap();
    let err = mgr.load_model(&model, None).unwrap_err();
    assert_eq!(err.to_string(), "Namespace org.x@1.0.0 is already declared");
}

/// [`ModelManager::add_models`] hits the same duplicate-namespace check.
#[test]
fn add_models_rejects_a_duplicate_namespace_with_the_ts_message() {
    let mut mgr = ModelManager::new().unwrap();
    let model = crate::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.x@1.0.0", "declarations": []
    });
    mgr.load_model(&model, Some("old.cto".into())).unwrap();
    let err = mgr
        .load_models([(&model, Some("new.cto".to_string()))])
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "Namespace org.x@1.0.0 specified in file new.cto is already declared in file old.cto"
    );
}

#[test]
fn resolves_by_exact_fqn_only() {
    let mgr = manager();
    assert!(mgr.get_declaration("org.example@1.0.0.Person").is_ok());
    // versions are mandatory, so an unversioned lookup does not resolve
    assert!(mgr.get_declaration("org.example.Manager").is_err());
    assert!(mgr.get_declaration("org.example@1.0.0.Nope").is_err());
}

#[test]
fn collects_inherited_properties_in_order() {
    let mgr = manager();
    let props = mgr.properties("org.example@1.0.0.Manager").unwrap();
    let names: Vec<&str> = props.iter().map(|(_, p)| p.name()).collect();
    // Manager's own first, then Employee, then Person up the chain.
    assert_eq!(names, ["title", "salary", "name"]);
}

#[test]
fn assignability_follows_inheritance() {
    let mgr = manager();
    assert!(
        mgr.is_assignable_to("org.example@1.0.0.Manager", "org.example@1.0.0.Person")
            .unwrap()
    );
    assert!(
        mgr.is_assignable_to("org.example@1.0.0.Manager", "org.example@1.0.0.Manager")
            .unwrap()
    );
    assert!(
        !mgr.is_assignable_to("org.example@1.0.0.Person", "org.example@1.0.0.Manager")
            .unwrap()
    );
}

#[test]
fn unresolved_super_type_is_hard_error() {
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.broken@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Orphan", "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Ghost" },
                      "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();
    assert!(mgr.properties("org.broken@1.0.0.Orphan").is_err());
}

/// TS: `EnumDeclaration extends ClassDeclaration` inherits
/// `getProperties` unchanged, so an enum's values come back the same way
/// a class's fields do (`Concept` itself has no properties, so an
/// enum's `Color` has none to inherit).
#[test]
fn get_all_properties_on_enum_gives_its_values() {
    let mgr = manager();
    let properties = mgr.properties("org.example@1.0.0.Color").unwrap();
    let names: Vec<&str> = properties.iter().map(|(_, p)| p.name()).collect();
    assert_eq!(names, ["RED"]);
    assert_eq!(properties[0].0, "org.example@1.0.0.Color");
    assert!(properties[0].1.is_enum_value());
}

/// [`manager`] plus `org.other@1.0.0`, which imports from it (and from a
/// namespace that is not loaded) and declares a concept and a scalar.
fn manager_with_imports() -> ModelManager {
    let mut mgr = manager();
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.other@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportTypes",
                      "namespace": "org.example@1.0.0", "types": ["Person", "Manager", "Color"] },
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "org.missing@1.0.0", "name": "Ghost" }
                ],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Team", "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "lead", "isArray": false, "isOptional": false,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" } },
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "colour", "isArray": false, "isOptional": false,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Color" } },
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "label", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "Email" }
                ]
            }),
            None,
        )
        .unwrap();
    mgr
}

#[test]
fn get_ast_unresolved_leaves_type_names_bare() {
    let mgr = manager_with_imports();
    let ast = mgr.models_ast(false, false).unwrap();
    let models = ast.get("models").and_then(Value::as_array).unwrap();
    assert_eq!(models.len(), 2, "the system namespaces are excluded");
    let other = models
        .iter()
        .find(|m| m.get("namespace").and_then(Value::as_str) == Some("org.other@1.0.0"))
        .unwrap();
    let lead_type = other.pointer("/declarations/0/properties/0/type").unwrap();
    assert_eq!(
        lead_type.get("name").and_then(Value::as_str),
        Some("Person")
    );
    assert!(lead_type.get("namespace").is_none());
}

#[test]
fn get_ast_resolved_adds_the_declaring_namespace() {
    let mgr = manager_with_clean_import();
    let ast = mgr.models_ast(true, false).unwrap();
    let models = ast.get("models").and_then(Value::as_array).unwrap();
    let clean = models
        .iter()
        .find(|m| m.get("namespace").and_then(Value::as_str) == Some("org.clean@1.0.0"))
        .unwrap();
    // `Team.lead: Person` — imported (unaliased) from `org.example@1.0.0`.
    let lead_type = clean.pointer("/declarations/0/properties/0/type").unwrap();
    assert_eq!(
        lead_type.get("namespace").and_then(Value::as_str),
        Some("org.example@1.0.0")
    );
    assert_eq!(
        lead_type.get("name").and_then(Value::as_str),
        Some("Person")
    );
    assert!(lead_type.get("resolvedName").is_none());
    // `Team.label: String` — a primitive, untouched.
    let label = clean.pointer("/declarations/0/properties/1").unwrap();
    assert_eq!(label.get("namespace"), None);
}

/// `Email` (`org.clean@1.0.0`) is a `StringScalar`, whose own case
/// resolves `namespace`/`name` on the node itself, not under `.type`.
#[test]
fn get_ast_resolved_resolves_a_scalar_declarations_own_name() {
    let mgr = manager_with_clean_import();
    let resolved = mgr
        .resolve_meta_model(mgr.model_file("org.clean@1.0.0").unwrap().ast())
        .unwrap();
    let email = resolved
        .get("declarations")
        .and_then(Value::as_array)
        .unwrap()
        .iter()
        .find(|d| d.get("name").and_then(Value::as_str) == Some("Email"))
        .unwrap();
    assert_eq!(
        email.get("namespace").and_then(Value::as_str),
        Some("org.clean@1.0.0")
    );
}

/// `Manager` is imported from `org.example@1.0.0` via `ImportTypes`;
/// resolving a super type that names it adds that namespace.
#[test]
fn resolve_meta_model_resolves_an_imported_super_type() {
    let mgr = manager_with_imports();
    let model = crate::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.super@1.0.0",
        "imports": [
            { "$class": "concerto.metamodel@1.0.0.ImportType",
              "namespace": "org.example@1.0.0", "name": "Manager" }
        ],
        "declarations": [
            { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Lead", "isAbstract": false,
              "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Manager" },
              "properties": [] }
        ]
    });
    let resolved = mgr.resolve_meta_model(&model).unwrap();
    let super_type = resolved.pointer("/declarations/0/superType").unwrap();
    assert_eq!(
        super_type.get("namespace").and_then(Value::as_str),
        Some("org.example@1.0.0")
    );
}

/// TS `resolveName`: a plain `Error`, "Name {name} not found", for a
/// type that resolves to no import and no local declaration.
#[test]
fn resolve_meta_model_rejects_an_unresolvable_name() {
    let mgr = manager_with_imports();
    let model = crate::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.broken@1.0.0",
        "declarations": [
            { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Orphan", "isAbstract": false,
              "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Ghost" },
              "properties": [] }
        ]
    });
    let err = mgr.resolve_meta_model(&model).unwrap_err();
    assert_eq!(err.to_string(), "Name Ghost not found");
}

/// TS `createNameTable`'s `ImportType` branch: a plain `Error`, when the
/// imported declaration itself is not in the target namespace.
#[test]
fn resolve_meta_model_rejects_an_import_of_an_undeclared_type() {
    let mgr = manager_with_imports();
    let model = crate::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.broken@1.0.0",
        "imports": [
            { "$class": "concerto.metamodel@1.0.0.ImportType",
              "namespace": "org.example@1.0.0", "name": "Nope" }
        ],
        "declarations": []
    });
    let err = mgr.resolve_meta_model(&model).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Declaration Nope in namespace org.example@1.0.0 not found"
    );
}

/// TS's `createNameTable` only reads the target model inside
/// `imp.types.forEach`, so an `ImportTypes` of no types from a namespace
/// that is not registered resolves; one naming a type is a `TypeError`.
#[test]
fn resolve_meta_model_accepts_an_empty_import_types_from_an_unknown_namespace() {
    let mgr = manager();
    let model = |types: crate::json::Value| {
        crate::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.lonely@1.0.0",
            "imports": [
                { "$class": "concerto.metamodel@1.0.0.ImportTypes",
                  "namespace": "org.missing@1.0.0", "types": types }
            ],
            "declarations": []
        })
    };
    assert_eq!(
        mgr.resolve_meta_model(&model(crate::json!([]))).unwrap(),
        model(crate::json!([]))
    );
    let err = mgr
        .resolve_meta_model(&model(crate::json!(["Thing"])))
        .unwrap_err();
    assert!(matches!(
        Some(err.contract()),
        Some(c) if c.kind == ErrorKind::MalformedInput
    ));
}

/// [`manager`] plus `org.clean@1.0.0`, which imports `Person` from it
/// and declares a concept and a scalar — [`manager_with_imports`]
/// without its deliberately unresolvable import, for tests that resolve
/// a whole model's metamodel ([`ModelManager::resolve_meta_model`]
/// walks every import, not just the ones a lookup happens to reach).
fn manager_with_clean_import() -> ModelManager {
    let mut mgr = manager();
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.clean@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "org.example@1.0.0", "name": "Person" }
                ],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Team", "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "lead", "isArray": false, "isOptional": false,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Person" } },
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "label", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "Email" }
                ]
            }),
            None,
        )
        .unwrap();
    mgr
}

fn decl(mgr: &ModelManager, fqn: &str) -> Node {
    Node::Declaration(mgr.declaration_id(fqn).unwrap())
}

fn file_node(mgr: &ModelManager, namespace: &str) -> Node {
    Node::ModelFile(mgr.model_file_id(namespace).unwrap())
}

/// The property of a class declaration, by name.
fn prop(mgr: &ModelManager, fqn: &str, name: &str) -> Node {
    let id = mgr
        .property_ids(mgr.declaration_id(fqn).unwrap())
        .find(|&id| mgr.property_by_id(id).unwrap().name() == name)
        .unwrap();
    Node::Property(id)
}

#[test]
fn handles_survive_later_loads() {
    let mut mgr = manager();
    let person = mgr.declaration_id("org.example@1.0.0.Person").unwrap();
    let file = mgr.model_file_id("org.example@1.0.0").unwrap();
    let state_version = mgr.state_version();

    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.later@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Person", "isAbstract": false, "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();

    assert!(mgr.state_version() > state_version);
    assert_eq!(mgr.declaration_id("org.example@1.0.0.Person"), Some(person));
    assert_eq!(mgr.model_file_id("org.example@1.0.0"), Some(file));
    assert_eq!(mgr.model_file_of(person), Some(file));
    assert_ne!(mgr.declaration_id("org.later@1.0.0.Person"), Some(person));
    // A handle round-trips through its raw index, as a binding passes it.
    assert_eq!(DeclId::from_index(person.index()), person);
}

#[test]
fn a_failed_load_changes_nothing() {
    let mut mgr = manager();
    let state_version = mgr.state_version();
    let model = crate::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.example@1.0.0", "declarations": []
    });
    assert!(mgr.load_model(&model, None).is_err());
    assert_eq!(mgr.state_version(), state_version);
    // The two system models plus `org.example@1.0.0` from `manager()`.
    assert_eq!(mgr.model_files().count(), 3);
}

#[test]
fn walks_the_graph_by_handle() {
    let mgr = manager();
    let file = mgr.model_file_id("org.example@1.0.0").unwrap();
    let names: Vec<&str> = mgr
        .declaration_ids(file)
        .map(|id| mgr.declaration(id).unwrap().name())
        .collect();
    assert_eq!(names, ["Person", "Employee", "Manager", "Color"]);

    let employee = mgr.declaration_id("org.example@1.0.0.Employee").unwrap();
    let props: Vec<PropId> = mgr.property_ids(employee).collect();
    assert_eq!(props.len(), 1);
    assert_eq!(mgr.property_by_id(props[0]).unwrap().name(), "salary");
    assert_eq!(mgr.parent_of(props[0]), Some(employee));

    // An enum's own values get `PropId`s too, addressed the same way a
    // class declaration's fields are.
    let color = mgr.declaration_id("org.example@1.0.0.Color").unwrap();
    let color_props: Vec<PropId> = mgr.property_ids(color).collect();
    assert_eq!(color_props.len(), 1);
    assert_eq!(mgr.property_by_id(color_props[0]).unwrap().name(), "RED");
    assert!(mgr.property_by_id(color_props[0]).unwrap().is_enum_value());
    assert_eq!(mgr.parent_of(color_props[0]), Some(color));
    // Model files are listed in load order: the decorator model, then the
    // root model (matching TS's `addDecoratorModel(); addRootModel();`).
    let namespaces: Vec<&str> = mgr.model_files().map(ModelFile::namespace).collect();
    assert_eq!(
        namespaces,
        [
            "concerto.decorator@1.0.0",
            "concerto@1.0.0",
            "org.example@1.0.0"
        ]
    );
}

/// TS: `Introspector.getClassDeclarations` (test/introspect/introspector.js).
#[test]
fn class_declarations_span_every_loaded_model_file_and_include_enums() {
    let mgr = manager();
    let names: Vec<&str> = mgr
        .class_declarations()
        .map(|id| mgr.declaration(id).unwrap().name())
        .collect();
    // Every user declaration, including the enum `Color` — TS's
    // `!isMapDeclaration?.() && !isScalarDeclaration?.()` leaves an enum
    // in, only a map or scalar out.
    for name in ["Person", "Employee", "Manager", "Color"] {
        assert!(names.contains(&name), "{name} missing from {names:?}");
    }
    // `Introspector.getClassDeclarations` reads
    // `modelManager.getModelFiles()` with no argument, which leaves out
    // the built-in decorator and root models by namespace (`EXCLUDE_NS`,
    // src/basemodelmanager.ts) — so their own class-like declarations,
    // such as the root model's `Concept`, are not in the result.
    assert!(!names.contains(&"Concept"));
}

/// A map or scalar declaration is left out of `class_declarations`, the
/// same way `Introspector.getClassDeclarations` leaves them out of TS's
/// `instanceof ClassDeclaration` filter.
#[test]
fn class_declarations_exclude_maps_and_scalars() {
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
        &crate::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.mapscalar@1.0.0",
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "Postcode" },
                { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "Lookup",
                  "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                  "value": { "$class": "concerto.metamodel@1.0.0.StringMapValueType" } },
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Person",
                  "isAbstract": false, "properties": [] }
            ]
        }),
        None,
    )
    .unwrap();
    let names: Vec<&str> = mgr
        .class_declarations()
        .map(|id| mgr.declaration(id).unwrap().name())
        .filter(|n| ["Postcode", "Lookup", "Person"].contains(n))
        .collect();
    assert_eq!(names, ["Person"]);
}

/// `child@1.0.0.Child { o Integer age }`, imported into `parent@1.0.0` as
/// `Kid` (`import child@1.0.0.{Child as Kid}`); `parent@1.0.0`'s own
/// `Child` concept has a `kid` field of that aliased type. The TS
/// original (`test/introspect/property.js` "Property - Test for property
/// types using Import Aliasing"; `test/data/aliasing/*.cto`) builds this
/// over `ModelManager.resolveMetaModel`, which this port does not have
/// yet; this is the same shape built directly from AST, the only
/// difference this suite's own three assertions can see (none of them
/// reads a decorator or a resolved type reference, the only things
/// `resolveMetaModel` would add).
fn aliasing_manager() -> ModelManager {
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "child@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Child", "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "age", "isArray": false, "isOptional": false }
                      ] }
                ]
            }),
            None,
        )
        .unwrap();
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "parent@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "child@1.0.0",
                      "types": ["Child"],
                      "aliasedTypes": [
                        { "$class": "concerto.metamodel@1.0.0.AliasedType", "name": "Child", "aliasedName": "Kid" }
                      ] }
                ],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Child", "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "kid", "isArray": false, "isOptional": false,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Kid" } }
                      ] }
                ]
            }),
            None,
        )
        .unwrap();
    mgr
}

/// TS: `property.getType().should.equal('Kid')` — the alias, not the
/// target's own name, since `Property.process` keeps only `this.ast.type.name`
/// (property.ts).
#[test]
fn an_aliased_import_s_property_keeps_the_local_alias_as_its_type() {
    let mgr = aliasing_manager();
    let child = mgr.declaration_id("parent@1.0.0.Child").unwrap();
    let kid = mgr
        .property_ids(child)
        .find(|id| mgr.property_by_id(*id).unwrap().name() == "kid")
        .unwrap();
    assert_eq!(mgr.property_by_id(kid).unwrap().type_name(), Some("Kid"));
}

/// TS: `property.getFullyQualifiedTypeName().should.equal('child@1.0.0.Child')`
/// — resolved through the import alias to the type it actually names.
#[test]
fn an_aliased_import_s_property_resolves_its_fully_qualified_type_name() {
    let mgr = aliasing_manager();
    let child = mgr.declaration_id("parent@1.0.0.Child").unwrap();
    let kid = mgr
        .property_ids(child)
        .find(|id| mgr.property_by_id(*id).unwrap().name() == "kid")
        .unwrap();
    assert_eq!(
        mgr.get_fully_qualified_type_name(&Node::Property(kid))
            .unwrap(),
        "child@1.0.0.Child"
    );
}

/// TS: `property.getFullyQualifiedName().should.equal('parent@1.0.0.Child.kid')`.
#[test]
fn an_aliased_import_s_property_has_its_own_fully_qualified_name() {
    let mgr = aliasing_manager();
    let child = mgr.declaration_id("parent@1.0.0.Child").unwrap();
    let kid = mgr
        .property_ids(child)
        .find(|id| mgr.property_by_id(*id).unwrap().name() == "kid")
        .unwrap();
    assert_eq!(
        mgr.get_fully_qualified_name(&Node::Property(kid)).unwrap(),
        "parent@1.0.0.Child.kid"
    );
}

#[test]
fn unknown_handles_name_nothing() {
    let mgr = manager();
    let stale = DeclId::from_index(u32::MAX);
    assert!(mgr.declaration(stale).is_none());
    assert!(mgr.model_file_of(stale).is_none());
    assert_eq!(mgr.property_ids(stale).count(), 0);
    assert!(mgr.property_by_id(PropId::from_index(u32::MAX)).is_none());
    assert_eq!(
        mgr.declaration_ids(ModelFileId::from_index(u32::MAX))
            .count(),
        0
    );
    assert!(mgr.get_model_file(&Node::Declaration(stale)).is_err());
    assert!(mgr.is_enum(&Node::Declaration(stale)).is_err());
}

#[test]
fn get_type_follows_model_file_get_type() {
    let mgr = manager_with_imports();
    let example = file_node(&mgr, "org.example@1.0.0");
    let other = file_node(&mgr, "org.other@1.0.0");
    let person = decl(&mgr, "org.example@1.0.0.Person");

    assert_eq!(
        mgr.get_type(&example, Some("Person")).unwrap(),
        Some(person)
    );
    // `getLocalType` takes a name that already starts with the namespace.
    assert_eq!(
        mgr.get_type(&example, Some("org.example@1.0.0.Person"))
            .unwrap(),
        Some(person)
    );
    assert_eq!(
        mgr.get_type(&example, Some("String")).unwrap(),
        Some(Node::Primitive("String"))
    );
    assert_eq!(mgr.get_type(&example, Some("Nope")).unwrap(), None);
    assert_eq!(mgr.get_type(&example, None).unwrap(), None);
    // Imported, from a loaded namespace and from one that is not.
    assert_eq!(mgr.get_type(&other, Some("Person")).unwrap(), Some(person));
    assert_eq!(mgr.get_type(&other, Some("Ghost")).unwrap(), None);
    // The built-in import of the system types.
    assert_eq!(
        mgr.get_type(&other, Some("Concept")).unwrap(),
        Some(decl(&mgr, "concerto@1.0.0.Concept"))
    );
    // Employee is declared over there but not imported.
    assert_eq!(mgr.get_type(&other, Some("Employee")).unwrap(), None);

    let err = mgr.get_type(&person, Some("Person")).unwrap_err();
    assert_eq!(err.to_string(), "modelFile.getType is not a function");
}

#[test]
fn answers_the_collaborator_getters() {
    let mgr = manager_with_imports();
    let employee = decl(&mgr, "org.example@1.0.0.Employee");
    let salary = prop(&mgr, "org.example@1.0.0.Employee", "salary");
    let lead = prop(&mgr, "org.other@1.0.0.Team", "lead");

    assert_eq!(mgr.get_parent(&salary).unwrap(), employee);
    assert_eq!(
        mgr.get_model_file(&employee).unwrap(),
        file_node(&mgr, "org.example@1.0.0")
    );
    assert_eq!(
        mgr.get_fully_qualified_name(&employee).unwrap(),
        "org.example@1.0.0.Employee"
    );
    assert_eq!(
        mgr.get_fully_qualified_name(&salary).unwrap(),
        "org.example@1.0.0.Employee.salary"
    );
    assert_eq!(
        mgr.get_type_name(&salary).unwrap().as_deref(),
        Some("Double")
    );
    assert_eq!(mgr.get_type_name(&lead).unwrap().as_deref(), Some("Person"));
    assert_eq!(
        mgr.get_fully_qualified_type_name(&salary).unwrap(),
        "Double"
    );
    assert_eq!(
        mgr.get_fully_qualified_type_name(&lead).unwrap(),
        "org.example@1.0.0.Person"
    );
    assert_eq!(
        mgr.get_ast_class(&employee).unwrap().as_deref(),
        Some("concerto.metamodel@1.0.0.ConceptDeclaration")
    );
    assert_eq!(
        mgr.get_ast_class(&salary).unwrap().as_deref(),
        Some("concerto.metamodel@1.0.0.DoubleProperty")
    );
    let supers: Vec<String> = mgr
        .get_all_super_type_declarations(&decl(&mgr, "org.example@1.0.0.Manager"))
        .unwrap()
        .iter()
        .map(|node| mgr.get_fully_qualified_name(node).unwrap())
        .collect();
    assert_eq!(
        supers,
        [
            "org.example@1.0.0.Employee",
            "org.example@1.0.0.Person",
            // `Person` has no `superType` of its own, so it implicitly
            // extends `Concept` (`ClassDeclaration` doc comment).
            "concerto@1.0.0.Concept"
        ]
    );
    let declarations = mgr
        .get_all_declarations(&file_node(&mgr, "org.other@1.0.0"))
        .unwrap();
    assert_eq!(
        declarations,
        [
            decl(&mgr, "org.other@1.0.0.Team"),
            decl(&mgr, "org.other@1.0.0.Email")
        ]
    );
}

#[test]
fn a_primitive_type_answers_as_a_js_string_does() {
    let mgr = manager();
    let string = Node::Primitive("String");
    assert_eq!(
        mgr.is_enum(&string).unwrap_err().to_string(),
        "typeDeclaration.isEnum is not a function"
    );
    assert_eq!(mgr.is_map_declaration(&string).unwrap(), None);
    assert_eq!(mgr.is_scalar_declaration(&string).unwrap(), None);
    assert_eq!(
        mgr.get_ast_class(&string).unwrap_err().to_string(),
        "Cannot read properties of undefined (reading '$class')"
    );
}

#[test]
fn ported_members_run_on_the_arena() {
    use crate::introspect::ScalarDeclaration;
    use crate::model_util;

    let mgr = manager_with_imports();
    let other = file_node(&mgr, "org.other@1.0.0");
    let lead = prop(&mgr, "org.other@1.0.0.Team", "lead");
    let colour = prop(&mgr, "org.other@1.0.0.Team", "colour");
    let label = prop(&mgr, "org.other@1.0.0.Team", "label");

    assert!(model_util::is_assignable_to(&mgr, &other, "Manager", &lead).unwrap());
    assert!(!model_util::is_assignable_to(&mgr, &other, "Color", &lead).unwrap());
    // Imported from a namespace that is not loaded: `getType` finds nothing.
    let err = model_util::is_assignable_to(&mgr, &other, "Ghost", &lead).unwrap_err();
    assert!(err.to_string().contains("Ghost"), "{err}");

    assert_eq!(model_util::is_enum(&mgr, &colour).unwrap(), Some(true));
    assert_eq!(model_util::is_enum(&mgr, &lead).unwrap(), Some(false));
    assert_eq!(model_util::is_map(&mgr, &colour).unwrap(), Some(false));
    assert_eq!(model_util::is_scalar(&mgr, &lead).unwrap(), Some(false));
    // `modelFile.getType('String')` is the string itself.
    assert!(model_util::is_enum(&mgr, &label).is_err());
    assert_eq!(model_util::is_scalar(&mgr, &label).unwrap(), None);

    let email = decl(&mgr, "org.other@1.0.0.Email");
    assert_eq!(
        model_util::is_valid_map_key_scalar(&mgr, Some(&email)).unwrap(),
        Some(true)
    );
    assert!(ScalarDeclaration::validate(&mgr, &email).is_ok());
}

/// PORTING.md 2.1: `location` is copied verbatim from the AST node the
/// caller passes, never recomputed and never hard-coded to `None`.
#[test]
fn resolve_type_name_carries_the_given_location_verbatim() {
    let mgr = ModelManager::new().unwrap();
    let location = crate::json!({
        "start": {"line": 3, "column": 1, "offset": 20},
        "end": {"line": 3, "column": 9, "offset": 28}
    });
    let err = mgr
        .resolve_type_name_at("org.does.not.exist@1.0.0", "Foo", Some(location.clone()))
        .unwrap_err();
    match Some(err.into_contract()) {
        Some(contract) => assert_eq!(contract.location, Some(location)),
        other => panic!("expected a Contract error, got {other:?}"),
    }
}

#[test]
fn resolve_type_name_with_no_location_carries_none() {
    let mgr = ModelManager::new().unwrap();
    let err = mgr
        .resolve_type_name_at("org.does.not.exist@1.0.0", "Foo", None)
        .unwrap_err();
    match Some(err.into_contract()) {
        Some(contract) => assert_eq!(contract.location, None),
        other => panic!("expected a Contract error, got {other:?}"),
    }
}

/// `org.base@1.0.0.Base`, and `org.dependent@1.0.0.Sub`, which extends it.
/// Loading `Sub` before `Base` with a single [`ModelManager::add_model`]
/// succeeds too (loading never validates on its own), but validating the
/// pair only succeeds once both are loaded, whatever order they loaded in;
/// [`ModelManager::add_models`] is what does both steps as one
/// all-or-nothing unit.
fn base_model() -> crate::json::Value {
    crate::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.base@1.0.0",
        "declarations": [
            { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Base", "isAbstract": false, "properties": [] }
        ]
    })
}

fn dependent_model() -> crate::json::Value {
    crate::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.dependent@1.0.0",
        "imports": [
            { "$class": "concerto.metamodel@1.0.0.ImportType",
              "namespace": "org.base@1.0.0", "name": "Base" }
        ],
        "declarations": [
            { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Sub", "isAbstract": false,
              "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Base" },
              "properties": [] }
        ]
    })
}

#[test]
fn add_models_relaxes_import_order() {
    let base = base_model();
    let dependent = dependent_model();

    // The dependency-last order would defeat a single `add_model` per
    // file followed by eager per-file validation; `add_models` adds both
    // first and validates once, so the order they are listed in does not
    // matter.
    let mut mgr = ModelManager::new().unwrap();
    let ids = mgr
        .load_models([(&dependent, None), (&base, None)])
        .unwrap();
    assert_eq!(ids.len(), 2);
    assert!(mgr.validate_models().is_ok());
    assert!(
        mgr.is_assignable_to("org.dependent@1.0.0.Sub", "org.base@1.0.0.Base")
            .unwrap()
    );

    // The reverse order validates just as cleanly.
    let mut mgr2 = ModelManager::new().unwrap();
    mgr2.load_models([(&base, None), (&dependent, None)])
        .unwrap();
    assert!(mgr2.validate_models().is_ok());
}

#[test]
fn add_models_rolls_back_the_whole_batch_on_validation_failure() {
    let mut mgr = manager();
    let state_version = mgr.state_version();
    let namespaces_before: Vec<String> = mgr
        .model_files()
        .map(|mf| mf.namespace().to_string())
        .collect();

    // `Sub` extends a `Base` that is never part of this batch, so the
    // batch-wide `validate_models` fails; the dependent model on its own
    // is otherwise well formed, so only the missing super type is at
    // fault.
    let dependent = dependent_model();
    let err = mgr.load_models([(&dependent, None)]).unwrap_err();
    assert!(err.to_string().contains("Base"), "{err}");

    // Nothing from the failed batch survives: not the new namespace, not
    // the state version, not the arena length.
    assert_eq!(mgr.state_version(), state_version);
    assert_eq!(mgr.model_file_id("org.dependent@1.0.0"), None);
    let namespaces_after: Vec<String> = mgr
        .model_files()
        .map(|mf| mf.namespace().to_string())
        .collect();
    assert_eq!(namespaces_after, namespaces_before);
}

#[test]
fn add_models_rolls_back_on_duplicate_namespace_within_the_batch() {
    let mut mgr = ModelManager::new().unwrap();
    let state_version = mgr.state_version();
    let base = base_model();

    assert!(mgr.load_models([(&base, None), (&base, None)]).is_err());

    assert_eq!(mgr.state_version(), state_version);
    assert_eq!(mgr.model_file_id("org.base@1.0.0"), None);
}

#[test]
fn add_models_rolls_back_on_duplicate_against_an_existing_model() {
    let mut mgr = manager();
    let state_version = mgr.state_version();
    let count_before = mgr.model_files().count();

    let clash = crate::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": "org.example@1.0.0", "declarations": []
    });
    let other = base_model();
    // The clash is listed second, so the first model in the batch does
    // get inserted before the failure — and must be undone too.
    assert!(mgr.load_models([(&other, None), (&clash, None)]).is_err());

    assert_eq!(mgr.state_version(), state_version);
    assert_eq!(mgr.model_files().count(), count_before);
    assert_eq!(mgr.model_file_id("org.base@1.0.0"), None);
}

#[test]
fn add_models_leaves_pre_existing_models_validating_on_success() {
    let mut mgr = manager();
    let base = base_model();
    let dependent = dependent_model();
    mgr.load_models([(&dependent, None), (&base, None)])
        .unwrap();
    // The pre-existing models (from `manager()`) are still there and
    // still validate, alongside the two the batch added.
    assert!(mgr.get_declaration("org.example@1.0.0.Manager").is_ok());
    assert!(mgr.validate_models().is_ok());
}

// BaseModelManager.resolveType, derivesFrom, isAssignableTo,
// getAssignableConcreteTypes, getModels, the get<Kind>Declarations
// family, filter, updateModelFile and deleteModelFile.

#[test]
fn resolve_type_passes_primitives_through() {
    let mgr = manager();
    assert_eq!(
        mgr.resolve_type("ctx", "String").unwrap(),
        "String".to_string()
    );
}

#[test]
fn resolve_type_resolves_a_local_type() {
    let mgr = manager();
    assert_eq!(
        mgr.resolve_type("ctx", "org.example@1.0.0.Employee")
            .unwrap(),
        "org.example@1.0.0.Employee"
    );
}

#[test]
fn resolve_type_rejects_an_unregistered_namespace() {
    let mgr = manager();
    let err = mgr
        .resolve_type("org.example@1.0.0.Person", "org.nope@1.0.0.Foo")
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "No registered namespace for type \"org.nope@1.0.0.Foo\" in \"org.example@1.0.0.Person\"."
    );
}

#[test]
fn resolve_type_rejects_an_imported_name() {
    let mgr = manager_with_imports();
    // `Person` is imported into `org.other@1.0.0`, not declared there.
    let err = mgr
        .resolve_type("ctx", "org.other@1.0.0.Person")
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "No type \"org.other@1.0.0.Person\" in namespace \"org.other@1.0.0\" for \"ctx\"."
    );
}

#[test]
fn derives_from_is_true_for_the_same_type() {
    let mgr = manager();
    assert!(
        mgr.derives_from("org.example@1.0.0.Employee", "org.example@1.0.0.Employee")
            .unwrap()
    );
}

#[test]
fn derives_from_walks_the_super_chain() {
    let mgr = manager();
    assert!(
        mgr.derives_from("org.example@1.0.0.Manager", "org.example@1.0.0.Person")
            .unwrap()
    );
    assert!(
        mgr.derives_from("org.example@1.0.0.Manager", "concerto@1.0.0.Concept")
            .unwrap()
    );
}

#[test]
fn derives_from_is_false_for_the_wrong_direction() {
    let mgr = manager();
    assert!(
        !mgr.derives_from("org.example@1.0.0.Person", "org.example@1.0.0.Manager")
            .unwrap()
    );
}

#[test]
fn derives_from_propagates_gettype_s_error() {
    let mgr = manager();
    assert!(
        mgr.derives_from("org.example@1.0.0.Nope", "org.example@1.0.0.Person")
            .is_err()
    );
}

/// TS 5.0.0 `derivesFrom('test@1.0.0.Color',
/// 'concerto@1.0.0.Concept')` is `true` for an enum — `EnumDeclaration
/// extends ClassDeclaration`, so `getSuperTypeDeclaration()` gives its
/// implicit `Concept` — and so is `isAssignableTo`. A scalar has no
/// super type at all (`false`).
#[test]
fn an_enum_derives_from_its_implicit_concept_super_type() {
    let mut mgr = manager();
    mgr.load_model(
        &crate::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.scalar@1.0.0",
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "SSN" }
            ]
        }),
        None,
    )
    .unwrap();
    let color = "org.example@1.0.0.Color";
    let concept = "concerto@1.0.0.Concept";
    assert!(mgr.derives_from(color, concept).unwrap());
    assert!(mgr.is_assignable_to(color, concept).unwrap());
    assert!(mgr.is_type_assignable_to(color, concept));
    assert!(!mgr.derives_from(color, "org.example@1.0.0.Person").unwrap());
    assert!(!mgr.derives_from("org.scalar@1.0.0.SSN", concept).unwrap());
}

/// DV-022 (maintainer-accepted): TS 5.0.0 `derivesFrom(map, other)`
/// throws a `TypeError` (`typeDeclaration.getSuperTypeDeclaration is not
/// a function`: `MapDeclaration` has none), and `isAssignableTo` with a
/// map `fqn` always throws one (`typeDeclaration.isAbstract is not a
/// function`). The engine answers instead: a map derives from, and is
/// assignable to, only itself. `derivesFrom(map, map)` is `true` in TS
/// too (the exact-name check comes before the walk).
///
/// A scalar is not part of DV-022: TS answers without throwing
/// (`getSuperTypeDeclaration()` is `null`, `isAbstract()` is `true`),
/// and the engine matches it, `isAssignableTo(scalar, scalar)` `false`
/// included.
#[test]
fn a_map_derives_only_from_itself_and_a_scalar_is_never_assignable() {
    let mut mgr = manager();
    mgr.load_model(
        &crate::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.dv022@1.0.0",
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "Lookup",
                  "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                  "value": { "$class": "concerto.metamodel@1.0.0.StringMapValueType" } },
                { "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "SSN" },
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Person",
                  "isAbstract": false, "properties": [] }
            ]
        }),
        None,
    )
    .unwrap();
    let map = "org.dv022@1.0.0.Lookup";
    let scalar = "org.dv022@1.0.0.SSN";
    let person = "org.dv022@1.0.0.Person";
    let concept = "concerto@1.0.0.Concept";

    // Map (DV-022): TS throws a TypeError for each `false` here and for
    // every `is_type_assignable_to`.
    assert!(!mgr.derives_from(map, concept).unwrap());
    assert!(!mgr.derives_from(map, person).unwrap());
    assert!(mgr.derives_from(map, map).unwrap());
    assert!(!mgr.is_type_assignable_to(map, concept));
    assert!(!mgr.is_type_assignable_to(map, person));
    assert!(mgr.is_type_assignable_to(map, map));

    // Scalar: TS parity.
    assert!(!mgr.derives_from(scalar, concept).unwrap());
    assert!(mgr.derives_from(scalar, scalar).unwrap());
    assert!(!mgr.is_type_assignable_to(scalar, concept));
    assert!(!mgr.is_type_assignable_to(scalar, scalar));
}

fn concept_with(name: &str, super_type: Option<&str>, properties: Value) -> Value {
    let mut decl = crate::json!({
        "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
        "name": name, "isAbstract": false, "properties": properties
    });
    if let Some(super_type) = super_type {
        decl["superType"] = crate::json!({
            "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": super_type
        });
    }
    decl
}

fn model(namespace: &str, imports: Value, declarations: Value) -> Value {
    crate::json!({
        "$class": "concerto.metamodel@1.0.0.Model",
        "namespace": namespace,
        "imports": imports,
        "declarations": declarations
    })
}

fn string_property(name: &str) -> Value {
    crate::json!({
        "$class": "concerto.metamodel@1.0.0.StringProperty",
        "name": name, "isArray": false, "isOptional": false
    })
}

fn object_property(name: &str, type_name: &str) -> Value {
    crate::json!({
        "$class": "concerto.metamodel@1.0.0.ObjectProperty",
        "name": name, "isArray": false, "isOptional": false,
        "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": type_name }
    })
}

fn import_type(namespace: &str, name: &str) -> Value {
    crate::json!({
        "$class": "concerto.metamodel@1.0.0.ImportType", "namespace": namespace, "name": name
    })
}

/// `c@1.0.0.Bar extends Foo { o Foo f }`'s super type, its inherited
/// properties' names, and what its field's type `Foo` resolves to, by
/// both the field-type resolvers (`getFullyQualifiedTypeName`, and the
/// lookup the validator and `resolveType` use).
fn bar_resolution(mgr: &ModelManager, short: &str) -> (String, Vec<String>, String, String) {
    let (super_fqn, _) = mgr.super_type("c@1.0.0.Bar").unwrap().unwrap();
    let names = mgr
        .properties("c@1.0.0.Bar")
        .unwrap()
        .into_iter()
        .map(|(_, p)| p.name().to_string())
        .collect();
    let field = mgr
        .model_file_fully_qualified_type_name("c@1.0.0", short)
        .unwrap();
    let resolved = mgr.resolve_type_name("c@1.0.0", short).unwrap();
    (super_fqn, names, field, resolved)
}

/// Two imports of the same local name — the last one
/// wins for `extends` and for a field type alike, as TS 5.0.0's
/// `importShortNames` `Map.set` does (TS: super type `b@1.0.0.Foo`,
/// properties `[f, b]`, field type `b@1.0.0.Foo`).
#[test]
fn the_last_of_two_imports_of_one_name_wins_for_extends_and_field_types() {
    let mut mgr = ModelManager::new().unwrap();
    for m in [
        model(
            "a@1.0.0",
            crate::json!([]),
            crate::json!([concept_with(
                "Foo",
                None,
                crate::json!([string_property("a")])
            )]),
        ),
        model(
            "b@1.0.0",
            crate::json!([]),
            crate::json!([concept_with(
                "Foo",
                None,
                crate::json!([string_property("b")])
            )]),
        ),
        model(
            "c@1.0.0",
            crate::json!([import_type("a@1.0.0", "Foo"), import_type("b@1.0.0", "Foo")]),
            crate::json!([concept_with(
                "Bar",
                Some("Foo"),
                crate::json!([object_property("f", "Foo")])
            )]),
        ),
    ] {
        mgr.load_model(&m, None).unwrap();
    }
    let (super_fqn, names, field, resolved) = bar_resolution(&mgr, "Foo");
    assert_eq!(super_fqn, "b@1.0.0.Foo");
    assert_eq!(names, ["f", "b"]);
    assert_eq!(field, "b@1.0.0.Foo");
    assert_eq!(resolved, "b@1.0.0.Foo");
    assert!(mgr.derives_from("c@1.0.0.Bar", "b@1.0.0.Foo").unwrap());
    assert!(!mgr.derives_from("c@1.0.0.Bar", "a@1.0.0.Foo").unwrap());
}

/// A user import of a system type name. The built-in
/// import `fromAst` appends comes last, so it wins for `extends` and
/// for a field type alike (TS 5.0.0, with
/// `dangerouslyAllowReservedSystemTypeNamesInUserModels`: super type
/// `concerto@1.0.0.Concept`, properties `[f]`, field type
/// `concerto@1.0.0.Concept`).
#[test]
fn the_built_in_import_wins_over_a_user_import_of_a_system_name() {
    let mut mgr = ModelManager::new().unwrap();
    mgr.set_dangerously_allow_reserved_system_type_names_in_user_models(true);
    for m in [
        model(
            "x@1.0.0",
            crate::json!([]),
            crate::json!([concept_with(
                "Concept",
                None,
                crate::json!([string_property("x")])
            )]),
        ),
        model(
            "c@1.0.0",
            crate::json!([import_type("x@1.0.0", "Concept")]),
            crate::json!([concept_with(
                "Bar",
                Some("Concept"),
                crate::json!([object_property("f", "Concept")])
            )]),
        ),
    ] {
        mgr.load_model(&m, None).unwrap();
    }
    let (super_fqn, names, field, resolved) = bar_resolution(&mgr, "Concept");
    assert_eq!(super_fqn, "concerto@1.0.0.Concept");
    assert_eq!(names, ["f"]);
    assert_eq!(field, "concerto@1.0.0.Concept");
    assert_eq!(resolved, "concerto@1.0.0.Concept");
}

#[test]
fn base_manager_is_assignable_to_matches_ts_including_the_abstract_check() {
    let mut mgr = manager();
    // Person has no explicit `isAbstract`; make an abstract type to
    // exercise the "false even against itself" branch TS's own test
    // covers (`isAssignableTo should return false when fqn is abstract`).
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.abs@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Base", "isAbstract": true, "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();

    assert!(!mgr.is_type_assignable_to("org.abs@1.0.0.Base", "org.abs@1.0.0.Base"));
    assert!(mgr.is_type_assignable_to("org.example@1.0.0.Employee", "org.example@1.0.0.Employee"));
    assert!(mgr.is_type_assignable_to("org.example@1.0.0.Employee", "org.example@1.0.0.Person"));
    assert!(!mgr.is_type_assignable_to("org.example@1.0.0.Person", "org.example@1.0.0.Employee"));
    assert!(!mgr.is_type_assignable_to("org.example@1.0.0.Nope", "org.example@1.0.0.Person"));
}

#[test]
fn get_assignable_concrete_types_leaves_out_the_abstract_base_and_absent_types() {
    let mut mgr = manager();
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.abs2@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Base", "isAbstract": true, "properties": [] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Child", "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Base" }, "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();

    let names: Vec<String> = mgr
        .get_assignable_concrete_types("org.abs2@1.0.0.Base")
        .into_iter()
        .filter_map(|id| mgr.declaration(id).map(|d| d.name().to_string()))
        .collect();
    assert_eq!(names, vec!["Child".to_string()]);
    assert!(
        mgr.get_assignable_concrete_types("org.abs2@1.0.0.Nope")
            .is_empty()
    );
}

#[test]
fn get_models_excludes_system_and_decorator_models() {
    let mgr = manager();
    let models = mgr.get_models(true);
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].0, "org.example@1.0.0.cto");
}

#[test]
fn get_models_names_a_file_from_its_file_name() {
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
        &crate::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.named@1.0.0", "declarations": []
        }),
        Some("https://example.org/models/".to_string()),
    )
    .unwrap();
    let models = mgr.get_models(true);
    assert_eq!(models, vec![("models".to_string(), None)]);
}

#[test]
fn model_file_by_file_name_finds_the_matching_file() {
    let mut mgr = ModelManager::new().unwrap();
    mgr.add_model_with_definitions(
        &crate::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.named@1.0.0", "declarations": []
        }),
        None,
        Some("models/org.named.cto".to_string()),
    )
    .unwrap();
    let found = mgr
        .model_file_by_file_name("models/org.named.cto")
        .expect("a file was registered under this name");
    assert_eq!(found.namespace(), "org.named@1.0.0");
}

#[test]
fn model_file_by_file_name_is_none_when_nothing_matches() {
    let mgr = manager();
    assert!(mgr.model_file_by_file_name("no-such-file.cto").is_none());
}

/// TS `getModelFileByFileName` calls `getModelFiles()` with no
/// argument, which excludes the built-in decorator and root models
/// (`EXCLUDE_NS`). So even though those files are registered under
/// exactly these names (`ModelManager::new`), looking either of them
/// up by file name must answer `None` (JS `undefined`), the same as
/// TS, not the system `ModelFile`.
#[test]
fn model_file_by_file_name_excludes_the_system_model_files() {
    let mgr = manager();
    assert!(mgr.model_file_by_file_name("concerto_1.0.0.cto").is_none());
    assert!(
        mgr.model_file_by_file_name("concerto_decorator_1.0.0.cto")
            .is_none()
    );
}

/// TS `getModelFileByFileName(undefined)` returns the first loaded
/// model file whose `getName()` is `undefined`, i.e. one added with no
/// file name; a named file never matches.
#[test]
fn model_file_by_optional_file_name_none_finds_the_unnamed_file() {
    let mut mgr = ModelManager::new().unwrap();
    assert!(mgr.model_file_by_optional_file_name(None).is_none());
    for (ns, name) in [
        ("org.named@1.0.0", Some("named.cto".to_string())),
        ("org.unnamed@1.0.0", None),
        ("org.unnamed2@1.0.0", None),
    ] {
        mgr.add_model_with_definitions(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": ns, "declarations": []
            }),
            None,
            name,
        )
        .unwrap();
    }
    assert_eq!(
        mgr.model_file_by_optional_file_name(None)
            .map(ModelFile::namespace),
        Some("org.unnamed@1.0.0")
    );
    assert_eq!(
        mgr.model_file_by_optional_file_name(Some("named.cto"))
            .map(ModelFile::namespace),
        Some("org.named@1.0.0")
    );
}

#[test]
fn get_concept_declarations_excludes_the_system_concepts() {
    let mgr = manager();
    let names: Vec<String> = mgr
        .get_concept_declarations()
        .into_iter()
        .filter_map(|id| mgr.declaration(id).map(|d| d.name().to_string()))
        .collect();
    assert_eq!(
        names,
        vec![
            "Person".to_string(),
            "Employee".to_string(),
            "Manager".to_string()
        ]
    );
}

#[test]
fn filter_keeps_only_matching_declarations_and_the_system_models() {
    let mgr = manager();
    let kept = mgr
        .filter_by_fqn(|fqn| fqn == "org.example@1.0.0.Person", false)
        .unwrap();
    assert!(kept.model_file("org.example@1.0.0").is_some());
    assert!(kept.get_declaration("org.example@1.0.0.Person").is_ok());
    assert!(kept.get_declaration("org.example@1.0.0.Employee").is_err());
    assert!(kept.model_file("concerto@1.0.0").is_some());
    assert!(kept.model_file("concerto.decorator@1.0.0").is_some());
}

#[test]
fn filter_drops_a_file_left_with_no_declarations() {
    let mgr = manager();
    let kept = mgr.filter_by_fqn(|_| false, true).unwrap();
    assert!(kept.model_file("org.example@1.0.0").is_none());
}

/// A manager holding `manager()`'s model and a user model that imports
/// and extends `concerto.decorator@1.0.0.Decorator`.
fn manager_with_decorator_subtype() -> ModelManager {
    let mut mgr = manager();
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "concerto.decorator@1.0.0", "name": "Decorator" }
                ],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "CustomDecorator",
                      "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Decorator" },
                      "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();
    mgr.validate_models().unwrap();
    mgr
}

/// Each model file's namespace and declaration names, in load order.
fn shape(mgr: &ModelManager) -> Vec<(String, Vec<String>)> {
    mgr.model_files()
        .map(|mf| {
            (
                mf.namespace().to_string(),
                mf.declarations()
                    .iter()
                    .map(|d| d.name().to_string())
                    .collect(),
            )
        })
        .collect()
}

/// BC-53: `filter` keeping every declaration returns a manager with the
/// source's namespaces, declarations and AST. TS 5.0.0 re-added the
/// decorator model and threw.
#[test]
fn filter_keeping_everything_round_trips() {
    let mgr = manager_with_decorator_subtype();
    let filtered = mgr.filter(|_, _| true).unwrap();
    assert_eq!(shape(&filtered), shape(&mgr));
    let all = AstOptions {
        resolve: false,
        include_system_models: true,
    };
    assert_eq!(filtered.ast(all).unwrap(), mgr.ast(all).unwrap());
    assert!(filtered.validate_models().is_ok());
    let by_fqn = mgr.filter_by_fqn(|_| true, false).unwrap();
    assert_eq!(shape(&by_fqn), shape(&mgr));
}

/// BC-53: a predicate that keeps a user type extending `Decorator` works,
/// and the result resolves it against its own decorator model.
#[test]
fn filter_keeps_a_user_type_extending_decorator() {
    let mgr = manager_with_decorator_subtype();
    let filtered = mgr
        .filter_by_fqn(|fqn| !fqn.starts_with("org.example@"), false)
        .unwrap();
    assert!(filtered.model_file("org.example@1.0.0").is_none());
    assert!(
        filtered
            .is_assignable_to(
                "org.acme@1.0.0.CustomDecorator",
                "concerto.decorator@1.0.0.Decorator"
            )
            .unwrap()
    );
    assert!(filtered.validate_models().is_ok());

    // A predicate that drops the decorator model's own declarations
    // (as a benchmark once did) keeps the user's import of `Decorator`
    // too: the decorator model stays whole in the result, and the
    // predicate is never asked about its declarations.
    let asked = std::cell::RefCell::new(Vec::new());
    let workaround = mgr
        .filter_by_fqn(
            |fqn| {
                asked.borrow_mut().push(fqn.to_string());
                !fqn.starts_with("concerto.decorator@")
            },
            false,
        )
        .unwrap();
    assert!(
        asked
            .borrow()
            .iter()
            .all(|fqn| !fqn.starts_with("concerto.decorator@") && !fqn.starts_with("concerto@"))
    );
    assert!(
        workaround
            .is_assignable_to(
                "org.acme@1.0.0.CustomDecorator",
                "concerto.decorator@1.0.0.Decorator"
            )
            .unwrap()
    );
    assert_eq!(shape(&workaround), shape(&mgr));
    assert!(workaround.validate_models().is_ok());
}

/// BC-53: the built-in models are kept whole whatever the predicate says
/// about their declarations, so a predicate dropping everything returns
/// just those, and one keeping only the decorator model's own
/// declarations returns the same.
#[test]
fn filter_dropping_everything_keeps_only_the_built_in_models() {
    let mgr = manager_with_decorator_subtype();
    let built_in = shape(&ModelManager::new().unwrap());
    assert_eq!(
        built_in
            .iter()
            .map(|(ns, _)| ns.as_str())
            .collect::<Vec<_>>(),
        vec!["concerto.decorator@1.0.0", "concerto@1.0.0"]
    );
    let none = mgr.filter(|_, _| false).unwrap();
    assert_eq!(shape(&none), built_in);
    let decorator_only = mgr
        .filter_by_fqn(|fqn| fqn.starts_with("concerto.decorator@"), false)
        .unwrap();
    assert_eq!(shape(&decorator_only), built_in);
    assert_eq!(
        shape(&mgr.filter_by_fqn(|_| false, true).unwrap()),
        built_in
    );
}

#[test]
fn update_model_file_replaces_the_registered_file() {
    let mgr = manager();
    let replacement = ModelFile::from_json(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.example@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Only", "isAbstract": false, "properties": [] }
                ]
            }),
            None,
        )
        .unwrap();
    let updated = mgr.update_model_file(replacement, true).unwrap();
    assert!(updated.get_declaration("org.example@1.0.0.Only").is_ok());
    assert!(updated.get_declaration("org.example@1.0.0.Person").is_err());
    // `mgr` itself is untouched.
    assert!(mgr.get_declaration("org.example@1.0.0.Person").is_ok());
}

/// The scratch copy that validates a file whose namespace is not
/// registered is this manager's arena with the file appended. It must be
/// exactly the manager a file-by-file rebuild
/// produces: the same files in the same order, the same handles and names,
/// the same state version, and empty caches. The files it keeps are
/// shared, not deep-cloned.
#[test]
fn with_model_file_registered_appends_exactly_as_a_rebuild_would() {
    let mgr = manager();
    // Warm the source's caches: the copy must not inherit them.
    assert!(mgr.properties("org.example@1.0.0.Person").is_ok());
    let fresh = ModelFile::from_json(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.new@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "A", "isAbstract": false, "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "s", "isArray": false, "isOptional": false }
                    ] },
                    { "$class": "concerto.metamodel@1.0.0.EnumDeclaration", "name": "E", "properties": [
                        { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "ONE" }
                    ] }
                ]
            }),
            Some("new.cto".into()),
        )
        .unwrap();
    let scratch = mgr
        .with_model_file_registered(Arc::new(fresh.clone()))
        .unwrap();

    let mut rebuilt = ModelManager::default();
    for existing in mgr.model_files() {
        rebuilt.insert(existing.clone()).unwrap();
    }
    rebuilt.insert(fresh.clone()).unwrap();

    let files = |m: &ModelManager| {
        m.files
            .iter()
            .map(|f| (f.model_file.namespace().to_string(), f.declarations.clone()))
            .collect::<Vec<_>>()
    };
    let decls = |m: &ModelManager| {
        m.declarations
            .iter()
            .map(|d| {
                (
                    d.model_file,
                    d.index,
                    d.properties.clone(),
                    d.fqn.to_string(),
                )
            })
            .collect::<Vec<_>>()
    };
    let props = |m: &ModelManager| {
        m.properties
            .iter()
            .map(|p| (p.declaration, p.index))
            .collect::<Vec<_>>()
    };
    let namespaces = |m: &ModelManager| {
        let mut v: Vec<_> = m.namespaces.iter().map(|(k, v)| (k.clone(), *v)).collect();
        v.sort();
        v
    };
    assert_eq!(files(&scratch), files(&rebuilt));
    assert_eq!(decls(&scratch), decls(&rebuilt));
    assert_eq!(props(&scratch), props(&rebuilt));
    assert_eq!(namespaces(&scratch), namespaces(&rebuilt));
    assert_eq!(scratch.state_version, rebuilt.state_version);
    assert_eq!(scratch.cache_counts(), (0, 0, 0));
    for (mine, theirs) in mgr.files.iter().zip(&scratch.files) {
        assert!(Arc::ptr_eq(&mine.model_file, &theirs.model_file));
    }
    assert!(
        scratch
            .model_file("org.new@1.0.0")
            .unwrap()
            .same_ast(&fresh)
    );
    // The source is untouched.
    assert!(mgr.model_file("org.new@1.0.0").is_none());
}

/// A file whose namespace *is* registered still takes the old file's
/// place in the order, as a file-by-file rebuild does.
#[test]
fn with_model_file_registered_replaces_in_place() {
    let mgr = manager();
    let replacement = ModelFile::from_json(
        &crate::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.example@1.0.0",
            "declarations": []
        }),
        None,
    )
    .unwrap();
    let scratch = mgr
        .with_model_file_registered(Arc::new(replacement.clone()))
        .unwrap();
    let order = |m: &ModelManager| {
        m.model_files()
            .map(|f| f.namespace().to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&scratch), order(&mgr));
    assert!(
        scratch
            .model_file("org.example@1.0.0")
            .unwrap()
            .same_ast(&replacement)
    );
    assert_eq!(
        scratch.state_version,
        u64::try_from(mgr.files.len()).unwrap()
    );
}

#[test]
fn update_model_file_rejects_an_unregistered_namespace() {
    let mgr = manager();
    let fresh = ModelFile::from_json(
        &crate::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.new@1.0.0", "declarations": []
        }),
        None,
    )
    .unwrap();
    let err = mgr.update_model_file(fresh, true).unwrap_err();
    assert_eq!(
        err.to_string(),
        "Model file for namespace org.new@1.0.0 not found"
    );
}

#[test]
fn delete_model_file_removes_the_namespace() {
    let mgr = manager();
    let deleted = mgr.delete_model_file("org.example@1.0.0").unwrap();
    assert!(deleted.model_file("org.example@1.0.0").is_none());
    assert!(mgr.model_file("org.example@1.0.0").is_some());
}

#[test]
fn delete_model_file_rejects_an_absent_namespace() {
    let mgr = manager();
    let err = mgr.delete_model_file("org.nope@1.0.0").unwrap_err();
    assert_eq!(err.to_string(), "Model file does not exist");
}

/// A downloaded `org.ext@1.0.0` declaring `E` (and `extra` when given),
/// as `updateExternalModels`' downloader returns it.
fn external(declarations: crate::json::Value) -> ModelFileSource {
    ModelFileSource {
        ast: crate::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.ext@1.0.0",
            "declarations": declarations
        }),
        definitions: Some("namespace org.ext@1.0.0".into()),
        file_name: Some("@example.com.ext.cto".into()),
    }
}

fn concept(name: &str) -> crate::json::Value {
    crate::json!({ "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": name, "isAbstract": false, "properties": [] })
}

#[test]
fn update_external_models_adds_then_updates_a_namespace() {
    let mut mgr = manager();
    let added = mgr
        .update_external_models([external(crate::json!([concept("E")]))])
        .unwrap();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0].file_name(), Some("@example.com.ext.cto"));
    assert!(mgr.get_declaration("org.ext@1.0.0.E").is_ok());
    assert!(mgr.model_file("org.ext@1.0.0").unwrap().is_external());

    // The same namespace again replaces it, in place.
    mgr.update_external_models([external(crate::json!([concept("F")]))])
        .unwrap();
    assert!(mgr.get_declaration("org.ext@1.0.0.F").is_ok());
    assert!(mgr.get_declaration("org.ext@1.0.0.E").is_err());
    assert!(mgr.get_declaration("org.example@1.0.0.Person").is_ok());
}

#[test]
fn update_external_models_with_nothing_downloaded_still_validates() {
    let mut mgr = manager();
    assert!(mgr.update_external_models([]).unwrap().is_empty());
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.bad@1.0.0",
                "declarations": [{ "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "B", "isAbstract": false,
                    "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Missing" },
                    "properties": [] }]
            }),
            None,
        )
        .unwrap();
    assert!(mgr.update_external_models([]).is_err());
}

#[test]
fn update_external_models_rolls_back_when_validation_fails() {
    let mut mgr = manager();
    let broken = crate::json!([{ "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "E", "isAbstract": false,
            "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Missing" },
            "properties": [] }]);
    assert!(
        mgr.update_external_models([external(crate::json!([concept("E")])), external(broken)])
            .is_err()
    );
    assert!(mgr.model_file("org.ext@1.0.0").is_none());
    assert!(mgr.get_declaration("org.example@1.0.0.Person").is_ok());
}

/// BC-11: a cyclic chain is an `IllegalModelException` naming the
/// cycle, from every entry point (TS 5.0.0 overflowed V8's stack or ran
/// out of memory, DV-013).
#[test]
fn circular_inheritance_is_an_illegal_model_error() {
    let concept = |name: &str, sup: &str| {
        crate::json!({ "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": name, "isAbstract": false, "properties": [],
                "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": sup } })
    };
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
        &crate::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.cycle@1.0.0",
            "declarations": [concept("A", "C"), concept("B", "A"), concept("C", "B")]
        }),
        None,
    )
    .unwrap();
    #[allow(deprecated)]
    let errors = [
        mgr.properties("org.cycle@1.0.0.A").unwrap_err(),
        mgr.validate_models().unwrap_err(),
        mgr.super_types("org.cycle@1.0.0.B").unwrap_err(),
        mgr.get_all_super_type_names("org.cycle@1.0.0.B")
            .unwrap_err(),
        mgr.is_assignable_to("org.cycle@1.0.0.A", "org.cycle@1.0.0.B")
            .unwrap_err(),
        mgr.is_assignable_to("org.cycle@1.0.0.A", "org.cycle@1.0.0.Other")
            .unwrap_err(),
        mgr.derives_from("org.cycle@1.0.0.C", "org.cycle@1.0.0.A")
            .unwrap_err(),
    ];
    for err in errors {
        let c = err.contract().clone();
        assert_eq!(c.kind, ErrorKind::IllegalModel);
        assert_eq!(c.code, "classdeclaration-circularinheritance");
        assert!(
            c.message()
                .starts_with("The super type chain of \"org.cycle@1.0.0.")
                && c.message().contains(" is circular: "),
            "{}",
            c.message()
        );
        assert_eq!(c.location, None);
    }
    let err = mgr.properties("org.cycle@1.0.0.A").unwrap_err();
    assert_eq!(
        err.contract().message(),
        "The super type chain of \"org.cycle@1.0.0.A\" is circular: org.cycle@1.0.0.A -> org.cycle@1.0.0.C -> org.cycle@1.0.0.B -> org.cycle@1.0.0.A."
    );
}

#[test]
#[allow(deprecated)]
fn a_super_type_imported_from_an_unregistered_namespace_is_not_defined() {
    // TS `ClassDeclaration._resolveSuperType`, for an *imported* super type,
    // resolves it through `this.modelFile.getModelManager().getType(fqnSuper)`
    // (`BaseModelManager.getType`), whose own unregistered-namespace check
    // raises "Namespace is not defined for type ...". `super_chain` walks
    // through `get_type_declaration`, so it reuses the same catalogue entry
    // (oracle fixture
    // `unit/ClassDeclaration.getIdentifierFieldName/557a5087518a8343fade9b97`).
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
            &crate::json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme.l2@1.0.0",
                "imports": [
                    { "$class": "concerto.metamodel@1.0.0.ImportType",
                      "namespace": "org.acme.l1@1.0.0", "name": "Base" }
                ],
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                      "name": "Vehicle", "isAbstract": false, "properties": [],
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Base" } }
                ]
            }),
            None,
        )
        .unwrap();
    // `org.acme.l1@1.0.0` (the import's target) is never added.
    for err in [
        mgr.identifier_field_name("org.acme.l2@1.0.0.Vehicle")
            .unwrap_err(),
        mgr.properties("org.acme.l2@1.0.0.Vehicle").unwrap_err(),
    ] {
        let c = err.contract().clone();
        assert_eq!(c.kind, ErrorKind::TypeNotFound);
        assert_eq!(
            c.message(),
            "Namespace is not defined for type \"org.acme.l1@1.0.0.Base\"."
        );
    }
}

/// Loads `org.other@1.0.0` (a concept `Shape`) and `org.main@1.0.0`
/// (a concept `Local`), which imports `Shape` under the alias `Figure`.
fn aliased_manager() -> ModelManager {
    let mut mgr = ModelManager::new().unwrap();
    mgr.load_model(
        &crate::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.other@1.0.0",
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Shape",
                  "isAbstract": false, "properties": [] }
            ]
        }),
        Some("other.cto".into()),
    )
    .unwrap();
    mgr.load_model(
        &crate::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.main@1.0.0",
            "imports": [
                { "$class": "concerto.metamodel@1.0.0.ImportTypes", "namespace": "org.other@1.0.0",
                  "types": ["Shape"],
                  "aliasedTypes": [ { "$class": "concerto.metamodel@1.0.0.AliasedType",
                                      "name": "Shape", "aliasedName": "Figure" } ] }
            ],
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Local",
                  "isAbstract": false, "properties": [] }
            ]
        }),
        Some("main.cto".into()),
    )
    .unwrap();
    mgr
}

/// TS `ModelFile.getType` by name: a primitive's own name, a local type's
/// and an aliased import's fully-qualified name, and `None` for an unknown
/// or unaliased name.
#[test]
fn model_file_type_name_answers_like_model_file_get_type() {
    let mgr = aliased_manager();
    let main = mgr.model_file_id("org.main@1.0.0").unwrap();
    let name = |t: &str| mgr.model_file_type_name(main, t).unwrap();
    assert_eq!(name("String").as_deref(), Some("String"));
    assert_eq!(name("Local").as_deref(), Some("org.main@1.0.0.Local"));
    assert_eq!(
        name("org.main@1.0.0.Local").as_deref(),
        Some("org.main@1.0.0.Local")
    );
    assert_eq!(name("Figure").as_deref(), Some("org.other@1.0.0.Shape"));
    assert_eq!(name("Shape"), None);
    assert_eq!(name("Missing"), None);
}

/// TS `BaseModelManager.getType` by name: the declaration's
/// fully-qualified name, or the `TypeNotFoundException` for an unknown
/// namespace or type.
#[test]
fn type_declaration_name_answers_like_get_type() {
    let mgr = aliased_manager();
    assert_eq!(
        mgr.type_declaration_name("org.other@1.0.0.Shape").unwrap(),
        "org.other@1.0.0.Shape"
    );
    for missing in [
        "org.nowhere@1.0.0.Shape",
        "org.other@1.0.0.Missing",
        "String",
    ] {
        let err = mgr.type_declaration_name(missing).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::TypeNotFound, "{missing}");
    }
}

/// TS `ModelFile.resolveType`: a primitive, a local type and an import
/// resolving in its own namespace pass; any other name is the
/// undeclared-type `IllegalModelException`, naming the file and carrying
/// the caller's location.
#[test]
fn model_file_resolve_type_rejects_an_undeclared_type() {
    let mgr = aliased_manager();
    let main = mgr.model_file_id("org.main@1.0.0").unwrap();
    for ok in ["Integer", "Local", "Figure"] {
        mgr.model_file_resolve_type(main, "ctx", ok, None).unwrap();
    }
    let location = crate::json!({ "start": { "line": 1 } });
    let err = mgr
        .model_file_resolve_type(main, "ctx", "Missing", Some(location.clone()))
        .unwrap_err();
    let contract = err.contract();
    assert_eq!(contract.kind, ErrorKind::IllegalModel);
    assert_eq!(contract.code, "modelfile-resolvetype-undecltype");
    assert_eq!(contract.model_file, Some(Some("main.cto".to_string())));
    assert_eq!(contract.location, Some(location));
}

/// TS `_throwAlreadyExists`: the plain `Error` naming both files for a
/// registered namespace; nothing for one that is not registered.
#[test]
fn check_namespace_available_names_both_files() {
    let mgr = aliased_manager();
    let err = mgr
        .check_namespace_available("org.other@1.0.0", Some("new.cto"))
        .unwrap_err();
    assert_eq!(err.kind(), ErrorKind::InvalidArgument);
    assert_eq!(
        err.to_string(),
        "Namespace org.other@1.0.0 specified in file new.cto is already declared in file other.cto"
    );
    mgr.check_namespace_available("org.free@1.0.0", None)
        .unwrap();
}

/// `update_external_models_naming_file` names the namespace of the
/// file whose validation failed, and leaves the manager unchanged.
#[test]
fn update_external_models_names_the_failing_file_and_rolls_back() {
    let mut mgr = aliased_manager();
    let before: Vec<String> = mgr
        .model_files()
        .map(|f| f.namespace().to_string())
        .collect();
    let broken = ModelFileSource {
        ast: crate::json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.broken@1.0.0",
            "declarations": [
                { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Bad",
                  "isAbstract": false,
                  "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Nowhere" },
                  "properties": [] }
            ]
        }),
        definitions: None,
        file_name: Some("@broken.cto".into()),
    };
    let (namespace, err) = mgr
        .update_external_models_naming_file([broken])
        .unwrap_err();
    assert_eq!(namespace.as_deref(), Some("org.broken@1.0.0"));
    assert_eq!(err.kind(), ErrorKind::IllegalModel);
    let after: Vec<String> = mgr
        .model_files()
        .map(|f| f.namespace().to_string())
        .collect();
    assert_eq!(before, after);
}

/// A concept `name` extending `super_type` (if any), with one string
/// property `field`.
fn p597_concept(name: &str, super_type: Option<&str>, field: &str) -> Value {
    let mut decl = crate::json!({
        "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": name, "isAbstract": false,
        "properties": [
            { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": field, "isArray": false, "isOptional": false }
        ]
    });
    if let Some(super_type) = super_type {
        decl["superType"] = crate::json!({ "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": super_type });
    }
    decl
}

/// A user model in `namespace` importing `Person` from the
/// `manager()` base and declaring `User extends Person`.
fn user_model(namespace: &str) -> Value {
    model(
        namespace,
        crate::json!([{ "$class": "concerto.metamodel@1.0.0.ImportType",
                                 "namespace": "org.example@1.0.0", "name": "Person" }]),
        crate::json!([p597_concept("User", Some("Person"), "login")]),
    )
}

/// The answers a server reads about one type.
fn answers(mgr: &ModelManager, fqn: &str) -> (Vec<String>, Vec<String>, Option<String>, bool) {
    (
        mgr.super_types(fqn)
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect(),
        mgr.properties(fqn)
            .unwrap()
            .into_iter()
            .map(|(owner, p)| format!("{owner}.{}", p.name()))
            .collect(),
        mgr.super_type(fqn).unwrap().map(|(n, _)| n),
        mgr.is_assignable_to(fqn, "org.example@1.0.0.Person")
            .unwrap(),
    )
}

/// A fork holds the same files (shared) under the same handles, with
/// the same options and validated marks, and starts with the base's
/// warmed caches; adding user models to the fork keeps every cached
/// base answer and changes none, and neither manager sees the other's
/// later changes.
#[test]
fn fork_shares_files_inherits_caches_and_is_isolated() {
    let mut base = manager();
    base.set_decorator_validation(crate::introspect::decorator::DecoratorValidationOptions {
        missing_decorator: Some("warn".into()),
        invalid_decorator: None,
    });
    base.validate_models().unwrap();
    let fqns = [
        "org.example@1.0.0.Person",
        "org.example@1.0.0.Employee",
        "org.example@1.0.0.Manager",
    ];
    let before: Vec<_> = fqns.iter().map(|f| answers(&base, f)).collect();
    let person = base.declaration_id("org.example@1.0.0.Manager").unwrap();
    let plan = crate::instance::plan::class_plan(&base, person).unwrap();
    assert!(plan.is_settled());
    let warmed = base.cache_counts();
    assert!(warmed.0 >= 3 && warmed.2 >= 1, "{warmed:?}");

    let mut fork = base.fork();
    assert_eq!(fork.cache_counts(), warmed);
    assert_eq!(fork.state_version(), base.state_version());
    assert_eq!(fork.decorator_validation(), base.decorator_validation());
    for (a, b) in base.shared_model_files().zip(fork.shared_model_files()) {
        assert!(Arc::ptr_eq(a, b));
    }
    for fqn in fqns {
        assert_eq!(base.declaration_id(fqn), fork.declaration_id(fqn));
    }
    let ns = base.model_file_id("org.example@1.0.0").unwrap();
    assert!(fork.known_valid(ns));

    // A request's user models: the base's cached answers stay, unchanged.
    fork.add_model_ast(&user_model("org.user@1.0.0"), Some("user.cto"))
        .unwrap();
    fork.validate_models().unwrap();
    let after_add = fork.cache_counts();
    assert!(
        after_add.0 >= warmed.0 && after_add.2 >= warmed.2,
        "{after_add:?} {warmed:?}"
    );
    let fork_plan = crate::instance::plan::class_plan(&fork, person).unwrap();
    assert!(
        Arc::ptr_eq(&plan, &fork_plan),
        "the base's plan is reused, not rebuilt"
    );
    let after: Vec<_> = fqns.iter().map(|f| answers(&fork, f)).collect();
    assert_eq!(before, after);
    assert!(
        fork.is_assignable_to("org.user@1.0.0.User", "org.example@1.0.0.Person")
            .unwrap()
    );

    // Isolation: the base never sees the fork's models, and a fork
    // never sees the base's or another fork's later ones.
    assert!(base.model_file("org.user@1.0.0").is_none());
    assert!(base.get_type_declaration("org.user@1.0.0.User").is_err());
    let mut other = base.fork();
    other
        .add_model_ast(&user_model("org.user@1.0.0"), Some("other.cto"))
        .unwrap();
    assert_eq!(
        fork.model_file("org.user@1.0.0").unwrap().file_name(),
        Some("user.cto")
    );
    assert_eq!(
        other.model_file("org.user@1.0.0").unwrap().file_name(),
        Some("other.cto")
    );
    base.add_model_ast(&user_model("org.later@1.0.0"), None)
        .unwrap();
    assert!(fork.model_file("org.later@1.0.0").is_none());
    assert!(other.model_file("org.later@1.0.0").is_none());
    let deleted = base.delete_model_file("org.example@1.0.0").unwrap();
    assert!(deleted.model_file("org.example@1.0.0").is_none());
    assert_eq!(
        after,
        fqns.iter().map(|f| answers(&fork, f)).collect::<Vec<_>>()
    );
}

/// An append keeps a cached answer only when it cannot change: a plan
/// with an unresolved field type is built again once the type's
/// namespace is added, and then resolves.
#[test]
fn an_append_rebuilds_an_unsettled_plan() {
    let mut mgr = ModelManager::new().unwrap();
    let mut holder = p597_concept("Holder", None, "name");
    holder["properties"].as_array_mut().unwrap().push(crate::json!({
            "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "later", "isArray": false, "isOptional": true,
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Later" }
        }));
    mgr.add_model_ast(
        &model(
            "org.holder@1.0.0",
            crate::json!([{ "$class": "concerto.metamodel@1.0.0.ImportType",
                                     "namespace": "org.later@1.0.0", "name": "Later" }]),
            crate::json!([holder]),
        ),
        None,
    )
    .unwrap();
    let id = mgr.declaration_id("org.holder@1.0.0.Holder").unwrap();
    let plan = crate::instance::plan::class_plan(&mgr, id).unwrap();
    assert!(!plan.is_settled());
    mgr.add_model_ast(
        &model(
            "org.later@1.0.0",
            crate::json!([]),
            crate::json!([p597_concept("Later", None, "x")]),
        ),
        None,
    )
    .unwrap();
    let rebuilt = crate::instance::plan::class_plan(&mgr, id).unwrap();
    assert!(!Arc::ptr_eq(&plan, &rebuilt));
    assert!(rebuilt.is_settled());
}

/// `filter` shares a file it keeps unchanged, and does not validate it
/// again only when the source had validated it and every file it
/// reaches is shared too; the result is the one the rebuilding filter
/// gave, and a source never validated still fails the same way.
#[test]
fn filter_shares_unchanged_files_and_keeps_validation() {
    let mut base = manager();
    base.add_model_ast(&user_model("org.user@1.0.0"), Some("user.cto"))
        .unwrap();
    base.validate_models().unwrap();
    let keep_all = |fqn: &str| !fqn.starts_with("concerto.decorator@");
    let result = base.filter_by_fqn(keep_all, false).unwrap();
    for ns in ["org.example@1.0.0", "org.user@1.0.0"] {
        let a = base
            .shared_model_files()
            .find(|f| f.namespace() == ns)
            .unwrap();
        let b = result
            .shared_model_files()
            .find(|f| f.namespace() == ns)
            .unwrap();
        assert!(Arc::ptr_eq(a, b), "{ns} is shared");
        let id = result.model_file_id(ns).unwrap();
        assert!(result.known_valid(id), "{ns} is known valid");
    }
    // Dropping `Manager` changes org.example: it is rebuilt, and the
    // user file, still shared, is validated again (its proof no longer
    // holds), exactly as before.
    let partial = base
        .filter_by_fqn(|fqn| keep_all(fqn) && !fqn.ends_with(".Manager"), false)
        .unwrap();
    let example = partial
        .shared_model_files()
        .find(|f| f.namespace() == "org.example@1.0.0")
        .unwrap();
    assert!(!base.shared_model_files().any(|f| Arc::ptr_eq(f, example)));
    let user = partial.model_file_id("org.user@1.0.0").unwrap();
    let proof = partial.files[user.slot()].proof.clone().unwrap();
    assert!(
        !partial.proof_holds(&proof),
        "its import was rebuilt: validated again"
    );
    assert!(partial.known_valid(user), "and it passed");
    let names = |m: &ModelManager| -> Vec<String> {
        m.super_types("org.user@1.0.0.User")
            .unwrap()
            .into_iter()
            .map(|(n, _)| n)
            .collect()
    };
    assert_eq!(names(&partial), names(&base));

    // A source that never validated an invalid file: the filter
    // validates it and throws, as the rebuilding filter did.
    let mut unvalidated = manager();
    unvalidated
        .add_model_ast(
            &model(
                "org.bad@1.0.0",
                crate::json!([]),
                crate::json!([p597_concept("Bad", Some("Nowhere"), "x")]),
            ),
            None,
        )
        .unwrap();
    let err = unvalidated.filter_by_fqn(keep_all, false).unwrap_err();
    let expected = unvalidated.validate_models().unwrap_err();
    assert_eq!(err.to_string(), expected.to_string());
    assert!(unvalidated.filter_by_fqn(keep_all, true).is_ok());
}

/// A proof holds only under the source's options, and the
/// validated marks go when an option changes or a batch rolls
/// back.
#[test]
fn validated_marks_follow_options_and_rollbacks() {
    let mut base = manager();
    base.validate_models().unwrap();
    let id = base.model_file_id("org.example@1.0.0").unwrap();
    assert!(base.known_valid(id));
    let proof = base.validity_proof("org.example@1.0.0").unwrap();
    let shared = base
        .shared_model_files()
        .find(|f| f.namespace() == "org.example@1.0.0")
        .cloned()
        .unwrap();

    let mut target = ModelManager::new().unwrap();
    target.set_dangerously_allow_reserved_system_type_names_in_user_models(true);
    let tid = target
        .add_shared_model_file_with_proof(Arc::clone(&shared), Some(Arc::clone(&proof)))
        .unwrap();
    assert!(
        !target.known_valid(tid),
        "other options: the proof does not hold"
    );
    let mut same = ModelManager::new().unwrap();
    let sid = same
        .add_shared_model_file_with_proof(shared, Some(proof))
        .unwrap();
    assert!(same.known_valid(sid));

    base.set_metamodel_validation(true);
    assert!(!base.known_valid(id));
    base.validate_models().unwrap();
    assert!(base.known_valid(id));

    // A batch whose validation fails restores the marks.
    let bad = model(
        "org.bad@1.0.0",
        crate::json!([]),
        crate::json!([p597_concept("Bad", Some("Nowhere"), "x")]),
    );
    let user = user_model("org.user@1.0.0");
    assert!(base.load_models([(&user, None), (&bad, None)]).is_err());
    assert!(base.known_valid(id));
    assert!(base.model_file("org.user@1.0.0").is_none());
}

/// An update or a removal adopts a rebuilt manager, and the state
/// version keeps rising: it never repeats one an earlier state had.
#[test]
fn state_version_never_repeats_across_updates_and_removals() {
    let mut mgr = manager();
    mgr.add_model_ast(&user_model("org.user@1.0.0"), Some("user.cto"))
        .unwrap();
    let mut seen = vec![mgr.state_version()];
    mgr.update_model_ast(&user_model("org.user@1.0.0"), Some("user2.cto"))
        .unwrap();
    seen.push(mgr.state_version());
    mgr.remove_model("org.user@1.0.0").unwrap();
    seen.push(mgr.state_version());
    mgr.add_model_ast(&user_model("org.other@1.0.0"), None)
        .unwrap();
    seen.push(mgr.state_version());
    assert!(seen.windows(2).all(|w| w[0] < w[1]), "{seen:?}");
    // A rebuilt manager on its own restarts its count; adopting it does not.
    let rebuilt = mgr.delete_model_file("org.other@1.0.0").unwrap();
    assert!(rebuilt.state_version() < mgr.state_version());
    let before = mgr.state_version();
    mgr.adopt(rebuilt);
    assert_eq!(mgr.state_version(), before + 1);
}

/// A removal, an update and the metamodel share the files they
/// keep (`Arc`), never deep-copy them.
#[test]
fn rebuilds_share_the_files_they_keep() {
    let mut mgr = manager();
    mgr.add_model_ast(&user_model("org.user@1.0.0"), None)
        .unwrap();
    let file = |m: &ModelManager, ns: &str| {
        m.shared_model_files()
            .find(|f| f.namespace() == ns)
            .cloned()
            .unwrap()
    };
    let example = file(&mgr, "org.example@1.0.0");
    let deleted = mgr.delete_model_file("org.user@1.0.0").unwrap();
    assert!(Arc::ptr_eq(&example, &file(&deleted, "org.example@1.0.0")));
    for (a, b) in mgr
        .shared_model_files()
        .take(2)
        .zip(deleted.shared_model_files())
    {
        assert!(Arc::ptr_eq(a, b), "the system files are shared");
    }
    let replacement = ModelFile::from_json(&user_model("org.user@1.0.0"), None).unwrap();
    let updated = mgr.update_model_file(replacement, false).unwrap();
    assert!(Arc::ptr_eq(&example, &file(&updated, "org.example@1.0.0")));
    let a = crate::instance::metamodel::metamodel_model_file().unwrap();
    let b = crate::instance::metamodel::metamodel_model_file().unwrap();
    assert!(Arc::ptr_eq(&a, &b));
}

/// A filter's result keeps every option of the manager it filters,
/// `metamodel_validation` included, as TS's
/// `new BaseModelManager({...this.options})` does.
#[test]
fn filter_keeps_every_option() {
    let mut mgr = manager();
    mgr.set_metamodel_validation(true);
    mgr.set_dangerously_allow_reserved_system_type_names_in_user_models(true);
    mgr.set_decorator_validation(crate::introspect::decorator::DecoratorValidationOptions {
        missing_decorator: Some("warn".into()),
        invalid_decorator: None,
    });
    let filtered = mgr.filter(|_, _| true).unwrap();
    assert_eq!(filtered.options, mgr.options);
    assert_eq!(mgr.fork().options, mgr.options);
    let deleted = mgr.delete_model_file("org.example@1.0.0").unwrap();
    assert_eq!(deleted.options, mgr.options);
}

/// External models are each registered once, shared with the list
/// returned, and a new namespace is appended in place.
#[test]
fn external_models_are_shared_with_the_list_returned() {
    let mut mgr = manager();
    let external = |ns: &str, name: &str| ModelFileSource {
        ast: model(
            ns,
            crate::json!([]),
            crate::json!([p597_concept(name, None, "x")]),
        ),
        definitions: None,
        file_name: Some(format!("@{ns}.cto")),
    };
    let added = mgr
        .update_external_models([
            external("org.ext.a@1.0.0", "A"),
            external("org.ext.b@1.0.0", "B"),
            external("org.ext.a@1.0.0", "A2"),
        ])
        .unwrap();
    assert_eq!(added.len(), 3);
    let held = |ns: &str| {
        mgr.shared_model_files()
            .find(|f| f.namespace() == ns)
            .cloned()
            .unwrap()
    };
    assert!(Arc::ptr_eq(&added[1], &held("org.ext.b@1.0.0")));
    assert!(Arc::ptr_eq(&added[2], &held("org.ext.a@1.0.0")));
    assert!(mgr.get_declaration("org.ext.a@1.0.0.A2").is_ok());
    assert!(mgr.get_declaration("org.ext.a@1.0.0.A").is_err());
}
