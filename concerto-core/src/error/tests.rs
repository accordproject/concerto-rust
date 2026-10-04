use super::*;

fn contract(code: &'static str, params: &[(&'static str, &str)]) -> ContractError {
    ContractError::new(
        ErrorKind::InvalidArgument,
        code,
        params.iter().map(|(k, v)| (*k, (*v).to_string())).collect(),
    )
}

// Golden tests: one per catalogue entry (PORTING.md 6.3), each the
// string the TS reference produces for the same params.

#[test]
fn golden_modelutil_getnamespace_nofnq() {
    assert_eq!(
        contract("modelutil-getnamespace-nofnq", &[]).message(),
        "FQN is invalid."
    );
}

#[test]
fn golden_modelutil_parsenamespace_nullorundefined() {
    assert_eq!(
        contract("modelutil-parsenamespace-nullorundefined", &[]).message(),
        "Namespace is null or undefined."
    );
}

#[test]
fn golden_modelutil_parsenamespace_invalidnamespace() {
    assert_eq!(
        contract(
            "modelutil-parsenamespace-invalidnamespace",
            &[("ns", "org.acme@1.0.0@2.3")]
        )
        .message(),
        "Invalid namespace org.acme@1.0.0@2.3"
    );
}

#[test]
fn golden_modelutil_isassignableto_cannotfindtype() {
    assert_eq!(
        contract(
            "modelutil-isassignableto-cannotfindtype",
            &[("typeName", "org.doge.Doge")]
        )
        .message(),
        "Cannot find type org.doge.Doge"
    );
}

#[test]
fn golden_metamodelutil_importfullyqualifiednames_unrecognizedimports() {
    assert_eq!(
        contract(
            "metamodelutil-importfullyqualifiednames-unrecognizedimports",
            &[("$class", "undefined")]
        )
        .message(),
        "Unrecognized imports undefined"
    );
}

#[test]
fn golden_validator_reporterror() {
    let mut err = contract("numbervalidator-constructor-nobounds", &[]);
    err.kind = ErrorKind::Validator;
    err.validator = Some(ValidatorReport {
        id: "null".into(),
        fqn: "org.acme.myField".into(),
        error_type: "DefaultValidatorException",
    });
    assert_eq!(
        err.message(),
        "Validator error for field `null`. org.acme.myField: Invalid range, lower and-or upper bound must be specified."
    );
}

#[test]
fn golden_numbervalidator_constructor_nobounds() {
    assert_eq!(
        contract("numbervalidator-constructor-nobounds", &[]).message(),
        "Invalid range, lower and-or upper bound must be specified."
    );
}

#[test]
fn golden_numbervalidator_constructor_lowerhigherthanupper() {
    assert_eq!(
        contract("numbervalidator-constructor-lowerhigherthanupper", &[]).message(),
        "Lower bound must be less than or equal to upper bound."
    );
}

#[test]
fn golden_numbervalidator_constructor_outsidelowerbound() {
    assert_eq!(
        contract(
            "numbervalidator-constructor-outsidelowerbound",
            &[("value", "5"), ("lowerBound", "10")]
        )
        .message(),
        "Value 5 is outside lower bound 10"
    );
}

#[test]
fn golden_numbervalidator_constructor_outsideupperbound() {
    assert_eq!(
        contract(
            "numbervalidator-constructor-outsideupperbound",
            &[("value", "60"), ("upperBound", "50")]
        )
        .message(),
        "Value 60 is outside upper bound 50"
    );
}

#[test]
fn golden_scalardeclaration_process_primitivename() {
    assert_eq!(
        contract(
            "scalardeclaration-process-primitivename",
            &[("scalarName", "String")]
        )
        .message(),
        "Invalid scalar name 'String'. Name conflicts with primitive type."
    );
}

#[test]
fn golden_scalardeclaration_validate_duplicateclassname() {
    assert_eq!(
        contract(
            "scalardeclaration-validate-duplicateclassname",
            &[("name", "org.acme@1.0.0.A")]
        )
        .message(),
        "Duplicate class name org.acme@1.0.0.A"
    );
}

// ---- StringValidator, CollectionSizeValidator ----

#[test]
fn golden_stringvalidator_constructor_invalidlength() {
    assert_eq!(
        contract("stringvalidator-constructor-invalidlength", &[]).message(),
        "Invalid string length, minLength and-or maxLength must be specified."
    );
}

#[test]
fn golden_stringvalidator_constructor_negativelength() {
    assert_eq!(
        contract("stringvalidator-constructor-negativelength", &[]).message(),
        "minLength and-or maxLength must be positive integers."
    );
}

#[test]
fn golden_stringvalidator_constructor_mingreaterthanmax() {
    assert_eq!(
        contract("stringvalidator-constructor-mingreaterthanmax", &[]).message(),
        "minLength must be less than or equal to maxLength."
    );
}

#[test]
fn golden_stringvalidator_constructor_invalidregex() {
    assert_eq!(
        contract(
            "stringvalidator-constructor-invalidregex",
            &[(
                "message",
                "Invalid regular expression: /^[A-z/: unterminated character class"
            )]
        )
        .message(),
        "Invalid regular expression: /^[A-z/: unterminated character class"
    );
}

#[test]
fn golden_stringvalidator_validate_belowminlength() {
    assert_eq!(
        contract(
            "stringvalidator-validate-belowminlength",
            &[("value", "w"), ("minLength", "2")]
        )
        .message(),
        "The string length of 'w' should be at least 2 characters."
    );
}

#[test]
fn golden_stringvalidator_validate_abovemaxlength() {
    assert_eq!(
        contract(
            "stringvalidator-validate-abovemaxlength",
            &[("value", "ABCD1234567"), ("maxLength", "10")]
        )
        .message(),
        "The string length of 'ABCD1234567' should not exceed 10 characters."
    );
}

#[test]
fn golden_stringvalidator_validate_regexmismatch() {
    assert_eq!(
        contract(
            "stringvalidator-validate-regexmismatch",
            &[("value", "xyz"), ("regex", "/^[A-z][A-z][0-9]{7}/")]
        )
        .message(),
        "Value 'xyz' failed to match validation regex: /^[A-z][A-z][0-9]{7}/"
    );
}

#[test]
fn golden_collectionsizevalidator_constructor_nosize() {
    assert_eq!(
        contract("collectionsizevalidator-constructor-nosize", &[]).message(),
        "Invalid collection size, minSize and/or maxSize must be specified."
    );
}

#[test]
fn golden_collectionsizevalidator_constructor_negativesize() {
    assert_eq!(
        contract("collectionsizevalidator-constructor-negativesize", &[]).message(),
        "minSize and/or maxSize must be positive integers."
    );
}

#[test]
fn golden_collectionsizevalidator_constructor_mingreaterthanmax() {
    assert_eq!(
        contract("collectionsizevalidator-constructor-mingreaterthanmax", &[]).message(),
        "minSize must be less than or equal to maxSize."
    );
}

#[test]
fn golden_collectionsizevalidator_validate_belowminsize() {
    assert_eq!(
        contract(
            "collectionsizevalidator-validate-belowminsize",
            &[("minSize", "2")]
        )
        .message(),
        "Collection must contain at least 2 elements."
    );
}

#[test]
fn golden_collectionsizevalidator_validate_abovemaxsize() {
    assert_eq!(
        contract(
            "collectionsizevalidator-validate-abovemaxsize",
            &[("maxSize", "3")]
        )
        .message(),
        "Collection must contain no more than 3 elements."
    );
}

#[test]
fn golden_engine_typeerror_readproperties() {
    assert_eq!(
        contract(
            "engine-typeerror-readproperties",
            &[("value", "undefined"), ("property", "$class")]
        )
        .message(),
        "Cannot read properties of undefined (reading '$class')"
    );
}

#[test]
fn golden_classdeclaration_circularinheritance() {
    let err = ContractError::new(
        ErrorKind::IllegalModel,
        "classdeclaration-circularinheritance",
        vec![
            ("type", "org.cycle@1.0.0.A".to_string()),
            (
                "cycle",
                "org.cycle@1.0.0.A -> org.cycle@1.0.0.C -> org.cycle@1.0.0.A".to_string(),
            ),
        ],
    );
    assert_eq!(
        err.message(),
        "The super type chain of \"org.cycle@1.0.0.A\" is circular: org.cycle@1.0.0.A -> org.cycle@1.0.0.C -> org.cycle@1.0.0.A."
    );
    assert_eq!(err.kind.ts_class(), "IllegalModelException");
}

#[test]
fn golden_engine_typeerror_notafunction() {
    assert_eq!(
        contract(
            "engine-typeerror-notafunction",
            &[("expression", "imp.types.forEach")]
        )
        .message(),
        "imp.types.forEach is not a function"
    );
}

#[test]
fn golden_engine_typeerror_inoperator() {
    assert_eq!(
        contract(
            "engine-typeerror-inoperator",
            &[("key", "$class"), ("value", "true")]
        )
        .message(),
        "Cannot use 'in' operator to search for '$class' in true"
    );
}

// ---- ResourceId (src/model/resourceid.ts) ----

#[test]
fn golden_resourceid_constructor_missingnamespace() {
    assert_eq!(
        contract("resourceid-constructor-missingnamespace", &[]).message(),
        "Missing namespace"
    );
}

#[test]
fn golden_resourceid_constructor_missingtype() {
    assert_eq!(
        contract("resourceid-constructor-missingtype", &[]).message(),
        "Missing type"
    );
}

#[test]
fn golden_resourceid_constructor_missingid() {
    assert_eq!(
        contract("resourceid-constructor-missingid", &[]).message(),
        "Missing id"
    );
}

#[test]
fn golden_resourceid_parseuri_invalidport() {
    assert_eq!(
        contract("resourceid-parseuri-invalidport", &[]).message(),
        "Invalid port"
    );
}

#[test]
fn golden_resourceid_fromuri_invaliduri() {
    assert_eq!(
        contract(
            "resourceid-fromuri-invaliduri",
            &[(
                "uri",
                "resource://NOT-A-URI:SUCH-WRONG/org.acme.l1@1.0.0.Person#123"
            )]
        )
        .message(),
        "Invalid URI: resource://NOT-A-URI:SUCH-WRONG/org.acme.l1@1.0.0.Person#123"
    );
}

#[test]
fn golden_resourceid_fromuri_invalidscheme() {
    assert_eq!(
        contract(
            "resourceid-fromuri-invalidscheme",
            &[("uri", "banana:org.acme.l1@1.0.0.Person#123")]
        )
        .message(),
        "Invalid URI scheme: banana:org.acme.l1@1.0.0.Person#123"
    );
}

#[test]
fn golden_resourceid_fromuri_invalidformat() {
    assert_eq!(
        contract(
            "resourceid-fromuri-invalidformat",
            &[(
                "uri",
                "resource://USER:PASSWORD@HOSTNAME:1567/org.acme.l1@1.0.0.Person#123"
            )]
        )
        .message(),
        "Invalid resource URI format: resource://USER:PASSWORD@HOSTNAME:1567/org.acme.l1@1.0.0.Person#123"
    );
}

/// Inline templates are template literals: an inserted value that looks
/// like another param, or like a `replace` substitution pattern, stays as
/// it is.
#[test]
fn inline_rendering_never_rescans_inserted_values() {
    let mut err = contract("numbervalidator-constructor-nobounds", &[]);
    err.kind = ErrorKind::Validator;
    err.validator = Some(ValidatorReport {
        id: "{fqn} $& $$".into(),
        fqn: "{msg}".into(),
        error_type: "DefaultValidatorException",
    });
    assert_eq!(
        err.message(),
        "Validator error for field `{fqn} $& $$`. {msg}: Invalid range, lower and-or upper bound must be specified."
    );
}

/// `Globalize` templates are the opposite of `Inline` on every edge case
/// 6.3 asks for: a repeated `{param}` is replaced at each occurrence, a
/// value containing another param's placeholder is substituted again
/// once that later param is applied, and `$$`/`$&` in a value follow JS
/// `String.prototype.replace` (section 2.2).
#[test]
fn globalize_rendering_rescans_inserted_values_and_applies_replace_patterns() {
    assert_eq!(
        contract(
            "factory-newinstance-missingidentifier",
            &[("type", "{namespace} $$ $&"), ("namespace", "org.acme")]
        )
        .message(),
        // The "type" pass runs first: "$$" -> "$", "$&" -> the matched
        // text "{type}" (not "{namespace}": $& is relative to *this*
        // pass's own match). That leaves a literal "{namespace}" in the
        // message, which the later "namespace" pass then substitutes too
        // (rescanning) — but the "{type}" that "$&" just inserted is not
        // reprocessed, because the "type" param has already run.
        "Missing identifier for Type \"org.acme $ {type}\" in namespace \"org.acme\"."
    );
}

// ---- BaseModelManager.validateAst ----

#[test]
fn golden_basemodelmanager_validateast_versionmismatch() {
    assert_eq!(
        contract(
            "basemodelmanager-validateast-versionmismatch",
            &[
                ("modelFileVersion", "99.0.0"),
                ("metamodelVersion", "1.0.0")
            ]
        )
        .message(),
        "Model file version 99.0.0 does not match metamodel version 1.0.0"
    );
}

#[test]
fn golden_basemodelmanager_validateast_wrapped() {
    assert_eq!(
        contract(
            "basemodelmanager-validateast-wrapped",
            &[(
                "message",
                "Unexpected properties for type concerto.metamodel@1.0.0.Model: undeclared"
            )]
        )
        .message(),
        "Unexpected properties for type concerto.metamodel@1.0.0.Model: undeclared"
    );
}

// ---- Model manager, model file and class declaration ----

#[test]
fn golden_typenotfounderror_defaultmessage() {
    assert_eq!(
        contract(
            "typenotfounderror-defaultmessage",
            &[("typeName", "org.acme.Doge")]
        )
        .message(),
        "Type \"org.acme.Doge\" not found."
    );
}

#[test]
fn golden_factory_newinstance_missingidentifier() {
    assert_eq!(
        contract(
            "factory-newinstance-missingidentifier",
            &[("type", "MyAsset"), ("namespace", "org.acme")]
        )
        .message(),
        "Missing identifier for Type \"MyAsset\" in namespace \"org.acme\"."
    );
}

#[test]
fn golden_factory_newinstance_invalididentifier() {
    assert_eq!(
        contract(
            "factory-newinstance-invalididentifier",
            &[("type", "MyAsset"), ("namespace", "org.acme")]
        )
        .message(),
        "Invalid or missing identifier for Type \"MyAsset\" in namespace \"org.acme\"."
    );
}

#[test]
fn golden_factory_newinstance_abstracttype() {
    assert_eq!(
        contract(
            "factory-newinstance-abstracttype",
            &[("type", "MyAsset"), ("namespace", "org.acme")]
        )
        .message(),
        "Cannot instantiate the abstract type \"MyAsset\" in the \"org.acme\" namespace."
    );
}

#[test]
fn golden_factory_newinstance_typenotdeclaredinns() {
    assert_eq!(
        contract(
            "factory-newinstance-typenotdeclaredinns",
            &[("type", "MyAsset"), ("namespace", "org.acme")]
        )
        .message(),
        "Cannot instantiate Type \"MyAsset\" in namespace \"org.acme\"."
    );
}

#[test]
fn golden_modelmanager_gettype_noregisteredns() {
    assert_eq!(
        contract(
            "modelmanager-gettype-noregisteredns",
            &[("type", "org.acme@1.0.0.Doge")]
        )
        .message(),
        "Namespace is not defined for type \"org.acme@1.0.0.Doge\"."
    );
}

// ---- Model manager, model file and class declaration, continued ----

#[test]
fn golden_modelmanager_resolvetype_nonsfortype() {
    assert_eq!(
        contract(
            "modelmanager-resolvetype-nonsfortype",
            &[("type", "org.acme.Foo"), ("context", "resolving type")]
        )
        .message(),
        "No registered namespace for type \"org.acme.Foo\" in \"resolving type\"."
    );
}

#[test]
fn golden_modelmanager_resolvetype_notypeinnsforcontext() {
    assert_eq!(
        contract(
            "modelmanager-resolvetype-notypeinnsforcontext",
            &[
                ("context", "resolving type"),
                ("type", "Foo"),
                ("namespace", "org.acme@1.0.0")
            ]
        )
        .message(),
        "No type \"Foo\" in namespace \"org.acme@1.0.0\" for \"resolving type\"."
    );
}

#[test]
fn golden_basemodelmanager_updatemodelfile_notfound() {
    assert_eq!(
        contract(
            "basemodelmanager-updatemodelfile-notfound",
            &[("namespace", "org.acme@1.0.0")]
        )
        .message(),
        "Model file for namespace org.acme@1.0.0 not found"
    );
}

#[test]
fn golden_basemodelmanager_deletemodelfile_notfound() {
    assert_eq!(
        contract("basemodelmanager-deletemodelfile-notfound", &[]).message(),
        "Model file does not exist"
    );
}

#[test]
fn golden_basemodelmanager_throwalreadyexists() {
    assert_eq!(
        contract(
            "basemodelmanager-throwalreadyexists",
            &[
                ("namespace", "org.acme@1.0.0"),
                ("prefix", " specified in file new.cto"),
                ("postfix", " in file old.cto")
            ]
        )
        .message(),
        "Namespace org.acme@1.0.0 specified in file new.cto is already declared in file old.cto"
    );
}

#[test]
fn golden_basemodelmanager_throwalreadyexists_without_names() {
    assert_eq!(
        contract(
            "basemodelmanager-throwalreadyexists",
            &[
                ("namespace", "org.acme@1.0.0"),
                ("prefix", ""),
                ("postfix", "")
            ]
        )
        .message(),
        "Namespace org.acme@1.0.0 is already declared"
    );
}

#[test]
fn golden_metamodelutil_createnametable_declarationnotfound() {
    assert_eq!(
        contract(
            "metamodelutil-createnametable-declarationnotfound",
            &[("name", "Foo"), ("namespace", "org.acme@1.0.0")]
        )
        .message(),
        "Declaration Foo in namespace org.acme@1.0.0 not found"
    );
}

#[test]
fn golden_metamodelutil_resolvename_notfound() {
    assert_eq!(
        contract("metamodelutil-resolvename-notfound", &[("name", "Foo")]).message(),
        "Name Foo not found"
    );
}

#[test]
fn golden_metamodelutil_resolvetypenames_unrecognizedclass() {
    assert_eq!(
        contract(
            "metamodelutil-resolvetypenames-unrecognizedclass",
            &[("class", "undefined")]
        )
        .message(),
        "Unrecognized $class undefined"
    );
}

#[test]
fn golden_modelmanager_gettype_notypeinns() {
    assert_eq!(
        contract(
            "modelmanager-gettype-notypeinns",
            &[("type", "Foo"), ("namespace", "org.acme@1.0.0")]
        )
        .message(),
        "Type \"Foo\" is not defined in namespace \"org.acme@1.0.0\"."
    );
}

#[test]
fn golden_modelmanager_gettype_duplicatensimport() {
    assert_eq!(
        contract(
            "modelmanager-gettype-duplicatensimport",
            &[
                ("namespace", "org.acme"),
                ("version1", "1.0.0"),
                ("version2", "2.0.0")
            ]
        )
        .message(),
        "Importing types from different versions (\"1.0.0\", \"2.0.0\") of the same namespace \"org.acme\" is not permitted."
    );
}

#[test]
fn golden_modelfile_resolvetype_undecltype() {
    assert_eq!(
        contract(
            "modelfile-resolvetype-undecltype",
            &[("type", "Foo"), ("context", "a field")]
        )
        .message(),
        "Undeclared type \"Foo\" in \"a field\"."
    );
}

#[test]
fn golden_modelfile_resolveimport_failfindimp() {
    assert_eq!(
        contract(
            "modelfile-resolveimport-failfindimp",
            &[
                ("type", "Foo"),
                ("imports", "org.acme.Bar"),
                ("namespace", "org.acme@1.0.0")
            ]
        )
        .message(),
        "Failed to find \"Foo\" in list of imports \"[org.acme.Bar]\" for namespace \"org.acme@1.0.0\"."
    );
}

#[test]
fn golden_modelfile_constructor_unrecmodelelem() {
    assert_eq!(
        contract(
            "modelfile-constructor-unrecmodelelem",
            &[("type", "concerto.metamodel@1.0.0.Foo")]
        )
        .message(),
        "Unrecognised model element \"concerto.metamodel@1.0.0.Foo\"."
    );
}

#[test]
fn golden_classdeclaration_validate_undefined_properties() {
    assert_eq!(
        contract(
            "classdeclaration-validate-undefined-properties",
            &[("class", "org.acme.Foo")]
        )
        .message(),
        "Properties of Class \"org.acme.Foo\" has to be defined."
    );
}

#[test]
fn golden_classdeclaration_process_unrecmodelelem() {
    assert_eq!(
        contract(
            "classdeclaration-process-unrecmodelelem",
            &[("type", "concerto.metamodel@1.0.0.Bar")]
        )
        .message(),
        "Unrecognised model element \"concerto.metamodel@1.0.0.Bar\"."
    );
}

#[test]
fn golden_classdeclaration_validate_selfextending() {
    assert_eq!(
        contract(
            "classdeclaration-validate-selfextending",
            &[("class", "Foo")]
        )
        .message(),
        "Class \"Foo\" cannot extend itself."
    );
}

#[test]
fn golden_classdeclaration_validate_identifiernotproperty() {
    assert_eq!(
        contract(
            "classdeclaration-validate-identifiernotproperty",
            &[("class", "Foo"), ("idField", "bar")]
        )
        .message(),
        "Class \"Foo\" is identified by field \"bar\", but does not contain this property."
    );
}

#[test]
fn golden_classdeclaration_validate_identifiernotstring() {
    assert_eq!(
        contract(
            "classdeclaration-validate-identifiernotstring",
            &[("class", "Foo"), ("idField", "bar")]
        )
        .message(),
        "Class \"Foo\" is identified by field \"bar\", but the type of the field is not \"String\"."
    );
}

#[test]
fn golden_classdeclaration_validate_duplicatefieldname() {
    assert_eq!(
        contract(
            "classdeclaration-validate-duplicatefieldname",
            &[("class", "Foo"), ("fieldName", "bar")]
        )
        .message(),
        "Class \"Foo\" has more than one field named \"bar\"."
    );
}

#[test]
fn golden_classdeclaration_getnestedproperty_doesnotexist() {
    assert_eq!(
        contract(
            "classdeclaration-getnestedproperty-doesnotexist",
            &[("propertyName", "missing"), ("fqn", "org.acme@1.0.0.Foo")]
        )
        .message(),
        "Property missing does not exist on org.acme@1.0.0.Foo"
    );
}

#[test]
fn golden_classdeclaration_getnestedproperty_primitiveorenum() {
    assert_eq!(
        contract(
            "classdeclaration-getnestedproperty-primitiveorenum",
            &[("propertyName", "bar"), ("propertyPath", "foo.bar.baz")]
        )
        .message(),
        "Property bar is a primitive or enum. Invalid property path: foo.bar.baz"
    );
}

/// TS `'Failed to find fully qualified type name for property ' +
/// this.name + ' with type ' + this.type` (src/introspect/property.ts),
/// checked against the frozen TS 5.0.0 reference. `this.type` is JS
/// `null` for an enum value, coerced to the literal string `null` by the
/// `+` concatenation.
#[test]
fn golden_property_getfullyqualifiedtypename_notfound() {
    assert_eq!(
        contract(
            "property-getfullyqualifiedtypename-notfound",
            &[("name", "status"), ("type", "null")]
        )
        .message(),
        "Failed to find fully qualified type name for property status with type null"
    );
}

#[test]
fn golden_property_process_invalidname() {
    assert_eq!(
        contract("property-process-invalidname", &[("name", "1bad")]).message(),
        "Invalid property name '1bad'"
    );
}

#[test]
fn golden_property_process_noname() {
    assert_eq!(
        contract(
            "property-process-noname",
            &[(
                "ast",
                "{\"$class\":\"concerto.metamodel@1.0.0.StringProperty\"}"
            )]
        )
        .message(),
        "No name for type {\"$class\":\"concerto.metamodel@1.0.0.StringProperty\"}"
    );
}

// Not a TS template: DIVERGENCES.md DV-017 (maintainer-accepted),
// Rust's replacement for TS's `TypeError` on a `RelationshipProperty`
// with no `type`.
#[test]
fn golden_property_process_relationshipnotype() {
    assert_eq!(
        contract("property-process-relationshipnotype", &[("name", "owner")]).message(),
        "Relationship owner must have a type"
    );
}

// Not a TS template: BC-45, Rust's check of a `DateTime` default
// value when it is applied.
#[test]
fn golden_typed_assignfielddefaults_datetime() {
    assert_eq!(
        contract(
            "typed-assignfielddefaults-datetime",
            &[("value", "2022-11-18"), ("fqn", "org.acme@1.0.0.Foo.bar")]
        )
        .message(),
        "Invalid default value `2022-11-18` for the DateTime field `org.acme@1.0.0.Foo.bar`: expected an ISO 8601 date-time with an offset, YYYY-MM-DDTHH:mm:ss[.SSS](Z|+HH:mm|-HH:mm), naming a real instant"
    );
}

// Not TS templates: BC-17, BC-19 and BC-20, the strict
// AST shape check at load.
#[test]
fn golden_modelfile_load_decoratorsnotarray() {
    assert_eq!(
        contract(
            "modelfile-load-decoratorsnotarray",
            &[("value", "\"💥emoji\"")]
        )
        .message(),
        "Invalid decorators. Expected array. Found \"💥emoji\""
    );
}

#[test]
fn golden_modelfile_load_namenotstring() {
    assert_eq!(
        contract("modelfile-load-namenotstring", &[("value", "1e308")]).message(),
        "Invalid name. Expected a string. Found 1e308"
    );
}

#[test]
fn golden_modelfile_load_supertypename() {
    assert_eq!(
        contract("modelfile-load-supertypename", &[("value", "\"\"")]).message(),
        "Invalid super type name. Expected a non-empty string. Found \"\""
    );
}

#[test]
fn golden_modelfile_load_nodenotobject() {
    assert_eq!(
        contract(
            "modelfile-load-nodenotobject",
            &[("key", "identified"), ("value", "true")]
        )
        .message(),
        "Invalid identified. Expected an object with a $class. Found true"
    );
}

#[test]
fn golden_modelfile_load_astshape() {
    assert_eq!(
        contract(
            "modelfile-load-astshape",
            &[(
                "message",
                "Unexpected properties for type concerto.metamodel@1.0.0.Model: undeclared"
            )]
        )
        .message(),
        "Model AST does not conform to the metamodel: Unexpected properties for type concerto.metamodel@1.0.0.Model: undeclared"
    );
}

#[test]
fn golden_modelfile_load_unreadable() {
    assert_eq!(
        contract(
            "modelfile-load-unreadable",
            &[("message", "missing field `name`")]
        )
        .message(),
        "Model AST could not be read: missing field `name`"
    );
}

// Not a TS template: DIVERGENCES.md DV-018 (maintainer-accepted),
// Rust's replacement for TS's `TypeError` on a `null` decorator node.
#[test]
fn golden_decorator_process_notobject() {
    assert_eq!(
        contract("decorator-process-notobject", &[("value", "null")]).message(),
        "Invalid decorator. Expected object. Found null"
    );
}

// TS: `Property.validate`'s own inline template
// (src/introspect/property.ts), checked against the
// frozen TS 5.0.0 reference.
#[test]
fn golden_property_validate_sizevalidator() {
    assert_eq!(
        contract(
            "property-validate-sizevalidator",
            &[("fqn", "org.acme@1.0.0.Foo.bar")]
        )
        .message(),
        "size validator can only be applied to array or map properties: org.acme@1.0.0.Foo.bar"
    );
}

// TS: `RelationshipDeclaration.validate`'s own inline templates
// (src/introspect/relationshipdeclaration.ts,61,80,86), checked
// against the frozen TS 5.0.0 reference.
#[test]
fn golden_relationshipdeclaration_validate_notype() {
    assert_eq!(
        contract("relationshipdeclaration-validate-notype", &[]).message(),
        "Relationship must have a type"
    );
}

#[test]
fn golden_relationshipdeclaration_validate_primitivetype() {
    assert_eq!(
        contract(
            "relationshipdeclaration-validate-primitivetype",
            &[("name", "bar"), ("type", "String")]
        )
        .message(),
        "Relationship bar cannot be to the primitive type String"
    );
}

#[test]
fn golden_relationshipdeclaration_validate_missingtype() {
    assert_eq!(
        contract(
            "relationshipdeclaration-validate-missingtype",
            &[("name", "bar"), ("type", "org.acme@1.0.0.Missing")]
        )
        .message(),
        "Relationship bar points to a missing type org.acme@1.0.0.Missing"
    );
}

#[test]
fn golden_relationshipdeclaration_validate_notidentified() {
    assert_eq!(
        contract(
            "relationshipdeclaration-validate-notidentified",
            &[("name", "bar"), ("type", "org.acme@1.0.0.Concept")]
        )
        .message(),
        "Relationship bar must be to a class that has an identifier, but this is to org.acme@1.0.0.Concept"
    );
}

// TS: `Field.getScalarField`'s own inline templates
// (src/introspect/field.ts,215), checked against the
// frozen TS 5.0.0 reference.
#[test]
fn golden_field_getscalarfield_notscalar() {
    assert_eq!(
        contract("field-getscalarfield-notscalar", &[("name", "bar")]).message(),
        "Field bar is not a scalar property."
    );
}

#[test]
fn golden_field_getscalarfield_unrecognizedtype() {
    assert_eq!(
        contract(
            "field-getscalarfield-unrecognizedtype",
            &[("class", "concerto.metamodel@1.0.0.MapScalar")]
        )
        .message(),
        "Unrecognized scalar type concerto.metamodel@1.0.0.MapScalar"
    );
}

// TS: `MapDeclaration.process`'s own inline templates
// (src/introspect/mapdeclaration.ts,67,71), checked
// against the frozen TS 5.0.0 reference.
#[test]
fn golden_mapdeclaration_process_missingkeyvalue() {
    assert_eq!(
        contract("mapdeclaration-process-missingkeyvalue", &[("name", "Foo")]).message(),
        "MapDeclaration must contain Key & Value properties Foo"
    );
}

#[test]
fn golden_mapdeclaration_process_invalidkey() {
    assert_eq!(
        contract("mapdeclaration-process-invalidkey", &[("name", "Foo")]).message(),
        "MapDeclaration must contain valid MapKeyType  Foo"
    );
}

#[test]
fn golden_mapdeclaration_process_invalidvalue() {
    assert_eq!(
        contract("mapdeclaration-process-invalidvalue", &[("name", "Foo")]).message(),
        "MapDeclaration must contain valid MapValueType, for MapDeclaration Foo"
    );
}

// TS: `MapKeyType.validate`'s own inline template
// (src/introspect/mapkeytype.ts), checked against the
// frozen TS 5.0.0 reference.
#[test]
fn golden_mapkeytype_validate_invalidscalar() {
    assert_eq!(
        contract(
            "mapkeytype-validate-invalidscalar",
            &[("type", "org.acme@1.0.0.Foo"), ("name", "Bar")]
        )
        .message(),
        "Scalar must be one of StringScalar, DateTimeScalar in context of MapKeyType. Invalid Scalar: org.acme@1.0.0.Foo, for MapDeclaration Bar"
    );
}

// TS: `MapValueType.validate`/`processType`'s own inline templates
// (src/introspect/mapvaluetype.ts,98,103,108), checked against the
// frozen TS 5.0.0 reference.
#[test]
fn golden_mapvaluetype_validate_mapnotsupported() {
    assert_eq!(
        contract(
            "mapvaluetype-validate-mapnotsupported",
            &[("type", "org.acme@1.0.0.Foo")]
        )
        .message(),
        "MapDeclaration as Map Type Value is not supported: org.acme@1.0.0.Foo"
    );
}

#[test]
fn golden_mapvaluetype_process_missingtype() {
    assert_eq!(
        contract("mapvaluetype-process-missingtype", &[("name", "Foo")]).message(),
        "ObjectMapValueType must contain property 'type', for MapDeclaration named Foo"
    );
}

#[test]
fn golden_mapvaluetype_process_malformedtype() {
    assert_eq!(
        contract("mapvaluetype-process-malformedtype", &[("name", "Foo")]).message(),
        "ObjectMapValueType type must contain property '$class' and property 'name', for MapDeclaration named Foo"
    );
}

#[test]
fn golden_mapvaluetype_process_invalidtypeclass() {
    assert_eq!(
        contract("mapvaluetype-process-invalidtypeclass", &[("name", "Foo")]).message(),
        "ObjectMapValueType type $class must be of TypeIdentifier for MapDeclaration named Foo"
    );
}

#[test]
fn golden_instancegenerator_newinstance_noconcreteclass() {
    assert_eq!(
        contract(
            "instancegenerator-newinstance-noconcreteclass",
            &[("type", "org.acme@1.0.0.Foo")]
        )
        .message(),
        "No concrete extending type for \"org.acme@1.0.0.Foo\"."
    );
}

#[test]
fn golden_serializer_tojson_notcobject() {
    assert_eq!(
        contract("serializer-tojson-notcobject", &[]).message(),
        "\"Serializer.toJSON\" only accepts \"Concept\", \"Event\", \"Asset\", \"Participant\" or \"Transaction\"."
    );
}

#[test]
fn golden_serializer_visit_unrecognised() {
    assert_eq!(
        contract(
            "serializer-visit-unrecognised",
            &[("name", "org.acme@1.0.0.Color.RED")]
        )
        .message(),
        "Unrecognised element \"org.acme@1.0.0.Color.RED\""
    );
}

#[test]
fn golden_engine_typeerror_convertnulltoobject() {
    assert_eq!(
        contract("engine-typeerror-convertnulltoobject", &[]).message(),
        "Cannot convert undefined or null to object"
    );
}

#[test]
fn golden_serializer_constructor_factorynull() {
    assert_eq!(
        contract("serializer-constructor-factorynull", &[]).message(),
        "\"Factory\" cannot be \"null\"."
    );
}

#[test]
fn golden_serializer_constructor_modelmanagernull() {
    assert_eq!(
        contract("serializer-constructor-modelmanagernull", &[]).message(),
        "\"ModelManager\" cannot be \"null\"."
    );
}

#[test]
fn golden_serializer_fromjson_noclass() {
    assert_eq!(
        contract("serializer-fromjson-noclass", &[]).message(),
        "Invalid JSON data. Does not contain a $class type identifier."
    );
}

#[test]
fn golden_serializer_fromjson_mapnotsupported() {
    assert_eq!(
        contract("serializer-fromjson-mapnotsupported", &[]).message(),
        "Attempting to create a Map declaration is not supported."
    );
}

#[test]
fn golden_serializer_fromjson_enumnotsupported() {
    assert_eq!(
        contract("serializer-fromjson-enumnotsupported", &[]).message(),
        "Attempting to create an ENUM declaration is not supported."
    );
}

#[test]
fn golden_factory_newresource_idregexmismatch() {
    assert_eq!(
        contract(
            "factory-newresource-idregexmismatch",
            &[("regex", "/\\d{3}/")]
        )
        .message(),
        "Provided id does not match regex: /\\d{3}/"
    );
}

#[test]
fn golden_factory_newresource_notidentifiable() {
    assert_eq!(
        contract(
            "factory-newresource-notidentifiable",
            &[("fqn", "org.acme@1.0.0.C")]
        )
        .message(),
        "Type is not identifiable org.acme@1.0.0.C"
    );
}

#[test]
fn golden_factory_newrelationship_notidentifiable() {
    assert_eq!(
        contract(
            "factory-newrelationship-notidentifiable",
            &[("fqn", "org.acme@1.0.0.C")]
        )
        .message(),
        "Cannot create a relationship to org.acme@1.0.0.C, it is not identifiable."
    );
}

#[test]
fn golden_factory_newtransaction_nsnotspecified() {
    assert_eq!(
        contract("factory-newtransaction-nsnotspecified", &[]).message(),
        "ns not specified"
    );
}

#[test]
fn golden_factory_newtransaction_typenotspecified() {
    assert_eq!(
        contract("factory-newtransaction-typenotspecified", &[]).message(),
        "type not specified"
    );
}

#[test]
fn golden_factory_newtransaction_notatransaction() {
    assert_eq!(
        contract(
            "factory-newtransaction-notatransaction",
            &[("fqn", "org.acme@1.0.0.A")]
        )
        .message(),
        "org.acme@1.0.0.A is not a transaction"
    );
}

#[test]
fn golden_factory_newevent_notanevent() {
    assert_eq!(
        contract(
            "factory-newevent-notanevent",
            &[("fqn", "org.acme@1.0.0.A")]
        )
        .message(),
        "org.acme@1.0.0.A is not an event"
    );
}

#[test]
fn golden_jsonpopulator_getassignableproperties_reservedproperties() {
    assert_eq!(
        contract(
            "jsonpopulator-getassignableproperties-reservedproperties",
            &[
                ("fqn", "org.acme@1.0.0.C"),
                ("properties", "$type, $namespace")
            ]
        )
        .message(),
        "Unexpected reserved properties for type org.acme@1.0.0.C: $type, $namespace"
    );
}

#[test]
fn golden_jsonpopulator_getassignableproperties_timestamp() {
    assert_eq!(
        contract(
            "jsonpopulator-getassignableproperties-timestamp",
            &[("fqn", "org.acme@1.0.0.C")]
        )
        .message(),
        "Unexpected property for type org.acme@1.0.0.C: $timestamp"
    );
}

#[test]
fn golden_jsonpopulator_validateproperties_unexpectedproperties() {
    assert_eq!(
        contract(
            "jsonpopulator-validateproperties-unexpectedproperties",
            &[("fqn", "org.acme@1.0.0.C"), ("properties", "a, b")]
        )
        .message(),
        "Unexpected properties for type org.acme@1.0.0.C: a, b"
    );
}

// ---- DeserializeOptions (accordproject/concerto#1273) ----

#[test]
fn golden_jsonpopulator_rejectunknownkeys_unknownproperties() {
    assert_eq!(
        contract(
            "jsonpopulator-rejectunknownkeys-unknownproperties",
            &[("fqn", "org.acme@1.0.0.C"), ("properties", "a, b")]
        )
        .message(),
        "Unexpected properties for type org.acme@1.0.0.C: a, b"
    );
}

#[test]
fn golden_jsonpopulator_rejectrequirednull_requirednull() {
    assert_eq!(
        contract(
            "jsonpopulator-rejectrequirednull-requirednull",
            &[
                ("path", "$.declarations[0].properties[0].name"),
                ("type", "String")
            ]
        )
        .message(),
        "Expected value at path `$.declarations[0].properties[0].name` to be of type `String`, but got null"
    );
}

#[test]
fn detail_codes_are_spelled_as_in_1273() {
    assert_eq!(DetailCode::UnknownProperty.as_str(), "UNKNOWN_PROPERTY");
    assert_eq!(DetailCode::TypeViolation.as_str(), "TYPE_VIOLATION");
}

#[test]
fn golden_jsonpopulator_visitfield_notarray() {
    assert_eq!(
        contract(
            "jsonpopulator-visitfield-notarray",
            &[("path", "$.a"), ("type", "String")]
        )
        .message(),
        "Expected value at path `$.a` to be an array of type `String`"
    );
}

#[test]
fn golden_jsonpopulator_converttoobject_wrongtype() {
    assert_eq!(
        contract(
            "jsonpopulator-converttoobject-wrongtype",
            &[("path", "$.a"), ("type", "Integer")]
        )
        .message(),
        "Expected value at path `$.a` to be of type `Integer`"
    );
}

#[test]
fn golden_jsonpopulator_converttoobject_datetimeformat() {
    assert_eq!(
        contract(
            "jsonpopulator-converttoobject-datetimeformat",
            &[("path", "$.d"), ("type", "DateTime")]
        )
        .message(),
        "Expected value at path `$.d` to be of type `DateTime` with format YYYY-MM-DDTHH:mm:ss[Z]"
    );
}

#[test]
fn golden_jsonpopulator_visitrelationshipdeclaration_notastring() {
    assert_eq!(
            contract("jsonpopulator-visitrelationshipdeclaration-notastring", &[("value", "[object Object]"), ("relationship", "RelationshipDeclaration {name=r, type=org.acme@1.0.0.A, array=false, optional=false}")]).message(),
            "Invalid JSON data. Found a value that is not a string: [object Object] for relationship RelationshipDeclaration {name=r, type=org.acme@1.0.0.A, array=false, optional=false}"
        );
}

#[test]
fn golden_jsonpopulator_visitrelationshipdeclaration_noclass() {
    assert_eq!(
            contract("jsonpopulator-visitrelationshipdeclaration-noclass", &[("value", "[object Object]"), ("relationship", "RelationshipDeclaration {name=r, type=org.acme@1.0.0.A, array=false, optional=false}")]).message(),
            "Invalid JSON data. Does not contain a $class type identifier: [object Object] for relationship RelationshipDeclaration {name=r, type=org.acme@1.0.0.A, array=false, optional=false}"
        );
}

#[test]
fn golden_jsonpopulator_visitrelationshipdeclaration_notstringorobject() {
    assert_eq!(
            contract("jsonpopulator-visitrelationshipdeclaration-notstringorobject", &[("value", "1"), ("relationship", "RelationshipDeclaration {name=r, type=org.acme@1.0.0.A, array=false, optional=false}")]).message(),
            "Invalid JSON data. Found a value that is not a string or object: 1 for relationship RelationshipDeclaration {name=r, type=org.acme@1.0.0.A, array=false, optional=false}"
        );
}

#[test]
fn golden_jsongenerator_visitclassdeclaration_notaresource() {
    assert_eq!(
        contract(
            "jsongenerator-visitclassdeclaration-notaresource",
            &[("obj", "Relationship {id=org.acme@1.0.0.A#1}")]
        )
        .message(),
        "Expected a Resource, but found Relationship {id=org.acme@1.0.0.A#1}"
    );
}

#[test]
fn golden_jsongenerator_getrelationshiptext_norelationship() {
    assert_eq!(
        contract(
            "jsongenerator-getrelationshiptext-norelationship",
            &[
                ("type", "org.acme@1.0.0.A"),
                ("obj", "Resource {id=org.acme@1.0.0.A#1}")
            ]
        )
        .message(),
        "Did not find a relationship for org.acme@1.0.0.A found Resource {id=org.acme@1.0.0.A#1}"
    );
}

#[test]
fn golden_typedstack_push_unexpectedtype() {
    assert_eq!(
        contract(
            "typedstack-push-unexpectedtype",
            &[("type", "Typed"), ("obj", "abc")]
        )
        .message(),
        "Did not find expected type Typed as argument to push. Found: abc"
    );
}

#[test]
fn golden_typed_tojson_useserializer() {
    assert_eq!(
        contract("typed-tojson-useserializer", &[]).message(),
        "Use Serializer.toJSON to convert resource instances to JSON objects."
    );
}

#[test]
fn golden_validatedresource_setpropertyvalue_undeclaredfield() {
    assert_eq!(
        contract(
            "validatedresource-setpropertyvalue-undeclaredfield",
            &[("id", "1"), ("propName", "x")]
        )
        .message(),
        "The instance with id 1 trying to set field x which is not declared in the model."
    );
}

#[test]
fn golden_validatedresource_addarrayvalue_notanarray() {
    assert_eq!(
        contract(
            "validatedresource-addarrayvalue-notanarray",
            &[("id", "1"), ("propName", "x")]
        )
        .message(),
        "The instance with id 1 trying to add array item x which is not declared as an array in the model."
    );
}

#[test]
fn golden_resourcevalidator_fieldtypeviolation() {
    assert_eq!(
        contract(
            "resourcevalidator-fieldtypeviolation",
            &[
                ("resourceId", "org.acme.Foo#1"),
                ("propertyName", "bar"),
                ("fieldType", "String"),
                ("value", "42"),
                ("typeOfValue", "number")
            ]
        )
        .message(),
        "Model violation in the \"org.acme.Foo#1\" instance. The field \"bar\" has a value of \"42\" (type of value: \"number\"). Expected type of value: \"String\"."
    );
}

#[test]
fn golden_resourcevalidator_notresourceorconcept() {
    assert_eq!(
        contract(
            "resourcevalidator-notresourceorconcept",
            &[
                ("resourceId", "org.acme.Foo#1"),
                ("classFQN", "org.acme.Bar"),
                ("invalidValue", "42")
            ]
        )
        .message(),
        "Model violation in the \"org.acme.Foo#1\" instance. Class \"org.acme.Bar\" has the value of \"42\". Expected a \"Resource\" or a \"Concept\"."
    );
}

#[test]
fn golden_resourcevalidator_notrelationship() {
    assert_eq!(
        contract(
            "resourcevalidator-notrelationship",
            &[
                ("resourceId", "org.acme.Foo#1"),
                ("classFQN", "org.acme.Bar"),
                ("invalidValue", "42")
            ]
        )
        .message(),
        "Model violation in the \"org.acme.Foo#1\" instance. Class \"org.acme.Bar\" has a value of \"42\". Expected a \"Relationship\"."
    );
}

#[test]
fn golden_resourcevalidator_missingrequiredproperty() {
    assert_eq!(
        contract(
            "resourcevalidator-missingrequiredproperty",
            &[("resourceId", "org.acme.Foo#1"), ("fieldName", "bar")]
        )
        .message(),
        "The instance \"org.acme.Foo#1\" is missing the required field \"bar\"."
    );
}

#[test]
fn golden_resourcevalidator_emptyidentifier() {
    assert_eq!(
        contract(
            "resourcevalidator-emptyidentifier",
            &[("resourceId", "org.acme.Foo#1")]
        )
        .message(),
        "Instance \"org.acme.Foo#1\" has an empty identifier."
    );
}

#[test]
fn golden_resourcevalidator_invalidenumvalue() {
    assert_eq!(
        contract(
            "resourcevalidator-invalidenumvalue",
            &[
                ("resourceId", "org.acme.Foo#1"),
                ("value", "BLUE"),
                ("fieldName", "color")
            ]
        )
        .message(),
        "Model violation in the \"org.acme.Foo#1\" instance. Invalid enum value of \"BLUE\" for the field \"color\"."
    );
}

#[test]
fn golden_resourcevalidator_abstractclass() {
    assert_eq!(
        contract(
            "resourcevalidator-abstractclass",
            &[("className", "org.acme.Foo")]
        )
        .message(),
        "The class \"org.acme.Foo\" is abstract and should not contain an instance."
    );
}

#[test]
fn golden_resourcevalidator_undeclaredfield() {
    assert_eq!(
        contract(
            "resourcevalidator-undeclaredfield",
            &[
                ("resourceId", "org.acme.Foo#1"),
                ("propertyName", "bar"),
                ("fullyQualifiedTypeName", "org.acme.Foo")
            ]
        )
        .message(),
        "Instance \"org.acme.Foo#1\" has a property named \"bar\", which is not declared in \"org.acme.Foo\"."
    );
}

#[test]
fn golden_resourcevalidator_invalidfieldassignment() {
    assert_eq!(
        contract(
            "resourcevalidator-invalidfieldassignment",
            &[
                ("resourceId", "org.acme.Foo#1"),
                ("propertyName", "bar"),
                ("objectType", "org.acme.Baz"),
                ("fieldType", "org.acme.Bar")
            ]
        )
        .message(),
        "Instance \"org.acme.Foo#1\" has a property \"bar\" with type \"org.acme.Baz\" that is not derived from \"org.acme.Bar\"."
    );
}

#[test]
fn golden_resourcevalidator_checkmaptype_expectedstring() {
    assert_eq!(
        contract(
            "resourcevalidator-checkmaptype-expectedstring",
            &[("mapFqn", "org.acme.Foo"), ("value", "42")]
        )
        .message(),
        "Model violation in org.acme.Foo. Expected Type of String but found '42' instead."
    );
}

#[test]
fn golden_resourcevalidator_checkmaptype_expecteddatetime() {
    assert_eq!(
        contract(
            "resourcevalidator-checkmaptype-expecteddatetime",
            &[("mapFqn", "org.acme.Foo"), ("value", "not-a-date")]
        )
        .message(),
        "Model violation in org.acme.Foo. Expected Type of DateTime but found 'not-a-date' instead."
    );
}

#[test]
fn golden_resourcevalidator_checkmaptype_expectedboolean() {
    assert_eq!(
        contract(
            "resourcevalidator-checkmaptype-expectedboolean",
            &[
                ("mapFqn", "org.acme.Foo"),
                ("type", "string"),
                ("value", "true")
            ]
        )
        .message(),
        "Model violation in org.acme.Foo. Expected Type of Boolean but found string instead, for value 'true'."
    );
}

#[test]
fn golden_resourcevalidator_visitmapdeclaration_notamap() {
    assert_eq!(
        contract(
            "resourcevalidator-visitmapdeclaration-notamap",
            &[("obj", "\"not-a-map\"")]
        )
        .message(),
        "Expected a Map, but found \"not-a-map\""
    );
}

#[test]
fn golden_resourcevalidator_checkrelationship_notidentifiable() {
    assert_eq!(
        contract("resourcevalidator-checkrelationship-notidentifiable", &[]).message(),
        "Cannot have a relationship to a field that is not identifiable."
    );
}

// ---- The model-validation checks' TS hardcoded strings, checked
//      against the frozen TS 5.0.0 reference ----

// TS: `ModelFile.validate`'s duplicate-name check
// (src/introspect/modelfile.ts).
#[test]
fn golden_modelfile_validate_duplicateclassname() {
    assert_eq!(
        contract(
            "modelfile-validate-duplicateclassname",
            &[("fqn", "org.acme@1.0.0.A")]
        )
        .message(),
        "Duplicate class name org.acme@1.0.0.A"
    );
}

// TS: `Declaration.validate` (src/introspect/declaration.ts).
#[test]
fn golden_declaration_validate_importclash() {
    assert_eq!(
        contract("declaration-validate-importclash", &[("name", "Address")]).message(),
        "Type 'Address' clashes with an imported type with the same name."
    );
}

// TS: `Decorated.validate` (src/introspect/decorated.ts); a
// decorator with no name reads `undefined`.
#[test]
fn golden_decorated_validate_duplicatedecorator() {
    assert_eq!(
        contract("decorated-validate-duplicatedecorator", &[("name", "Term")]).message(),
        "Duplicate decorator Term"
    );
    assert_eq!(
        contract(
            "decorated-validate-duplicatedecorator",
            &[("name", "undefined")]
        )
        .message(),
        "Duplicate decorator undefined"
    );
}

// TS: `ClassDeclaration._resolveSuperType` and `getSuperTypeDeclaration`
// (src/introspect/classdeclaration.ts,190,553).
#[test]
fn golden_engine_validateinstanceas_notassignable() {
    assert_eq!(
        contract(
            "engine-validateinstanceas-notassignable",
            &[
                ("type", "org.acme@1.0.0.Cat"),
                ("declared", "org.acme@1.0.0.Dog")
            ]
        )
        .message(),
        "'org.acme@1.0.0.Cat' is not assignable to 'org.acme@1.0.0.Dog'"
    );
}

#[test]
fn golden_classdeclaration_resolvesupertype_notfound() {
    assert_eq!(
        contract(
            "classdeclaration-resolvesupertype-notfound",
            &[("superType", "Vehicle")]
        )
        .message(),
        "Could not find super type Vehicle"
    );
}

#[test]
fn golden_classdeclaration_resolvesupertype_kindmismatch() {
    assert_eq!(
        contract(
            "classdeclaration-resolvesupertype-kindmismatch",
            &[
                ("kind", "AssetDeclaration"),
                ("name", "Car"),
                ("superKind", "ParticipantDeclaration"),
                ("superName", "Person"),
            ]
        )
        .message(),
        "AssetDeclaration (Car) cannot extend ParticipantDeclaration (Person)"
    );
}

// TS: `ClassDeclaration.validate` (src/introspect/classdeclaration.ts,
// 258, 263).
#[test]
fn golden_classdeclaration_validate_identifieroptional() {
    assert_eq!(
        contract("classdeclaration-validate-identifieroptional", &[]).message(),
        "Identifying fields cannot be optional."
    );
}

#[test]
fn golden_classdeclaration_validate_redeclaredidentifier() {
    assert_eq!(
        contract(
            "classdeclaration-validate-redeclaredidentifier",
            &[("superType", "org.acme@1.0.0.Base"), ("idField", "id")]
        )
        .message(),
        "Super class org.acme@1.0.0.Base has an explicit identifier id that cannot be redeclared."
    );
}

/// A call-site code with no catalogue entry fails a debug build instead
/// of rendering the code itself as the message.
#[test]
#[cfg(debug_assertions)]
#[should_panic(expected = "has no catalogue entry")]
fn an_uncatalogued_code_fails_a_debug_build() {
    let _ = ContractError::new(ErrorKind::IllegalModel, "no-such-code", vec![]);
}

/// The one non-catalogue renderer: [`ContractError::pre_port`] carries a
/// hand-written message verbatim. This is not a golden test against the TS
/// reference; it only pins the passthrough behaviour.
#[test]
fn golden_pre_port() {
    let err = ContractError::pre_port(
        ErrorKind::IllegalModel,
        "duplicate namespace: org.acme@1.0.0".into(),
        None,
    );
    assert_eq!(err.code, "pre-port");
    assert_eq!(err.message(), "duplicate namespace: org.acme@1.0.0");
}

/// [`ContractError::type_not_found`] appends `typeName` after the
/// template's own params (table 2.3), without disturbing rendering.
#[test]
fn type_not_found_constructor_adds_type_name_param() {
    let err = ContractError::type_not_found(
        "modelmanager-gettype-noregisteredns",
        vec![("type", "org.acme@1.0.0.Doge".to_string())],
        "org.acme@1.0.0.Doge".to_string(),
        None,
    );
    assert_eq!(err.kind, ErrorKind::TypeNotFound);
    assert_eq!(
        err.params.last(),
        Some(&("typeName", "org.acme@1.0.0.Doge".to_string()))
    );
    assert_eq!(
        err.message(),
        "Namespace is not defined for type \"org.acme@1.0.0.Doge\"."
    );
}

/// The catalogue holds exactly the en.json keys the ported throw sites
/// use, plus `factory-newinstance-*` and
/// `typenotfounderror-defaultmessage`, and those keys are exactly its
/// [`Renderer::Globalize`] entries (a [`Renderer::Inline`] entry is an
/// inline template given an en.json-style name, such as
/// `modelutil-parsenamespace-invalidnamespace`). `catalogue.rs` checks
/// the other way (every entry has a golden test).
#[test]
fn od5_catalogue_scope_is_present() {
    const OD5_EN_JSON_KEYS: &[&str] = &[
        // Used today: ModelUtil.getNamespace (src/modelutil.ts, RUST).
        "modelutil-getnamespace-nofnq",
        // Not used by a call site of their own.
        "typenotfounderror-defaultmessage",
        "factory-newinstance-missingidentifier",
        "factory-newinstance-invalididentifier",
        "factory-newinstance-abstracttype",
        "factory-newinstance-typenotdeclaredinns",
        // BaseModelManager.resolveType/getType (RUST) and ModelFile
        // (constructor, resolveType, resolveImport, validate; RUST).
        "modelmanager-gettype-noregisteredns",
        "modelmanager-resolvetype-nonsfortype",
        "modelmanager-resolvetype-notypeinnsforcontext",
        "modelmanager-gettype-notypeinns",
        "modelmanager-gettype-duplicatensimport",
        "modelfile-resolvetype-undecltype",
        "modelfile-resolveimport-failfindimp",
        "modelfile-constructor-unrecmodelelem",
        // ClassDeclaration.process/validate (RUST).
        "classdeclaration-validate-undefined-properties",
        "classdeclaration-process-unrecmodelelem",
        "classdeclaration-validate-selfextending",
        "classdeclaration-validate-identifiernotproperty",
        "classdeclaration-validate-identifiernotstring",
        "classdeclaration-validate-duplicatefieldname",
        // InstanceGenerator.findConcreteSubclass, reached from
        // newInstance (RUST).
        "instancegenerator-newinstance-noconcreteclass",
        // Serializer.toJSON (HYBRID).
        "serializer-tojson-notcobject",
        // ResourceValidator (HYBRID): every report* method.
        "resourcevalidator-fieldtypeviolation",
        "resourcevalidator-notresourceorconcept",
        "resourcevalidator-notrelationship",
        "resourcevalidator-missingrequiredproperty",
        "resourcevalidator-emptyidentifier",
        "resourcevalidator-invalidenumvalue",
        "resourcevalidator-abstractclass",
        "resourcevalidator-undeclaredfield",
        "resourcevalidator-invalidfieldassignment",
        // `Serializer.constructor` (tests/oracle/ledger.rs,
        // `PLAN_OWNER_OVERRIDES`).
        "serializer-constructor-factorynull",
        "serializer-constructor-modelmanagernull",
    ];
    for key in OD5_EN_JSON_KEYS {
        assert!(
            catalogue_entry(key).is_some(),
            "{key} missing from the catalogue"
        );
        assert_eq!(catalogue_entry(key).unwrap().renderer, Renderer::Globalize);
    }
    let globalize_entries: std::collections::HashSet<&str> = CATALOGUE
        .iter()
        .filter(|entry| entry.renderer == Renderer::Globalize)
        .map(|entry| entry.code)
        .collect();
    let od5_keys: std::collections::HashSet<&str> = OD5_EN_JSON_KEYS.iter().copied().collect();
    assert_eq!(
        globalize_entries, od5_keys,
        "every Renderer::Globalize entry must be exactly the OD-5 scope, no more and no less"
    );
}

#[test]
fn illegal_model_decoration_matches_the_ts_constructor() {
    let mut err = contract(
        "scalardeclaration-process-primitivename",
        &[("scalarName", "String")],
    );
    err.kind = ErrorKind::IllegalModel;
    assert_eq!(
        err.final_message(),
        "Invalid scalar name 'String'. Name conflicts with primitive type. "
    );
    err.model_file = Some(Some("org.acme.cto".into()));
    err.location = Some(crate::json!({
        "start": {"line": 1, "column": 2}, "end": {"line": 3, "column": 4}
    }));
    assert_eq!(
        err.final_message(),
        "Invalid scalar name 'String'. Name conflicts with primitive type. File 'org.acme.cto': line 1 column 2, to line 3 column 4. "
    );
}

#[test]
fn type_not_found_displays_name() {
    let err = Error::type_not_found("org.acme@1.0.0.Foo");
    assert!(err.to_string().contains("org.acme@1.0.0.Foo"));
}

#[test]
fn illegal_model_displays_message() {
    let err = Error::illegal_model("missing 'namespace'", Some("model.json".into()), None);
    assert!(err.to_string().contains("missing 'namespace'"));
}

/// A `Range` whose `start` and `end` positions carry the given numbers.
fn range(start: [f64; 3], end: [f64; 3]) -> concerto_metamodel::concerto_metamodel_1_0_0::Range {
    let position = |[line, column, offset]: [f64; 3]| {
        crate::json!({
            "$class": "concerto.metamodel@1.0.0.Position",
            "line": line, "column": column, "offset": offset
        })
    };
    crate::json::from_value(crate::json!({
        "$class": "concerto.metamodel@1.0.0.Range",
        "start": position(start),
        "end": position(end)
    }))
    .unwrap()
}

/// The `start` and `end` positions of a [`location_value`] result, as
/// JSON text, the way `JSON.stringify` would write them.
fn positions(start: [f64; 3], end: [f64; 3]) -> (String, String) {
    let value = location_value(&range(start, end)).unwrap();
    let text = |key: &str| {
        let position = &value[key];
        format!(
            "{},{},{}",
            position["line"], position["column"], position["offset"]
        )
    };
    (text("start"), text("end"))
}

#[test]
fn location_value_writes_integral_numbers_as_integers() {
    // Both nested positions are rewritten, not just the first.
    assert_eq!(
        positions([3.0, 1.0, 20.0], [3.0, 9.0, 28.0]),
        ("3,1,20".to_string(), "3,9,28".to_string())
    );
    let value = location_value(&range([3.0, 1.0, 20.0], [3.0, 9.0, 28.0])).unwrap();
    assert!(value["start"]["line"].is_i64());
    assert!(value["end"]["offset"].is_i64());
}

#[test]
fn location_value_keeps_negative_integers_and_zeroes_negative_zero() {
    assert_eq!(
        positions([-3.0, -0.0, 0.0], [-1.0, -0.0, -9_007_199_254_740_993.0]),
        (
            "-3,0,0".to_string(),
            // -(2^53 + 1) is not an f64; it rounds to -(2^53), as in JS.
            "-1,0,-9007199254740992".to_string()
        )
    );
}

#[test]
fn location_value_leaves_non_integral_numbers_alone() {
    assert_eq!(
        positions([1.5, 0.25, -2.75], [1.0, 2.0, 3.0]),
        ("1.5,0.25,-2.75".to_string(), "1,2,3".to_string())
    );
}

#[test]
fn location_value_writes_integers_above_2_pow_53_as_their_exact_value() {
    // 2^53 + 2 and 2^63 + 2^11 are exact f64 integers, written as their
    // exact integer value. That equals the JS number, but is not always
    // JSON.stringify's text: JS prints 2^63 + 2^11 as
    // 9223372036854778000 (see `location_value`).
    assert_eq!(
        positions(
            [9_007_199_254_740_994.0, 1.0, 1.0],
            [9_223_372_036_854_777_856.0, 1.0, 1.0]
        ),
        (
            "9007199254740994,1,1".to_string(),
            "9223372036854777856,1,1".to_string()
        )
    );
    // 2^64 and beyond do not fit an integer JSON number here; the float
    // form is kept (documented on `location_value`).
    let value = location_value(&range(
        [18_446_744_073_709_551_616.0, 1.0, 1.0],
        [1.0, 1.0, 1.0],
    ))
    .unwrap();
    assert!(value["start"]["line"].is_f64());
}
