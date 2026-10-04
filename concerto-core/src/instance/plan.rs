//! The cached per-generation **validation plan** of the instance paths.
//!
//! ajv compiles a schema once into a validator specialised for it. The
//! instance layer here otherwise interprets the model graph on every call:
//! it walks super types, looks properties up by name, resolves each field's
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
//!   the enum or class declaration it visits, or a relationship;
//! - the identifier field's regex validator, for `Factory.newResource`.
//!
//! The plan is always on, and it is the only route: it serves `validate`
//! (`ResourceValidator`, one walk), both `fromJSON` populators, the
//! `toJSON` generator, the factory's identifier check, and
//! `setPropertyValue`/`addArrayValue` (including the WASM
//! `validatePropertyBinary` fast path).
//!
//! # Behaviour
//!
//! A plan is built lazily, on the first instance call that meets the
//! declaration, and is total: whatever fails while building it is recorded
//! in it, never swallowed, and raised at the point the check that needs it
//! runs, which is where the model lookups ran before there was a plan:
//!
//! - the declaration's own chain (an unresolvable super type, a cyclic
//!   chain): [`class_plan`] returns the recorded error, the one
//!   `getProperties()`/`getIdentifierFieldName()` raise;
//! - a property whose type does not resolve: [`PlanKind::Unresolved`], raised
//!   when a value of the property is checked (or, for a relationship, at the
//!   point `checkRelationship` resolves its declared type);
//! - a validator whose construction throws: [`Prepared::Failed`], raised
//!   where the validator would have been built for the value;
//! - a map whose key or value type does not resolve: [`MapSlot::Unresolved`],
//!   raised for the first entry that reaches that slot.
//!
//! So the plan changes no throw scenario and no exception class, and the
//! first error is reported in the same order.
//!
//! # Invalidation and memory
//!
//! Plans live in [`ModelManager`]'s plan cache, a per-declaration slot table
//! next to the inheritance cache (`class_cache`), under a `Mutex` so the
//! manager stays `Sync`. `invalidate_caches` clears both on every change to
//! the registered files (add, a failed batch's rollback, the metamodel's
//! temporary load), so a plan only ever describes the current model
//! generation; `updateModelFile`, `deleteModelFile`, `clearModelFiles` and
//! `updateExternalModels` build a new manager, with empty caches. A plan
//! holds handles (`DeclId`, `PropId`), never borrows, so it cannot outlive
//! the arena it indexes, and is dropped with the cache. A plan costs about
//! 300 bytes per planned property.
//!
//! # Testing
//!
//! With the dev-only `validation-plan-testing` feature (never enabled by a
//! release build), `testing` can run a closure that builds every plan
//! afresh instead of reading the cache, so a test can check that a cached
//! plan (and a cached build error) gives the same outcome as a fresh one,
//! and count the plans a manager holds.

use std::sync::Arc;

use crate::hash::{SeededHashMap, SeededHashSet};

use crate::error::{Error, Result};
use crate::introspect::scalar::ScalarValidator;
use crate::introspect::validators::{CollectionSizeValidator, NumberValidator, StringValidator};
use crate::introspect::{Declaration, Property};
use crate::model_manager::{DeclId, ModelManager, PropId};
use crate::model_util;

use super::model::{Field, FieldType};
use super::validate::FieldElement;

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
    /// A field whose type is an enum.
    Enum(EnumPlan),
    /// A field whose type is a map.
    Map {
        /// The map declaration.
        decl: DeclId,
        /// Its key and value kinds, resolved once; the error
        /// `ModelUtil.isScalar(mapDeclaration.getKey())` raises when the
        /// key's type does not resolve, which a map value meets before its
        /// first entry.
        entries: std::result::Result<MapPlan, Error>,
    },
    /// A field whose type is a concept-like declaration.
    Class(DeclId),
    /// A relationship, with its resolved target's fully-qualified name.
    Relationship(Arc<str>),
    /// An enum declaration's own value member.
    EnumValue,
    /// The type did not resolve: the error resolving it raises, at the point
    /// the type is needed.
    Unresolved(Error),
}

/// An enum declaration and its value names.
#[derive(Debug)]
pub struct EnumPlan {
    /// The enum declaration.
    pub decl: DeclId,
    /// Its value names.
    pub values: SeededHashSet<Box<str>>,
}

impl EnumPlan {
    fn of(mm: &ModelManager, decl: DeclId) -> Self {
        let values = match mm.declaration(decl) {
            Some(Declaration::Enum(e)) => e.values().iter().map(|v| v.name().into()).collect(),
            _ => SeededHashSet::default(),
        };
        Self { decl, values }
    }
}

/// What `ResourceValidator.checkMapType` does with one map key or value,
/// resolved once from the map declaration.
#[derive(Debug)]
pub enum MapSlot {
    /// The primitive type name its value is checked against (`String`,
    /// `DateTime`, `Boolean`; any other name checks nothing, as in TS).
    Primitive(&'static str),
    /// An enum declaration: `visitEnumDeclaration`.
    Enum(EnumPlan),
    /// A class declaration: `visitClassDeclaration`.
    Class(DeclId),
    /// A relationship map value: `checkRelationship`, against the declared
    /// target type resolved in the map's namespace (or the error resolving
    /// it, which `checkRelationship` raises once the value's own target has
    /// checked out).
    Relationship(std::result::Result<Arc<str>, Error>),
    /// Nothing is checked (an object slot whose type is neither an enum, a
    /// class nor, under a scalar key, a scalar).
    Skip,
    /// The slot's type did not resolve: the error resolving it, raised for
    /// each entry that reaches the slot.
    Unresolved(Error),
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
    /// Building it threw: the error, raised where the validator would have
    /// been built for a value.
    Failed(Error),
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
    #[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
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
    index: SeededHashMap<Box<str>, u32>,
    /// The identifier field's regex validator, for `Factory.newResource`
    /// (`idFullField?.validator`), when it has a regex.
    pub id_regex: Prepared<StringValidator>,
    /// The error building the plan met (the declaration's chain does not
    /// resolve), which [`class_plan`] returns in place of the plan.
    failure: Option<Error>,
    /// Whether every part of the plan resolved and was built
    /// ([`ClassPlan::is_settled`]).
    settled: bool,
}

impl ClassPlan {
    /// True when every property's kind (a map's key and value too) resolved
    /// and every validator was built or is absent. Such a plan reads only
    /// resolved declarations, so adding a model file cannot change it and the
    /// plan cache keeps it across an append
    /// (`ModelManager::keep_caches_for_append`).
    pub fn is_settled(&self) -> bool {
        self.settled
    }

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

    /// [`super::model::field`] of the property at `index`, from the plan:
    /// the same field, or the same error when its type does not resolve.
    pub fn field<'a>(&'a self, mm: &'a ModelManager, index: usize) -> Result<Field<'a>> {
        let (owner_fqn, property) = self.property(mm, index);
        let field_type = match &self.props[index].kind {
            PlanKind::Primitive(t) => FieldType::Primitive(t),
            PlanKind::Scalar { decl, primitive } => {
                let Some(Declaration::Scalar(s)) = mm.declaration(*decl) else {
                    unreachable!("a scalar kind's declaration is a scalar");
                };
                FieldType::Scalar {
                    primitive: *primitive,
                    default_value: s.default_value(),
                    validator: s.validator(),
                }
            }
            PlanKind::Enum(e) => FieldType::Enum(mm.decl_fqn(e.decl)?),
            PlanKind::Map { decl, .. } => FieldType::Map(mm.decl_fqn(*decl)?),
            PlanKind::Class(d) => FieldType::Class(mm.decl_fqn(*d)?),
            PlanKind::Relationship(t) => FieldType::Relationship(std::borrow::Cow::Borrowed(&**t)),
            PlanKind::EnumValue => FieldType::EnumValue,
            PlanKind::Unresolved(e) => return Err(e.clone()),
        };
        Ok(Field {
            owner_fqn,
            property,
            field_type,
        })
    }
}

/// The plan of declaration `id`, built on first use and cached until the
/// registered files change; the error its chain raises when it does not
/// resolve (or `id` is not a class-like or enum declaration), cached with
/// it.
pub fn class_plan(mm: &ModelManager, id: DeclId) -> Result<Arc<ClassPlan>> {
    #[cfg(feature = "validation-plan-testing")]
    if testing::is_uncached() {
        return checked(Arc::new(build(mm, id)));
    }
    let plan = mm
        .cached_plan(id, || Some(build(mm, id)))
        .expect("a plan is always built");
    checked(plan)
}

/// [`class_plan`] by fully-qualified name: the declaration `getType` finds,
/// so an unknown name raises what [`ModelManager::properties`] raises for
/// it.
pub fn class_plan_by_name(mm: &ModelManager, fqn: &str) -> Result<Arc<ClassPlan>> {
    class_plan(mm, mm.type_declaration(fqn)?)
}

fn checked(plan: Arc<ClassPlan>) -> Result<Arc<ClassPlan>> {
    match &plan.failure {
        Some(err) => Err(err.clone()),
        None => Ok(plan),
    }
}

fn build(mm: &ModelManager, id: DeclId) -> ClassPlan {
    let (chain, prop_ids) = match mm.class_chain_and_properties(id) {
        Ok(found) => found,
        Err(err) => {
            return ClassPlan {
                decl: id,
                is_abstract: false,
                identifier_owner: None,
                chain: Box::default(),
                props: Box::default(),
                index: SeededHashMap::default(),
                id_regex: Prepared::None,
                failure: Some(err),
                settled: false,
            };
        }
    };
    // The chain resolved, so `id` is a class-like or enum declaration.
    let is_abstract = match mm.declaration(id) {
        Some(Declaration::Class(c)) => c.is_abstract(),
        Some(Declaration::Enum(e)) => e.is_abstract(),
        _ => false,
    };
    let identifier_owner = chain
        .iter()
        .copied()
        .find(|d| mm.own_identifier_field_name_of(*d).is_some());
    let mut props = Vec::with_capacity(prop_ids.len());
    let mut index = SeededHashMap::default();
    for (i, prop) in prop_ids.iter().copied().enumerate() {
        let owner = mm
            .property_owner_of(prop)
            .expect("a chain's property handle is live");
        let (owner_fqn, property) = mm
            .property_with_owner_of(prop)
            .expect("a chain's property handle is live");
        index
            .entry(property.name().into())
            .or_insert(u32::try_from(i).expect("fewer than 2^32 properties"));
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
        failure: None,
        settled: false,
    };
    plan.id_regex = identifier_regex(mm, &plan);
    plan.settled = !matches!(plan.id_regex, Prepared::Failed(_))
        && plan.props.iter().all(|p| {
            kind_is_settled(&p.kind)
                && !matches!(p.validator, Prepared::Failed(_))
                && !matches!(p.size, Prepared::Failed(_))
        });
    plan
}

/// Whether a property's kind resolved completely (a map's key and value, and
/// a map relationship's target, too). A kind holding a resolution error
/// could resolve once another model file is added, so its plan is not
/// settled.
fn kind_is_settled(kind: &PlanKind) -> bool {
    let slot_is_settled = |slot: &MapSlot| {
        !matches!(slot, MapSlot::Unresolved(_) | MapSlot::Relationship(Err(_)))
    };
    match kind {
        PlanKind::Unresolved(_) => false,
        PlanKind::Map { entries, .. } => entries
            .as_ref()
            .is_ok_and(|m| slot_is_settled(&m.key) && slot_is_settled(&m.value)),
        _ => true,
    }
}

/// A property's type, resolved as [`super::model::field`] resolves it, with
/// the error that raises when it does not resolve.
fn resolve_kind(mm: &ModelManager, owner_fqn: &str, property: &Property) -> PlanKind {
    let resolve = |name: &str| -> Result<String> {
        let namespace = model_util::get_namespace(Some(owner_fqn))?;
        mm.resolve_type_name_at(namespace, name, None)
    };
    let object = |name: &str| -> Result<PlanKind> {
        let fqn = resolve(name)?;
        let id = mm
            .declaration_id(&fqn)
            .ok_or_else(|| Error::type_not_found(fqn.clone()))?;
        mm.decl_fqn(id)?;
        Ok(
            match mm
                .declaration(id)
                .expect("declaration_id returns a live handle")
            {
                Declaration::Enum(_) => PlanKind::Enum(EnumPlan::of(mm, id)),
                Declaration::Scalar(s) => PlanKind::Scalar {
                    decl: id,
                    primitive: s.processed_type(),
                },
                Declaration::Map(_) => PlanKind::Map {
                    decl: id,
                    entries: map_plan(mm, id),
                },
                Declaration::Class(_) => PlanKind::Class(id),
            },
        )
    };
    match property {
        Property::Relationship(rp) => match resolve(&rp.type_.name) {
            Ok(fqn) => PlanKind::Relationship(fqn.into()),
            Err(err) => PlanKind::Unresolved(err),
        },
        Property::Object(op) => object(&op.type_.name).unwrap_or_else(PlanKind::Unresolved),
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
/// `ResourceValidator.visitMapDeclaration`/`checkMapType` resolve them for
/// each entry: the error `ModelUtil.isScalar(mapDeclaration.getKey())`
/// raises, or each slot (with the error resolving it, if any).
pub(super) fn map_plan(mm: &ModelManager, id: DeclId) -> std::result::Result<MapPlan, Error> {
    let Some(Declaration::Map(map)) = mm.declaration(id) else {
        unreachable!("map_plan is only called for a map declaration");
    };
    let map_fqn = mm.decl_fqn(id)?;
    let key_is_scalar = super::validate::map_key_is_scalar(mm, map_fqn, map)?;
    let key = map_slot(mm, map_fqn, map.key_kind(), map.key_type(), key_is_scalar);
    let value = if map.value_kind() == "RelationshipMapValueType"
        && let Some(type_id) = map.value_type()
    {
        MapSlot::Relationship(
            model_util::get_namespace(Some(map_fqn))
                .and_then(|namespace| mm.resolve_type_name_at(namespace, &type_id.name, None))
                .map(Arc::from),
        )
    } else {
        map_slot(mm, map_fqn, map.value_kind(), map.value_type(), key_is_scalar)
    };
    Ok(MapPlan { key, value })
}

/// One slot of [`map_plan`], mirroring `checkMapType`'s resolution.
fn map_slot(
    mm: &ModelManager,
    map_fqn: &str,
    kind: &str,
    type_id: Option<&concerto_metamodel::concerto_metamodel_1_0_0::TypeIdentifier>,
    key_is_scalar: bool,
) -> MapSlot {
    if !super::validate::is_object_map_kind(kind) {
        return MapSlot::Primitive(super::validate::kind_primitive_name(kind));
    }
    let Some(ti) = type_id else {
        return MapSlot::Skip;
    };
    let resolved = model_util::get_namespace(Some(map_fqn))
        .and_then(|namespace| mm.resolve_type_name_at(namespace, &ti.name, None))
        .and_then(|fqn| {
            mm.declaration_id(&fqn)
                .ok_or_else(|| Error::type_not_found(fqn))
        });
    let id = match resolved {
        Ok(id) => id,
        Err(err) => return MapSlot::Unresolved(err),
    };
    let decl = mm
        .declaration(id)
        .expect("declaration_id returns a live handle");
    if key_is_scalar && let Some(scalar) = decl.as_scalar() {
        MapSlot::Primitive(scalar.processed_type().unwrap_or_default())
    } else if decl.is_enum_declaration() {
        MapSlot::Enum(EnumPlan::of(mm, id))
    } else if decl.is_class_declaration() {
        MapSlot::Class(id)
    } else {
        MapSlot::Skip
    }
}

fn prepared<T>(r: Result<T>) -> Prepared<T> {
    match r {
        Ok(v) => Prepared::Built(v),
        Err(err) => Prepared::Failed(err),
    }
}

/// The validators `ResourceValidator` builds for each value of the
/// property, built once, with the same element.
fn prepare_validators(
    mm: &ModelManager,
    owner_fqn: &str,
    property: &Property,
    kind: &PlanKind,
) -> (Prepared<ValueValidator>, Prepared<CollectionSizeValidator>) {
    let elem = FieldElement::new(owner_fqn, property);
    let size = match property.size_validator() {
        Some(sv) => prepared(CollectionSizeValidator::new(&elem, sv, None)),
        None => Prepared::None,
    };
    let number = |lower, upper| {
        prepared(NumberValidator::from_bounds(&elem, lower, upper).map(ValueValidator::Number))
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
        (_, PlanKind::Scalar { decl, .. }) => match mm.declaration(*decl) {
            Some(Declaration::Scalar(s)) => match s.validator() {
                Some(ScalarValidator::Number(_)) => Prepared::Built(ValueValidator::ScalarNumber),
                Some(ScalarValidator::String(built)) => {
                    prepared(built.for_field(&elem).map(ValueValidator::String))
                }
                None => Prepared::None,
            },
            _ => Prepared::None,
        },
        _ => Prepared::None,
    };
    (validator, size)
}

/// `model::identifier_regex`, from the plan: the identifying property's
/// regex validator, or the error building it raises.
fn identifier_regex(mm: &ModelManager, plan: &ClassPlan) -> Prepared<StringValidator> {
    let Some(id_field) = plan.identifier_field(mm) else {
        return Prepared::None;
    };
    let class_decl = mm
        .declaration(plan.decl)
        .expect("a plan's declaration handle is live");
    let type_ref = super::model::TypeRef {
        mm,
        id: plan.decl,
        decl: class_decl,
    };
    match super::model::identifier_regex(&type_ref, id_field) {
        Ok(Some(v)) => Prepared::Built(v),
        Ok(None) => Prepared::None,
        Err(err) => Prepared::Failed(err),
    }
}

/// Test- and dev-only access to the plan (the `validation-plan-testing`
/// feature): no stability promise, and never enabled by a release build.
#[cfg(feature = "validation-plan-testing")]
pub mod testing {
    use std::cell::Cell;

    use crate::model_manager::ModelManager;

    thread_local! {
        static UNCACHED: Cell<bool> = const { Cell::new(false) };
    }

    pub(super) fn is_uncached() -> bool {
        UNCACHED.with(Cell::get)
    }

    /// Runs `f` with every plan built afresh on this thread, never read from
    /// (or written to) a manager's plan cache: a fresh plan, and a fresh
    /// build error, for each instance call in it. For parity tests only.
    pub fn uncached<R>(f: impl FnOnce() -> R) -> R {
        struct Restore(bool);
        impl Drop for Restore {
            fn drop(&mut self) {
                UNCACHED.with(|off| off.set(self.0));
            }
        }
        let _restore = Restore(UNCACHED.with(|off| off.replace(true)));
        f()
    }

    /// The number of plans `mm` holds, and of their properties.
    pub fn stats(mm: &ModelManager) -> (usize, usize) {
        mm.plan_cache_stats()
    }
}

#[cfg(test)]
mod tests {
    //! A stale plan is never used. Each test builds plans by
    //! validating, changes the model, then validates an instance whose
    //! answer depends on the change. And a cached build error is the error a
    //! fresh build raises.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use serde_json::{Value, json};

    use super::class_plan;
    use crate::error::ErrorKind;
    use crate::instance::validate::{ValidateOptions, validate_instance};
    use crate::introspect::model_file::ModelFile;
    use crate::model_manager::{ModelFileSource, ModelManager};

    const NS: &str = "org.acme@1.0.0";

    /// The number of plans `mm` holds, and of their properties.
    fn stats(mm: &ModelManager) -> (usize, usize) {
        mm.plan_cache_stats()
    }

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
        assert!(stats(&mm).0 > 0);
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

    /// `concept A { o org.other@1.0.0.B b }`, importing `B` from a namespace
    /// that may not be loaded.
    fn a_model() -> Value {
        json!({
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
        })
    }

    #[test]
    fn adding_a_model_in_place_drops_a_plan_that_could_not_resolve_a_type() {
        // `A` loaded before `B`'s namespace: the plan records `b` as
        // unresolved, and validating a `b` raises the error resolving it.
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
        let mut mm = manager(&a_model());
        assert!(check(&mm, instance.clone()).is_err());
        assert!(stats(&mm).0 > 0);
        mm.load_model(&b, None).unwrap();
        assert_eq!(stats(&mm), (0, 0));
        check(&mm, instance).unwrap();
    }

    /// A plan whose property type does not resolve raises, from the cache,
    /// the very error a fresh build records; and a declaration whose chain
    /// does not resolve caches that error in its plan.
    #[test]
    fn a_cached_build_error_is_the_fresh_build_s_error() {
        let mm = manager(&a_model());
        let instance = json!({
            "$class": "org.acme@1.0.0.A",
            "b": { "$class": "org.other@1.0.0.B", "s": "x" }
        });
        let fresh = check(&mm, instance.clone()).unwrap_err();
        let cached = check(&mm, instance).unwrap_err();
        assert_eq!(fresh, cached);
        let a = mm.declaration_id("org.acme@1.0.0.A").unwrap();
        let plan = class_plan(&mm, a).unwrap();
        assert_eq!(plan.field(&mm, 0).unwrap_err(), fresh);

        // `concept C extends Missing {}`: no plan, and the chain's own error.
        let mut mm = ModelManager::new().unwrap();
        mm.load_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": NS,
                "declarations": [{
                    "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                    "name": "C", "isAbstract": false,
                    "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Missing" },
                    "properties": []
                }]
            }),
            None,
        )
        .ok();
        if let Some(c) = mm.declaration_id("org.acme@1.0.0.C") {
            let expected = mm.properties("org.acme@1.0.0.C").unwrap_err();
            assert_eq!(class_plan(&mm, c).unwrap_err(), expected);
            assert_eq!(class_plan(&mm, c).unwrap_err(), expected);
        }
    }
}
