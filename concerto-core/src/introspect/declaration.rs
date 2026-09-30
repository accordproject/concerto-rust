//! Internal representations of the Concerto declarations.
//!
//! Concerto's JavaScript runtime models declarations as an inheritance
//! hierarchy: concept, asset, participant, transaction and event all extend a
//! common class declaration. Inheritance like that isn't idiomatic in Rust, so
//! the five class-like declarations are represented by a single
//! [`ClassDeclaration`] over the matching generated `mm::*Declaration` struct,
//! tagged with a [`ClassKind`], while enums, scalars and maps are the other
//! variants of the [`Declaration`] sum type. Each variant is selected by
//! matching on the node's `$class`.

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::derive::{DeclarationKind, Named};
use crate::error::{ContractError, Error, ErrorKind, Result};
use crate::introspect::decorator::{
    Decorator, WithDecorators, parse_decorator_list, parse_decorators,
};
use crate::introspect::model_file::unreadable_ast;
use crate::introspect::property::Property;
use crate::introspect::scalar::{self, ScalarDeclaration};
use crate::introspect::typed_ast::{self, TypedDeclaration, TypedProperty};
use crate::introspect::{
    DeclarationKind, HasValidators, Named, Typed, declared_class, qualified_class,
};
use crate::model_util::{is_system_property, is_valid_identifier, qualify, short_name};

/// Which class-like declaration a [`ClassDeclaration`] represents. Its
/// [`DeclarationKind`] is the metamodel `$class` short name for the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, DeclarationKind)]
#[non_exhaustive]
pub enum ClassKind {
    /// A `concept`.
    #[concerto(kind = "ConceptDeclaration")]
    Concept,
    /// An `asset` (system-identifiable).
    #[concerto(kind = "AssetDeclaration")]
    Asset,
    /// A `participant` (system-identifiable).
    #[concerto(kind = "ParticipantDeclaration")]
    Participant,
    /// A `transaction`.
    #[concerto(kind = "TransactionDeclaration")]
    Transaction,
    /// An `event`.
    #[concerto(kind = "EventDeclaration")]
    Event,
}

impl ClassKind {
    /// The metamodel `$class` short name for this kind, such as
    /// `ConceptDeclaration`.
    pub fn declaration_kind(self) -> &'static str {
        DeclarationKind::declaration_kind(&self)
    }

    pub(crate) fn from_short(short: &str) -> Option<Self> {
        Some(match short {
            "ConceptDeclaration" => Self::Concept,
            "AssetDeclaration" => Self::Asset,
            "ParticipantDeclaration" => Self::Participant,
            "TransactionDeclaration" => Self::Transaction,
            "EventDeclaration" => Self::Event,
            _ => return None,
        })
    }
}

/// The generated struct behind a [`ClassDeclaration`], one variant per kind.
#[derive(Debug, Clone)]
pub(crate) enum ClassNode {
    Concept(mm::ConceptDeclaration),
    Asset(mm::AssetDeclaration),
    Participant(mm::ParticipantDeclaration),
    Transaction(mm::TransactionDeclaration),
    Event(mm::EventDeclaration),
}

/// Picks the same field out of whichever generated struct a [`ClassNode`]
/// holds; the five share the metamodel's class declaration fields.
macro_rules! class_field {
    ($node:expr, $decl:ident => $field:expr) => {
        match $node {
            ClassNode::Concept($decl) => $field,
            ClassNode::Asset($decl) => $field,
            ClassNode::Participant($decl) => $field,
            ClassNode::Transaction($decl) => $field,
            ClassNode::Event($decl) => $field,
        }
    };
}

impl ClassNode {
    /// Sets the node's `identified` ([`identified_from_ast`]).
    pub(crate) fn set_identified(&mut self, identified: Option<mm::Identified>) {
        class_field!(self, d => d.identified = identified);
    }

    /// Sets the node's `location`.
    pub(crate) fn set_location(&mut self, location: Option<mm::Range>) {
        class_field!(self, d => d.location = location);
    }
}

/// A class-like declaration's `identified`, read from its AST value as
/// `ClassDeclaration.process` reads it (P5-61 keeps this lenient read:
/// BC-19's shape check accepts any value with no own keys here, a number,
/// a boolean, `""`, `[]` or `{}`, as well as a well-formed node). TS tests
/// `this.ast.identified` for truthiness, then compares its `$class` with the
/// `IdentifiedBy` class by `===`:
/// - a falsy value is no identity;
/// - an `IdentifiedBy` gives `this.idField = this.ast.identified.name`,
///   read only by truthiness afterwards (`if (this.idField)`), so a falsy
///   name is no identity either;
/// - anything else truthy is system identity (`idField = '$identifier'`,
///   `addIdentifierField()`), whatever it holds.
///
/// An `IdentifiedBy` with a truthy name that is not a string is an error
/// (the shape check rejects it first).
pub(crate) fn identified_from_ast(
    value: &serde_json::Value,
) -> std::result::Result<Option<mm::Identified>, serde_json::Error> {
    if !crate::ecma::is_truthy(value) {
        return Ok(None);
    }
    if value.get("$class").and_then(serde_json::Value::as_str)
        == Some("concerto.metamodel@1.0.0.IdentifiedBy")
    {
        if !value.get("name").is_some_and(crate::ecma::is_truthy) {
            return Ok(None);
        }
        return serde::Deserialize::deserialize(value).map(Some);
    }
    Ok(Some(mm::Identified::Identified))
}

/// A concept-like declaration: concept, asset, participant, transaction or
/// event, distinguished by [`ClassDeclaration::kind`].
///
/// It wraps the generated `mm::*Declaration` struct for its kind. The one
/// field held apart is the property list: a class declaration's `properties`
/// may hold an `EnumProperty`, which the generated `mm::Property` union does
/// not cover, so each property is kept as a [`Property`] (itself a newtype
/// over its generated struct) and the generated struct's own `properties` is
/// left empty.
///
/// Two more things are folded in at load time rather than read verbatim from
/// the AST:
///
/// - `implicit_super_type`: a class whose AST carries no `superType` extends
///   one implicitly, unless it is the system model's own `Concept`
///   declaration (the root of the hierarchy, which has none). Which type is
///   *not* uniformly `Concept` (TS: `ModelFile.fromAst`,
///   src/introspect/modelfile.ts, not `ClassDeclaration.process`): an
///   `Asset`/`Participant`/`Transaction`/`Event`-kind class defaults to its
///   own kind (an asset with no `extends` implicitly extends `Asset`, and so
///   on); only a `Concept`-kind class (or an enum, [`EnumDeclaration`]) falls
///   back to `Concept` itself. [`super_type`] returns this whenever the AST
///   itself has none, so every other member that reads it (resolution,
///   `validate`, `toString`) sees the same single effective super type TS
///   keeps in `this.superType`.
/// - the `$identifier`/`$timestamp` system fields: the loader
///   (`Declaration::from_typed`) appends them to `properties` the same way `addIdentifierField`/
///   `addTimestampField` do, so [`own_properties`] carries them like any
///   other field from here on.
///
/// [`super_type`]: ClassDeclaration::super_type
/// [`own_properties`]: ClassDeclaration::own_properties
#[derive(Debug, Clone)]
pub struct ClassDeclaration {
    node: ClassNode,
    properties: Vec<Property>,
    implicit_super_type: Option<mm::TypeIdentifier>,
    decorators: Vec<Decorator>,
}

js_compat_pub! {
    /// [`ClassDeclaration::process_decision`]'s result: the `superType`/`idField`
    /// decision `ClassDeclaration.process` (src/introspect/classdeclaration.ts)
    /// makes before its `ast.properties` loop.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct ProcessDecision {
        /// TS: `this.superType`, once `process()` has set it.
        pub super_type: Option<String>,
        /// TS: `this.idField`, once `process()` has set it.
        pub id_field: Option<String>,
        /// Whether the view must still call its own `addIdentifierField()`
        /// (pushes a real `Field` view, constructed in TS; since P4-07 that
        /// view's `process` delegates to the Rust `fieldProcess` binding).
        pub add_identifier_field: bool,
        /// Whether the view must still call its own `addTimestampField()`.
        pub add_timestamp_field: bool,
    }
}

impl ClassDeclaration {
    /// The kind of class-like declaration this is.
    pub fn kind(&self) -> ClassKind {
        match self.node {
            ClassNode::Concept(_) => ClassKind::Concept,
            ClassNode::Asset(_) => ClassKind::Asset,
            ClassNode::Participant(_) => ClassKind::Participant,
            ClassNode::Transaction(_) => ClassKind::Transaction,
            ClassNode::Event(_) => ClassKind::Event,
        }
    }

    /// Abstract types can't be instantiated on their own.
    pub fn is_abstract(&self) -> bool {
        class_field!(&self.node, d => d.is_abstract)
    }

    /// The super type this declaration extends. `None` only for the system
    /// model's own `Concept` declaration, the root of the hierarchy; every
    /// other class-like declaration has one, whether the AST names it
    /// explicitly or, when the AST carries no `superType` at all, implicitly
    /// (the struct doc comment).
    ///
    /// TS: after `ClassDeclaration.process` has run, `this.superType`
    /// (src/introspect/classdeclaration.ts).
    pub fn super_type(&self) -> Option<&mm::TypeIdentifier> {
        class_field!(&self.node, d => d.super_type.as_ref()).or(self.implicit_super_type.as_ref())
    }

    /// The properties declared directly on this type. Inherited properties are
    /// not included; those are gathered separately by walking the supertype
    /// chain.
    pub fn own_properties(&self) -> &[Property] {
        &self.properties
    }

    /// The source location, if the AST carried one.
    pub fn location(&self) -> Option<&mm::Range> {
        class_field!(&self.node, d => d.location.as_ref())
    }

    /// True if this class declaration's own AST declares an identity,
    /// whether system-assigned or explicit. Unlike TS's inherited
    /// `isIdentified()` (src/introspect/classdeclaration.ts), this does not
    /// walk the super type chain: it answers the same question as TS's own
    /// `this.idField`, which is what the callers in this crate that gate on
    /// it need (the `validate()` block this field controls, PORTING.md 2.1).
    /// A subtype's *inherited* identity is `ModelManager::identifier_field_name`
    /// (crate::model_manager::ModelManager::identifier_field_name).
    pub fn is_identified(&self) -> bool {
        self.identified().is_some()
    }

    /// The name of the field that provides this class's own identity, for a
    /// type that is identified by one of its own fields (`identified by
    /// field`). A system-identified type (`identified`) or a type with no
    /// own identity both return `None`; unlike
    /// [`ClassDeclaration::is_identified`], never true from inheritance.
    ///
    /// TS: `ClassDeclaration.isExplicitlyIdentified` reduces to this exact
    /// check (`!!this.idField && this.idField !== '$identifier'`): the
    /// explicit branch always holds a name other than `$identifier`, so the
    /// two are equivalent.
    pub fn identifier_field_name(&self) -> Option<&str> {
        match self.identified() {
            Some(mm::Identified::IdentifiedBy(by)) => Some(&by.name),
            _ => None,
        }
    }

    /// [`ClassDeclaration::identifier_field_name`], but also giving
    /// `$identifier` for a system-identified type. This is the per-class
    /// step [`ModelManager::identifier_field_name`]
    /// (crate::model_manager::ModelManager::identifier_field_name) walks up
    /// the super type chain: own explicit or system identity, or `None` to
    /// keep climbing.
    pub(crate) fn own_identifier_field_name(&self) -> Option<&str> {
        match self.identified() {
            Some(mm::Identified::IdentifiedBy(by)) => Some(&by.name),
            Some(mm::Identified::Identified) => Some("$identifier"),
            None => None,
        }
    }

    fn identified(&self) -> Option<&mm::Identified> {
        class_field!(&self.node, d => d.identified.as_ref())
    }

    /// `true` if this class declaration's own AST declares an *explicit*
    /// identifier (`identified by field`, never the system `identified`).
    /// Never true from inheritance, matching [`ClassDeclaration::identifier_field_name`].
    ///
    /// TS: `ClassDeclaration.isExplicitlyIdentified` (src/introspect/classdeclaration.ts):
    /// `!!this.idField && this.idField !== '$identifier'`, which
    /// [`ClassDeclaration::identifier_field_name`]'s own doc comment already
    /// notes reduces to this exact check.
    pub fn is_explicitly_identified(&self) -> bool {
        self.identifier_field_name().is_some()
    }

    /// `true` if this class is the definition of an asset.
    ///
    /// TS: `ClassDeclaration.isAsset` (src/introspect/classdeclaration.ts):
    /// `this.type === AssetDeclaration $class`.
    pub fn is_asset(&self) -> bool {
        matches!(self.kind(), ClassKind::Asset)
    }

    /// `true` if this class is the definition of a participant.
    ///
    /// TS: `ClassDeclaration.isParticipant`.
    pub fn is_participant(&self) -> bool {
        matches!(self.kind(), ClassKind::Participant)
    }

    /// `true` if this class is the definition of a transaction.
    ///
    /// TS: `ClassDeclaration.isTransaction`.
    pub fn is_transaction(&self) -> bool {
        matches!(self.kind(), ClassKind::Transaction)
    }

    /// `true` if this class is the definition of an event.
    ///
    /// TS: `ClassDeclaration.isEvent`.
    pub fn is_event(&self) -> bool {
        matches!(self.kind(), ClassKind::Event)
    }

    /// `true` if this class is the definition of a concept.
    ///
    /// TS: `ClassDeclaration.isConcept`.
    pub fn is_concept(&self) -> bool {
        matches!(self.kind(), ClassKind::Concept)
    }

    /// `false`: a Rust [`ClassDeclaration`] is one of the five concept-like
    /// kinds and is never an enum (enums are [`Declaration::Enum`]).
    ///
    /// TS: `ClassDeclaration.isEnum` (src/introspect/classdeclaration.ts):
    /// `this.type === EnumDeclaration $class`, which is never true for one of
    /// these five kinds; `EnumDeclaration` inherits the method unchanged, so
    /// the oracle also records `true` results under this op, for an actual
    /// `EnumDeclaration` receiver — those are `Declaration::is_enum_declaration`
    /// instead (same check, TS's `this.type` and Rust's variant tag agree).
    pub fn is_enum(&self) -> bool {
        false
    }

    /// `false`: never true for one of the five concept-like kinds, for the
    /// same reason as [`ClassDeclaration::is_enum`].
    ///
    /// TS: `ClassDeclaration.isMapDeclaration`.
    pub fn is_map_declaration(&self) -> bool {
        false
    }

    js_compat_pub! {
        /// The string representation TS's `ClassDeclaration.toString`
        /// (src/introspect/classdeclaration.ts) builds: `super_type_name` is the
        /// raw (unqualified) name TS keeps in `this.superType` — the AST's own
        /// `superType.name`, or the implicit `'Concept'` — never a resolved FQN.
        pub fn to_string(fqn: &str, super_type_name: Option<&str>, is_abstract: bool) -> String {
            let super_part = super_type_name.map_or_else(String::new, |n| format!(" super={n}"));
            // `EnumDeclaration` overrides `toString`, so a `ClassDeclaration`
            // receiver is never an enum here (`is_enum` above).
            format!("ClassDeclaration {{id={fqn}{super_part} enum=false abstract={is_abstract}}}")
        }
    }

    /// `true` for the system model's own `Concept` declaration: the root of
    /// the class hierarchy, the one declaration that has no super type at
    /// all, explicit or implicit.
    ///
    /// TS: the exemption in `ClassDeclaration.process`
    /// (src/introspect/classdeclaration.ts): `this.modelFile.isSystemModelFile()
    /// && this.name === 'Concept'`.
    fn is_system_concept(namespace: &str, name: &str) -> bool {
        is_system_model_namespace(namespace) && name == "Concept"
    }

    js_compat_pub! {
        /// TS: the kind-compatibility check in `ClassDeclaration._resolveSuperType`
        /// (src/introspect/classdeclaration.ts): `classDecl.declarationKind() !==
        /// 'ConceptDeclaration' && this.declarationKind() !== classDecl.declarationKind()`,
        /// negated (`true` when compatible — a subtype may always extend a
        /// concept, and otherwise both sides must be the same kind). Each side is
        /// the receiver's own `declarationKind()` string
        /// ([`DeclarationKind::declaration_kind`]); resolving the super type
        /// declaration itself is a collaborator call the binding still makes.
        pub fn kinds_compatible(child_kind: &str, super_kind: &str) -> bool {
            super_kind == "ConceptDeclaration" || child_kind == super_kind
        }
    }

    js_compat_pub! {
        /// TS: the super-type identifier redeclaration check in
        /// `ClassDeclaration.validate` (src/introspect/classdeclaration.ts), the
        /// block guarded by `superType.isIdentified()` (the caller checks that
        /// before calling this): `true` when the super type's existing
        /// identifier cannot be redeclared. Resolving `superType` itself is a
        /// collaborator call the binding still makes.
        pub fn identifier_redeclare_conflict(
            child_is_system_identified: bool,
            super_is_system_identified: bool,
            super_is_explicitly_identified: bool,
        ) -> bool {
            if child_is_system_identified {
                !super_is_system_identified
            } else {
                super_is_explicitly_identified
            }
        }
    }

    js_compat_pub! {
        /// TS: `ClassDeclaration.isAsset`/`isParticipant`/`isTransaction`/
        /// `isEvent`/`isConcept`/`isEnum`/`isMapDeclaration`
        /// (src/introspect/classdeclaration.ts): each compares `this.type` (the
        /// AST's own `$class`, already set by `process()`) against one metamodel
        /// `$class`'s short name. `ast_class` is the receiver's `this.type`;
        /// `want` is the metamodel short name to compare against
        /// (`"AssetDeclaration"`, …).
        pub fn is_kind(ast_class: &str, want: &str) -> bool {
            short_name(ast_class) == want
        }
    }

    js_compat_pub! {
        /// The `superType`/`idField` decision `ClassDeclaration.process` makes
        /// before its `ast.properties` loop (src/introspect/classdeclaration.ts;
        /// the loop itself builds `Field`/`RelationshipDeclaration`/
        /// `EnumValueDeclaration` views, kept in TS). TS: `if (this.ast.superType)
        /// { this.superType = this.ast.superType.name; } else if (!(isSystemModelFile
        /// && name === 'Concept')) { this.superType = 'Concept'; }` — a truthiness
        /// test on the AST *node*, not on its `name`.
        ///
        /// `explicit_super_type` tells this function only whether that outer
        /// truthiness test took the first branch at all (`Some`) or fell through
        /// to the implicit-default branch (`None`) — **not** what
        /// `this.ast.superType.name` itself was. When the outer node is truthy,
        /// TS's own plain, unconditional assignment (`this.superType =
        /// this.ast.superType.name`) can leave `this.superType` as a string, but
        /// also as `undefined`, `null`, a number, a boolean, or any other JSON
        /// value the AST carries; a bare `Option<&str>` cannot represent all of
        /// those (accordproject/concerto-rust#217, #219), so a caller that needs
        /// that raw value on its own snapshot (the WASM binding does) threads it
        /// through separately and never reads `ProcessDecision::super_type` on
        /// this branch at all — this function's own `Some(t) => Some(t.to_string())`
        /// exists only to keep the type honest for direct unit testing and any
        /// caller that genuinely has nothing more specific than a string; the
        /// binding passes a placeholder here and ignores what comes back.
        /// `identified_class` is `this.ast.identified.$class`; `identified_name`
        /// is `this.ast.identified.name`, again exactly as given (only
        /// meaningful for an explicit `IdentifiedBy`, and subject to the same
        /// raw-value caveat as `explicit_super_type`). `fqn` is `this.fqn`, read
        /// once `this.name` and `this.modelFile` are set (`Declaration.process`
        /// runs first).
        pub fn process_decision(
            explicit_super_type: Option<&str>,
            is_system_model_file: bool,
            name: &str,
            identified_class: Option<&str>,
            identified_name: Option<&str>,
            fqn: &str,
        ) -> ProcessDecision {
            let super_type = match explicit_super_type {
                Some(t) => Some(t.to_string()),
                None if Self::is_system_concept_file(is_system_model_file, name) => None,
                None => Some("Concept".to_string()),
            };

            let (id_field, add_identifier_field) = match identified_class {
                None => (None, false),
                Some(class) if class == qualified_class("IdentifiedBy") => {
                    (identified_name.map(str::to_string), false)
                }
                Some(_) => (Some("$identifier".to_string()), true),
            };

            let add_timestamp_field =
                fqn == "concerto@1.0.0.Transaction" || fqn == "concerto@1.0.0.Event";

            ProcessDecision {
                super_type,
                id_field,
                add_identifier_field,
                add_timestamp_field,
            }
        }
    }

    /// `process_decision`'s own exemption test: unlike [`Self::is_system_concept`]
    /// (which also needs the namespace, not available to the binding at this
    /// point), the caller already knows whether its model file is the system
    /// model file.
    fn is_system_concept_file(is_system_model_file: bool, name: &str) -> bool {
        is_system_model_file && name == "Concept"
    }

    /// Builds the declaration from the node the typed read gave
    /// ([`Declaration::from_typed`]): the implicit super type, the
    /// `$identifier`/`$timestamp` system fields and the validator checks.
    /// The system fields are appended exactly as `ClassDeclaration.process`'s
    /// `addIdentifierField`/`addTimestampField` append them in TS: after the
    /// AST's own properties, bypassing the per-property `isSystemProperty`
    /// guard that rejects a `$`-prefixed name from the AST itself.
    /// `namespace` is the namespace of the model file this declaration is
    /// being loaded into, needed for the implicit `Concept` super type and
    /// to recognise the system model's own `Transaction`/`Event`.
    fn finish(
        kind: ClassKind,
        node: ClassNode,
        mut properties: Vec<Property>,
        decorators: Vec<Decorator>,
        namespace: &str,
    ) -> Result<Self> {
        // P5-48: borrowed; `node` is only moved into the result at the end.
        let name: &str = class_field!(&node, d => d.name.as_str());

        let implicit_super_type = if class_field!(&node, d => d.super_type.is_some())
            || Self::is_system_concept(namespace, name)
        {
            None
        } else {
            // TS: not `ClassDeclaration.process`'s own implicit-`Concept`
            // fallback (which only ever fires for a `ConceptDeclaration`, an
            // `EnumDeclaration`, or a scalar/map — none of those wrapped
            // here — because every other kind's AST already has a
            // `superType` by the time `process` sees it). `ModelFile.fromAst`
            // (src/introspect/modelfile.ts) injects it first, per kind, for
            // exactly the four identified kinds: an `AssetDeclaration` with
            // no `superType` defaults to `Asset`, a `TransactionDeclaration`
            // to `Transaction`, an `EventDeclaration` to `Event`, a
            // `ParticipantDeclaration` to `Participant` — never the generic
            // `Concept` — so `ClassDeclaration.process`'s own fallback
            // always finds `this.ast.superType` already set for these four
            // and takes its *other* branch (`this.superType =
            // this.ast.superType.name`), not this one. Only `kind ==
            // ClassKind::Concept` reaches `process`'s own fallback, unset by
            // `fromAst` (its `case ConceptDeclaration` injects nothing).
            let implicit_name = match kind {
                ClassKind::Concept => "Concept",
                ClassKind::Asset => "Asset",
                ClassKind::Participant => "Participant",
                ClassKind::Transaction => "Transaction",
                ClassKind::Event => "Event",
            };
            Some(mm::TypeIdentifier {
                _class: qualified_class("TypeIdentifier"),
                name: implicit_name.to_string(),
                namespace: None,
                resolved_name: None,
            })
        };

        // TS: ClassDeclaration.addIdentifierField, called from `process`
        // whenever the AST carries an `identified` node (system or
        // explicit-by-field alike; an explicit `identified by` field is
        // already in `properties` from the AST, so only the system case adds
        // one here).
        if matches!(
            class_field!(&node, d => d.identified.as_ref()),
            Some(mm::Identified::Identified)
        ) {
            properties.push(Property::String(WithDecorators::new(
                mm::StringProperty {
                    name: "$identifier".to_string(),
                    is_array: false,
                    is_optional: false,
                    size_validator: None,
                    decorators: None,
                    location: None,
                    default_value: None,
                    validator: None,
                    length_validator: None,
                },
                Vec::new(),
            )));
        }

        // TS: ClassDeclaration.addTimestampField, called from `process` only
        // for the system model's own `Transaction`/`Event` declarations
        // (`this.fqn === 'concerto@1.0.0.Transaction' || ... === '...Event'`);
        // every other Transaction/Event inherits the field through
        // `getProperties()` walking up to one of these two. The check is on
        // `namespace`/`name` alone, not `kind`: like every system root
        // declaration, `Transaction` and `Event` are themselves
        // `ConceptDeclaration` nodes in the metamodel AST (`ClassKind::Concept`
        // here) — a class's *own* `$class` names the kind it was declared
        // with (`transaction Payment {}` is a `TransactionDeclaration`), not
        // what it extends, exactly as TS's own `this.ast.$class` is.
        if is_system_model_namespace(namespace) && (name == "Transaction" || name == "Event") {
            properties.push(Property::DateTime(WithDecorators::new(
                mm::DateTimeProperty {
                    name: "$timestamp".to_string(),
                    is_array: false,
                    is_optional: false,
                    size_validator: None,
                    decorators: None,
                    location: None,
                },
                Vec::new(),
            )));
        }

        // TS: each property's own `NumberValidator`/`StringValidator`/
        // `CollectionSizeValidator` construction, part of `Property.process`/
        // `Field.process` (property.ts, field.ts) — deferred to here, in AST
        // order, rather than run from `Property::try_from`
        // ([`Property::check_bound_validators`]'s doc comment), since only
        // this scope has the namespace and class name the error messages
        // need. The two synthesized system fields above never carry a
        // validator, so checking every property here (not just the AST's
        // own) is a no-op for them.
        // P5-48: built only when a property has a validator to rebuild.
        let fqn = if properties.iter().any(Property::has_bound_validators) {
            qualify(namespace, name)
        } else {
            String::new()
        };
        for property in &properties {
            property.check_bound_validators(&fqn)?;
        }

        Ok(Self {
            node,
            properties,
            implicit_super_type,
            decorators,
        })
    }

    /// The decorators attached to this declaration.
    ///
    /// TS: `Decorated.getDecorators` (src/introspect/decorated.ts).
    pub fn decorators(&self) -> &[Decorator] {
        &self.decorators
    }
}

/// TS: `ModelFile.isSystemModelFile` (src/introspect/modelfile.ts).
fn is_system_model_namespace(namespace: &str) -> bool {
    namespace.starts_with("concerto@") || namespace == "concerto"
}

impl Named for ClassDeclaration {
    /// The declaration's short name (without namespace).
    fn name(&self) -> &str {
        class_field!(&self.node, d => &d.name)
    }
}

impl DeclarationKind for ClassDeclaration {
    fn declaration_kind(&self) -> &'static str {
        self.kind().declaration_kind()
    }
}

impl ClassDeclaration {
    /// The declaration's short name (without namespace).
    ///
    /// TS: Declaration.getName (src/introspect/declaration.ts)
    pub fn name(&self) -> &str {
        Named::name(self)
    }
}

/// Loads a scalar declaration: the generated node for its `$class` (a
/// strict read), the name check (TS `Declaration.process`), then the ported
/// `ScalarDeclaration.process`.
fn load_scalar(
    short: &str,
    value: &serde_json::Value,
    namespace: &str,
    file_name: Option<&str>,
) -> Result<ScalarDeclaration> {
    let bad = |e: serde_json::Error| unreadable_ast(&e, file_name);
    let v = value;
    let node = match short {
        "BooleanScalar" => {
            mm::ScalarDeclaration::BooleanScalar(serde::Deserialize::deserialize(v).map_err(bad)?)
        }
        "IntegerScalar" => {
            mm::ScalarDeclaration::IntegerScalar(serde::Deserialize::deserialize(v).map_err(bad)?)
        }
        "LongScalar" => {
            mm::ScalarDeclaration::LongScalar(serde::Deserialize::deserialize(v).map_err(bad)?)
        }
        "DoubleScalar" => {
            mm::ScalarDeclaration::DoubleScalar(serde::Deserialize::deserialize(v).map_err(bad)?)
        }
        "StringScalar" => {
            mm::ScalarDeclaration::StringScalar(serde::Deserialize::deserialize(v).map_err(bad)?)
        }
        _ => {
            mm::ScalarDeclaration::DateTimeScalar(serde::Deserialize::deserialize(v).map_err(bad)?)
        }
    };
    let name = scalar::node_name(&node);
    check_declaration_name(name, value.get("location"), file_name)?;
    let fqn = qualify(namespace, name);
    let processed = ScalarDeclaration::process(value, file_name, &|| Ok::<_, Error>(fqn.clone()))?;
    let scalar = ScalarDeclaration::new(node, processed, parse_decorators(value));
    scalar.check_validators()?;
    Ok(scalar)
}

/// A top-level declaration within a model file.
#[derive(Debug, Clone, Named, DeclarationKind)]
#[concerto(delegate)]
#[allow(clippy::large_enum_variant)]
#[non_exhaustive]
pub enum Declaration {
    /// A concept-like declaration (see [`ClassDeclaration`]).
    Class(ClassDeclaration),
    /// An enumeration.
    Enum(EnumDeclaration),
    /// A scalar alias over a primitive.
    Scalar(ScalarDeclaration),
    /// A map type.
    Map(MapDeclaration),
}

/// An enumeration declaration: the generated [`mm::EnumDeclaration`] plus its
/// processed decorators (module doc on [`WithDecorators`]), and its values
/// read as [`Property`] rather than the generated `mm::EnumProperty` list
/// the node itself still carries — the same reason [`ClassDeclaration`] keeps
/// its own `properties` apart from its generated node (module doc there):
/// [`Property`] is what carries each value's own processed decorators, and
/// TS `Decorated.validate`'s duplicate-decorator and `decoratorValidation`
/// checks run over an enum's values exactly as they do over a class's
/// properties (PORTING.md; [`crate::validation`]).
///
/// TS's `EnumDeclaration extends ClassDeclaration`
/// (src/introspect/enumdeclaration.ts) and overrides only `toString` and
/// `declarationKind`; every other `ClassDeclaration` member — identity,
/// properties, the implicit `Concept` super type, `isAbstract` and so on —
/// reaches an enum unchanged. The methods below give this type the same
/// answers [`ClassDeclaration`] gives, over the metamodel's narrower
/// `EnumDeclaration` AST shape (no `isAbstract`, `identified` or `superType`
/// field at all: the grammar never writes them for an enum), so a caller that
/// needs a class-like fact from either kind can read it the same way (see
/// `model_manager::ClassLike`).
#[derive(Debug, Clone)]
pub struct EnumDeclaration {
    inner: WithDecorators<mm::EnumDeclaration>,
    /// The enum's values, read once at load time so
    /// [`EnumDeclaration::own_properties`] can hand out `&Property`s the
    /// arena's `PropId`s address, the same way
    /// [`ClassDeclaration::own_properties`] does.
    values: Vec<Property>,
}

impl Named for EnumDeclaration {
    fn name(&self) -> &str {
        &self.inner.name
    }
}

impl DeclarationKind for EnumDeclaration {
    fn declaration_kind(&self) -> &'static str {
        "EnumDeclaration"
    }
}

impl EnumDeclaration {
    /// The enum's short name (without namespace).
    ///
    /// TS: Declaration.getName (src/introspect/declaration.ts)
    pub fn name(&self) -> &str {
        Named::name(self)
    }
}

impl crate::introspect::Decorated for EnumDeclaration {
    fn decorators(&self) -> &[Decorator] {
        self.inner.decorators()
    }
}

impl EnumDeclaration {
    /// The enum's values, each carrying its own processed decorators.
    pub fn values(&self) -> &[Property] {
        &self.values
    }

    js_compat_pub! {
        /// The string representation TS's `EnumDeclaration.toString`
        /// (src/introspect/enumdeclaration.ts) builds: `'EnumDeclaration {id=' +
        /// this.getFullyQualifiedName() + '}'`, an override of
        /// [`ClassDeclaration::to_string`] with no super type or abstract flag.
        pub fn to_string(fqn: &str) -> String {
            format!("EnumDeclaration {{id={fqn}}}")
        }
    }

    /// The enum's values.
    ///
    /// TS: `ClassDeclaration.getOwnProperties`, inherited unchanged.
    pub fn own_properties(&self) -> &[Property] {
        &self.values
    }

    /// `false`: the metamodel's `EnumDeclaration` AST carries no `isAbstract`
    /// field, so TS's `this.abstract` (set only when `this.ast.isAbstract` is
    /// truthy) is never set for one.
    ///
    /// TS: `ClassDeclaration.isAbstract`, inherited unchanged.
    pub fn is_abstract(&self) -> bool {
        false
    }

    /// `None`: the metamodel's `EnumDeclaration` AST carries no `identified`
    /// field, so TS's `this.idField` is never set for one — its identity, like
    /// every class-like declaration's, can still come from its super type
    /// (`ModelManager::identifier_field_name` walks past this).
    ///
    /// TS: `ClassDeclaration.getIdentifierFieldName`'s own (non-inherited)
    /// step, `this.idField`, inherited unchanged.
    pub fn own_identifier_field_name(&self) -> Option<&str> {
        None
    }

    /// The source location, if the AST carried one.
    pub fn location(&self) -> Option<&mm::Range> {
        self.inner.location.as_ref()
    }

    /// The implicit `Concept` super type every enum has: the metamodel's
    /// `EnumDeclaration` AST carries no `superType` field at all (unlike
    /// [`ClassDeclaration`], whose AST shape allows one), so TS's
    /// `this.ast.superType` is always falsy for one and `ClassDeclaration.process`
    /// always takes its implicit branch (`this.superType = 'Concept'`) —
    /// never the system-root exemption, which only ever applies to the system
    /// model's own `Concept` declaration, itself a [`ClassDeclaration`], never
    /// an enum.
    ///
    /// TS: `ClassDeclaration.process`'s implicit super type, inherited
    /// unchanged (src/introspect/classdeclaration.ts).
    pub fn implicit_super_type(&self) -> mm::TypeIdentifier {
        mm::TypeIdentifier {
            _class: qualified_class("TypeIdentifier"),
            name: "Concept".to_string(),
            namespace: None,
            resolved_name: None,
        }
    }
}

/// A map declaration: the generated [`mm::MapDeclaration`] (the typed read
/// is strict, so its key and value are always of a kind the metamodel
/// declares, with the `type` their kind requires), plus its processed
/// decorators and those of its key and value.
///
/// TS `MapKeyType`/`MapValueType.process` (mapkeytype.ts, mapvaluetype.ts)
/// each run `Decorated.process()` on their own AST node — a `MapKeyType`/
/// `MapValueType` is a `Decorated` in its own right in TS — so the key's and
/// value's decorators are read here too, alongside the map's own (#152).
#[derive(Debug, Clone)]
pub struct MapDeclaration {
    node: mm::MapDeclaration,
    decorators: Vec<Decorator>,
    key_decorators: Vec<Decorator>,
    value_decorators: Vec<Decorator>,
}

impl Named for MapDeclaration {
    fn name(&self) -> &str {
        &self.node.name
    }
}

impl DeclarationKind for MapDeclaration {
    fn declaration_kind(&self) -> &'static str {
        "MapDeclaration"
    }
}

impl MapDeclaration {
    /// The map's short name (without namespace).
    ///
    /// TS: Declaration.getName (src/introspect/declaration.ts)
    pub fn name(&self) -> &str {
        Named::name(self)
    }
}

impl crate::introspect::Decorated for MapDeclaration {
    fn decorators(&self) -> &[Decorator] {
        &self.decorators
    }
}

impl MapDeclaration {
    /// The metamodel `$class` short name of the key node, such as
    /// `StringMapKeyType`.
    pub fn key_kind(&self) -> &str {
        match &self.node.key {
            mm::MapKeyType::StringMapKeyType(_) => "StringMapKeyType",
            mm::MapKeyType::DateTimeMapKeyType(_) => "DateTimeMapKeyType",
            mm::MapKeyType::ObjectMapKeyType(_) => "ObjectMapKeyType",
        }
    }

    /// The metamodel `$class` short name of the value node, such as
    /// `ObjectMapValueType`.
    pub fn value_kind(&self) -> &str {
        match &self.node.value {
            mm::MapValueType::BooleanMapValueType(_) => "BooleanMapValueType",
            mm::MapValueType::DateTimeMapValueType(_) => "DateTimeMapValueType",
            mm::MapValueType::StringMapValueType(_) => "StringMapValueType",
            mm::MapValueType::IntegerMapValueType(_) => "IntegerMapValueType",
            mm::MapValueType::LongMapValueType(_) => "LongMapValueType",
            mm::MapValueType::DoubleMapValueType(_) => "DoubleMapValueType",
            mm::MapValueType::ObjectMapValueType(_) => "ObjectMapValueType",
            mm::MapValueType::RelationshipMapValueType(_) => "RelationshipMapValueType",
        }
    }

    /// The type the key refers to, for a key that is not a primitive.
    pub fn key_type(&self) -> Option<&mm::TypeIdentifier> {
        match &self.node.key {
            mm::MapKeyType::ObjectMapKeyType(k) => Some(&k.type_),
            _ => None,
        }
    }

    /// The type the value refers to, for a value that is not a primitive.
    pub fn value_type(&self) -> Option<&mm::TypeIdentifier> {
        match &self.node.value {
            mm::MapValueType::ObjectMapValueType(v) => Some(&v.type_),
            mm::MapValueType::RelationshipMapValueType(v) => Some(&v.type_),
            _ => None,
        }
    }

    /// The key node's own decorators. TS `MapKeyType` extends `Decorated`
    /// and reads these in its constructor (`MapKeyType.process`,
    /// mapkeytype.ts), independently of the map's own `getDecorators()`.
    pub fn key_decorators(&self) -> &[Decorator] {
        &self.key_decorators
    }

    /// The value node's own decorators. TS `MapValueType` extends
    /// `Decorated` and reads these in its constructor (`MapValueType
    /// .process`, mapvaluetype.ts), independently of the map's own
    /// `getDecorators()`.
    pub fn value_decorators(&self) -> &[Decorator] {
        &self.value_decorators
    }

    /// `MapKeyType.getType` (src/introspect/mapkeytype.ts): the primitive
    /// name for a `String`/`DateTime` key, or the raw (unresolved) referenced
    /// type name for an object key, exactly as `processType` sets `this.type`
    /// from `this.ast.type.name` without consulting the model manager.
    pub fn key_type_name(&self) -> &str {
        match self.key_kind() {
            "DateTimeMapKeyType" => "DateTime",
            "StringMapKeyType" => "String",
            _ => self.key_type().map_or("", |t| t.name.as_str()),
        }
    }

    /// `MapValueType.getType` (src/introspect/mapvaluetype.ts): the primitive
    /// name for a primitive value, or the raw (unresolved) referenced type
    /// name for an object or relationship value.
    pub fn value_type_name(&self) -> &str {
        match self.value_kind() {
            "BooleanMapValueType" => "Boolean",
            "DateTimeMapValueType" => "DateTime",
            "StringMapValueType" => "String",
            "IntegerMapValueType" => "Integer",
            "LongMapValueType" => "Long",
            "DoubleMapValueType" => "Double",
            _ => self.value_type().map_or("", |t| t.name.as_str()),
        }
    }

    js_compat_pub! {
        /// `MapDeclaration.toString` (src/introspect/mapdeclaration.ts):
        /// `MapDeclaration {id=<fully qualified name>}`.
        pub fn to_string(fully_qualified_name: &str) -> String {
            format!("MapDeclaration {{id={fully_qualified_name}}}")
        }
    }

    /// Reads a map declaration node into the generated struct (a strict
    /// read: a key or value of a kind the metamodel does not declare, or
    /// without the `type` its kind requires, is an error), with the
    /// decorators of the map and of its key and value.
    fn from_json(value: &serde_json::Value, file_name: Option<&str>) -> Result<Self> {
        let node: mm::MapDeclaration =
            serde::Deserialize::deserialize(value).map_err(|e| unreadable_ast(&e, file_name))?;
        Ok(Self {
            node,
            decorators: parse_decorators(value),
            key_decorators: value.get("key").map(parse_decorators).unwrap_or_default(),
            value_decorators: value.get("value").map(parse_decorators).unwrap_or_default(),
        })
    }
}

impl Declaration {
    /// The declaration's short name (without namespace).
    ///
    /// TS: Declaration.getName (src/introspect/declaration.ts)
    pub fn name(&self) -> &str {
        Named::name(self)
    }

    /// The metamodel `$class` short name of the declaration, such as
    /// `ConceptDeclaration` or `StringScalar`.
    pub fn declaration_kind(&self) -> &'static str {
        DeclarationKind::declaration_kind(self)
    }
}

impl Typed for Declaration {
    /// The primitive type of a scalar declaration; every other declaration has
    /// none.
    fn type_name(&self) -> Option<&str> {
        match self {
            Self::Scalar(s) => s.type_name(),
            Self::Class(_) | Self::Enum(_) | Self::Map(_) => None,
        }
    }
}

impl Declaration {
    /// Borrow this as a [`ClassDeclaration`], if it is one.
    pub fn as_class(&self) -> Option<&ClassDeclaration> {
        match self {
            Self::Class(c) => Some(c),
            _ => None,
        }
    }

    /// Borrow this as a [`ScalarDeclaration`], if it is one.
    pub fn as_scalar(&self) -> Option<&ScalarDeclaration> {
        match self {
            Self::Scalar(s) => Some(s),
            _ => None,
        }
    }

    /// Borrow this as a [`MapDeclaration`], if it is one.
    pub fn as_map(&self) -> Option<&MapDeclaration> {
        match self {
            Self::Map(m) => Some(m),
            _ => None,
        }
    }

    /// `true` if this is a concept-like (class) declaration.
    pub fn is_class_declaration(&self) -> bool {
        matches!(self, Self::Class(_))
    }

    /// `true` if this is an enum declaration.
    pub fn is_enum_declaration(&self) -> bool {
        matches!(self, Self::Enum(_))
    }

    /// `true` if this is a scalar declaration.
    pub fn is_scalar_declaration(&self) -> bool {
        matches!(self, Self::Scalar(_))
    }

    /// `true` if this is a map declaration.
    pub fn is_map_declaration(&self) -> bool {
        matches!(self, Self::Map(_))
    }
}

impl TryFrom<&serde_json::Value> for Declaration {
    type Error = Error;

    /// Loads a declaration outside any namespace or file.
    fn try_from(value: &serde_json::Value) -> Result<Self> {
        Self::from_model_json(value, "", None)
    }
}

impl Declaration {
    /// Loads a declaration node of the model file for `namespace`, named
    /// `file_name`: both are what the TS declaration reads from its model
    /// file when it reports an error.
    pub(crate) fn from_model_json(
        value: &serde_json::Value,
        namespace: &str,
        file_name: Option<&str>,
    ) -> Result<Self> {
        // TS: `ModelFile.fromAst`'s `switch (thing.$class)` (modelfile.ts)
        // matches the *full* metamodel `$class` strings — seven declaration
        // kinds and exactly six scalar kinds — and sends anything else,
        // including a missing `$class`, a bare short name or another
        // namespace's, to its `default` case before any declaration is
        // constructed.
        let class = declared_class(value);
        let Some(kind) = class
            .strip_prefix("concerto.metamodel@1.0.0.")
            .filter(|kind| is_recognised_kind(kind))
        else {
            // The catalogue's own `{type}` is `thing.$class` verbatim,
            // interpolated as JS does (`undefined` when absent); TS passes
            // the model file but no location.
            let shown = value
                .get("$class")
                .map_or_else(|| "undefined".to_string(), crate::ecma::to_js_string);
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "modelfile-constructor-unrecmodelelem",
                vec![("type", shown)],
            );
            err.model_file = Some(file_name.map(str::to_string));
            return Err(err.into());
        };
        match kind {
            "MapDeclaration" => {
                let map = MapDeclaration::from_json(value, file_name)?;
                check_declaration_name(map.name(), value.get("location"), file_name)?;
                Ok(Self::Map(map))
            }
            "EnumDeclaration" => Self::from_typed(
                typed_ast::declaration_from_value(value)
                    .map_err(|e| unreadable_ast(&e, file_name))?,
                namespace,
                file_name,
            ),
            kind if ClassKind::from_short(kind).is_some() => Self::from_typed(
                typed_ast::declaration_from_value(value)
                    .map_err(|e| unreadable_ast(&e, file_name))?,
                namespace,
                file_name,
            ),
            scalar => Ok(Self::Scalar(load_scalar(
                scalar, value, namespace, file_name,
            )?)),
        }
    }

    /// A declaration read by the typed AST path
    /// ([`crate::introspect::typed_ast`]): a class-like or enum declaration
    /// read into its generated struct, or any other kind read as its own
    /// JSON subtree and loaded by [`Declaration::from_model_json`]. Runs
    /// the loader's checks on it, in TS's order: the declaration's name
    /// (`Declaration.process`), then each property's name
    /// (`ClassDeclaration.process`'s loop, `Property.process`), then, for a
    /// class-like declaration, its validators.
    pub(crate) fn from_typed(
        declaration: TypedDeclaration,
        namespace: &str,
        file_name: Option<&str>,
    ) -> Result<Self> {
        match declaration {
            TypedDeclaration::Ast(value) => Self::from_model_json(&value, namespace, file_name),
            TypedDeclaration::Class {
                kind,
                node,
                properties,
                decorators,
                location,
            } => {
                check_declaration_name(
                    class_field!(&node, d => &d.name),
                    location.as_ref(),
                    file_name,
                )?;
                check_property_names(&properties, location.as_ref())
                    .and_then(|()| {
                        ClassDeclaration::finish(
                            kind,
                            node,
                            properties.into_iter().map(|p| p.property).collect(),
                            parse_decorator_list(decorators.as_ref()),
                            namespace,
                        )
                    })
                    .map(Self::Class)
                    .map_err(|e| with_model_file(e, file_name))
            }
            TypedDeclaration::Enum {
                node,
                values,
                decorators,
                location,
            } => {
                check_declaration_name(&node.name, location.as_ref(), file_name)?;
                check_property_names(&values, location.as_ref())
                    .map_err(|e| with_model_file(e, file_name))?;
                Ok(Self::Enum(EnumDeclaration {
                    inner: WithDecorators::new(node, parse_decorator_list(decorators.as_ref())),
                    values: values.into_iter().map(|p| p.property).collect(),
                }))
            }
        }
    }
}

/// TS: `ClassDeclaration.process`'s properties loop (classdeclaration.ts,
/// inherited by `EnumDeclaration`): a reserved system property name is
/// rejected before the property is built, with the *declaration's*
/// location; then `Property.process` rejects a name that is not a valid
/// identifier, with the property's own.
fn check_property_names(
    properties: &[TypedProperty],
    declaration: Option<&serde_json::Value>,
) -> Result<()> {
    for TypedProperty { property, location } in properties {
        let name = property.name();
        if is_system_property(name) {
            // The model file's name is filled in by the caller
            // (`with_model_file`).
            return Err(ContractError::pre_port(
                ErrorKind::IllegalModel,
                format!("Invalid field name '{name}'"),
                declaration.cloned(),
            )
            .into());
        }
        if !is_valid_identifier(name) {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "property-process-invalidname",
                vec![("name", name.to_string())],
            );
            err.location = location.clone();
            return Err(err.into());
        }
    }
    Ok(())
}

/// TS: `ClassDeclaration.process` passes `this.modelFile` to every
/// `IllegalModelException` it throws, so an `IllegalModel` contract error
/// raised while a class-like or enum declaration is built names the file
/// being loaded (`file_name`) unless it already names one.
fn with_model_file(mut err: Error, file_name: Option<&str>) -> Error {
    if err.ported().is_some_and(|contract| {
        contract.kind == ErrorKind::IllegalModel && contract.model_file.is_none()
    }) {
        err.contract_mut().model_file = Some(file_name.map(str::to_string));
    }
    err
}

/// The `$class` short names TS `ModelFile.fromAst` recognises, once the
/// `concerto.metamodel@1.0.0.` prefix has matched.
fn is_recognised_kind(kind: &str) -> bool {
    ClassKind::from_short(kind).is_some()
        || matches!(
            kind,
            "EnumDeclaration"
                | "MapDeclaration"
                | "BooleanScalar"
                | "IntegerScalar"
                | "LongScalar"
                | "DoubleScalar"
                | "StringScalar"
                | "DateTimeScalar"
        )
}

/// TS: `Declaration.process`'s name check — `new IllegalModelException(
/// \`Invalid class name '${this.ast.name}'\`, this.modelFile,
/// this.ast.location)` when `ModelUtil.isValidIdentifier(this.ast.name)`
/// fails. The typed read has already required a string `name`.
fn check_declaration_name(
    name: &str,
    location: Option<&serde_json::Value>,
    file_name: Option<&str>,
) -> Result<()> {
    if is_valid_identifier(name) {
        return Ok(());
    }
    let mut err = ContractError::pre_port(
        ErrorKind::IllegalModel,
        format!("Invalid class name '{name}'"),
        location.cloned(),
    );
    err.model_file = Some(file_name.map(str::to_string));
    Err(err.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decl(json: serde_json::Value) -> Declaration {
        Declaration::try_from(&json).expect("valid declaration")
    }

    #[test]
    fn parses_concept_with_typed_properties() {
        let d = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Person",
            "isAbstract": false,
            "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Thing" },
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "firstName", "isArray": false, "isOptional": false },
                { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "age", "isArray": false, "isOptional": true }
            ]
        }));

        let c = d.as_class().expect("class");
        assert_eq!(c.kind(), ClassKind::Concept);
        assert_eq!(c.name(), "Person");
        assert!(!c.is_abstract());
        assert_eq!(c.super_type().map(|t| t.name.as_str()), Some("Thing"));
        assert_eq!(c.own_properties().len(), 2);
        assert_eq!(c.own_properties()[0].type_name(), Some("String"));
        assert!(c.own_properties()[1].is_optional());

        assert!(d.is_class_declaration());
        assert!(!d.is_enum_declaration());
    }

    /// `identified: {$class: IdentifiedBy, name: null}`: TS's
    /// `this.idField = this.ast.identified.name` also ends up `null` here,
    /// read by a plain truthiness check (`if (this.idField)`), so this loads
    /// with no id field at all — not a decode error either.
    #[test]
    fn an_explicit_null_identified_name_loads_with_no_id_field() {
        let d = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Person",
            "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": null },
            "properties": []
        }));

        let c = d.as_class().expect("class");
        assert!(c.own_properties().is_empty());
    }

    /// accordproject/concerto-rust#244: TS compares `this.ast.identified.$class`
    /// to the metamodel's own full FQN (`concerto.metamodel@1.0.0.IdentifiedBy`)
    /// with strict `===` (`classdeclaration.ts`), never merely the short name
    /// after the last `.`. A class whose `identified.$class` is some other
    /// namespace's `IdentifiedBy` — for example `foo.IdentifiedBy` — must NOT
    /// take the explicit-identifier branch: TS's strict comparison fails, so
    /// it falls to the `else` branch instead, exactly as if `$class` held any
    /// other unrelated string (system-identified, `idField = '$identifier'`,
    /// `addIdentifierField()` runs). Before this fix, matching by short name
    /// alone (`short_name(class) == "IdentifiedBy"`) wrongly took the
    /// explicit branch here, using the field named `email` as the identifier
    /// instead of adding the system `$identifier` field.
    #[test]
    fn an_identified_class_field_from_a_foreign_namespace_is_not_matched_as_identified_by() {
        let d = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Person",
            "identified": { "$class": "foo.IdentifiedBy", "name": "email" },
            "properties": []
        }));

        let c = d.as_class().expect("class");
        assert!(c.is_identified());
        assert!(
            !c.is_explicitly_identified(),
            "a foreign-namespace $class ending in IdentifiedBy must be system-identified, not explicit"
        );
        assert_eq!(c.own_identifier_field_name(), Some("$identifier"));
        assert!(
            c.own_properties().iter().any(|p| p.name() == "$identifier"),
            "the system $identifier field must be added"
        );
    }

    /// accordproject/concerto-rust#217 review finding 2 ("only half fixed"):
    /// `identified.name` values that are falsy but not nullish — `0`,
    /// `false`, `""` — must load with no id field at all too, exactly like
    /// an explicit `null` ([`an_explicit_null_identified_name_loads_with_no_id_field`]).
    /// TS's `this.idField = this.ast.identified.name` is a plain assignment,
    /// taken exactly as given, and every downstream read of it —
    /// `if (this.idField)` — is a plain truthiness check: `0`/`false`/`""`
    /// are all falsy there, so none of them ever become a property name TS
    /// goes on to look up. Before this fix, `from_json` only special-cased
    /// an explicit `null`, so these three loaded with the field name kept
    /// verbatim ("0"/"false"/"") and then failed `check_identifier` with
    /// "does not contain this property" — a model TS loads with no error at
    /// all.
    #[test]
    fn a_falsy_non_nullish_identified_name_loads_with_no_id_field() {
        for name in [
            serde_json::json!(0),
            serde_json::json!(false),
            serde_json::json!(""),
        ] {
            let d = decl(serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Person",
                "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": name },
                "properties": []
            }));

            let c = d.as_class().expect("class");
            assert!(
                !c.is_identified(),
                "identified.name {name:?} -> is_identified"
            );
        }
    }

    #[test]
    fn asset_kind_is_tagged() {
        let d = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.AssetDeclaration",
            "name": "Vehicle",
            "isAbstract": false,
            "properties": []
        }));
        assert_eq!(d.declaration_kind(), "AssetDeclaration");
        assert_eq!(d.as_class().unwrap().kind(), ClassKind::Asset);
    }

    #[test]
    fn parses_enum_and_scalar_and_map() {
        let e = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.EnumDeclaration",
            "name": "Color",
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "RED" },
                { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "GREEN" }
            ]
        }));
        assert!(e.is_enum_declaration());
        assert!(!e.is_class_declaration());
        assert_eq!(e.name(), "Color");

        let s = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringScalar",
            "name": "Email",
            "validator": { "$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": ".*", "flags": "" }
        }));
        assert_eq!(s.as_scalar().unwrap().scalar_type(), "String");
        assert_eq!(s.name(), "Email");
        assert!(s.is_scalar_declaration());

        let m = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MapDeclaration",
            "name": "Dictionary",
            "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
            "value": { "$class": "concerto.metamodel@1.0.0.StringMapValueType" }
        }));
        assert!(m.is_map_declaration());
        assert_eq!(m.name(), "Dictionary");
    }

    #[test]
    fn unknown_declaration_kind_errors() {
        assert!(
            Declaration::try_from(&serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.WidgetDeclaration",
                "name": "X"
            }))
            .is_err()
        );
    }

    #[test]
    fn missing_class_is_rejected() {
        // TS `fromAst`'s `default` case, `thing.$class` interpolated as
        // `undefined` (P2-08 review: this used to be a pre-port message).
        let err = Declaration::try_from(&serde_json::json!({ "name": "X" }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "Unrecognised model element \"undefined\"."
        );
    }

    #[test]
    fn non_array_properties_is_rejected() {
        assert!(
            Declaration::try_from(&serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Bad",
                "properties": { "not": "an array" }
            }))
            .is_err()
        );
    }

    #[test]
    fn a_declaration_name_must_be_an_identifier() {
        for kind in ["ConceptDeclaration", "EnumDeclaration"] {
            let err = Declaration::try_from(&serde_json::json!({
                "$class": format!("concerto.metamodel@1.0.0.{kind}"),
                "name": "1Bad", "isAbstract": false, "properties": []
            }));
            assert_eq!(
                err.unwrap_err().to_string(),
                "Invalid class name '1Bad'",
                "{kind} with a bad name should be rejected"
            );
        }
    }

    /// TS `ModelFile.fromAst` matches the full metamodel `$class` strings
    /// and the six scalar kinds exactly; anything else — a bare short name,
    /// another namespace's, an unknown `*Scalar`, a missing `$class` — is
    /// "Unrecognised model element", ahead of the name check, naming the
    /// file and no location (P2-08 review).
    #[test]
    fn only_the_exact_metamodel_classes_are_recognised() {
        let cases = [
            (
                serde_json::json!("ConceptDeclaration"),
                "ConceptDeclaration",
            ),
            (
                serde_json::json!("other.ns@1.0.0.ConceptDeclaration"),
                "other.ns@1.0.0.ConceptDeclaration",
            ),
            (
                serde_json::json!("concerto.metamodel@1.0.0.FooScalar"),
                "concerto.metamodel@1.0.0.FooScalar",
            ),
            (serde_json::Value::Null, "null"),
        ];
        for (class, shown) in cases {
            for name in ["Good", "1bad"] {
                let err = Declaration::from_model_json(
                    &serde_json::json!({
                        "$class": class, "name": name, "isAbstract": false, "properties": [],
                        "location": {
                            "$class": "concerto.metamodel@1.0.0.Range",
                            "start": { "$class": "concerto.metamodel@1.0.0.Position", "offset": 0, "line": 1, "column": 1 },
                            "end": { "$class": "concerto.metamodel@1.0.0.Position", "offset": 5, "line": 1, "column": 6 }
                        }
                    }),
                    "org.acme@1.0.0",
                    Some("x.cto"),
                )
                .unwrap_err();
                let Some(err) = err.ported().cloned() else {
                    panic!("expected a contract error, got {err:?}");
                };
                assert_eq!(err.kind, ErrorKind::IllegalModel);
                assert_eq!(err.location, None);
                assert_eq!(
                    err.final_message(),
                    format!("Unrecognised model element \"{shown}\". File 'x.cto': ")
                );
            }
        }
        let err = Declaration::from_model_json(
            &serde_json::json!({ "name": "A" }),
            "org.acme@1.0.0",
            None,
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "Unrecognised model element \"undefined\".");
    }

    /// TS `Declaration.process` checks the name before
    /// `ClassDeclaration.process` looks at the fields, so a bad name wins
    /// over a system property name (P2-08 review), and the error names the
    /// file.
    #[test]
    fn an_invalid_class_name_is_reported_before_a_system_field_name() {
        let err = Declaration::from_model_json(
            &serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "1bad", "isAbstract": false,
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "$class",
                      "isArray": false, "isOptional": false }
                ]
            }),
            "org.acme@1.0.0",
            Some("x.cto"),
        )
        .unwrap_err();
        let Some(err) = err.ported().cloned() else {
            panic!("expected a contract error, got {err:?}");
        };
        assert_eq!(
            err.final_message(),
            "Invalid class name '1bad' File 'x.cto': "
        );
    }

    #[test]
    fn scalar_reports_its_concrete_kind() {
        let s = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "Email"
        }));
        assert_eq!(s.declaration_kind(), "StringScalar");
    }

    #[test]
    fn scalar_with_reversed_range_is_rejected() {
        let err = Declaration::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.IntegerScalar",
            "name": "Score",
            "validator": {
                "$class": "concerto.metamodel@1.0.0.IntegerDomainValidator",
                "lower": 10, "upper": 5
            }
        }));
        assert!(err.unwrap_err().to_string().contains("Lower bound"));
    }

    #[test]
    fn scalar_with_valid_range_is_accepted() {
        assert!(
            Declaration::try_from(&serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.IntegerScalar",
                "name": "Score",
                "validator": {
                    "$class": "concerto.metamodel@1.0.0.IntegerDomainValidator",
                    "lower": 0, "upper": 10
                }
            }))
            .is_ok()
        );
    }

    /// The code of a `modelfile-load-unreadable` error (P5-61): a node the
    /// typed read cannot read, an `IllegalModelException`.
    fn unreadable(err: Error) -> String {
        let Some(contract) = err.ported() else {
            panic!("expected a contract error, got {err:?}");
        };
        assert_eq!(contract.kind, ErrorKind::IllegalModel, "{err}");
        assert_eq!(contract.code, "modelfile-load-unreadable", "{err}");
        err.to_string()
    }

    /// P5-61: a class declaration's `properties` must be an array of
    /// property nodes whose `$class` is a full metamodel property class
    /// (BC-19's shape check rejects anything else first). A malformed one
    /// is a `modelfile-load-unreadable` error, not TS 5.0.0's per-site
    /// guards (`classdeclaration-validate-undefined-properties`,
    /// `classdeclaration-process-unrecmodelelem`).
    #[test]
    fn a_malformed_properties_value_is_an_unreadable_ast() {
        let class = |properties: Option<serde_json::Value>| {
            let mut node = serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Bad",
                "isAbstract": false
            });
            if let Some(properties) = properties {
                node["properties"] = properties;
            }
            node
        };
        let property = |class: &str| {
            serde_json::json!([
                { "$class": class, "name": "firstName", "isArray": false, "isOptional": false }
            ])
        };
        for node in [
            class(None),
            class(Some(serde_json::json!({ "not": "an array" }))),
            class(Some(property("StringProperty"))),
            class(Some(property(
                "concerto.metamodel@1.0.0.StringPropertyconcerto.metamodel@1.0.0.StringProperty",
            ))),
            class(Some(serde_json::json!([null]))),
        ] {
            let err =
                Declaration::from_model_json(&node, "org.acme@1.0.0", Some("x.cto")).unwrap_err();
            unreadable(err);
        }
    }

    /// P5-61: a map declaration is read strictly into the generated struct,
    /// so a key or value of a kind the metamodel does not declare, a
    /// missing key, value or name, or an object value without a well-formed
    /// `type` is a `modelfile-load-unreadable` error. TS 5.0.0's per-site
    /// guards for these shapes (`MapDeclaration must contain ...`, the
    /// `'in'` operator `TypeError`) are gone: BC-19's shape check rejects
    /// every one of them first.
    #[test]
    fn a_malformed_map_is_an_unreadable_ast() {
        let object_value = |ty: serde_json::Value| serde_json::json!({ "$class": "concerto.metamodel@1.0.0.ObjectMapValueType", "type": ty });
        let mut shapes = Vec::new();
        for class in [
            "StringMapKeyType",
            "foo.StringMapKeyType",
            "concerto.metamodel@1.0.0.IntegerMapKeyType",
        ] {
            shapes.push(map_to_nope(
                serde_json::json!({ "$class": class }),
                serde_json::json!({}),
            ));
        }
        for value in [
            serde_json::json!({ "$class": "StringMapValueType" }),
            serde_json::json!({ "$class": "concerto.metamodel@1.0.0.ObjectMapValueType" }),
            object_value(serde_json::Value::Null),
            object_value(serde_json::json!(true)),
            object_value(serde_json::json!([])),
            object_value(serde_json::json!({ "$class": null, "name": "Foo" })),
        ] {
            let mut node = map_to_nope(string_key(), serde_json::json!({}));
            node["value"] = value;
            shapes.push(node);
        }
        for key in ["key", "value", "name"] {
            let mut node = map_to_nope(string_key(), serde_json::json!({}));
            node.as_object_mut().unwrap().remove(key);
            shapes.push(node);
        }
        shapes.push(map_to_nope(string_key(), serde_json::json!({ "name": 5 })));
        shapes.push(map_to_nope(
            string_key(),
            serde_json::json!({ "decorators": "x" }),
        ));
        for node in shapes {
            let err = Declaration::try_from(&node).unwrap_err();
            unreadable(err);
        }
    }

    #[test]
    fn an_enum_property_in_a_class_declaration_is_kept() {
        let d = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "C",
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "s" }
            ]
        }));
        let c = d.as_class().unwrap();
        assert_eq!(c.own_properties().len(), 1);
        assert!(c.own_properties()[0].is_enum_value());
    }

    /// TS `ModelFile.fromAst` matches the fully-qualified `$class`, so a
    /// short scalar `$class` is an unrecognised model element (P2-08 review:
    /// the pre-port loader used to accept it).
    #[test]
    fn a_scalar_class_given_as_the_short_name_is_unrecognised() {
        let err = Declaration::try_from(&serde_json::json!({
            "$class": "StringScalar",
            "name": "Email"
        }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "Unrecognised model element \"StringScalar\"."
        );
    }

    #[test]
    fn unknown_scalar_kind_errors() {
        let err = Declaration::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MysteryScalar",
            "name": "X"
        }));
        // TS `fromAst` lists the six scalar kinds exactly (P2-08 review).
        assert_eq!(
            err.unwrap_err().to_string(),
            "Unrecognised model element \"concerto.metamodel@1.0.0.MysteryScalar\"."
        );
    }

    /// A map declaration with the given key, and an object value naming `Nope`.
    fn map_to_nope(key: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
        let mut map = serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MapDeclaration",
            "name": "M",
            "key": key,
            "value": {
                "$class": "concerto.metamodel@1.0.0.ObjectMapValueType",
                "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Nope" }
            }
        });
        for (field, value) in extra.as_object().unwrap() {
            map[field] = value.clone();
        }
        map
    }

    fn string_key() -> serde_json::Value {
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.StringMapKeyType" })
    }

    #[test]
    fn a_well_formed_map_is_typed() {
        let d = decl(map_to_nope(string_key(), serde_json::json!({})));
        let map = d.as_map().unwrap();
        assert_eq!(map.key_kind(), "StringMapKeyType");
        assert_eq!(map.value_kind(), "ObjectMapValueType");
        assert_eq!(map.value_type().map(|t| t.name.as_str()), Some("Nope"));
    }

    /// A `MapDeclaration` with the given key and value nodes.
    fn map_with(key: serde_json::Value, value: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MapDeclaration",
            "name": "MapPermutation1",
            "key": key,
            "value": value,
        })
    }

    fn kind(short: &str) -> serde_json::Value {
        serde_json::json!({ "$class": format!("concerto.metamodel@1.0.0.{short}") })
    }

    fn object_kind(short: &str, type_name: &str) -> serde_json::Value {
        serde_json::json!({
            "$class": format!("concerto.metamodel@1.0.0.{short}"),
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": type_name },
        })
    }

    // TS: MapDeclaration test/introspect/mapdeclaration.js `#getKey` "should
    // return the correct Type when called".
    #[test]
    fn key_type_name_is_string_for_a_string_key() {
        let d = decl(map_with(
            kind("StringMapKeyType"),
            kind("StringMapValueType"),
        ));
        assert_eq!(d.as_map().unwrap().key_type_name(), "String");
    }

    #[test]
    fn key_type_name_is_datetime_for_a_datetime_key() {
        let d = decl(map_with(
            kind("DateTimeMapKeyType"),
            kind("StringMapValueType"),
        ));
        assert_eq!(d.as_map().unwrap().key_type_name(), "DateTime");
    }

    // TS: "should return the correct Type when called - Scalar String/DateTime":
    // an object key's type is the raw referenced name, unresolved.
    #[test]
    fn key_type_name_is_the_raw_referenced_name_for_an_object_key() {
        let d = decl(map_with(
            object_kind("ObjectMapKeyType", "GUID"),
            kind("StringMapValueType"),
        ));
        assert_eq!(d.as_map().unwrap().key_type_name(), "GUID");
    }

    // TS: MapDeclaration test/introspect/mapdeclaration.js `#getValue` "should
    // return the correct Type when called", one case per primitive value kind.
    #[test]
    fn value_type_name_covers_every_primitive_kind() {
        let cases = [
            ("BooleanMapValueType", "Boolean"),
            ("DateTimeMapValueType", "DateTime"),
            ("StringMapValueType", "String"),
            ("IntegerMapValueType", "Integer"),
            ("LongMapValueType", "Long"),
            ("DoubleMapValueType", "Double"),
        ];
        for (mm_kind, expected) in cases {
            let d = decl(map_with(kind("StringMapKeyType"), kind(mm_kind)));
            assert_eq!(
                d.as_map().unwrap().value_type_name(),
                expected,
                "{mm_kind} should report {expected}"
            );
        }
    }

    // TS: "should return the correct values when called - Scalar
    // String/DateTime", and the relationship value case: an object or
    // relationship value's type is the raw referenced name, unresolved.
    #[test]
    fn value_type_name_is_the_raw_referenced_name_for_an_object_or_relationship_value() {
        let d = decl(map_with(
            kind("StringMapKeyType"),
            object_kind("ObjectMapValueType", "GUID"),
        ));
        assert_eq!(d.as_map().unwrap().value_type_name(), "GUID");

        let d = decl(map_with(
            kind("StringMapKeyType"),
            object_kind("RelationshipMapValueType", "Person"),
        ));
        assert_eq!(d.as_map().unwrap().value_type_name(), "Person");
    }

    // TS: `#toString` "should give the correct value for Map Declaration".
    #[test]
    fn to_string_matches_ts() {
        assert_eq!(
            MapDeclaration::to_string("com.acme@1.0.0.Dictionary"),
            "MapDeclaration {id=com.acme@1.0.0.Dictionary}"
        );
    }

    // TS: `#Introspect` "should return the correct value on introspection".
    #[test]
    fn declaration_kind_and_is_map_declaration_agree_with_ts() {
        let d = decl(map_with(
            kind("StringMapKeyType"),
            kind("StringMapValueType"),
        ));
        assert_eq!(d.declaration_kind(), "MapDeclaration");
        assert!(d.is_map_declaration());
        assert!(!d.is_class_declaration());
        assert!(!d.is_enum_declaration());
        assert!(!d.is_scalar_declaration());
    }

    /// #152, closing the "map key/value decorators aren't read" gap: TS
    /// `MapKeyType`/`MapValueType.process` (mapkeytype.ts, mapvaluetype.ts)
    /// each run `Decorated.process()` on their own AST node, independently
    /// of the map's own decorators.
    #[test]
    fn key_and_value_decorators_are_read_independently_of_the_map_s_own() {
        let mut key = kind("StringMapKeyType");
        key["decorators"] = serde_json::json!([
            { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "onKey", "arguments": [] }
        ]);
        let mut value = kind("StringMapValueType");
        value["decorators"] = serde_json::json!([
            { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "onValue1", "arguments": [] },
            { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "onValue2", "arguments": [] }
        ]);
        let mut map = map_with(key, value);
        map["decorators"] = serde_json::json!([
            { "$class": "concerto.metamodel@1.0.0.Decorator", "name": "onMap", "arguments": [] }
        ]);
        let decl = decl(map);
        let d = decl.as_map().unwrap();

        assert_eq!(
            d.key_decorators()
                .iter()
                .map(Decorator::name)
                .collect::<Vec<_>>(),
            vec!["onKey"]
        );
        assert_eq!(
            d.value_decorators()
                .iter()
                .map(Decorator::name)
                .collect::<Vec<_>>(),
            vec!["onValue1", "onValue2"]
        );
        assert_eq!(
            crate::introspect::Decorated::decorators(d)
                .iter()
                .map(Decorator::name)
                .collect::<Vec<_>>(),
            vec!["onMap"]
        );
    }

    /// When a caller has an explicit super type value on hand (even a
    /// placeholder), `process_decision` returns it verbatim and never
    /// substitutes the implicit `'Concept'` default — the "has a `superType`
    /// node at all" signal is `Some(_)` itself, not this string's content.
    /// The real WASM binding (`classDeclarationProcess`) never reads this
    /// value for that branch: it threads the AST's own raw
    /// `superType.name` (which might be `undefined`, `null`, a number, …)
    /// straight through to its snapshot instead, exactly because a bare
    /// `Option<&str>` cannot represent every JSON shape a fuzzed AST can put
    /// there (accordproject/concerto-rust#217, #219) — telling `undefined`
    /// apart from an explicit `null` (both "no super type to resolve" in
    /// different ways: TS's `_resolveSuperType`/`getProperties` treat a
    /// `null` `this.superType` as nothing to resolve, but leave `undefined`
    /// to fail resolution and raise "Could not find super type undefined")
    /// is that caller's job, verified at the binding/smoke-check level, not
    /// this pure decision function's.
    #[test]
    fn an_explicit_super_type_is_returned_verbatim_not_defaulted() {
        let decision =
            ClassDeclaration::process_decision(Some("placeholder"), false, "C", None, None, "ns.C");
        assert_eq!(decision.super_type.as_deref(), Some("placeholder"));
    }

    /// The AST naming no `superType` node at all takes the implicit
    /// `'Concept'` default.
    #[test]
    fn an_absent_super_type_node_takes_the_implicit_default() {
        let decision = ClassDeclaration::process_decision(None, false, "C", None, None, "ns.C");
        assert_eq!(decision.super_type.as_deref(), Some("Concept"));
    }

    /// The system model's own `Concept` declaration is still the one
    /// exemption from the implicit default.
    #[test]
    fn the_system_concept_declaration_has_no_implicit_super_type() {
        let decision = ClassDeclaration::process_decision(
            None,
            true,
            "Concept",
            None,
            None,
            "concerto@1.0.0.Concept",
        );
        assert_eq!(decision.super_type, None);
    }
}
