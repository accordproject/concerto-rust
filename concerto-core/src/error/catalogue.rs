//! The message catalogue (PORTING.md section 2.2, OD-5).
//!
//! Rust owns the message templates the ported units of `concerto-core`
//! throw. This file holds two kinds of entry:
//!
//! - the `messages/en.json` keys OD-5 scopes in, ported verbatim: every key
//!   a RUST or HYBRID member's throw site uses, plus the
//!   `factory-newinstance-*` keys and `typenotfounderror-defaultmessage` that
//!   OD-5 pre-approves ahead of their own call site (P3-01, table 2.3);
//! - the inline templates (template literals and string concatenations,
//!   2.2 step 2) of the members the P0-04b trial ported.
//!
//! It does not yet hold the inline templates of every RUST or HYBRID member:
//! each member's own port adds its inline templates, with their golden
//! tests, in the same PR (6.3). No unused `en.json` key is ported:
//! `composer-*`, `whereastvalidator-*`, `like` and `test-*` have no throw
//! site in `concerto-core` and stay in TS.
//!
//! **Deriving the OD-5 scope.** "Every `en.json` key used by a RUST or
//! HYBRID member" (OD-5) means: take the ledger
//! (`migration/ledger/SEAM_LEDGER.tsv`, commit `c48423c`, PORTING.md OD-6)
//! rows whose `classification` is `RUST` or `HYBRID`, for each one grep its
//! `file`/`class`/`member` in the frozen TS reference for
//! `Globalize.messageFormatter(...)` or `Globalize.formatMessage(...)`, and
//! port every key that turns up (2.2 step 1), plus the pre-approved keys
//! above. The call is written several ways in the reference —
//! `Globalize.messageFormatter('key')`, `Globalize('en').messageFormatter('key')`,
//! and with the key on the line after the opening parenthesis
//! (`classdeclaration.ts:278`, `resourcevalidator.ts:592`) — so a
//! line-oriented `grep` misses some of them. The reproducible form, run
//! from `packages/concerto-core` in the TS checkout, reads each file whole
//! and prints `file:line:key` with the line of the `Globalize` token:
//!
//! ```text
//! perl -0777 -ne 'while (/Globalize\s*(?:\(\s*[^)]*\))?\s*\.\s*(?:messageFormatter|formatMessage)\s*\(\s*([\x27"`])([^\x27"`]+)\1/g) { my $l = (substr($_, 0, $-[0]) =~ tr/\n//) + 1; print "$ARGV:$l:$2\n" }' $(find src -name '*.ts')
//! ```
//!
//! Each hit is then attributed to the ledger row for its `file` whose
//! `line` is the nearest one at or above it, and kept when that row is
//! `RUST` or `HYBRID`. At `c48423c` the command finds 34 call sites (32
//! distinct keys), of which 28 call sites (26 distinct keys) are in a RUST
//! or HYBRID row; with the five pre-approved keys that is the 31 keys
//! `od5_catalogue_scope_is_present` (mod.rs) lists. The hits it drops are
//! `Factory.newResource` (TS in the TSV; its three keys are pre-approved
//! anyway), `Serializer.constructor` (TS: `serializer-constructor-*`) and
//! `TypeNotFoundException.constructor` (TS; pre-approved). The P1-07
//! attribution index, OD-10, automates this once it exists; until then a
//! P1/P2/P3 task that finds a key the census missed adds it here, citing
//! the ledger row. The trial (P0-04b) and the first cut of P1-05
//! covered only the keys their own units' call sites used; this file now
//! also carries the P1-05 exit-condition sweep over `BaseModelManager`
//! (`resolveType`, `getType`), `ModelFile` (`constructor`, `resolveType`,
//! `resolveImport`, `validate`), `ClassDeclaration` (`process`, `validate`),
//! `InstanceGenerator` (RUST: `findConcreteSubclass`, reached from
//! `newInstance`) and `Serializer.toJSON` (HYBRID), plus the nine
//! `resourcevalidator-*` keys `ResourceValidator` (HYBRID, every `visit*`
//! and `report*` method) uses — none of these units has its own call site
//! yet, so each entry below is pre-approved the same way
//! `factory-newinstance-*` is (2.2), and the unit that ports the member
//! deletes the pre-approval note from its doc comment.
//!
//! One further entry, `"pre-port"`, is not a TS template at all: it is the
//! escape hatch [`super::ContractError::pre_port`] uses for a call site that
//! has not yet been faithfully ported (module doc on [`super`]).

use super::{CatalogueEntry, Renderer};

/// The message catalogue. Every entry but `"pre-port"` is a verbatim TS
/// template, byte for byte, with the throw site(s) it was ported from.
pub const CATALOGUE: &[CatalogueEntry] = &[
    // ---- P0-04b trial payload (ModelUtil, NumberValidator, ScalarDeclaration),
    //      absorbed unchanged (PORTING.md, the maintainer's second #42 comment) ----
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
    // ---- P2-02 additions (StringValidator, CollectionSizeValidator) ----
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
        // (V8 in TS, `regress` here), passed through verbatim (OD-4).
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
    // ---- P1-05 additions ----
    CatalogueEntry {
        code: "typenotfounderror-defaultmessage",
        template: "Type \"{typeName}\" not found.",
        renderer: Renderer::Globalize,
        // OD-5: pre-approved for the catalogue ahead of its call site
        // (TypeNotFoundException's default message, table 2.3), which lands
        // wherever a port raises `TypeNotFoundException(typeName)` with no
        // custom message.
        sources: &["src/typenotfoundexception.ts:37 (messages/en.json)"],
    },
    CatalogueEntry {
        code: "modelmanager-gettype-noregisteredns",
        template: "Namespace is not defined for type \"{type}\".",
        renderer: Renderer::Globalize,
        // BaseModelManager.getType's unregistered-namespace path. Reused,
        // faithfully, by model_manager::ModelManager::resolve_type_name for
        // its equivalent check (PORTING.md section 7.2): both ask whether a
        // namespace is loaded before resolving a name inside it. A future
        // P2-08 port of ModelManager.getType itself uses the same entry.
        // ModelFile.validate throws the same key (RUST) for the same check
        // over an import's namespace, one template, two throw sites (2.2).
        sources: &[
            "src/basemodelmanager.ts:661",
            "src/introspect/modelfile.ts:251",
        ],
    },
    CatalogueEntry {
        code: "factory-newinstance-missingidentifier",
        template: "Missing identifier for Type \"{type}\" in namespace \"{namespace}\".",
        renderer: Renderer::Globalize,
        // OD-5 / #32 point 4: one of Factory.newResource's model checks;
        // not yet called (P3-01 delegates them to Rust).
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
        // No TS call site: the key is in messages/en.json but nothing in
        // src/ uses it. OD-5 pre-approves it with the other three.
        sources: &["messages/en.json only (no TS call site)"],
    },
    // ---- P1-05 exit-condition sweep: the rest of the OD-5 scope the first
    //      cut of this file missed (module doc). Each of these units has no
    //      call site yet, so the entry is pre-approved the same way
    //      `factory-newinstance-*` is, ahead of the P1/P2/P3 task that ports
    //      its unit and deletes the "not yet called" note. ----
    CatalogueEntry {
        code: "modelmanager-resolvetype-nonsfortype",
        template: "No registered namespace for type \"{type}\" in \"{context}\".",
        renderer: Renderer::Globalize,
        // BaseModelManager.resolveType (RUST); not yet called.
        sources: &["src/basemodelmanager.ts:591"],
    },
    CatalogueEntry {
        code: "modelmanager-resolvetype-notypeinnsforcontext",
        template: "No type \"{type}\" in namespace \"{namespace}\" for \"{context}\".",
        renderer: Renderer::Globalize,
        // BaseModelManager.resolveType (RUST); not yet called.
        sources: &["src/basemodelmanager.ts:602"],
    },
    CatalogueEntry {
        code: "modelmanager-gettype-notypeinns",
        template: "Type \"{type}\" is not defined in namespace \"{namespace}\".",
        renderer: Renderer::Globalize,
        // BaseModelManager.getType (RUST) and ModelFile.validate (RUST),
        // one template, two throw sites (2.2); neither is called yet.
        sources: &[
            "src/basemodelmanager.ts:669",
            "src/introspect/modelfile.ts:276",
        ],
    },
    CatalogueEntry {
        code: "modelmanager-gettype-duplicatensimport",
        template: "Importing types from different versions (\"{version1}\", \"{version2}\") of the same namespace \"{namespace}\" is not permitted.",
        renderer: Renderer::Globalize,
        // ModelFile.validate (RUST); not yet called.
        sources: &["src/introspect/modelfile.ts:266"],
    },
    CatalogueEntry {
        code: "modelfile-resolvetype-undecltype",
        template: "Undeclared type \"{type}\" in \"{context}\".",
        renderer: Renderer::Globalize,
        // ModelFile.resolveType (RUST); not yet called. TS passes the AST
        // `fileLocation` argument as the third IllegalModelException
        // argument here, copied verbatim per PORTING.md 2.1 once ported.
        sources: &["src/introspect/modelfile.ts:326"],
    },
    CatalogueEntry {
        code: "modelfile-resolveimport-failfindimp",
        template: "Failed to find \"{type}\" in list of imports \"[{imports}]\" for namespace \"{namespace}\".",
        renderer: Renderer::Globalize,
        // ModelFile.resolveImport (RUST). `imports` is
        // `JSON.stringify(this.imports)` (3.1).
        sources: &["src/introspect/modelfile.ts:373"],
    },
    CatalogueEntry {
        code: "modelfile-constructor-unrecmodelelem",
        template: "Unrecognised model element \"{type}\".",
        renderer: Renderer::Globalize,
        // ModelFile.fromAst, called from the ModelFile constructor (HYBRID);
        // not yet called. Same English text as
        // `classdeclaration-process-unrecmodelelem` below, but a distinct
        // `en.json` key (and so a distinct catalogue entry, `code` being
        // what OD-10 attributes fixtures by): see `catalogue_is_complete`'s
        // doc comment.
        sources: &["src/introspect/modelfile.ts:859"],
    },
    CatalogueEntry {
        code: "classdeclaration-validate-undefined-properties",
        template: "Properties of Class \"{class}\" has to be defined.",
        renderer: Renderer::Globalize,
        // ClassDeclaration.process (RUST); not yet called.
        sources: &["src/introspect/classdeclaration.ts:102"],
    },
    CatalogueEntry {
        code: "classdeclaration-process-unrecmodelelem",
        template: "Unrecognised model element \"{type}\".",
        renderer: Renderer::Globalize,
        // ClassDeclaration.process (RUST); not yet called. Same English text
        // as `modelfile-constructor-unrecmodelelem` above; see that entry's
        // note.
        sources: &["src/introspect/classdeclaration.ts:130"],
    },
    CatalogueEntry {
        code: "classdeclaration-validate-selfextending",
        template: "Class \"{class}\" cannot extend itself.",
        renderer: Renderer::Globalize,
        // ClassDeclaration.validate (RUST); not yet called.
        sources: &["src/introspect/classdeclaration.ts:217"],
    },
    CatalogueEntry {
        code: "classdeclaration-validate-identifiernotproperty",
        template: "Class \"{class}\" is identified by field \"{idField}\", but does not contain this property.",
        renderer: Renderer::Globalize,
        // ClassDeclaration.validate (RUST); not yet called.
        sources: &["src/introspect/classdeclaration.ts:228"],
    },
    CatalogueEntry {
        code: "classdeclaration-validate-identifiernotstring",
        template: "Class \"{class}\" is identified by field \"{idField}\", but the type of the field is not \"String\".",
        renderer: Renderer::Globalize,
        // ClassDeclaration.validate (RUST); not yet called.
        sources: &["src/introspect/classdeclaration.ts:241"],
    },
    CatalogueEntry {
        code: "classdeclaration-validate-duplicatefieldname",
        template: "Class \"{class}\" has more than one field named \"{fieldName}\".",
        renderer: Renderer::Globalize,
        // ClassDeclaration.validate (RUST); not yet called. The pre-port
        // check in validation.rs (`check_unique_field_names`) stands in for
        // it until the task that ports ClassDeclaration.validate replaces
        // its message with this entry. The call spans two lines in TS
        // (`Globalize('en').messageFormatter(` then the key), which is why
        // the line-oriented grep this module doc used to give missed it.
        sources: &["src/introspect/classdeclaration.ts:278"],
    },
    // ---- P2-03 additions (ClassDeclaration.getNestedProperty's own two
    //      inline templates; #47) ----
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
        // A plain `Error`, not an `IllegalModelException` (`ErrorKind::Error`
        // at the throw site in model_manager.rs): the one throw in
        // `getNestedProperty` that TS does not build through
        // `IllegalModelException`.
        sources: &["src/introspect/classdeclaration.ts:593"],
    },
    CatalogueEntry {
        code: "instancegenerator-newinstance-noconcreteclass",
        template: "No concrete extending type for \"{type}\".",
        renderer: Renderer::Globalize,
        // InstanceGenerator.findConcreteSubclass (RUST), reached from
        // newInstance; not yet called. Thrown as a plain `Error`, not
        // `IllegalModelException` (ErrorKind::Error, table 2.3).
        sources: &["src/serializer/instancegenerator.ts:204"],
    },
    CatalogueEntry {
        code: "serializer-tojson-notcobject",
        template: "\"Serializer.toJSON\" only accepts \"Concept\", \"Event\", \"Asset\", \"Participant\" or \"Transaction\".",
        renderer: Renderer::Globalize,
        // Serializer.toJSON (HYBRID); not yet called. `Globalize.formatMessage`
        // (no params), thrown as a plain `Error` (ErrorKind::Error, table 2.3).
        sources: &["src/serializer.ts:102"],
    },
    // ResourceValidator (HYBRID): every `report*` method's key, none called yet.
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
    // ---- P3-01: ResourceValidator's own inline templates (plain `Error`,
    //      ErrorKind::Error — never `ValidationException`, table 2.3), 2.2
    //      step 2. Each `${expr}` becomes a named `{param}`. ----
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
        // TS: `'Expected a Map, but found ' + JSON.stringify(obj)`: a string
        // concatenation, so `{obj}` is the caller's own `JSON.stringify`
        // text (2.1: "Where TS calls JSON.stringify(value) first ... the
        // param is that JSON text").
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
    // ---- P2-01 review fix: ResourceId (`src/model/resourceid.ts`), the
    //      ledger group's SEAM_LEDGER.tsv planned_task P2-01+P4-03 members
    //      the first P2-01 pass left unported (parseUri, the constructor,
    //      fromURI, toURI). None of these are en.json/Globalize keys, so
    //      none is in the OD-5 scope test; each is an inline template
    //      (2.2 step 2), ported with its unit in this same PR (6.3). ----
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
    // ---- P2-04 additions (Property.getFullyQualifiedTypeName's own inline
    //      template; #48) ----
    CatalogueEntry {
        code: "property-getfullyqualifiedtypename-notfound",
        template: "Failed to find fully qualified type name for property {name} with type {type}",
        renderer: Renderer::Inline,
        // A plain `Error`, not an `IllegalModelException` (`ErrorKind::Error`
        // at the throw site in model_manager.rs).
        sources: &["src/introspect/property.ts:218"],
    },
    // ---- P4-07 additions (Property.process's own inline template; #66) ----
    CatalogueEntry {
        code: "property-process-invalidname",
        template: "Invalid property name '{name}'",
        renderer: Renderer::Inline,
        sources: &["src/introspect/property.ts:86"],
    },
    // ---- P4-07 additions (Property.validate, RelationshipDeclaration.validate
    //      and MapDeclaration/MapKeyType/MapValueType's own inline templates; #66) ----
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
    // ---- P3-01b additions (Serializer, Factory, JSONPopulator, JSONGenerator
    //      and the Resource-mutating members; accordproject/concerto-rust#124) ----
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
        // and `JSONGenerator.visit`, for an introspection object (a scalar
        // declaration, an enum value): `JSON.stringify` meets the model
        // manager again through the model file and throws before the
        // `Error` is built. The cycle it names is always the same one.
        code: "engine-typeerror-circularjson",
        template: "Converting circular structure to JSON\n    --> starting at object with constructor 'ModelManager'\n    |     property 'modelFiles' -> object with constructor 'Object'\n    |     property 'concerto.decorator@1.0.0' -> object with constructor 'ModelFile'\n    --- property 'modelManager' closes the circle",
        renderer: Renderer::Inline,
        sources: &[
            "V8 (JSON.stringify of a cyclic object), src/serializer/jsonpopulator.ts:124, src/serializer/jsongenerator.ts:72",
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
    /// This does *not* also require every entry's `template` to be unique:
    /// `en.json` itself gives two different keys
    /// (`modelfile-constructor-unrecmodelelem`,
    /// `classdeclaration-process-unrecmodelelem`) the same English text, and
    /// OD-5 ports each key it scopes in regardless (2.2 step 1: "the
    /// catalogue key is `<key>`"). `code` is what a fixture is attributed to
    /// (OD-10), so it is `code`, not `template`, that must not collide.
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
