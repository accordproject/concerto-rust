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
        sources: &["src/basemodelmanager.ts:661"],
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

    /// Every entry is unique, cites its source, and (`"pre-port"` excepted)
    /// has a golden test in `mod.rs`, named after its code (checked by name,
    /// PORTING.md 6.3).
    #[test]
    fn catalogue_is_complete() {
        let golden_tests_source = include_str!("mod.rs");
        for (i, entry) in CATALOGUE.iter().enumerate() {
            assert!(!entry.sources.is_empty(), "{} cites no source", entry.code);
            assert!(
                CATALOGUE[..i]
                    .iter()
                    .all(|e| e.code != entry.code && e.template != entry.template),
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
