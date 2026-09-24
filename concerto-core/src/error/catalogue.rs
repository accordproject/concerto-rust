//! The message catalogue (PORTING.md section 2.2, OD-5).
//!
//! Rust owns the message templates the ported units of `concerto-core`
//! throw: this is the verbatim port of the `messages/en.json` keys and the
//! inline templates that a RUST or HYBRID member's throw site uses (OD-5),
//! plus the `factory-newinstance-*` keys and `typenotfounderror-defaultmessage`
//! OD-5 pre-approves ahead of their own call site (P3-01, table 2.3). No
//! unused `en.json` key is ported: `composer-*`, `whereastvalidator-*`,
//! `like` and `test-*` have no throw site in `concerto-core` and stay in TS.
//!
//! **Deriving the OD-5 scope.** "Every `en.json` key used by a RUST or
//! HYBRID member" (OD-5) means: take the ledger
//! (`migration/ledger/SEAM_LEDGER.tsv`, commit `c48423c`, PORTING.md OD-6)
//! rows whose `classification` is `RUST` or `HYBRID`, for each one grep its
//! `file`/`class`/`member` in the frozen TS reference for
//! `Globalize.messageFormatter(...)` or `Globalize.formatMessage(...)`, and
//! port every key that turns up (2.2 step 1), plus the pre-approved keys
//! above. The reproducible form of that grep, run from
//! `packages/concerto-core/src` in the TS checkout:
//! `grep -n "Globalize\.\(messageFormatter\|formatMessage\)" <file>` for
//! each ledger row's `file`, filtered to the methods named in `member` (the
//! P1-07 attribution index, OD-10, automates this once it exists; until
//! then a P1/P2/P3 task that finds a key the census missed adds it here,
//! citing the ledger row). The trial (P0-04b) and the first cut of P1-05
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
        // OD-5 / #32 point 4: one of Factory.newResource's four model
        // checks; not yet called (P3-01 delegates them to Rust).
        sources: &["src/factory.ts (messages/en.json)"],
    },
    CatalogueEntry {
        code: "factory-newinstance-invalididentifier",
        template: "Invalid or missing identifier for Type \"{type}\" in namespace \"{namespace}\".",
        renderer: Renderer::Globalize,
        sources: &["src/factory.ts (messages/en.json)"],
    },
    CatalogueEntry {
        code: "factory-newinstance-abstracttype",
        template: "Cannot instantiate the abstract type \"{type}\" in the \"{namespace}\" namespace.",
        renderer: Renderer::Globalize,
        sources: &["src/factory.ts (messages/en.json)"],
    },
    CatalogueEntry {
        code: "factory-newinstance-typenotdeclaredinns",
        template: "Cannot instantiate Type \"{type}\" in namespace \"{namespace}\".",
        renderer: Renderer::Globalize,
        sources: &["src/factory.ts (messages/en.json)"],
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
        // ModelFile.resolveImport (RUST); not yet called. `imports` is
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
