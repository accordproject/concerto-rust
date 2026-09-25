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
use serde::de::Error as _;

use crate::derive::{DeclarationKind, Named};
use crate::error::{ConcertoError, ContractError, ErrorKind, Result};
use crate::introspect::decorator::{Decorator, WithDecorators, parse_decorators};
use crate::introspect::property::Property;
use crate::introspect::scalar::{self, ScalarDeclaration};
use crate::introspect::{
    DeclarationKind, HasValidators, Named, Typed, declared_class, qualified_class,
};
use crate::model_util::{
    MAP_KEY_KINDS, MAP_VALUE_KINDS, get_fully_qualified_name, get_short_name, is_valid_identifier,
};

/// Which class-like declaration a [`ClassDeclaration`] represents. Its
/// [`DeclarationKind`] is the metamodel `$class` short name for the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, DeclarationKind)]
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
    fn from_short(short: &str) -> Option<Self> {
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
enum ClassNode {
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
/// - the `$identifier`/`$timestamp` system fields: [`ClassDeclaration::from_json`]
///   appends them to `properties` the same way `addIdentifierField`/
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
    /// A subtype's *inherited* identity is [`ModelManager::identifier_field_name`]
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

    /// TS: `ClassDeclaration.isAsset`/`isParticipant`/`isTransaction`/
    /// `isEvent`/`isConcept`/`isEnum`/`isMapDeclaration`
    /// (src/introspect/classdeclaration.ts): each compares `this.type` (the
    /// AST's own `$class`, already set by `process()`) against one metamodel
    /// `$class`'s short name. `ast_class` is the receiver's `this.type`;
    /// `want` is the metamodel short name to compare against
    /// (`"AssetDeclaration"`, …).
    pub fn is_kind(ast_class: &str, want: &str) -> bool {
        get_short_name(ast_class) == want
    }

    /// The `superType`/`idField` decision `ClassDeclaration.process` makes
    /// before its `ast.properties` loop (src/introspect/classdeclaration.ts;
    /// the loop itself builds `Field`/`RelationshipDeclaration`/
    /// `EnumValueDeclaration` views, kept in TS). `explicit_super_type` is
    /// `this.ast.superType.name`, when the AST names one. `identified_class`
    /// is `this.ast.identified.$class`; `identified_name` is
    /// `this.ast.identified.name` (only meaningful for an explicit
    /// `IdentifiedBy`). `fqn` is `this.fqn`, read once `this.name` and
    /// `this.modelFile` are set (`Declaration.process` runs first).
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
            Some(class) if get_short_name(class) == "IdentifiedBy" => {
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

    /// `process_decision`'s own exemption test: unlike [`Self::is_system_concept`]
    /// (which also needs the namespace, not available to the binding at this
    /// point), the caller already knows whether its model file is the system
    /// model file.
    fn is_system_concept_file(is_system_model_file: bool, name: &str) -> bool {
        is_system_model_file && name == "Concept"
    }

    /// Reads the declaration fields into the generated struct for `kind`,
    /// then each property into a [`Property`]. The properties are read from
    /// the node itself, so the generated struct is given an empty list, then
    /// the `$identifier`/`$timestamp` system fields are appended exactly as
    /// `ClassDeclaration.process`'s `addIdentifierField`/`addTimestampField`
    /// append them in TS: after the AST's own properties, bypassing the
    /// per-property `isSystemProperty` guard that rejects a `$`-prefixed name
    /// from the AST itself. `namespace` is the namespace of the model file
    /// this declaration is being loaded into, needed for the implicit
    /// `Concept` super type and to recognise the system model's own
    /// `Transaction`/`Event` (below).
    fn from_json(kind: ClassKind, value: &serde_json::Value, namespace: &str) -> Result<Self> {
        let mut fields = value.clone();
        if let Some(object) = fields.as_object_mut() {
            object.insert("properties".into(), serde_json::Value::Array(Vec::new()));
        }
        let bad = |e: serde_json::Error| ConcertoError::IllegalModel {
            message: format!("invalid {}: {e}", kind.declaration_kind()),
            file_name: None,
            location: None,
        };
        let node = match kind {
            ClassKind::Concept => ClassNode::Concept(serde_json::from_value(fields).map_err(bad)?),
            ClassKind::Asset => ClassNode::Asset(serde_json::from_value(fields).map_err(bad)?),
            ClassKind::Participant => {
                ClassNode::Participant(serde_json::from_value(fields).map_err(bad)?)
            }
            ClassKind::Transaction => {
                ClassNode::Transaction(serde_json::from_value(fields).map_err(bad)?)
            }
            ClassKind::Event => ClassNode::Event(serde_json::from_value(fields).map_err(bad)?),
        };
        let name = class_field!(&node, d => d.name.clone());

        let implicit_super_type = if class_field!(&node, d => d.super_type.is_some())
            || Self::is_system_concept(namespace, &name)
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

        let mut properties = parse_properties(value)?;

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
        let fqn = get_fully_qualified_name(namespace, &name);
        for property in &properties {
            property.check_bound_validators(&fqn)?;
        }

        Ok(Self {
            node,
            properties,
            implicit_super_type,
            decorators: parse_decorators(value),
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

/// Loads a scalar declaration: the generated node for its `$class` (the
/// loader's structural check), then the ported `ScalarDeclaration.process`.
/// The name is checked first, as `Declaration.process` runs before it in TS.
fn load_scalar(
    short: &str,
    value: &serde_json::Value,
    namespace: &str,
    file_name: Option<&str>,
) -> Result<ScalarDeclaration> {
    let bad = |e: serde_json::Error| ConcertoError::IllegalModel {
        message: format!("invalid {short}: {e}"),
        file_name: None,
        location: None,
    };
    let v = value.clone();
    let node = match short {
        "BooleanScalar" => {
            mm::ScalarDeclaration::BooleanScalar(serde_json::from_value(v).map_err(bad)?)
        }
        "IntegerScalar" => {
            mm::ScalarDeclaration::IntegerScalar(serde_json::from_value(v).map_err(bad)?)
        }
        "LongScalar" => mm::ScalarDeclaration::LongScalar(serde_json::from_value(v).map_err(bad)?),
        "DoubleScalar" => {
            mm::ScalarDeclaration::DoubleScalar(serde_json::from_value(v).map_err(bad)?)
        }
        "StringScalar" => {
            mm::ScalarDeclaration::StringScalar(serde_json::from_value(v).map_err(bad)?)
        }
        "DateTimeScalar" => {
            mm::ScalarDeclaration::DateTimeScalar(serde_json::from_value(v).map_err(bad)?)
        }
        other => {
            return Err(ConcertoError::IllegalModel {
                message: format!("unknown scalar type: {other}"),
                file_name: None,
                location: None,
            });
        }
    };
    // The name was already checked by `Declaration::from_model_json`
    // (`check_declaration_name`), TS `Declaration.process`.
    let name = scalar::node_name(&node);
    let fqn = get_fully_qualified_name(namespace, name);
    let processed =
        ScalarDeclaration::process(value, file_name, &|| Ok::<_, ConcertoError>(fqn.clone()))?;
    let scalar = ScalarDeclaration::new(node, processed, parse_decorators(value));
    scalar.check_validators()?;
    Ok(scalar)
}

/// A top-level declaration within a model file.
#[derive(Debug, Clone, Named, DeclarationKind)]
#[concerto(delegate)]
#[allow(clippy::large_enum_variant)]
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

impl crate::introspect::Decorated for EnumDeclaration {
    fn get_decorators(&self) -> &[Decorator] {
        self.inner.decorators()
    }
}

impl EnumDeclaration {
    fn from_json(value: &serde_json::Value) -> Result<Self> {
        Ok(Self {
            inner: WithDecorators::new(
                serde_json::from_value(value.clone()).map_err(|e| ConcertoError::IllegalModel {
                    message: format!("invalid EnumDeclaration: {e}"),
                    file_name: None,
                    location: None,
                })?,
                parse_decorators(value),
            ),
            values: parse_properties(value)?,
        })
    }

    /// The enum's values, each carrying its own processed decorators.
    pub fn values(&self) -> &[Property] {
        &self.values
    }

    /// The string representation TS's `EnumDeclaration.toString`
    /// (src/introspect/enumdeclaration.ts) builds: `'EnumDeclaration {id=' +
    /// this.getFullyQualifiedName() + '}'`, an override of
    /// [`ClassDeclaration::to_string`] with no super type or abstract flag.
    pub fn to_string(fqn: &str) -> String {
        format!("EnumDeclaration {{id={fqn}}}")
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

/// A map declaration.
///
/// A map is read for its name, and for the kind (the `$class` short name) and
/// the referenced `type`, if any, of its key and of its value, plus its own
/// processed decorators (`Decorated.getDecorators()` is faithful for a map).
/// The map's own location, and the location and decorators of its key and
/// value, are not read.
///
/// [`MapVariant::Typed`] is a newtype over the generated
/// [`mm::MapDeclaration`]. It is used whenever that struct can hold all four
/// key/value facts; a malformed `decorators` or `location` on the key or
/// value node is left out of it, because neither is read.
///
/// [`MapVariant::Untyped`] keeps the four facts as read from the node. It
/// is used only when the key or the value (or both) is something
/// `mm::MapKeyType` or `mm::MapValueType` cannot represent:
///
/// - it is missing, or its kind is not one the metamodel declares (for
///   example `IntegerMapKeyType`);
/// - its kind carries a `type`, but the `type` is missing or is not a
///   `TypeIdentifier`;
/// - its kind carries no `type`, but the node has a well-formed `type` anyway.
///
/// Either way, a key or value kind the specification does not allow reaches
/// semantic validation, which reports it, and a referenced type is checked
/// there whichever variant holds it.
#[derive(Debug, Clone, Named)]
#[allow(clippy::large_enum_variant)]
enum MapVariant {
    /// The key and value are both representable by the generated union types.
    Typed(mm::MapDeclaration),
    /// The key or value is not representable by the generated union types.
    Untyped {
        /// The map's short name.
        name: String,
        /// The `$class` short name of the key node, or `""` if there is none.
        key_kind: String,
        /// The type the key node refers to, if it has a well-formed `type`.
        key_type: Option<mm::TypeIdentifier>,
        /// The `$class` short name of the value node, or `""` if there is none.
        value_kind: String,
        /// The type the value node refers to, if it has a well-formed `type`.
        value_type: Option<mm::TypeIdentifier>,
    },
}

/// A map declaration: the [`MapVariant`] read from its key and value nodes,
/// plus its processed decorators (module doc on
/// [`crate::introspect::decorator::WithDecorators`]; kept as a plain field
/// here rather than that wrapper, since a map is not a newtype over one
/// generated node — `MapVariant::Untyped` is not a generated node at all).
///
/// TS `MapDeclaration.getDecorators()` reads the same real decorators as any
/// other declaration; nothing about the key/value fallback above extends to
/// them.
#[derive(Debug, Clone)]
pub struct MapDeclaration {
    variant: MapVariant,
    decorators: Vec<Decorator>,
}

impl Named for MapDeclaration {
    fn name(&self) -> &str {
        self.variant.name()
    }
}

impl DeclarationKind for MapDeclaration {
    fn declaration_kind(&self) -> &'static str {
        "MapDeclaration"
    }
}

impl crate::introspect::Decorated for MapDeclaration {
    fn get_decorators(&self) -> &[Decorator] {
        &self.decorators
    }
}

impl MapDeclaration {
    /// The metamodel `$class` short name of the key node, such as
    /// `StringMapKeyType`.
    pub fn key_kind(&self) -> &str {
        match &self.variant {
            MapVariant::Typed(m) => match &m.key {
                mm::MapKeyType::StringMapKeyType(_) => "StringMapKeyType",
                mm::MapKeyType::DateTimeMapKeyType(_) => "DateTimeMapKeyType",
                mm::MapKeyType::ObjectMapKeyType(_) => "ObjectMapKeyType",
            },
            MapVariant::Untyped { key_kind, .. } => key_kind,
        }
    }

    /// The metamodel `$class` short name of the value node, such as
    /// `ObjectMapValueType`.
    pub fn value_kind(&self) -> &str {
        match &self.variant {
            MapVariant::Typed(m) => match &m.value {
                mm::MapValueType::BooleanMapValueType(_) => "BooleanMapValueType",
                mm::MapValueType::DateTimeMapValueType(_) => "DateTimeMapValueType",
                mm::MapValueType::StringMapValueType(_) => "StringMapValueType",
                mm::MapValueType::IntegerMapValueType(_) => "IntegerMapValueType",
                mm::MapValueType::LongMapValueType(_) => "LongMapValueType",
                mm::MapValueType::DoubleMapValueType(_) => "DoubleMapValueType",
                mm::MapValueType::ObjectMapValueType(_) => "ObjectMapValueType",
                mm::MapValueType::RelationshipMapValueType(_) => "RelationshipMapValueType",
            },
            MapVariant::Untyped { value_kind, .. } => value_kind,
        }
    }

    /// The type the key refers to, for a key that is not a primitive.
    pub fn key_type(&self) -> Option<&mm::TypeIdentifier> {
        match &self.variant {
            MapVariant::Typed(m) => match &m.key {
                mm::MapKeyType::ObjectMapKeyType(k) => Some(&k.type_),
                _ => None,
            },
            MapVariant::Untyped { key_type, .. } => key_type.as_ref(),
        }
    }

    /// The type the value refers to, for a value that is not a primitive.
    pub fn value_type(&self) -> Option<&mm::TypeIdentifier> {
        match &self.variant {
            MapVariant::Typed(m) => match &m.value {
                mm::MapValueType::ObjectMapValueType(v) => Some(&v.type_),
                mm::MapValueType::RelationshipMapValueType(v) => Some(&v.type_),
                _ => None,
            },
            MapVariant::Untyped { value_type, .. } => value_type.as_ref(),
        }
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

    /// `MapDeclaration.toString` (src/introspect/mapdeclaration.ts):
    /// `MapDeclaration {id=<fully qualified name>}`.
    pub fn to_string(fully_qualified_name: &str) -> String {
        format!("MapDeclaration {{id={fully_qualified_name}}}")
    }

    /// Whether this map's key and value were both representable by the
    /// generated union types (used only by this module's own tests).
    #[cfg(test)]
    fn is_typed(&self) -> bool {
        matches!(self.variant, MapVariant::Typed(_))
    }

    fn from_json(value: &serde_json::Value) -> Result<Self> {
        let bad = |e: serde_json::Error| ConcertoError::IllegalModel {
            message: format!("invalid MapDeclaration: {e}"),
            file_name: None,
            location: None,
        };
        let name: String = match value.get("name") {
            Some(name) => serde_json::from_value(name.clone()).map_err(bad)?,
            None => return Err(bad(serde_json::Error::missing_field("name"))),
        };

        // TS `MapDeclaration.process` (src/introspect/mapdeclaration.ts): a
        // missing key or value node, an invalid `MapKeyType`, or an invalid
        // `MapValueType` is an `IllegalModelException` naming this map, not a
        // silent fallback to the untyped representation below.
        // Built through `ContractError::pre_port`, not the raw `IllegalModel`
        // variant, so [`crate::error::ContractError::final_message`] applies
        // the `IllegalModelException` constructor's own trailing
        // `" " + suffix` (PORTING.md 2.1) — empty here (no file, no
        // location), but still present, exactly as every TS
        // `IllegalModelException` message ends in a space.
        let illegal_model = |message: String| -> ConcertoError {
            ContractError::pre_port(ErrorKind::IllegalModel, message, None).into()
        };
        let key_node = value.get("key").filter(|v| !v.is_null());
        let value_node = value.get("value").filter(|v| !v.is_null());
        if key_node.is_none() || value_node.is_none() {
            return Err(illegal_model(format!(
                "MapDeclaration must contain Key & Value properties {name}"
            )));
        }

        let key_kind = node_kind(Some(key_node.unwrap()));
        let key_type = type_reference(Some(key_node.unwrap()));
        let value_kind = node_kind(Some(value_node.unwrap()));
        let value_type = type_reference(Some(value_node.unwrap()));
        let decorators = parse_decorators(value);

        if !MAP_KEY_KINDS.contains(&key_kind.as_str()) {
            return Err(illegal_model(format!(
                "MapDeclaration must contain valid MapKeyType  {name}"
            )));
        }
        if !MAP_VALUE_KINDS.contains(&value_kind.as_str()) {
            return Err(illegal_model(format!(
                "MapDeclaration must contain valid MapValueType, for MapDeclaration {name}"
            )));
        }

        // TS `MapValueType.processType` (src/introspect/mapvaluetype.ts): an
        // `ObjectMapValueType`/`RelationshipMapValueType` node must carry a
        // well-formed `type` (a `TypeIdentifier` with `$class` and `name`).
        if value_kind == "ObjectMapValueType" || value_kind == "RelationshipMapValueType" {
            let value_node = value_node.unwrap();
            let type_node = value_node.get("type").filter(|v| !v.is_null());
            let Some(type_node) = type_node else {
                return Err(illegal_model(format!(
                    "ObjectMapValueType must contain property 'type', for MapDeclaration named {name}"
                )));
            };
            let class_field = type_node.get("$class");
            let name_field = type_node.get("name");
            if class_field.is_none() || name_field.is_none() {
                return Err(illegal_model(format!(
                    "ObjectMapValueType type must contain property '$class' and property 'name', for MapDeclaration named {name}"
                )));
            }
            if class_field.and_then(|c| c.as_str())
                != Some("concerto.metamodel@1.0.0.TypeIdentifier")
            {
                return Err(illegal_model(format!(
                    "ObjectMapValueType type $class must be of TypeIdentifier for MapDeclaration named {name}"
                )));
            }
        }

        if let Some(variant) = typed_map(value, &key_kind, &value_kind).map(MapVariant::Typed) {
            let candidate = Self {
                variant,
                decorators: decorators.clone(),
            };
            if candidate.key_type().is_some() == key_type.is_some()
                && candidate.value_type().is_some() == value_type.is_some()
            {
                return Ok(candidate);
            }
        }
        Ok(Self {
            variant: MapVariant::Untyped {
                name,
                key_kind,
                key_type,
                value_kind,
                value_type,
            },
            decorators,
        })
    }
}

/// Deserializes a map node into the generated [`mm::MapDeclaration`], if its
/// key and value kinds are ones the generated unions declare. The key and
/// value `$class` are qualified from their short names, and a `decorators` or
/// `location` that does not deserialize is dropped, since neither is read.
fn typed_map(
    value: &serde_json::Value,
    key_kind: &str,
    value_kind: &str,
) -> Option<mm::MapDeclaration> {
    if !MAP_KEY_KINDS.contains(&key_kind) || !MAP_VALUE_KINDS.contains(&value_kind) {
        return None;
    }
    let mut node = value.clone();
    let map = node.as_object_mut()?;
    drop_unreadable_annotations(map);
    for (field, kind) in [("key", key_kind), ("value", value_kind)] {
        let part = map.get_mut(field)?.as_object_mut()?;
        part.insert(
            "$class".into(),
            serde_json::Value::String(qualified_class(kind)),
        );
        drop_unreadable_annotations(part);
    }
    serde_json::from_value(node).ok()
}

/// Removes a `decorators` or `location` entry that does not deserialize into
/// its generated type.
fn drop_unreadable_annotations(node: &mut serde_json::Map<String, serde_json::Value>) {
    if node
        .get("decorators")
        .is_some_and(|d| serde_json::from_value::<Option<Vec<mm::Decorator>>>(d.clone()).is_err())
    {
        node.remove("decorators");
    }
    if node
        .get("location")
        .is_some_and(|l| serde_json::from_value::<Option<mm::Range>>(l.clone()).is_err())
    {
        node.remove("location");
    }
}

/// The `$class` short name of a map key or value node.
fn node_kind(node: Option<&serde_json::Value>) -> String {
    node.map(|n| get_short_name(declared_class(n)).to_string())
        .unwrap_or_default()
}

/// The type a map key or value node points at. Primitive keys and values carry
/// no reference, so they give `None`.
fn type_reference(node: Option<&serde_json::Value>) -> Option<mm::TypeIdentifier> {
    serde_json::from_value(node?.get("type")?.clone()).ok()
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

fn parse_properties(value: &serde_json::Value) -> Result<Vec<Property>> {
    match value.get("properties") {
        None => Ok(Vec::new()),
        Some(serde_json::Value::Array(arr)) => arr
            .iter()
            .map(|property| {
                // TS: `ClassDeclaration.process`'s loop (classdeclaration.ts,
                // inherited by `EnumDeclaration`) rejects a system property
                // name before building that property, with the *declaration's*
                // `this.ast.location`, not the property's (P2-08: this used to
                // be left to `Property::try_from`, which has no declaration
                // location to give).
                if let Some(name) = property.get("name").and_then(serde_json::Value::as_str)
                    && crate::model_util::is_system_property(name)
                {
                    // The model file's name is filled in by
                    // `Declaration::from_model_json` (`with_model_file`).
                    return Err(ContractError::pre_port(
                        ErrorKind::IllegalModel,
                        format!("Invalid field name '{name}'"),
                        value.get("location").cloned(),
                    )
                    .into());
                }
                Property::try_from(property)
            })
            .collect(),
        Some(_) => Err(ConcertoError::IllegalModel {
            message: "'properties' must be an array".into(),
            file_name: None,
            location: None,
        }),
    }
}

impl TryFrom<&serde_json::Value> for Declaration {
    type Error = ConcertoError;

    /// Loads a declaration outside any namespace or file.
    fn try_from(value: &serde_json::Value) -> Result<Self> {
        Self::from_model_json(value, "", None)
    }
}

impl Declaration {
    /// Loads a declaration of the model file for `namespace`, named
    /// `file_name`: both are what the TS declaration reads from its model file
    /// when it reports an error.
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
        // constructed (P2-08 review: this used to match the short name
        // alone, and any `*Scalar`).
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

        // TS: `Declaration.process` (declaration.ts), reached through every
        // declaration kind's own `super.process()` before anything
        // kind-specific — so an invalid name is reported ahead of, say, a
        // system property name among a class's fields (P2-08 review).
        check_declaration_name(value, file_name)?;

        if let Some(class_kind) = ClassKind::from_short(kind) {
            return Ok(Self::Class(
                ClassDeclaration::from_json(class_kind, value, namespace)
                    .map_err(|e| with_model_file(e, file_name))?,
            ));
        }

        let declaration = match kind {
            "EnumDeclaration" => Self::Enum(
                EnumDeclaration::from_json(value).map_err(|e| with_model_file(e, file_name))?,
            ),
            "MapDeclaration" => Self::Map(MapDeclaration::from_json(value)?),
            scalar => Self::Scalar(load_scalar(scalar, value, namespace, file_name)?),
        };
        Ok(declaration)
    }
}

/// TS: `ClassDeclaration.process` passes `this.modelFile` to every
/// `IllegalModelException` it throws, so an `IllegalModel` contract error
/// raised while a class-like or enum declaration is built names the file
/// being loaded (`file_name`) unless it already names one.
fn with_model_file(err: ConcertoError, file_name: Option<&str>) -> ConcertoError {
    match err {
        ConcertoError::Contract(mut contract)
            if contract.kind == ErrorKind::IllegalModel && contract.model_file.is_none() =>
        {
            contract.model_file = Some(file_name.map(str::to_string));
            ConcertoError::Contract(contract)
        }
        other => other,
    }
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
/// fails (an absent name interpolates as `undefined`).
fn check_declaration_name(value: &serde_json::Value, file_name: Option<&str>) -> Result<()> {
    let name = value.get("name");
    if let Some(serde_json::Value::String(name)) = name
        && is_valid_identifier(name)
    {
        return Ok(());
    }
    let shown = name.map_or_else(|| "undefined".to_string(), crate::ecma::to_js_string);
    let mut err = ContractError::pre_port(
        ErrorKind::IllegalModel,
        format!("Invalid class name '{shown}'"),
        value.get("location").cloned(),
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
        assert_eq!(s.as_scalar().unwrap().scalar_type(), Some("String"));
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
                let ConcertoError::Contract(err) = err else {
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
        let ConcertoError::Contract(err) = err else {
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

    #[test]
    fn non_array_properties_is_reported_verbatim() {
        let err = Declaration::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Bad",
            "properties": { "not": "an array" }
        }));
        assert_eq!(
            err.unwrap_err().to_string(),
            "illegal model: 'properties' must be an array"
        );
    }

    #[test]
    fn a_class_declaration_with_no_properties_field_loads_with_none() {
        let d = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Empty"
        }));
        assert!(d.as_class().unwrap().own_properties().is_empty());
    }

    #[test]
    fn a_class_declaration_property_class_may_be_given_as_the_short_name() {
        let d = decl(serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "Person",
            "properties": [
                { "$class": "StringProperty", "name": "firstName", "isArray": false, "isOptional": false }
            ]
        }));
        let c = d.as_class().unwrap();
        assert_eq!(c.own_properties().len(), 1);
        assert_eq!(c.own_properties()[0].type_name(), Some("String"));
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

    #[test]
    fn a_malformed_class_header_is_reported_before_its_properties() {
        let err = Declaration::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": "C",
            "decorators": "x",
            "properties": [
                { "$class": "concerto.metamodel@1.0.0.MysteryProperty", "name": "s" }
            ]
        }));
        assert!(
            err.unwrap_err()
                .to_string()
                .starts_with("illegal model: invalid ConceptDeclaration: ")
        );
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
        assert!(map.is_typed());
        assert_eq!(map.key_kind(), "StringMapKeyType");
        assert_eq!(map.value_kind(), "ObjectMapValueType");
        assert_eq!(map.value_type().map(|t| t.name.as_str()), Some("Nope"));
    }

    #[test]
    fn a_map_with_malformed_decorators_or_location_is_typed_and_keeps_its_value_type() {
        for extra in [
            serde_json::json!({ "decorators": "x" }),
            serde_json::json!({ "location": 1 }),
        ] {
            let d = decl(map_to_nope(string_key(), extra.clone()));
            let map = d.as_map().unwrap();
            assert!(map.is_typed(), "{extra}");
            assert_eq!(map.value_type().map(|t| t.name.as_str()), Some("Nope"));
        }
    }

    #[test]
    fn a_map_key_class_may_be_given_as_the_short_name() {
        let d = decl(map_to_nope(
            serde_json::json!({ "$class": "StringMapKeyType" }),
            serde_json::json!({}),
        ));
        let map = d.as_map().unwrap();
        assert!(map.is_typed());
        assert_eq!(map.key_kind(), "StringMapKeyType");
        assert_eq!(map.value_type().map(|t| t.name.as_str()), Some("Nope"));
    }

    // TS `MapDeclaration.process` (mapdeclaration.ts): `ModelUtil.isValidMapKey`
    // rejects a key kind the metamodel's key union does not declare at
    // construction time, before the map ever loads — not deferred to
    // semantic validation the way an untyped fallback would suggest.
    #[test]
    fn a_map_with_an_unrecognised_key_kind_is_rejected_at_construction() {
        let err = Declaration::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MapDeclaration",
            "name": "Lookup",
            "key": { "$class": "concerto.metamodel@1.0.0.IntegerMapKeyType" },
            "value": { "$class": "concerto.metamodel@1.0.0.StringMapValueType" }
        }))
        .unwrap_err();
        assert!(
            err.to_string()
                .contains("MapDeclaration must contain valid MapKeyType  Lookup")
        );
    }

    #[test]
    fn a_map_with_no_key_is_rejected_at_construction() {
        let mut node = map_to_nope(string_key(), serde_json::json!({}));
        node.as_object_mut().unwrap().remove("key");
        let err = Declaration::try_from(&node).unwrap_err();
        assert!(
            err.to_string()
                .contains("MapDeclaration must contain Key & Value properties M")
        );
    }

    #[test]
    fn an_object_map_value_with_no_type_is_rejected_at_construction() {
        let err = Declaration::try_from(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.MapDeclaration",
            "name": "M",
            "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
            "value": { "$class": "concerto.metamodel@1.0.0.ObjectMapValueType" }
        }))
        .unwrap_err();
        assert!(err.to_string().contains(
            "ObjectMapValueType must contain property 'type', for MapDeclaration named M"
        ));
    }

    #[test]
    fn a_type_on_a_primitive_map_key_is_still_kept() {
        let d = decl(map_to_nope(
            serde_json::json!({
                "$class": "concerto.metamodel@1.0.0.StringMapKeyType",
                "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "K" }
            }),
            serde_json::json!({}),
        ));
        let map = d.as_map().unwrap();
        assert_eq!(map.key_kind(), "StringMapKeyType");
        assert_eq!(map.key_type().map(|t| t.name.as_str()), Some("K"));
    }

    /// TS `Declaration.process` rejects a missing or non-string name with
    /// its own "Invalid class name" message before anything map-specific
    /// (P2-08 review: this used to pin the serde decoding message instead).
    #[test]
    fn a_map_with_no_name_is_rejected_as_an_invalid_class_name() {
        let mut node = map_to_nope(string_key(), serde_json::json!({}));
        node.as_object_mut().unwrap().remove("name");
        let err = Declaration::try_from(&node);
        assert_eq!(
            err.unwrap_err().to_string(),
            "Invalid class name 'undefined'"
        );

        let err =
            Declaration::try_from(&map_to_nope(string_key(), serde_json::json!({ "name": 5 })));
        assert_eq!(err.unwrap_err().to_string(), "Invalid class name '5'");
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
}
