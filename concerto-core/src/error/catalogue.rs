//! The message catalogue (PORTING.md section 2.2): the message templates the
//! engine's errors render.
//!
//! - `messages/en.json` keys, verbatim: every key a ported throw site uses,
//!   plus `factory-newinstance-*` and `typenotfounderror-defaultmessage`
//!   (table 2.3). Keys with no throw site in concerto-core (`composer-*`,
//!   `whereastvalidator-*`, `like`, `test-*`) stay out.
//! - The inline templates (template literals and string concatenations,
//!   2.2 step 2) of the ported throw sites, each `${expr}` a named `{param}`.
//! - `"pre-port"`, the escape hatch [`super::ContractError::pre_port`] uses
//!   for a message with no catalogue entry (module doc on [`super`]).
//! - Replacements for a TS crash (category `maintainer-accepted` in
//!   DIVERGENCES.md, citing their DV row in `sources`), and checks TS does
//!   not make, added as breaking changes (citing their BC row).

use super::{CatalogueEntry, Renderer};

/// The message catalogue. Every entry but `"pre-port"` and the
/// maintainer-accepted DIVERGENCES.md replacements (each cites its DV row in
/// `sources`) is a verbatim TS template, byte for byte, with the throw
/// site(s) it was ported from.
pub const CATALOGUE: &[CatalogueEntry] = &[
    // ---- ModelUtil, NumberValidator, ScalarDeclaration ----
    CatalogueEntry {
        code: "modelutil-getnamespace-nofnq",
        template: "FQN is invalid.",
        renderer: Renderer::Globalize,
        sources: &["src/modelutil.ts:93"],
    },
    CatalogueEntry {
        code: "modelutil-parsenamespace-nullorundefined",
        template: "Namespace is null or undefined.",
        renderer: Renderer::Inline,
        sources: &["src/modelutil.ts:124"],
    },
    CatalogueEntry {
        code: "modelutil-parsenamespace-invalidnamespace",
        template: "Invalid namespace {ns}",
        renderer: Renderer::Inline,
        sources: &["src/modelutil.ts:130", "src/modelutil.ts:136"],
    },
    CatalogueEntry {
        code: "modelutil-isassignableto-cannotfindtype",
        template: "Cannot find type {typeName}",
        renderer: Renderer::Inline,
        sources: &["src/modelutil.ts:196"],
    },
    CatalogueEntry {
        code: "metamodelutil-importfullyqualifiednames-unrecognizedimports",
        template: "Unrecognized imports {$class}",
        renderer: Renderer::Inline,
        sources: &["@accordproject/concerto-metamodel@3.17.0 lib/metamodelutil.js:257"],
    },
    CatalogueEntry {
        code: "validator-reporterror",
        template: "Validator error for field `{id}`. {fqn}: {msg}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/validator.ts:82"],
    },
    CatalogueEntry {
        code: "numbervalidator-constructor-nobounds",
        template: "Invalid range, lower and-or upper bound must be specified.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/numbervalidator.ts:65"],
    },
    CatalogueEntry {
        code: "numbervalidator-constructor-lowerhigherthanupper",
        template: "Lower bound must be less than or equal to upper bound.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/numbervalidator.ts:70"],
    },
    CatalogueEntry {
        code: "numbervalidator-constructor-outsidelowerbound",
        template: "Value {value} is outside lower bound {lowerBound}",
        renderer: Renderer::Inline,
        sources: &[
            "src/introspect/numbervalidator.ts:77",
            "src/introspect/numbervalidator.ts:111",
        ],
    },
    CatalogueEntry {
        code: "numbervalidator-constructor-outsideupperbound",
        template: "Value {value} is outside upper bound {upperBound}",
        renderer: Renderer::Inline,
        sources: &[
            "src/introspect/numbervalidator.ts:81",
            "src/introspect/numbervalidator.ts:115",
        ],
    },
    // ---- StringValidator, CollectionSizeValidator ----
    CatalogueEntry {
        code: "stringvalidator-constructor-invalidlength",
        template: "Invalid string length, minLength and-or maxLength must be specified.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/stringvalidator.ts:65"],
    },
    CatalogueEntry {
        code: "stringvalidator-constructor-negativelength",
        template: "minLength and-or maxLength must be positive integers.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/stringvalidator.ts:67"],
    },
    CatalogueEntry {
        code: "stringvalidator-constructor-mingreaterthanmax",
        template: "minLength must be less than or equal to maxLength.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/stringvalidator.ts:71"],
    },
    CatalogueEntry {
        code: "stringvalidator-constructor-invalidregex",
        // Not a TS template: the message is whatever the regex engine threw
        // (V8 in TS, `regress` here), passed through verbatim.
        template: "{message}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/stringvalidator.ts:84"],
    },
    CatalogueEntry {
        code: "stringvalidator-validate-belowminlength",
        template: "The string length of '{value}' should be at least {minLength} characters.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/stringvalidator.ts:104"],
    },
    CatalogueEntry {
        code: "stringvalidator-validate-abovemaxlength",
        template: "The string length of '{value}' should not exceed {maxLength} characters.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/stringvalidator.ts:107"],
    },
    CatalogueEntry {
        code: "stringvalidator-validate-regexmismatch",
        template: "Value '{value}' failed to match validation regex: {regex}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/stringvalidator.ts:111"],
    },
    CatalogueEntry {
        code: "collectionsizevalidator-constructor-nosize",
        template: "Invalid collection size, minSize and/or maxSize must be specified.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/collectionsizevalidator.ts:50"],
    },
    CatalogueEntry {
        code: "collectionsizevalidator-constructor-negativesize",
        template: "minSize and/or maxSize must be positive integers.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/collectionsizevalidator.ts:52"],
    },
    CatalogueEntry {
        code: "collectionsizevalidator-constructor-mingreaterthanmax",
        template: "minSize must be less than or equal to maxSize.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/collectionsizevalidator.ts:56"],
    },
    CatalogueEntry {
        code: "collectionsizevalidator-validate-belowminsize",
        template: "Collection must contain at least {minSize} elements.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/collectionsizevalidator.ts:69"],
    },
    CatalogueEntry {
        code: "collectionsizevalidator-validate-abovemaxsize",
        template: "Collection must contain no more than {maxSize} elements.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/collectionsizevalidator.ts:71"],
    },
    CatalogueEntry {
        code: "scalardeclaration-process-primitivename",
        template: "Invalid scalar name '{scalarName}'. Name conflicts with primitive type.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/scalardeclaration.ts:66"],
    },
    CatalogueEntry {
        code: "scalardeclaration-validate-duplicateclassname",
        template: "Duplicate class name {name}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/scalardeclaration.ts:138"],
    },
    CatalogueEntry {
        code: "engine-typeerror-readproperties",
        template: "Cannot read properties of {value} (reading '{property}')",
        renderer: Renderer::Inline,
        sources: &["V8 (property read on null or undefined)"],
    },
    CatalogueEntry {
        code: "engine-typeerror-notafunction",
        template: "{expression} is not a function",
        renderer: Renderer::Inline,
        sources: &["V8 (call of a non-function)"],
    },
    CatalogueEntry {
        code: "engine-validateinstanceas-notassignable",
        template: "'{type}' is not assignable to '{declared}'",
        renderer: Renderer::Inline,
        // `validate_instance_as`'s check that the value's own `$class` is
        // assignable to the type asked for; TS has no such entry point.
        sources: &["concerto-core validate_instance_as (no TS throw site)"],
    },
    CatalogueEntry {
        code: "engine-typeerror-inoperator",
        template: "Cannot use 'in' operator to search for '{key}' in {value}",
        renderer: Renderer::Inline,
        // `MapValueType.processType`'s `'$class' in ast.type` throws this
        // when `ast.type` is present but not an object.
        sources: &["V8 ('in' operator with a non-object right-hand side)"],
    },
    // ---- Model manager, model file and class declaration ----
    CatalogueEntry {
        code: "typenotfounderror-defaultmessage",
        template: "Type \"{typeName}\" not found.",
        renderer: Renderer::Globalize,
        // TypeNotFoundException's default message (table 2.3).
        sources: &["src/typenotfoundexception.ts:37 (messages/en.json)"],
    },
    CatalogueEntry {
        code: "modelmanager-gettype-noregisteredns",
        template: "Namespace is not defined for type \"{type}\".",
        renderer: Renderer::Globalize,
        // BaseModelManager.getType's unregistered-namespace path, and
        // ModelFile.validate's check of an import's namespace;
        // `resolve_type_name` raises it for the same check.
        sources: &[
            "src/basemodelmanager.ts:661",
            "src/introspect/modelfile.ts:251",
        ],
    },
    CatalogueEntry {
        code: "factory-newinstance-missingidentifier",
        template: "Missing identifier for Type \"{type}\" in namespace \"{namespace}\".",
        renderer: Renderer::Globalize,
        // One of Factory.newResource's model checks.
        sources: &["src/factory.ts:115"],
    },
    CatalogueEntry {
        code: "factory-newinstance-invalididentifier",
        template: "Invalid or missing identifier for Type \"{type}\" in namespace \"{namespace}\".",
        renderer: Renderer::Globalize,
        sources: &["src/factory.ts:107"],
    },
    CatalogueEntry {
        code: "factory-newinstance-abstracttype",
        template: "Cannot instantiate the abstract type \"{type}\" in the \"{namespace}\" namespace.",
        renderer: Renderer::Globalize,
        sources: &["src/factory.ts:94"],
    },
    CatalogueEntry {
        code: "factory-newinstance-typenotdeclaredinns",
        template: "Cannot instantiate Type \"{type}\" in namespace \"{namespace}\".",
        renderer: Renderer::Globalize,
        // No TS call site: the key is in messages/en.json only.
        sources: &["messages/en.json only (no TS call site)"],
    },
    CatalogueEntry {
        code: "modelmanager-resolvetype-nonsfortype",
        template: "No registered namespace for type \"{type}\" in \"{context}\".",
        renderer: Renderer::Globalize,
        // BaseModelManager.resolveType.
        sources: &["src/basemodelmanager.ts:591"],
    },
    CatalogueEntry {
        code: "modelmanager-resolvetype-notypeinnsforcontext",
        template: "No type \"{type}\" in namespace \"{namespace}\" for \"{context}\".",
        renderer: Renderer::Globalize,
        // BaseModelManager.resolveType.
        sources: &["src/basemodelmanager.ts:602"],
    },
    CatalogueEntry {
        code: "basemodelmanager-updatemodelfile-notfound",
        template: "Model file for namespace {namespace} not found",
        renderer: Renderer::Inline,
        // BaseModelManager.updateModelFile: a plain `Error`.
        sources: &["src/basemodelmanager.ts:353"],
    },
    CatalogueEntry {
        code: "basemodelmanager-deletemodelfile-notfound",
        template: "Model file does not exist",
        renderer: Renderer::Inline,
        // BaseModelManager.deleteModelFile: a plain `Error`.
        sources: &["src/basemodelmanager.ts:372"],
    },
    CatalogueEntry {
        code: "basemodelmanager-throwalreadyexists",
        template: "Namespace {namespace}{prefix} is already declared{postfix}",
        renderer: Renderer::Inline,
        // BaseModelManager._throwAlreadyExists: a plain `Error`.
        // `prefix`/`postfix` are pre-formatted (" specified in file {name}"
        // / " in file {name}"), empty when that model file has no name.
        sources: &["src/basemodelmanager.ts:226"],
    },
    CatalogueEntry {
        code: "metamodelutil-createnametable-declarationnotfound",
        template: "Declaration {name} in namespace {namespace} not found",
        renderer: Renderer::Inline,
        // MetaModelUtil.createNameTable (`resolveMetaModel`): a plain
        // `Error`.
        sources: &["@accordproject/concerto-metamodel@3.17.0 lib/metamodelutil.js:75,90"],
    },
    CatalogueEntry {
        code: "metamodelutil-resolvename-notfound",
        template: "Name {name} not found",
        renderer: Renderer::Inline,
        // MetaModelUtil.resolveName (`resolveMetaModel`): a plain `Error`.
        sources: &["@accordproject/concerto-metamodel@3.17.0 lib/metamodelutil.js:117"],
    },
    CatalogueEntry {
        code: "metamodelutil-resolvetypenames-unrecognizedclass",
        template: "Unrecognized $class {class}",
        renderer: Renderer::Inline,
        // MetaModelUtil.resolveTypeNames: a plain `Error`, only for a node
        // with no (or an empty) `$class`.
        sources: &["@accordproject/concerto-metamodel@3.17.0 lib/metamodelutil.js:196"],
    },
    CatalogueEntry {
        code: "modelmanager-gettype-notypeinns",
        template: "Type \"{type}\" is not defined in namespace \"{namespace}\".",
        renderer: Renderer::Globalize,
        // BaseModelManager.getType and ModelFile.validate.
        sources: &[
            "src/basemodelmanager.ts:669",
            "src/introspect/modelfile.ts:276",
        ],
    },
    CatalogueEntry {
        code: "modelmanager-gettype-duplicatensimport",
        template: "Importing types from different versions (\"{version1}\", \"{version2}\") of the same namespace \"{namespace}\" is not permitted.",
        renderer: Renderer::Globalize,
        // ModelFile.validate.
        sources: &["src/introspect/modelfile.ts:266"],
    },
    CatalogueEntry {
        code: "modelfile-resolvetype-undecltype",
        template: "Undeclared type \"{type}\" in \"{context}\".",
        renderer: Renderer::Globalize,
        // ModelFile.resolveType, with its `fileLocation` argument as the
        // location (PORTING.md 2.1).
        sources: &["src/introspect/modelfile.ts:326"],
    },
    CatalogueEntry {
        code: "modelfile-resolveimport-failfindimp",
        template: "Failed to find \"{type}\" in list of imports \"[{imports}]\" for namespace \"{namespace}\".",
        renderer: Renderer::Globalize,
        // ModelFile.resolveImport. `imports` is `JSON.stringify(this.imports)`.
        sources: &["src/introspect/modelfile.ts:373"],
    },
    CatalogueEntry {
        code: "modelfile-constructor-unrecmodelelem",
        template: "Unrecognised model element \"{type}\".",
        renderer: Renderer::Globalize,
        // ModelFile.fromAst. Same text as
        // `classdeclaration-process-unrecmodelelem`, but a distinct key
        // (`catalogue_is_complete`'s doc comment).
        sources: &["src/introspect/modelfile.ts:859"],
    },
    CatalogueEntry {
        code: "classdeclaration-validate-undefined-properties",
        template: "Properties of Class \"{class}\" has to be defined.",
        renderer: Renderer::Globalize,
        // ClassDeclaration.process. Not raised: the typed read requires
        // `properties` to be an array, so a class without one is a
        // `modelfile-load-unreadable` error (BR-09).
        sources: &["src/introspect/classdeclaration.ts:102"],
    },
    CatalogueEntry {
        code: "classdeclaration-process-unrecmodelelem",
        template: "Unrecognised model element \"{type}\".",
        renderer: Renderer::Globalize,
        // ClassDeclaration.process. Not raised: an unrecognised property
        // `$class` is a `modelfile-load-unreadable` error (BR-09).
        sources: &["src/introspect/classdeclaration.ts:130"],
    },
    CatalogueEntry {
        code: "classdeclaration-validate-selfextending",
        template: "Class \"{class}\" cannot extend itself.",
        renderer: Renderer::Globalize,
        // ClassDeclaration.validate.
        sources: &["src/introspect/classdeclaration.ts:217"],
    },
    CatalogueEntry {
        code: "classdeclaration-validate-identifiernotproperty",
        template: "Class \"{class}\" is identified by field \"{idField}\", but does not contain this property.",
        renderer: Renderer::Globalize,
        // ClassDeclaration.validate.
        sources: &["src/introspect/classdeclaration.ts:228"],
    },
    CatalogueEntry {
        code: "classdeclaration-validate-identifiernotstring",
        template: "Class \"{class}\" is identified by field \"{idField}\", but the type of the field is not \"String\".",
        renderer: Renderer::Globalize,
        // ClassDeclaration.validate.
        sources: &["src/introspect/classdeclaration.ts:241"],
    },
    CatalogueEntry {
        code: "classdeclaration-validate-duplicatefieldname",
        template: "Class \"{class}\" has more than one field named \"{fieldName}\".",
        renderer: Renderer::Globalize,
        // ClassDeclaration.validate (`check_unique_field_names`).
        sources: &["src/introspect/classdeclaration.ts:278"],
    },
    // ---- ClassDeclaration.getNestedProperty's inline templates ----
    CatalogueEntry {
        code: "classdeclaration-getnestedproperty-doesnotexist",
        template: "Property {propertyName} does not exist on {fqn}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/classdeclaration.ts:586"],
    },
    CatalogueEntry {
        code: "classdeclaration-getnestedproperty-primitiveorenum",
        template: "Property {propertyName} is a primitive or enum. Invalid property path: {propertyPath}",
        renderer: Renderer::Inline,
        // A plain `Error`: the one throw in `getNestedProperty` not built as
        // an `IllegalModelException`.
        sources: &["src/introspect/classdeclaration.ts:593"],
    },
    CatalogueEntry {
        code: "instancegenerator-newinstance-noconcreteclass",
        template: "No concrete extending type for \"{type}\".",
        renderer: Renderer::Globalize,
        // InstanceGenerator.findConcreteSubclass: a plain `Error`.
        sources: &["src/serializer/instancegenerator.ts:204"],
    },
    CatalogueEntry {
        code: "serializer-tojson-notcobject",
        template: "\"Serializer.toJSON\" only accepts \"Concept\", \"Event\", \"Asset\", \"Participant\" or \"Transaction\".",
        renderer: Renderer::Globalize,
        // Serializer.toJSON: a plain `Error`, `Globalize.formatMessage` with
        // no params.
        sources: &["src/serializer.ts:102"],
    },
    // ResourceValidator: every `report*` method's key.
    CatalogueEntry {
        code: "resourcevalidator-fieldtypeviolation",
        template: "Model violation in the \"{resourceId}\" instance. The field \"{propertyName}\" has a value of \"{value}\" (type of value: \"{typeOfValue}\"). Expected type of value: \"{fieldType}\".",
        renderer: Renderer::Globalize,
        sources: &["src/serializer/resourcevalidator.ts:543"],
    },
    CatalogueEntry {
        code: "resourcevalidator-notresourceorconcept",
        template: "Model violation in the \"{resourceId}\" instance. Class \"{classFQN}\" has the value of \"{invalidValue}\". Expected a \"Resource\" or a \"Concept\".",
        renderer: Renderer::Globalize,
        sources: &["src/serializer/resourcevalidator.ts:561"],
    },
    CatalogueEntry {
        code: "resourcevalidator-notrelationship",
        template: "Model violation in the \"{resourceId}\" instance. Class \"{classFQN}\" has a value of \"{invalidValue}\". Expected a \"Relationship\".",
        renderer: Renderer::Globalize,
        sources: &["src/serializer/resourcevalidator.ts:577"],
    },
    CatalogueEntry {
        code: "resourcevalidator-missingrequiredproperty",
        template: "The instance \"{resourceId}\" is missing the required field \"{fieldName}\".",
        renderer: Renderer::Globalize,
        sources: &["src/serializer/resourcevalidator.ts:592"],
    },
    CatalogueEntry {
        code: "resourcevalidator-emptyidentifier",
        template: "Instance \"{resourceId}\" has an empty identifier.",
        renderer: Renderer::Globalize,
        sources: &["src/serializer/resourcevalidator.ts:606"],
    },
    CatalogueEntry {
        code: "resourcevalidator-invalidenumvalue",
        template: "Model violation in the \"{resourceId}\" instance. Invalid enum value of \"{value}\" for the field \"{fieldName}\".",
        renderer: Renderer::Globalize,
        sources: &["src/serializer/resourcevalidator.ts:620"],
    },
    CatalogueEntry {
        code: "resourcevalidator-abstractclass",
        template: "The class \"{className}\" is abstract and should not contain an instance.",
        renderer: Renderer::Globalize,
        sources: &["src/serializer/resourcevalidator.ts:635"],
    },
    CatalogueEntry {
        code: "resourcevalidator-undeclaredfield",
        template: "Instance \"{resourceId}\" has a property named \"{propertyName}\", which is not declared in \"{fullyQualifiedTypeName}\".",
        renderer: Renderer::Globalize,
        sources: &["src/serializer/resourcevalidator.ts:650"],
    },
    CatalogueEntry {
        code: "resourcevalidator-invalidfieldassignment",
        template: "Instance \"{resourceId}\" has a property \"{propertyName}\" with type \"{objectType}\" that is not derived from \"{fieldType}\".",
        renderer: Renderer::Globalize,
        sources: &["src/serializer/resourcevalidator.ts:668"],
    },
    // ---- ResourceValidator's inline templates (plain `Error`, never
    //      `ValidationException`, table 2.3) ----
    CatalogueEntry {
        code: "resourcevalidator-checkmaptype-expectedstring",
        template: "Model violation in {mapFqn}. Expected Type of String but found '{value}' instead.",
        renderer: Renderer::Inline,
        sources: &["src/serializer/resourcevalidator.ts:152"],
    },
    CatalogueEntry {
        code: "resourcevalidator-checkmaptype-expecteddatetime",
        template: "Model violation in {mapFqn}. Expected Type of DateTime but found '{value}' instead.",
        renderer: Renderer::Inline,
        sources: &["src/serializer/resourcevalidator.ts:157"],
    },
    CatalogueEntry {
        code: "resourcevalidator-checkmaptype-expectedboolean",
        template: "Model violation in {mapFqn}. Expected Type of Boolean but found {type} instead, for value '{value}'.",
        renderer: Renderer::Inline,
        sources: &["src/serializer/resourcevalidator.ts:163"],
    },
    CatalogueEntry {
        code: "resourcevalidator-visitmapdeclaration-notamap",
        // TS: `'Expected a Map, but found ' + JSON.stringify(obj)`: `{obj}` is
        // the caller's `JSON.stringify` text (2.1).
        template: "Expected a Map, but found {obj}",
        renderer: Renderer::Inline,
        sources: &["src/serializer/resourcevalidator.ts:183"],
    },
    CatalogueEntry {
        code: "resourcevalidator-checkrelationship-notidentifiable",
        template: "Cannot have a relationship to a field that is not identifiable.",
        renderer: Renderer::Inline,
        sources: &["src/serializer/resourcevalidator.ts:503"],
    },
    // ---- ResourceId (`src/model/resourceid.ts`) inline templates ----
    CatalogueEntry {
        code: "resourceid-constructor-missingnamespace",
        template: "Missing namespace",
        renderer: Renderer::Inline,
        sources: &["src/model/resourceid.ts:122"],
    },
    CatalogueEntry {
        code: "resourceid-constructor-missingtype",
        template: "Missing type",
        renderer: Renderer::Inline,
        sources: &["src/model/resourceid.ts:125"],
    },
    CatalogueEntry {
        code: "resourceid-constructor-missingid",
        template: "Missing id",
        renderer: Renderer::Inline,
        sources: &["src/model/resourceid.ts:128"],
    },
    CatalogueEntry {
        code: "resourceid-parseuri-invalidport",
        template: "Invalid port",
        renderer: Renderer::Inline,
        sources: &["src/model/resourceid.ts:89"],
    },
    CatalogueEntry {
        code: "resourceid-fromuri-invaliduri",
        template: "Invalid URI: {uri}",
        renderer: Renderer::Inline,
        sources: &["src/model/resourceid.ts:156"],
    },
    CatalogueEntry {
        code: "resourceid-fromuri-invalidscheme",
        template: "Invalid URI scheme: {uri}",
        renderer: Renderer::Inline,
        sources: &["src/model/resourceid.ts:162"],
    },
    CatalogueEntry {
        code: "resourceid-fromuri-invalidformat",
        template: "Invalid resource URI format: {uri}",
        renderer: Renderer::Inline,
        sources: &["src/model/resourceid.ts:165"],
    },
    // ---- Property.getFullyQualifiedTypeName's inline template ----
    CatalogueEntry {
        code: "property-getfullyqualifiedtypename-notfound",
        template: "Failed to find fully qualified type name for property {name} with type {type}",
        renderer: Renderer::Inline,
        // A plain `Error`.
        sources: &["src/introspect/property.ts:218"],
    },
    // ---- Property.process's inline templates ----
    CatalogueEntry {
        code: "property-process-invalidname",
        template: "Invalid property name '{name}'",
        renderer: Renderer::Inline,
        sources: &["src/introspect/property.ts:86"],
    },
    CatalogueEntry {
        code: "property-process-noname",
        template: "No name for type {ast}",
        renderer: Renderer::Inline,
        // A plain `Error`.
        sources: &[
            "src/introspect/property.ts:124",
            "src/introspect/property.ts:137",
        ],
    },
    // ---- Not a TS template (DV-017): TS's `Property.process` crashes with
    //      a V8 `TypeError` on a `RelationshipProperty` whose `type` is
    //      missing or `null`; this is worded like
    //      `relationshipdeclaration-validate-*`. ----
    CatalogueEntry {
        code: "property-process-relationshipnotype",
        template: "Relationship {name} must have a type",
        renderer: Renderer::Inline,
        sources: &[
            "concerto-rust DV-017: replaces the V8 TypeError at src/introspect/property.ts:165 (maintainer-accepted, accordproject/concerto-rust#218)",
        ],
    },
    // ---- Not a TS template (DV-018): TS's `Decorator.process` crashes
    //      with a V8 `TypeError` on a `null` decorator; this is worded like
    //      `Decorator.validate`'s messages. `{value}` is `null` or
    //      `undefined`. ----
    CatalogueEntry {
        code: "decorator-process-notobject",
        template: "Invalid decorator. Expected object. Found {value}",
        renderer: Renderer::Inline,
        sources: &[
            "concerto-rust DV-018: replaces the V8 TypeError at src/introspect/decorator.ts:139 (maintainer-accepted, accordproject/concerto-rust#218)",
        ],
    },
    // ---- Property.validate, RelationshipDeclaration.validate and
    //      MapDeclaration/MapKeyType/MapValueType inline templates ----
    CatalogueEntry {
        code: "property-validate-sizevalidator",
        template: "size validator can only be applied to array or map properties: {fqn}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/property.ts:161"],
    },
    CatalogueEntry {
        code: "relationshipdeclaration-validate-notype",
        template: "Relationship must have a type",
        renderer: Renderer::Inline,
        sources: &["src/introspect/relationshipdeclaration.ts:54"],
    },
    CatalogueEntry {
        code: "relationshipdeclaration-validate-primitivetype",
        template: "Relationship {name} cannot be to the primitive type {type}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/relationshipdeclaration.ts:61"],
    },
    CatalogueEntry {
        code: "relationshipdeclaration-validate-missingtype",
        template: "Relationship {name} points to a missing type {type}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/relationshipdeclaration.ts:80"],
    },
    CatalogueEntry {
        code: "relationshipdeclaration-validate-notidentified",
        template: "Relationship {name} must be to a class that has an identifier, but this is to {type}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/relationshipdeclaration.ts:86"],
    },
    // ---- Field.getScalarField's inline templates ----
    CatalogueEntry {
        code: "field-getscalarfield-notscalar",
        template: "Field {name} is not a scalar property.",
        renderer: Renderer::Inline,
        // A plain `Error`.
        sources: &["src/introspect/field.ts:186"],
    },
    CatalogueEntry {
        code: "field-getscalarfield-unrecognizedtype",
        template: "Unrecognized scalar type {class}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/field.ts:215"],
    },
    CatalogueEntry {
        code: "mapdeclaration-process-missingkeyvalue",
        template: "MapDeclaration must contain Key & Value properties {name}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/mapdeclaration.ts:63"],
    },
    CatalogueEntry {
        code: "mapdeclaration-process-invalidkey",
        template: "MapDeclaration must contain valid MapKeyType  {name}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/mapdeclaration.ts:67"],
    },
    CatalogueEntry {
        code: "mapdeclaration-process-invalidvalue",
        template: "MapDeclaration must contain valid MapValueType, for MapDeclaration {name}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/mapdeclaration.ts:71"],
    },
    CatalogueEntry {
        code: "mapkeytype-validate-invalidscalar",
        template: "Scalar must be one of StringScalar, DateTimeScalar in context of MapKeyType. Invalid Scalar: {type}, for MapDeclaration {name}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/mapkeytype.ts:78"],
    },
    CatalogueEntry {
        code: "mapvaluetype-validate-mapnotsupported",
        template: "MapDeclaration as Map Type Value is not supported: {type}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/mapvaluetype.ts:78"],
    },
    CatalogueEntry {
        code: "mapvaluetype-process-missingtype",
        template: "ObjectMapValueType must contain property 'type', for MapDeclaration named {name}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/mapvaluetype.ts:98"],
    },
    CatalogueEntry {
        code: "mapvaluetype-process-malformedtype",
        template: "ObjectMapValueType type must contain property '$class' and property 'name', for MapDeclaration named {name}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/mapvaluetype.ts:103"],
    },
    CatalogueEntry {
        code: "mapvaluetype-process-invalidtypeclass",
        template: "ObjectMapValueType type $class must be of TypeIdentifier for MapDeclaration named {name}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/mapvaluetype.ts:108"],
    },
    // ---- Serializer, Factory, JSONPopulator, JSONGenerator and the
    //      Resource-mutating members ----
    CatalogueEntry {
        code: "engine-typeerror-convertnulltoobject",
        template: "Cannot convert undefined or null to object",
        renderer: Renderer::Inline,
        sources: &["V8 (Object.keys of null or undefined)"],
    },
    CatalogueEntry {
        code: "serializer-constructor-factorynull",
        template: "\"Factory\" cannot be \"null\".",
        renderer: Renderer::Globalize,
        sources: &["src/serializer.ts:57"],
    },
    CatalogueEntry {
        code: "serializer-constructor-modelmanagernull",
        template: "\"ModelManager\" cannot be \"null\".",
        renderer: Renderer::Globalize,
        sources: &["src/serializer.ts:59"],
    },
    CatalogueEntry {
        code: "serializer-fromjson-noclass",
        template: "Invalid JSON data. Does not contain a $class type identifier.",
        renderer: Renderer::Inline,
        sources: &["src/serializer.ts:146"],
    },
    CatalogueEntry {
        code: "serializer-fromjson-mapnotsupported",
        template: "Attempting to create a Map declaration is not supported.",
        renderer: Renderer::Inline,
        sources: &["src/serializer.ts:166"],
    },
    CatalogueEntry {
        code: "serializer-fromjson-enumnotsupported",
        template: "Attempting to create an ENUM declaration is not supported.",
        renderer: Renderer::Inline,
        sources: &["src/serializer.ts:168"],
    },
    CatalogueEntry {
        code: "factory-newresource-idregexmismatch",
        template: "Provided id does not match regex: {regex}",
        renderer: Renderer::Inline,
        sources: &["src/factory.ts:127"],
    },
    CatalogueEntry {
        code: "factory-newresource-notidentifiable",
        template: "Type is not identifiable {fqn}",
        renderer: Renderer::Inline,
        sources: &["src/factory.ts:131"],
    },
    CatalogueEntry {
        code: "factory-newrelationship-notidentifiable",
        template: "Cannot create a relationship to {fqn}, it is not identifiable.",
        renderer: Renderer::Inline,
        sources: &["src/factory.ts:190"],
    },
    CatalogueEntry {
        code: "factory-newtransaction-nsnotspecified",
        template: "ns not specified",
        renderer: Renderer::Inline,
        sources: &["src/factory.ts:211", "src/factory.ts:240"],
    },
    CatalogueEntry {
        code: "factory-newtransaction-typenotspecified",
        template: "type not specified",
        renderer: Renderer::Inline,
        sources: &["src/factory.ts:213", "src/factory.ts:242"],
    },
    CatalogueEntry {
        code: "factory-newtransaction-notatransaction",
        template: "{fqn} is not a transaction",
        renderer: Renderer::Inline,
        sources: &["src/factory.ts:219"],
    },
    CatalogueEntry {
        code: "factory-newevent-notanevent",
        template: "{fqn} is not an event",
        renderer: Renderer::Inline,
        sources: &["src/factory.ts:248"],
    },
    CatalogueEntry {
        code: "jsonpopulator-getassignableproperties-reservedproperties",
        template: "Unexpected reserved properties for type {fqn}: {properties}",
        renderer: Renderer::Inline,
        sources: &["src/serializer/jsonpopulator.ts:62"],
    },
    CatalogueEntry {
        code: "jsonpopulator-getassignableproperties-timestamp",
        template: "Unexpected property for type {fqn}: $timestamp",
        renderer: Renderer::Inline,
        sources: &["src/serializer/jsonpopulator.ts:69"],
    },
    CatalogueEntry {
        code: "jsonpopulator-validateproperties-unexpectedproperties",
        template: "Unexpected properties for type {fqn}: {properties}",
        renderer: Renderer::Inline,
        sources: &["src/serializer/jsonpopulator.ts:93"],
    },
    CatalogueEntry {
        code: "jsonpopulator-rejectunknownkeys-unknownproperties",
        template: "Unexpected properties for type {fqn}: {properties}",
        renderer: Renderer::Inline,
        sources: &[
            "accordproject/concerto#1273 rejectUnknownKeys (no TS call site; the text of jsonpopulator-validateproperties-unexpectedproperties)",
        ],
    },
    CatalogueEntry {
        code: "jsonpopulator-rejectrequirednull-requirednull",
        template: "Expected value at path `{path}` to be of type `{type}`, but got null",
        renderer: Renderer::Inline,
        sources: &["accordproject/concerto#1273 rejectRequiredNull (no TS call site)"],
    },
    CatalogueEntry {
        code: "jsonpopulator-visitfield-notarray",
        template: "Expected value at path `{path}` to be an array of type `{type}`",
        renderer: Renderer::Inline,
        sources: &[
            "src/serializer/jsonpopulator.ts:254",
            "src/serializer/jsonpopulator.ts:421",
        ],
    },
    CatalogueEntry {
        code: "jsonpopulator-converttoobject-wrongtype",
        template: "Expected value at path `{path}` to be of type `{type}`",
        renderer: Renderer::Inline,
        sources: &[
            "src/serializer/jsonpopulator.ts:337",
            "src/serializer/jsonpopulator.ts:349",
            "src/serializer/jsonpopulator.ts:357",
            "src/serializer/jsonpopulator.ts:360",
            "src/serializer/jsonpopulator.ts:369",
            "src/serializer/jsonpopulator.ts:377",
            "src/serializer/jsonpopulator.ts:385",
        ],
    },
    CatalogueEntry {
        code: "jsonpopulator-converttoobject-datetimeformat",
        template: "Expected value at path `{path}` to be of type `{type}` with format YYYY-MM-DDTHH:mm:ss[Z]",
        renderer: Renderer::Inline,
        sources: &["src/serializer/jsonpopulator.ts:345"],
    },
    CatalogueEntry {
        code: "jsonpopulator-visitrelationshipdeclaration-notastring",
        template: "Invalid JSON data. Found a value that is not a string: {value} for relationship {relationship}",
        renderer: Renderer::Inline,
        sources: &[
            "src/serializer/jsonpopulator.ts:433",
            "src/serializer/jsonpopulator.ts:457",
        ],
    },
    CatalogueEntry {
        code: "jsonpopulator-visitrelationshipdeclaration-noclass",
        template: "Invalid JSON data. Does not contain a $class type identifier: {value} for relationship {relationship}",
        renderer: Renderer::Inline,
        sources: &[
            "src/serializer/jsonpopulator.ts:438",
            "src/serializer/jsonpopulator.ts:462",
        ],
    },
    CatalogueEntry {
        code: "jsonpopulator-visitrelationshipdeclaration-notstringorobject",
        template: "Invalid JSON data. Found a value that is not a string or object: {value} for relationship {relationship}",
        renderer: Renderer::Inline,
        sources: &["src/serializer/jsonpopulator.ts:474"],
    },
    CatalogueEntry {
        code: "jsongenerator-visitclassdeclaration-notaresource",
        template: "Expected a Resource, but found {obj}",
        renderer: Renderer::Inline,
        sources: &["src/serializer/jsongenerator.ts:122"],
    },
    CatalogueEntry {
        code: "jsongenerator-getrelationshiptext-norelationship",
        template: "Did not find a relationship for {type} found {obj}",
        renderer: Renderer::Inline,
        sources: &["src/serializer/jsongenerator.ts:308"],
    },
    CatalogueEntry {
        code: "typedstack-push-unexpectedtype",
        template: "Did not find expected type {type} as argument to push. Found: {obj}",
        renderer: Renderer::Inline,
        sources: &["@accordproject/concerto-util@5.0.0 src/typedstack.ts (TypedStack.push)"],
    },
    CatalogueEntry {
        code: "typed-tojson-useserializer",
        template: "Use Serializer.toJSON to convert resource instances to JSON objects.",
        renderer: Renderer::Inline,
        sources: &["src/model/typed.ts:209"],
    },
    CatalogueEntry {
        code: "validatedresource-setpropertyvalue-undeclaredfield",
        template: "The instance with id {id} trying to set field {propName} which is not declared in the model.",
        renderer: Renderer::Inline,
        sources: &[
            "src/model/validatedresource.ts:56",
            "src/model/validatedresource.ts:83",
        ],
    },
    CatalogueEntry {
        code: "validatedresource-addarrayvalue-notanarray",
        template: "The instance with id {id} trying to add array item {propName} which is not declared as an array in the model.",
        renderer: Renderer::Inline,
        sources: &["src/model/validatedresource.ts:89"],
    },
    CatalogueEntry {
        // `'Unrecognised ' + JSON.stringify(thing)` in `JSONPopulator.visit`
        // and `JSONGenerator.visit`, for an introspection object: TS 5.0.0
        // threw V8's circular-structure `TypeError` (DV-010); BC-08 names
        // the element by its fully-qualified name.
        code: "serializer-visit-unrecognised",
        template: "Unrecognised element \"{name}\"",
        renderer: Renderer::Inline,
        sources: &[
            "concerto-rust P5-63 / BC-08 (DV-010): replaces the V8 TypeError at src/serializer/jsonpopulator.ts:124, src/serializer/jsongenerator.ts:72",
        ],
    },
    CatalogueEntry {
        // A cyclic inheritance chain, reported from every entry point
        // (BC-11), where TS 5.0.0 overflowed V8's stack (DV-013).
        code: "classdeclaration-circularinheritance",
        template: "The super type chain of \"{type}\" is circular: {cycle}.",
        renderer: Renderer::Inline,
        sources: &[
            "concerto-rust P5-63 / BC-11 (DV-013): replaces the V8 RangeError of ClassDeclaration.getProperties, src/introspect/classdeclaration.ts",
        ],
    },
    // ---- BaseModelManager.validateAst (`instance::metamodel`) ----
    CatalogueEntry {
        code: "basemodelmanager-validateast-versionmismatch",
        template: "Model file version {modelFileVersion} does not match metamodel version {metamodelVersion}",
        renderer: Renderer::Inline,
        sources: &["src/basemodelmanager.ts:283"],
    },
    CatalogueEntry {
        // `throw new MetamodelException(error.message)`: the underlying
        // error's message, passed through unchanged.
        code: "basemodelmanager-validateast-wrapped",
        template: "{message}",
        renderer: Renderer::Inline,
        sources: &["src/basemodelmanager.ts:296"],
    },
    // ---- Not a TS template (BC-45): TS does not check a `DateTime`
    //      default value; it must be a strict `DateTime` string when it is
    //      applied to an instance. ----
    CatalogueEntry {
        code: "typed-assignfielddefaults-datetime",
        template: "Invalid default value `{value}` for the DateTime field `{fqn}`: expected an ISO 8601 date-time with an offset, YYYY-MM-DDTHH:mm:ss[.SSS](Z|+HH:mm|-HH:mm), naming a real instant",
        renderer: Renderer::Inline,
        sources: &[
            "concerto-rust P5-24 / BC-45: no TS throw site (Typed.assignFieldDefaults builds dayjs.utc(default) unchecked, src/model/typed.ts)",
        ],
    },
    // ---- Not TS templates (BC-19, with BC-17 and BC-20): the strict AST
    //      shape check at model load (`instance::check_ast_shape`), where
    //      TS 5.0.0 loads these ASTs or throws a V8 `TypeError` (BC-18). ----
    CatalogueEntry {
        code: "modelfile-load-decoratorsnotarray",
        template: "Invalid decorators. Expected array. Found {value}",
        renderer: Renderer::Inline,
        sources: &[
            "concerto-rust P5-49 / BC-17: no TS throw site (Decorated.process iterates a non-array decorators value, src/introspect/decorated.ts)",
        ],
    },
    CatalogueEntry {
        code: "modelfile-load-namenotstring",
        template: "Invalid name. Expected a string. Found {value}",
        renderer: Renderer::Inline,
        sources: &[
            "concerto-rust P5-49 / BC-20: no TS throw site (names are coerced with String(), src/introspect/*.ts)",
        ],
    },
    CatalogueEntry {
        code: "modelfile-load-supertypename",
        template: "Invalid super type name. Expected a non-empty string. Found {value}",
        renderer: Renderer::Inline,
        sources: &[
            "concerto-rust P5-49 / BC-20: no TS throw site (ClassDeclaration._resolveSuperType tests the name's truthiness, src/introspect/classdeclaration.ts)",
        ],
    },
    CatalogueEntry {
        code: "modelfile-load-nodenotobject",
        template: "Invalid {key}. Expected an object with a $class. Found {value}",
        renderer: Renderer::Inline,
        sources: &[
            "concerto-rust P5-61 / BC-19: no TS throw site (ClassDeclaration.process and the validator constructors read identified, sizeValidator, lengthValidator and validator with no type check, src/introspect/*.ts)",
        ],
    },
    CatalogueEntry {
        code: "modelfile-load-astshape",
        template: "Model AST does not conform to the metamodel: {message}",
        renderer: Renderer::Inline,
        sources: &[
            "concerto-rust P5-49 / BC-19: no TS throw site (validateAst's check, src/basemodelmanager.ts:296, run at model load and re-thrown as an IllegalModelException)",
        ],
    },
    CatalogueEntry {
        code: "modelfile-load-unreadable",
        template: "Model AST could not be read: {message}",
        renderer: Renderer::Inline,
        sources: &[
            "concerto-rust P5-61 / BR-09: no TS throw site (the typed AST read, the only model loader, fails on a node it cannot read; with BC-19's shape check on, the check rejects such an AST first)",
        ],
    },
    // ---- Model validation checks: TS's own hardcoded strings, verbatim ----
    CatalogueEntry {
        code: "modelfile-validate-duplicateclassname",
        template: "Duplicate class name {fqn}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/modelfile.ts:293"],
    },
    CatalogueEntry {
        code: "declaration-validate-importclash",
        template: "Type '{name}' clashes with an imported type with the same name.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/declaration.ts:91"],
    },
    CatalogueEntry {
        code: "decorated-validate-duplicatedecorator",
        template: "Duplicate decorator {name}",
        renderer: Renderer::Inline,
        sources: &["src/introspect/decorated.ts:143"],
    },
    CatalogueEntry {
        code: "classdeclaration-resolvesupertype-notfound",
        template: "Could not find super type {superType}",
        renderer: Renderer::Inline,
        sources: &[
            "src/introspect/classdeclaration.ts:184",
            "src/introspect/classdeclaration.ts:553",
        ],
    },
    CatalogueEntry {
        code: "classdeclaration-resolvesupertype-kindmismatch",
        template: "{kind} ({name}) cannot extend {superKind} ({superName})",
        renderer: Renderer::Inline,
        sources: &["src/introspect/classdeclaration.ts:190"],
    },
    CatalogueEntry {
        code: "classdeclaration-validate-identifieroptional",
        template: "Identifying fields cannot be optional.",
        renderer: Renderer::Inline,
        sources: &["src/introspect/classdeclaration.ts:249"],
    },
    CatalogueEntry {
        code: "classdeclaration-validate-redeclaredidentifier",
        template: "Super class {superType} has an explicit identifier {idField} that cannot be redeclared.",
        renderer: Renderer::Inline,
        sources: &[
            "src/introspect/classdeclaration.ts:258",
            "src/introspect/classdeclaration.ts:263",
        ],
    },
    // Not a TS template: see the module doc and `ContractError::pre_port`.
    CatalogueEntry {
        code: "pre-port",
        template: "",
        renderer: Renderer::Raw,
        sources: &["concerto-rust: not yet a faithful TS port (PORTING.md section 7.2)"],
    },
];

/// Looks up a catalogue entry.
pub fn catalogue_entry(code: &str) -> Option<&'static CatalogueEntry> {
    CATALOGUE.iter().find(|entry| entry.code == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every entry's `code` is unique, it cites its source, and (`"pre-port"`
    /// excepted) it has a golden test in `mod.rs`, named after its code
    /// (checked by name, PORTING.md 6.3).
    ///
    /// Templates need not be unique: `en.json` gives two keys
    /// (`modelfile-constructor-unrecmodelelem`,
    /// `classdeclaration-process-unrecmodelelem`) the same text, and a
    /// fixture is attributed by `code`.
    #[test]
    fn catalogue_is_complete() {
        let golden_tests_source = include_str!("mod.rs");
        for (i, entry) in CATALOGUE.iter().enumerate() {
            assert!(!entry.sources.is_empty(), "{} cites no source", entry.code);
            assert!(
                CATALOGUE[..i].iter().all(|e| e.code != entry.code),
                "{} is duplicated",
                entry.code
            );
            if entry.code == "pre-port" {
                continue;
            }
            let golden = format!("fn golden_{}()", entry.code.replace('-', "_"));
            assert!(
                golden_tests_source.contains(&golden),
                "{} has no golden test",
                entry.code
            );
        }
    }

    /// The `"pre-port"` entry itself is tested (`golden_pre_port`, mod.rs),
    /// but is exempt from the by-name check above because its code does not
    /// spell a TS message key.
    #[test]
    fn pre_port_entry_uses_the_raw_renderer() {
        let entry = catalogue_entry("pre-port").expect("pre-port entry must exist");
        assert_eq!(entry.renderer, Renderer::Raw);
    }
}
