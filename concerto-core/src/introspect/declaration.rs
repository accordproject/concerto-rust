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

use concerto_metamodel::Name;
use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

use crate::derive::{DeclarationKind, Named};
use crate::error::{ContractError, Error, ErrorKind, Result};
use crate::introspect::decorator::{
    Decorator, WithDecorators, parse_decorator_list, parse_decorators,
};
use crate::introspect::kept::{Kept, Location};
use crate::introspect::model_file::unreadable_ast;
use crate::introspect::property::Property;
use crate::introspect::scalar::{self, ScalarDeclaration};
use crate::introspect::typed_ast::{self, PropertyKept, TypedDeclaration, TypedProperties};
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
    /// Applies [`identity`] to the node's `identified`.
    pub(crate) fn normalize_identified(&mut self) {
        class_field!(self, d => d.identified = identity(d.identified.take()));
    }

    /// The node's `name` and `superType`, for BC-19's shape check
    /// ([`crate::introspect::shape`]).
    pub(crate) fn name_and_super_type(&self) -> (&str, Option<&mm::TypeIdentifier>) {
        class_field!(self, d => (d.name.as_str(), d.super_type.as_ref()))
    }

    /// Whether the node's `identified` is the system identifier
    /// (`Identified`), for which the loader adds an `$identifier` property
    /// (`ClassDeclaration::finish`).
    pub(crate) fn is_system_identified(&self) -> bool {
        matches!(
            class_field!(self, d => d.identified.as_ref()),
            Some(mm::Identified::Identified)
        )
    }

    /// Sets the node's `identified`, which the typed read decodes apart
    /// from the node's own decode (`crate::introspect::typed_ast`, P5-76).
    pub(crate) fn set_identified(&mut self, identified: Option<mm::Identified>) {
        class_field!(self, d => d.identified = identified);
    }

    /// Sets the node's `decorators`, which the typed read decodes apart
    /// from the node's own decode (`crate::introspect::typed_ast`, P5-76).
    pub(crate) fn set_decorators(&mut self, decorators: Option<Vec<mm::Decorator>>) {
        class_field!(self, d => d.decorators = decorators);
    }

    /// Sets the node's `location`.
    pub(crate) fn set_location(&mut self, location: Option<mm::Range>) {
        class_field!(self, d => d.location = location);
    }
}

/// A class-like declaration's `identified`, as decoded strictly into the
/// generated struct, with the one rule `ClassDeclaration.process` applies to
/// a well-formed node: an `IdentifiedBy` gives `this.idField =
/// this.ast.identified.name`, read only by truthiness afterwards
/// (`if (this.idField)`), so an empty name is no identity.
///
/// Since P5-61 the value is a node or `null`: BC-19's shape check rejects
/// anything else (`modelfile-load-nodenotobject`), and with the check off
/// the strict decode does (accordproject/concerto-rust#393).
pub(crate) fn identity(identified: Option<mm::Identified>) -> Option<mm::Identified> {
    match identified {
        Some(mm::Identified::IdentifiedBy(by)) if by.name.is_empty() => None,
        other => other,
    }
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
    /// P5-93: one of [`implicit_super_types`]' nodes, shared, where each
    /// class used to build its own.
    implicit_super_type: Option<&'static mm::TypeIdentifier>,
    decorators: Vec<Decorator>,
}

/// The names of the system properties a class is given
/// (`ClassDeclaration::finish`), each read once and shared (P5-93).
static IDENTIFIER_NAME: std::sync::LazyLock<Name> =
    std::sync::LazyLock::new(|| Name::from("$identifier"));
static TIMESTAMP_NAME: std::sync::LazyLock<Name> =
    std::sync::LazyLock::new(|| Name::from("$timestamp"));

/// A shared system property name: a clone, which counts a reference.
fn system_name(name: &std::sync::LazyLock<Name>) -> Name {
    Name::clone(name)
}

/// A `TypeIdentifier`'s `$class`.
const TYPE_IDENTIFIER_CLASS: &str = "concerto.metamodel@1.0.0.TypeIdentifier";

/// The implicit super type nodes: `Concept`, `Asset`, `Participant`,
/// `Transaction` and `Event` (`ClassDeclaration::finish`).
fn implicit_super_types() -> &'static [mm::TypeIdentifier; 5] {
    static NODES: std::sync::LazyLock<[mm::TypeIdentifier; 5]> = std::sync::LazyLock::new(|| {
        ["Concept", "Asset", "Participant", "Transaction", "Event"].map(|name| mm::TypeIdentifier {
            _class: TYPE_IDENTIFIER_CLASS.into(),
            name: name.into(),
            namespace: None,
            resolved_name: None,
        })
    });
    &NODES
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
        class_field!(&self.node, d => d.super_type.as_ref()).or(self.implicit_super_type)
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
            let index = match kind {
                ClassKind::Concept => 0,
                ClassKind::Asset => 1,
                ClassKind::Participant => 2,
                ClassKind::Transaction => 3,
                ClassKind::Event => 4,
            };
            Some(&implicit_super_types()[index])
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
                    name: system_name(&IDENTIFIER_NAME),
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
                    name: system_name(&TIMESTAMP_NAME),
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
        "BooleanScalar" => mm::ScalarDeclaration::BooleanScalar(
            typed_ast::strict_variant_from_value(v).map_err(bad)?,
        ),
        "IntegerScalar" => mm::ScalarDeclaration::IntegerScalar(
            typed_ast::strict_variant_from_value(v).map_err(bad)?,
        ),
        "LongScalar" => {
            mm::ScalarDeclaration::LongScalar(typed_ast::strict_variant_from_value(v).map_err(bad)?)
        }
        "DoubleScalar" => mm::ScalarDeclaration::DoubleScalar(
            typed_ast::strict_variant_from_value(v).map_err(bad)?,
        ),
        "StringScalar" => mm::ScalarDeclaration::StringScalar(
            typed_ast::strict_variant_from_value(v).map_err(bad)?,
        ),
        _ => mm::ScalarDeclaration::DateTimeScalar(
            typed_ast::strict_variant_from_value(v).map_err(bad)?,
        ),
    };
    let name = scalar::node_name(&node);
    check_declaration_name(name, || value.get("location").cloned(), file_name)?;
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
            _class: TYPE_IDENTIFIER_CLASS.into(),
            name: "Concept".into(),
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
        let node: mm::MapDeclaration = typed_ast::strict_variant_from_value(value)
            .map_err(|e| unreadable_ast(&e, file_name))?;
        Ok(Self {
            node,
            decorators: parse_decorators(value),
            key_decorators: value.get("key").map(parse_decorators).unwrap_or_default(),
            value_decorators: value.get("value").map(parse_decorators).unwrap_or_default(),
        })
    }

    /// [`MapDeclaration::from_json`], for the node as the typed read keeps
    /// it (a [`Kept`], P5-93): the same declaration as from its `Value`.
    fn from_kept(value: &Kept, file_name: Option<&str>) -> Result<Self> {
        let node: mm::MapDeclaration = value
            .strict_variant_decode()
            .map_err(|e| unreadable_ast(&e, file_name))?;
        let decorators_of = |node: Option<&Kept>| {
            node.map(|node| parse_decorator_list(node.get("decorators")))
                .unwrap_or_default()
        };
        Ok(Self {
            node,
            decorators: decorators_of(Some(value)),
            key_decorators: decorators_of(value.get("key")),
            value_decorators: decorators_of(value.get("value")),
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
                check_declaration_name(map.name(), || value.get("location").cloned(), file_name)?;
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
            // As `from_model_json` loads a map declaration's `Value`.
            TypedDeclaration::Map(node) => {
                let map = MapDeclaration::from_kept(&node, file_name)?;
                check_declaration_name(
                    map.name(),
                    || node.get("location").map(Kept::to_value),
                    file_name,
                )?;
                Ok(Self::Map(map))
            }
            TypedDeclaration::Class {
                kind,
                node,
                properties,
                decorators,
                location,
                ..
            } => {
                check_declaration_name(
                    class_field!(&node, d => &d.name),
                    || location.as_ref().map(Location::to_value),
                    file_name,
                )?;
                check_property_names(&properties, location.as_ref())
                    .and_then(|()| {
                        ClassDeclaration::finish(
                            kind,
                            node,
                            properties.properties,
                            decorators.map(|read| read.list).unwrap_or_default(),
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
                check_declaration_name(
                    &node.name,
                    || location.as_ref().map(Location::to_value),
                    file_name,
                )?;
                check_property_names(&values, location.as_ref())
                    .map_err(|e| with_model_file(e, file_name))?;
                Ok(Self::Enum(EnumDeclaration {
                    inner: WithDecorators::new(
                        node,
                        decorators.map(|read| read.list).unwrap_or_default(),
                    ),
                    values: values.properties,
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
    properties: &TypedProperties,
    declaration: Option<&Location>,
) -> Result<()> {
    for (property, PropertyKept { location, .. }) in properties.iter() {
        let name = property.name();
        if is_system_property(name) {
            // The model file's name is filled in by the caller
            // (`with_model_file`).
            return Err(ContractError::pre_port(
                ErrorKind::IllegalModel,
                format!("Invalid field name '{name}'"),
                declaration.map(Location::to_value),
            )
            .into());
        }
        if !is_valid_identifier(name) {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "property-process-invalidname",
                vec![("name", name.to_string())],
            );
            err.location = location.as_ref().map(Location::to_value);
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
    location: impl FnOnce() -> Option<serde_json::Value>,
    file_name: Option<&str>,
) -> Result<()> {
    if is_valid_identifier(name) {
        return Ok(());
    }
    let mut err = ContractError::pre_port(
        ErrorKind::IllegalModel,
        format!("Invalid class name '{name}'"),
        location(),
    );
    err.model_file = Some(file_name.map(str::to_string));
    Err(err.into())
}

#[cfg(test)]
mod tests;
