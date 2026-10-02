//! P5-80 (accordproject/concerto-rust#424) spike, analysis only: a cached
//! per-declaration **validation plan** for the instance paths. Not merged.
//!
//! ajv compiles a schema once into a validator specialised for it. The
//! instance layer here interprets the model graph on every call instead: it
//! walks super types, looks properties up by name, resolves each field's
//! type in its owner's namespace, and rebuilds string and number validators
//! for every value. A [`ClassPlan`] does that work once per class-like or
//! enum declaration and keeps the answer until the registered files change:
//!
//! - a flat property table, own properties then inherited ones, in
//!   `getProperties()` order (the [`ModelManager`]'s cached chain), with a
//!   name index for `getProperty`;
//! - each property's resolved kind ([`PlanKind`]), with the scalar's
//!   primitive and validator already read;
//! - the field's string/number validator and collection size validator,
//!   built once (regexes pre-compiled);
//! - the declaration in the chain that gives the identifier field, and
//!   whether the class is abstract;
//! - the inheritance chain, for `$class` assignability checks;
//! - an enum's value set;
//! - a map-typed field's key and value kinds ([`MapPlan`]): each slot's
//!   type resolved in the map's namespace, once, to the primitive it checks,
//!   the enum or class declaration it visits, or a relationship.
//!
//! # Behaviour
//!
//! A plan is built lazily, on the first instance call that meets the
//! declaration, and never raises: whatever fails while building it (an
//! unresolvable field type, a validator whose construction throws, a cyclic
//! chain) is recorded as "unplanned", and the caller takes the unplanned
//! path for that piece, which raises exactly the error it raised before. So
//! the plan changes no throw scenario and no exception class.
//!
//! # Invalidation and memory
//!
//! Plans live in [`ModelManager`]'s plan cache, a per-declaration slot table
//! next to the inheritance cache (`class_cache`, P5-06/P5-13), under a
//! `Mutex` so the manager stays `Sync`. `invalidate_caches` clears both on
//! every change to the registered files (add, a failed batch's rollback,
//! the metamodel's temporary load); `updateModelFile`, `deleteModelFile`,
//! `clearModelFiles` and `updateExternalModels` build a new manager, with
//! empty caches. A plan holds handles (`DeclId`, `PropId`), never borrows,
//! so it cannot outlive the arena it indexes, and is dropped with the
//! cache.
//!
//! # The switch
//!
//! [`set_enabled`] turns the plan on and off process-wide (on by default in
//! this prototype), so before and after run in one build: off, every
//! caller takes the unplanned path.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rustc_hash::{FxHashMap, FxHashSet};

use crate::error::{ContractError, Error, ErrorKind};
use crate::introspect::scalar::ScalarValidator;
use crate::introspect::validators::{CollectionSizeValidator, NumberValidator, StringValidator};
use crate::introspect::{Declaration, Property};
use crate::model_manager::{DeclId, ModelManager, PropId};
use crate::model_util;

use super::model::{Field, FieldType};
use super::validate::FieldElement;

static ENABLED: AtomicBool = AtomicBool::new(true);

/// Turns the validation plan on or off for the whole process.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

/// Whether the validation plan is on.
pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// What a property's declared type resolves to.
#[derive(Debug)]
pub enum PlanKind {
    /// A primitive field (`Boolean`, `String`, ...): its type name.
    Primitive(&'static str),
    /// A field whose type is a scalar.
    Scalar {
        /// The scalar declaration.
        decl: DeclId,
        /// The primitive it aliases (`processedType`).
        primitive: Option<&'static str>,
    },
    /// A field whose type is an enum, with the enum's value names.
    Enum {
        /// The enum declaration.
        decl: DeclId,
        /// Its value names.
        values: FxHashSet<Box<str>>,
    },
    /// A field whose type is a map.
    Map {
        /// The map declaration.
        decl: DeclId,
        /// Its key and value kinds, resolved once; `None` when either did
        /// not resolve, and the caller takes the unplanned path, which
        /// raises the same error (or none, for an empty map) it always did.
        entries: Option<MapPlan>,
    },
    /// A field whose type is a concept-like declaration.
    Class(DeclId),
    /// A relationship, with its resolved target's fully-qualified name.
    Relationship(Arc<str>),
    /// An enum declaration's own value member.
    EnumValue,
    /// The type did not resolve: the caller takes the unplanned path, which
    /// raises the same error it always did.
    Unresolved,
}

/// What `ResourceValidator.checkMapType` does with one map key or value,
/// resolved once from the map declaration.
#[derive(Debug)]
pub enum MapSlot {
    /// The primitive type name its value is checked against (`String`,
    /// `DateTime`, `Boolean`; any other name checks nothing, as in TS).
    Primitive(Box<str>),
    /// An enum declaration: `visitEnumDeclaration`.
    Enum(DeclId),
    /// A class declaration: `visitClassDeclaration`.
    Class(DeclId),
    /// A relationship map value: `checkRelationship`.
    Relationship,
    /// Nothing is checked (an object slot whose type is neither an enum, a
    /// class nor, under a scalar key, a scalar).
    Skip,
}

/// A map declaration's resolved key and value kinds.
#[derive(Debug)]
pub struct MapPlan {
    /// The key's.
    pub key: MapSlot,
    /// The value's.
    pub value: MapSlot,
}

/// A validator built once for a property.
#[derive(Debug)]
pub enum Prepared<T> {
    /// The property has none.
    None,
    /// Built.
    Built(T),
    /// Building it threw: the caller takes the unplanned path.
    Unplanned,
}

/// The value validator of a field: its own (a primitive's), or its
/// scalar's.
#[derive(Debug)]
pub enum ValueValidator {
    /// A `String` field's or a `String` scalar's.
    String(StringValidator),
    /// A number field's (built from `{lower, upper}`).
    Number(NumberValidator),
    /// A number scalar's own validator, already built with the scalar.
    ScalarNumber,
}

/// One property of a [`ClassPlan`].
#[derive(Debug)]
pub struct PlanProp {
    /// The property.
    pub prop: PropId,
    /// The declaration that declares it.
    pub owner: DeclId,
    /// What its type resolves to.
    pub kind: PlanKind,
    /// Its value validator.
    pub validator: Prepared<ValueValidator>,
    /// Its collection size validator.
    pub size: Prepared<CollectionSizeValidator>,
}

/// The plan of one class-like or enum declaration.
#[derive(Debug)]
pub struct ClassPlan {
    /// The declaration.
    pub decl: DeclId,
    /// TS `isAbstract()` (an enum's AST flag, a class's).
    pub is_abstract: bool,
    /// The declaration in the chain whose own identifier field is the
    /// inherited one (TS `getIdentifierFieldName()`), if any.
    pub identifier_owner: Option<DeclId>,
    /// The declaration, then each super type up to the root.
    pub chain: Box<[DeclId]>,
    /// Every property, own then inherited, in `getProperties()` order.
    pub props: Box<[PlanProp]>,
    /// The first property of each name (TS `getProperty`).
    index: FxHashMap<Box<str>, u32>,
    /// The identifier field's regex validator, for `Factory.newResource`
    /// (`idFullField?.validator`), when it has a regex.
    pub id_regex: Prepared<StringValidator>,
}

impl ClassPlan {
    /// The index into [`ClassPlan::props`] of the property called `name`.
    pub fn find(&self, name: &str) -> Option<usize> {
        self.index.get(name).map(|i| *i as usize)
    }

    /// Whether a property called `name` exists.
    pub fn contains(&self, name: &str) -> bool {
        self.index.contains_key(name)
    }

    /// The inherited identifier field's name (TS `getIdentifierFieldName()`).
    pub fn identifier_field<'a>(&self, mm: &'a ModelManager) -> Option<&'a str> {
        self.identifier_owner
            .and_then(|id| mm.own_identifier_field_name_of(id))
    }

    /// Whether this declaration is `fqn` or inherits from it, as
    /// [`ModelManager::is_assignable_to`] answers for a class-like `sub`.
    pub fn is_assignable_to(&self, mm: &ModelManager, fqn: &str) -> bool {
        self.chain
            .iter()
            .any(|id| mm.decl_fqn(*id).is_ok_and(|f| f == fqn))
    }

    /// The property at `index`, with its owner's fully-qualified name.
    pub fn property<'a>(&self, mm: &'a ModelManager, index: usize) -> (&'a str, &'a Property) {
        let p = &self.props[index];
        mm.property_with_owner_of(p.prop)
            .expect("a plan's property handle is live")
    }

    /// [`super::model::field`] of the property at `index`, from the plan;
    /// `None` when the plan could not resolve it.
    pub fn field<'a>(&self, mm: &'a ModelManager, index: usize) -> Option<Field<'a>> {
        let (owner_fqn, property) = self.property(mm, index);
        let field_type = match &self.props[index].kind {
            PlanKind::Primitive(t) => FieldType::Primitive(t),
            PlanKind::Scalar { decl, primitive } => {
                let Some(Declaration::Scalar(s)) = mm.declaration(*decl) else {
                    return None;
                };
                FieldType::Scalar {
                    primitive: *primitive,
                    default_value: s.default_value(),
                    validator: s.validator(),
                }
            }
            PlanKind::Enum { decl, .. } => FieldType::Enum(mm.decl_fqn(*decl).ok()?),
            PlanKind::Map { decl, .. } => FieldType::Map(mm.decl_fqn(*decl).ok()?),
            PlanKind::Class(d) => FieldType::Class(mm.decl_fqn(*d).ok()?),
            PlanKind::Relationship(t) => FieldType::Relationship(t.to_string()),
            PlanKind::EnumValue => FieldType::EnumValue,
            PlanKind::Unresolved => return None,
        };
        Some(Field {
            owner_fqn,
            property,
            field_type,
        })
    }
}

/// The plan of declaration `id`, built on first use and cached until the
/// registered files change; `None` when the plan is off, or when `id` is
/// not a class-like or enum declaration whose chain resolves (the caller
/// then takes the unplanned path).
pub fn class_plan(mm: &ModelManager, id: DeclId) -> Option<Arc<ClassPlan>> {
    if !enabled() {
        return None;
    }
    mm.cached_plan(id, || build(mm, id))
}

/// [`class_plan`] by fully-qualified name.
pub fn class_plan_by_name(mm: &ModelManager, fqn: &str) -> Option<Arc<ClassPlan>> {
    if !enabled() {
        return None;
    }
    class_plan(mm, mm.declaration_id(fqn)?)
}

fn build(mm: &ModelManager, id: DeclId) -> Option<ClassPlan> {
    let is_abstract = match mm.declaration(id)? {
        Declaration::Class(c) => c.is_abstract(),
        Declaration::Enum(e) => e.is_abstract(),
        Declaration::Scalar(_) | Declaration::Map(_) => return None,
    };
    let (chain, prop_ids) = mm.class_chain_and_properties(id).ok()?;
    let identifier_owner = chain
        .iter()
        .copied()
        .find(|d| mm.own_identifier_field_name_of(*d).is_some());
    let mut props = Vec::with_capacity(prop_ids.len());
    let mut index = FxHashMap::default();
    for (i, prop) in prop_ids.iter().copied().enumerate() {
        let owner = mm.property_owner_of(prop)?;
        let (owner_fqn, property) = mm.property_with_owner_of(prop)?;
        index
            .entry(property.name().into())
            .or_insert(u32::try_from(i).ok()?);
        let kind = resolve_kind(mm, owner_fqn, property);
        let (validator, size) = prepare_validators(mm, owner_fqn, property, &kind);
        props.push(PlanProp {
            prop,
            owner,
            kind,
            validator,
            size,
        });
    }
    let mut plan = ClassPlan {
        decl: id,
        is_abstract,
        identifier_owner,
        chain: chain.into(),
        props: props.into(),
        index,
        id_regex: Prepared::None,
    };
    plan.id_regex = identifier_regex(mm, &plan);
    Some(plan)
}

fn resolve_kind(mm: &ModelManager, owner_fqn: &str, property: &Property) -> PlanKind {
    let resolve = |name: &str| -> Option<String> {
        let namespace = model_util::get_namespace(Some(owner_fqn)).ok()?;
        mm.resolve_type_name_at(namespace, name, None).ok()
    };
    match property {
        Property::Relationship(rp) => match resolve(&rp.type_.name) {
            Some(fqn) => PlanKind::Relationship(fqn.into()),
            None => PlanKind::Unresolved,
        },
        Property::Object(op) => {
            let Some(id) = resolve(&op.type_.name).and_then(|fqn| mm.declaration_id(&fqn)) else {
                return PlanKind::Unresolved;
            };
            match mm.declaration(id) {
                Some(Declaration::Enum(e)) => PlanKind::Enum {
                    decl: id,
                    values: e.values().iter().map(|v| v.name().into()).collect(),
                },
                Some(Declaration::Scalar(s)) => PlanKind::Scalar {
                    decl: id,
                    primitive: s.processed_type(),
                },
                Some(Declaration::Map(_)) => PlanKind::Map {
                    decl: id,
                    entries: map_plan(mm, id),
                },
                Some(Declaration::Class(_)) => PlanKind::Class(id),
                None => PlanKind::Unresolved,
            }
        }
        Property::Enum(_) => PlanKind::EnumValue,
        Property::Boolean(_) => PlanKind::Primitive("Boolean"),
        Property::String(_) => PlanKind::Primitive("String"),
        Property::Integer(_) => PlanKind::Primitive("Integer"),
        Property::Long(_) => PlanKind::Primitive("Long"),
        Property::Double(_) => PlanKind::Primitive("Double"),
        Property::DateTime(_) => PlanKind::Primitive("DateTime"),
    }
}

/// The map declaration `id`'s key and value kinds, resolved the way
/// `visit_map_declaration` and `check_map_type` resolve them for each entry;
/// `None` when any resolution fails.
pub(super) fn map_plan(mm: &ModelManager, id: DeclId) -> Option<MapPlan> {
    let Some(Declaration::Map(map)) = mm.declaration(id) else {
        return None;
    };
    let map_fqn = mm.decl_fqn(id).ok()?;
    let key_is_scalar = super::validate::map_key_is_scalar(mm, map_fqn, map).ok()?;
    let key = map_slot(mm, map_fqn, map.key_kind(), map.key_type(), key_is_scalar)?;
    let value = if map.value_kind() == "RelationshipMapValueType" && map.value_type().is_some() {
        MapSlot::Relationship
    } else {
        map_slot(mm, map_fqn, map.value_kind(), map.value_type(), key_is_scalar)?
    };
    Some(MapPlan { key, value })
}

/// One slot of [`map_plan`], mirroring `check_map_type`'s resolution.
fn map_slot(
    mm: &ModelManager,
    map_fqn: &str,
    kind: &str,
    type_id: Option<&concerto_metamodel::concerto_metamodel_1_0_0::TypeIdentifier>,
    key_is_scalar: bool,
) -> Option<MapSlot> {
    if !super::validate::is_object_map_kind(kind) {
        return Some(MapSlot::Primitive(super::validate::kind_primitive_name(kind).into()));
    }
    let Some(ti) = type_id else {
        return Some(MapSlot::Skip);
    };
    let namespace = model_util::get_namespace(Some(map_fqn)).ok()?;
    let fqn = mm.resolve_type_name_at(namespace, &ti.name, None).ok()?;
    let id = mm.declaration_id(&fqn)?;
    let decl = mm.declaration(id)?;
    Some(if key_is_scalar && let Some(scalar) = decl.as_scalar() {
        MapSlot::Primitive(scalar.processed_type().unwrap_or_default().into())
    } else if decl.is_enum_declaration() {
        MapSlot::Enum(id)
    } else if decl.is_class_declaration() {
        MapSlot::Class(id)
    } else {
        MapSlot::Skip
    })
}

fn prepared<T>(r: crate::error::Result<T>) -> Prepared<T> {
    r.map_or(Prepared::Unplanned, Prepared::Built)
}

fn invalid_string_validator(e: serde_json::Error) -> Error {
    Error::from(ContractError::pre_port(
        ErrorKind::InvalidArgument,
        format!("invalid string validator: {e}"),
        None,
    ))
}

/// The validators `ResourceValidator` builds for each value of the
/// property, built once, with the same element.
fn prepare_validators(
    mm: &ModelManager,
    owner_fqn: &str,
    property: &Property,
    kind: &PlanKind,
) -> (Prepared<ValueValidator>, Prepared<CollectionSizeValidator>) {
    let elem = FieldElement::new(mm, owner_fqn, property);
    let size = match property.size_validator() {
        Some(sv) => prepared(CollectionSizeValidator::new(&elem, sv, None)),
        None => Prepared::None,
    };
    let number = |lower, upper| {
        let ast = super::validate::number_validator_ast(lower, upper);
        prepared(NumberValidator::new(&elem, &ast).map(ValueValidator::Number))
    };
    let validator = match (property, kind) {
        (Property::String(sp), _) if sp.validator.is_some() || sp.length_validator.is_some() => {
            prepared(
                StringValidator::new(&elem, sp.validator.as_ref(), sp.length_validator.as_ref(), None)
                    .map(ValueValidator::String),
            )
        }
        (Property::Integer(ip), _) => ip.validator.as_ref().map_or(Prepared::None, |v| number(v.lower, v.upper)),
        (Property::Long(lp), _) => lp.validator.as_ref().map_or(Prepared::None, |v| number(v.lower, v.upper)),
        (Property::Double(dp), _) => dp.validator.as_ref().map_or(Prepared::None, |v| number(v.lower, v.upper)),
        (_, PlanKind::Scalar { decl, .. }) => {
            let Some(Declaration::Scalar(s)) = mm.declaration(*decl) else {
                return (Prepared::Unplanned, size);
            };
            match s.validator() {
                Some(ScalarValidator::Number(_)) => Prepared::Built(ValueValidator::ScalarNumber),
                Some(ScalarValidator::String {
                    validator,
                    length_validator,
                }) => {
                    let build = || -> crate::error::Result<StringValidator> {
                        let validator = validator
                            .as_ref()
                            .map(|v| serde_json::from_value(v.clone()).map_err(invalid_string_validator))
                            .transpose()?;
                        let length_validator = length_validator
                            .as_ref()
                            .map(|v| serde_json::from_value(v.clone()).map_err(invalid_string_validator))
                            .transpose()?;
                        StringValidator::new(&elem, validator.as_ref(), length_validator.as_ref(), None)
                    };
                    prepared(build().map(ValueValidator::String))
                }
                None => Prepared::None,
            }
        }
        _ => Prepared::None,
    };
    (validator, size)
}

/// `model::identifier_regex`, from the plan: the identifying property's
/// regex validator.
fn identifier_regex(mm: &ModelManager, plan: &ClassPlan) -> Prepared<StringValidator> {
    let Some(id_field) = plan.identifier_field(mm) else {
        return Prepared::None;
    };
    let Some(class_decl) = mm.declaration(plan.decl) else {
        return Prepared::Unplanned;
    };
    let type_ref = super::model::TypeRef {
        mm,
        id: plan.decl,
        decl: class_decl,
    };
    match super::model::identifier_regex(&type_ref, id_field) {
        Ok(Some(v)) => Prepared::Built(v),
        Ok(None) => Prepared::None,
        Err(_) => Prepared::Unplanned,
    }
}

/// What the plan cache holds, for a memory estimate: the number of plans
/// and their properties.
pub fn stats(mm: &ModelManager) -> (usize, usize) {
    mm.plan_cache_stats()
}

#[cfg(test)]
mod tests {
    //! P5-80 item 6: a stale plan is never used. Each test builds plans by
    //! validating, changes the model, then validates an instance whose
    //! answer depends on the change.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use serde_json::{Value, json};

    use super::stats;
    use crate::error::ErrorKind;
    use crate::instance::validate::{ValidateOptions, validate_instance};
    use crate::introspect::model_file::ModelFile;
    use crate::model_manager::{ModelFileSource, ModelManager};

    const NS: &str = "org.acme@1.0.0";

    /// `concept P { o String name [regex] [o Integer age] }`.
    fn p_model(regex: Option<&str>, with_age: bool) -> Value {
        let mut name = json!({
            "$class": "concerto.metamodel@1.0.0.StringProperty",
            "name": "name", "isArray": false, "isOptional": false
        });
        if let Some(pattern) = regex {
            name["validator"] = json!({
                "$class": "concerto.metamodel@1.0.0.StringRegexValidator",
                "pattern": pattern, "flags": ""
            });
        }
        let mut properties = vec![name];
        if with_age {
            properties.push(json!({
                "$class": "concerto.metamodel@1.0.0.IntegerProperty",
                "name": "age", "isArray": false, "isOptional": false
            }));
        }
        json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": NS,
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "P", "isAbstract": false, "properties": properties
            }]
        })
    }

    fn manager(model: &Value) -> ModelManager {
        let mut mm = ModelManager::new().unwrap();
        mm.load_model(model, Some("p.cto".into())).unwrap();
        mm
    }

    fn check(mm: &ModelManager, v: Value) -> crate::error::Result<()> {
        validate_instance(mm, &v, &ValidateOptions::default())
    }

    fn x() -> Value {
        json!({ "$class": "org.acme@1.0.0.P", "name": "x" })
    }

    #[test]
    fn update_model_file_validates_against_the_new_regex() {
        let mm = manager(&p_model(None, false));
        check(&mm, x()).unwrap();
        assert!(stats(&mm).0 > 0 || !super::enabled());
        let mf = ModelFile::from_json(&p_model(Some("^y"), false), Some("p.cto".into())).unwrap();
        let updated = mm.update_model_file(mf, true).unwrap();
        let err = check(&updated, x()).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Validation);
        check(&updated, json!({ "$class": "org.acme@1.0.0.P", "name": "y" })).unwrap();
        // The manager it was updated from keeps its own answer.
        check(&mm, x()).unwrap();
    }

    #[test]
    fn update_model_file_validates_against_a_new_property() {
        let mm = manager(&p_model(None, false));
        check(&mm, x()).unwrap();
        let mf = ModelFile::from_json(&p_model(None, true), Some("p.cto".into())).unwrap();
        let updated = mm.update_model_file(mf, true).unwrap();
        // `age` is now required...
        assert!(check(&updated, x()).is_err());
        // ...and declared.
        check(&updated, json!({ "$class": "org.acme@1.0.0.P", "name": "x", "age": 3 })).unwrap();
        assert!(check(&mm, json!({ "$class": "org.acme@1.0.0.P", "name": "x", "age": 3 })).is_err());
    }

    #[test]
    fn delete_then_add_validates_against_the_new_model() {
        let mm = manager(&p_model(None, false));
        check(&mm, x()).unwrap();
        let mut deleted = mm.delete_model_file(NS).unwrap();
        assert_eq!(stats(&deleted), (0, 0));
        assert!(check(&deleted, x()).is_err());
        deleted.load_model(&p_model(Some("^y"), false), None).unwrap();
        assert!(check(&deleted, x()).is_err());
    }

    #[test]
    fn update_external_models_validates_against_the_downloaded_model() {
        let mut mm = manager(&p_model(None, false));
        check(&mm, x()).unwrap();
        mm.update_external_models([ModelFileSource {
            ast: p_model(Some("^y"), false),
            definitions: None,
            file_name: Some("@external/p.cto".into()),
        }])
        .unwrap();
        assert_eq!(stats(&mm), (0, 0));
        assert_eq!(check(&mm, x()).unwrap_err().kind(), ErrorKind::Validation);
    }

    #[test]
    fn adding_a_model_in_place_drops_a_plan_that_could_not_resolve_a_type() {
        // `concept A { o org.other@1.0.0.B b }`, loaded before `B`'s
        // namespace: the plan records `b` as unresolved, and validating a
        // `b` raises the unplanned path's type error.
        let a = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": NS,
            "imports": [{
                "$class": "concerto.metamodel@1.0.0.ImportType",
                "namespace": "org.other@1.0.0", "name": "B"
            }],
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "A", "isAbstract": false,
                "properties": [{
                    "$class": "concerto.metamodel@1.0.0.ObjectProperty",
                    "name": "b", "isArray": false, "isOptional": false,
                    "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "B" }
                }]
            }]
        });
        let b = json!({
            "$class": "concerto.metamodel@1.0.0.Model",
            "namespace": "org.other@1.0.0",
            "declarations": [{
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "B", "isAbstract": false,
                "properties": [{
                    "$class": "concerto.metamodel@1.0.0.StringProperty",
                    "name": "s", "isArray": false, "isOptional": false
                }]
            }]
        });
        let instance = json!({
            "$class": "org.acme@1.0.0.A",
            "b": { "$class": "org.other@1.0.0.B", "s": "x" }
        });
        let mut mm = manager(&a);
        assert!(check(&mm, instance.clone()).is_err());
        assert!(stats(&mm).0 > 0 || !super::enabled());
        mm.load_model(&b, None).unwrap();
        assert_eq!(stats(&mm), (0, 0));
        check(&mm, instance).unwrap();
    }
}
