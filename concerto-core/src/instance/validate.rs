//! The instance validator: a port of `ResourceValidator`
//! (`src/serializer/resourcevalidator.ts`), task P3-01
//! (`accordproject/concerto-rust#56`).
//!
//! This folds `concerto-validate-rs` into `concerto-core` (plan decision
//! D3): that crate validated a JSON AST against the (hardcoded) Concerto
//! metamodel by walking its declarations and merging inherited properties.
//! [`validate_instance`] generalises the same walk to *any* model loaded
//! into a [`ModelManager`], which is what makes it an instance validator
//! rather than only a metamodel validator, and fixes `concerto-validate-rs`'s
//! four confirmed bugs (plan §1.3), each noted at the point it is fixed
//! below:
//!
//! - only the direct super type's properties were merged, wrongly rejecting
//!   a valid multi-level type (fixed by [`ModelManager::get_all_properties`],
//!   which already walks the whole super type chain);
//! - abstract and nested `$class` values were not checked (fixed: every
//!   object, at any depth, is re-resolved by its own `$class` and checked
//!   with [`ClassDeclaration::is_abstract`]);
//! - Long, DateTime, relationships, enums, maps and scalars had no support
//!   (all six are implemented below);
//! - errors were stringly typed (fixed: every error is a [`ContractError`]
//!   with the P1-05 `{kind, code, params, location}` shape, structured, not
//!   a `String`).
//!
//! # Scope: a typed value, not raw wire JSON
//!
//! TS's `ResourceValidator` runs over an in-memory `Resource`, already
//! populated by `JSONPopulator`: primitive fields hold JS values, a
//! `DateTime` field holds a `dayjs` object (`checkItem` tests `typeof obj
//! === 'object' && typeof obj.isBefore === 'function'`, which does **not**
//! itself re-validate the date's shape — an invalid-but-still-a-`Dayjs`
//! value passes this check in TS too), and a relationship field holds a
//! `Relationship` instance (`obj instanceof Relationship`). Rust has no such
//! runtime object, and building one (a full `JSONPopulator` port, which owns
//! the actual coercion: string → `Dayjs`, URI → `Relationship`) is out of
//! *this module's* scope (the issue's `ResourceValidator`/`JSONPopulator`
//! reference is for parity of **checks and messages** in each of the two
//! ported members; `JSONPopulator` itself is a separate porting task, not
//! yet done). What this module accepts is therefore a `Value` shaped like
//! wire JSON (`$class`-tagged, primitive fields as plain JSON) but with two
//! reserved markers standing in for the two non-JSON runtime types
//! `JSONPopulator` would have produced, so this validator's checks are
//! `instanceof`-shaped, not shape/parse-shaped, exactly like TS's own
//! post-population checks:
//!
//! - [`DAYJS_TAG`] (`"$$dayjs"`) on an object marks an already-coerced
//!   `DateTime` value (its doc comment has the detail);
//! - [`RELATIONSHIP_TAG`] (`"$$relationship"`) on an object marks an
//!   already-coerced `Relationship` value, carrying the pointed-at type as
//!   `$class` (its doc comment on [`check_relationship`] has the detail).
//!
//! A caller that already has real wire JSON (a `DateTime` as an ISO string,
//! a relationship as a URI string) is expected to coerce it into this shape
//! first — the native oracle harness's `tests/oracle/recipe.rs` does exactly
//! that when it replays an oracle `"typed"` fixture, which is how
//! [`validate_instance`] is exercised against real `Resource.validate`
//! fixtures (task P3-01 review) without a `JSONPopulator` port. This is a
//! scope decision about *what runs* (`JSONPopulator`'s own coercion is not
//! ported here), not a behavioural divergence from a ported TS member, so it
//! is documented here rather than in `DIVERGENCES.md` (PORTING.md 7.3, which
//! is for a *ported* member's own faithfully-kept quirk). It does mean an
//! untagged `DateTime`/relationship value — one that was never run through
//! the coercion step this module does not implement — is always rejected
//! here as a field type violation, which is the *correct*, TS-faithful
//! verdict for that case (an uncoerced value on a `Resource` field is
//! exactly what `checkItem`'s `instanceof`-style check rejects in TS too),
//! not an approximation of it.
//!
//! A third marker, [`UNDEFINED_TAG`] ([`js_undefined`]), stands for a JS
//! `undefined` held *inside* a value, such as an array element
//! (`["a", undefined, "b"]`) or a map value. JSON has no `undefined`, and
//! `null` is a different JS value (`typeof null` is `'object'`,
//! `${null}` is `null`), so collapsing one into the other changes the
//! words TS reports (`checkItem` reports an `undefined` item as a field type
//! violation of value `undefined`, type `undefined`).
//!
//! # JS engine errors
//!
//! Where TS calls a method that its argument may not have, the V8
//! `TypeError` is part of the behaviour and is ported (PORTING.md 2.2 step
//! 3): `reportInvalidFieldAssignment` calls `obj.getFullyQualifiedType()`,
//! and `reportNotResouceViolation`/`reportNotRelationshipViolation` call
//! `value.toString()`, on whatever value reached them (DV-008,
//! [`invalid_field_assignment_shape`], [`js_method_receiver_error`]).
//!
//! # Walk
//!
//! [`validate_instance`] is the entry point (TS `Resource.validate`): it
//! resolves the root value's own `$class` and calls
//! [`visit_class_declaration`], which is the port of
//! `ResourceValidator.visitClassDeclaration` and recurses through
//! [`visit_property`] (`Property.accept`/`visitField`/
//! `visitRelationshipDeclaration`) and [`visit_map_declaration`]
//! (`MapDeclaration.accept`), mirroring the TS visitor one function per
//! method, in the same order, so that the first error raised matches
//! (PORTING.md 2.4).

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use serde_json::Value;

use crate::ecma;
use crate::error::{ConcertoError, ContractError, ErrorKind, Result};
use crate::introspect::scalar::ScalarValidator;
use crate::introspect::validators::{CollectionSizeValidator, NumberValidator, StringValidator};
use crate::introspect::{Declaration, FullyQualified, Named, Property, Typed};
use crate::model_manager::{ModelManager, ValidatedElement};
use crate::model_util;

/// TS `SerializerOptions`, the two fields `ResourceValidator`'s constructor
/// reads (`resourcevalidator.ts` lines 53-58).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ValidateOptions {
    /// TS `options.convertResourcesToRelationships`.
    pub convert_resources_to_relationships: bool,
    /// TS `options.permitResourcesForRelationships`.
    pub permit_resources_for_relationships: bool,
}

/// TS `parameters`: the mutable state threaded through the whole visit.
struct Params<'a> {
    mm: &'a ModelManager,
    options: &'a ValidateOptions,
    /// TS `parameters.rootResourceIdentifier`.
    root_resource_identifier: String,
    /// TS `parameters.currentIdentifier`.
    current_identifier: Option<String>,
}

/// Validates `value` (a JSON instance, `$class`-tagged the way a Resource
/// serializes) against the model loaded into `mm`.
///
/// TS: `Resource.validate` (src/model/resource.ts), which resolves its own
/// type (`this.getModelManager().getType(this.getFullyQualifiedType())`,
/// always its own `$class`, since `this` supplies both) and hands it to
/// `ResourceValidator` as `classDeclaration`, with `[this]` as the visitor
/// stack's only entry.
pub fn validate_instance(
    mm: &ModelManager,
    value: &Value,
    options: &ValidateOptions,
) -> Result<()> {
    validate_instance_from(mm, value, options, String::new())
}

/// [`validate_instance`], with the `rootResourceIdentifier` the caller
/// starts the walk with (task P3-01b): `ValidatedResource.validate` sets it
/// to the instance's `getFullyQualifiedIdentifier()`, and `Serializer.toJSON`
/// sets none, which a report made before the walk sets one prints as
/// `undefined`.
pub fn validate_instance_from(
    mm: &ModelManager,
    value: &Value,
    options: &ValidateOptions,
    root_resource_identifier: String,
) -> Result<()> {
    let declared_fqn = value
        .get("$class")
        .and_then(Value::as_str)
        .ok_or_else(|| {
            // Not a TS-reachable path: a real `Resource` always has a
            // `$class` (it is how `getFullyQualifiedType()` answers at
            // all). A JSON document with none has no declared type to
            // report a violation against, so this is a harness-level
            // error, not a ported TS message.
            ContractError::pre_port(
                ErrorKind::Error,
                "cannot validate an instance with no $class".to_string(),
                None,
            )
        })?
        .to_string();
    let mut params = Params {
        mm,
        options,
        root_resource_identifier,
        current_identifier: None,
    };
    visit_class_declaration(&mut params, &declared_fqn, value)
}

/// Validates one property value, as `ValidatedResource.setPropertyValue`
/// and `addArrayValue` do before they assign it: `field.accept(this.$validator,
/// parameters)` with `value` alone on the stack and the instance's
/// `getFullyQualifiedIdentifier()` as `rootResourceIdentifier` (task P3-01b,
/// accordproject/concerto-rust#124).
///
/// `owner_fqn` is the declaration that declares `property` (its
/// `getParent()`), as [`ModelManager::get_property`] reports it.
///
/// TS: `field.accept(this.$validator, parameters)` in
/// `ValidatedResource.setPropertyValue`/`addArrayValue`
/// (src/model/validatedresource.ts), which dispatches to
/// `ResourceValidator.visitField` or `visitRelationshipDeclaration`.
pub fn validate_property_value(
    mm: &ModelManager,
    owner_fqn: &str,
    property: &Property,
    value: &Value,
    root_resource_identifier: String,
    options: &ValidateOptions,
) -> Result<()> {
    let mut params = Params {
        mm,
        options,
        root_resource_identifier,
        current_identifier: None,
    };
    visit_property(&mut params, owner_fqn, property, value)
}

// ---------------------------------------------------------------------
// visitClassDeclaration
// ---------------------------------------------------------------------

/// TS: `ResourceValidator.visitClassDeclaration` (resourcevalidator.ts:205).
/// `declared_fqn` is TS's `classDeclaration` argument: the *declared* type
/// in scope (the field's declared type, or the root's own type), which may
/// differ from `value`'s own, more specific `$class`.
fn visit_class_declaration(p: &mut Params, declared_fqn: &str, value: &Value) -> Result<()> {
    visit_class_declaration_dispatch(p, declared_fqn, value, false)
}

/// [`visit_class_declaration`], reached through [`check_map_type`] for a map
/// key/value's declared class type (accordproject/concerto-rust#194).
///
/// TS's `JSONPopulator.processMapType` is the only place that wraps its
/// `modelManager.getType(...)` lookup in a `try`/`catch`: on failure `decl`
/// stays `undefined`, and the parsed JSON object is returned exactly as
/// received, never becoming a `Resource`. Reached again here through
/// `ResourceValidator.checkMapType`'s `thing.accept(this, parameters)`, that
/// same object's own `$class` (if it has one at all) is exactly as
/// unresolvable as it was during populate — the same `modelManager` never
/// changes in between — so `obj instanceof Resource` is false in TS, not a
/// `TypeNotFoundException` from re-resolving that `$class`. Every *other*
/// caller of `visit_class_declaration` validates a value `JSONPopulator`
/// already turned into a genuine `Resource` (or a harness-constructed
/// wire-JSON stand-in for one, module doc "Scope"), where an unresolvable
/// own `$class` is a real `TypeNotFoundException`, so this distinction is
/// scoped to the map-value call alone.
fn visit_map_value_class_declaration(
    p: &mut Params,
    declared_fqn: &str,
    value: &Value,
) -> Result<()> {
    visit_class_declaration_dispatch(p, declared_fqn, value, true)
}

fn visit_class_declaration_dispatch(
    p: &mut Params,
    declared_fqn: &str,
    value: &Value,
    is_map_value: bool,
) -> Result<()> {
    // `obj instanceof Resource`: a `Relationship` ([`RELATIONSHIP_TAG`]) is
    // `Identifiable` but not a `Resource`.
    let Some(obj) = as_js_object(value).filter(|o| !o.contains_key(RELATIONSHIP_TAG)) else {
        return Err(not_resource_violation(p, declared_fqn, value));
    };
    let Some(own_fqn) = obj.get("$class").and_then(Value::as_str) else {
        return Err(not_resource_violation(p, declared_fqn, value));
    };
    let own_fqn = own_fqn.to_string();

    // `toBeAssignedClassDeclaration = modelManager.getType(obj.getFullyQualifiedType())`
    // — bug fix (nested/abstract `$class` unchecked): every object's own
    // `$class`, at any depth, is resolved and checked here, not only the
    // outermost one.
    let to_be_assigned = match p.mm.get_declaration(&own_fqn) {
        Ok(decl) => decl,
        // See `visit_map_value_class_declaration`'s doc.
        Err(_) if is_map_value => {
            return Err(not_resource_violation_with(p, declared_fqn, value, false));
        }
        Err(e) => {
            return Err(remap_type_not_found(
                e,
                &own_fqn,
                "modelmanager-gettype-notypeinns",
            ));
        }
    };
    let to_be_assigned_fqn = own_fqn.clone();
    let Some(class) = to_be_assigned.as_class() else {
        // `obj` resolves to an enum/scalar/map `$class`: not a TS-reachable
        // path (a Resource is never constructed with one of those types),
        // so this is a harness error, not a ported message.
        return Err(ContractError::pre_port(
            ErrorKind::Error,
            format!("'{own_fqn}' is not a class-like type and cannot back a Resource"),
            None,
        )
        .into());
    };
    let identifier_field_name = p.mm.identifier_field_name(&to_be_assigned_fqn)?;

    // `if(obj instanceof Identifiable) { parameters.rootResourceIdentifier =
    // obj.getFullyQualifiedIdentifier(); }`. Every `obj` reaching this point
    // is a `Resource`, and every `Resource` extends `Identifiable`
    // unconditionally in TS (`resource.ts`) — this does *not* depend on
    // whether `own_fqn`'s declared type happens to have an identifier field
    // (bug fix, found from the P3-01 review's oracle evidence: the previous
    // version gated this on [`ModelManager::is_identified`], so a non-identified
    // nested concept never updated `rootResourceIdentifier` on the way down,
    // unlike TS). `getFullyQualifiedIdentifier()`'s own truthiness check on
    // `getIdentifier()` (`identifiable.ts`) is what decides whether the
    // `#id` suffix appears — [`fully_qualified_identifier`] carries that
    // part faithfully (an absent or empty identifier both fall back to the
    // bare fqn, exactly as a falsy `""`/`undefined` would in TS).
    let own_id_field = identifier_field_name
        .clone()
        .unwrap_or_else(|| "$identifier".to_string());
    let own_id = obj.get(&own_id_field).and_then(Value::as_str);
    p.root_resource_identifier = fully_qualified_identifier(&own_fqn, own_id);

    // `if(toBeAssignedClassDeclaration.isAbstract())` — bug fix (abstract
    // `$class` unchecked): this now runs for every nested object, not only
    // the root.
    if class.is_abstract() {
        return Err(abstract_class(&to_be_assigned_fqn));
    }

    // `let props = Object.getOwnPropertyNames(obj)` — bug fix (only the
    // direct super type was merged): `get_all_properties` walks the whole
    // chain, so a property declared two or more levels up is found.
    let all_properties = p.mm.get_all_properties(&to_be_assigned_fqn)?;
    let declared_is_identified = p.mm.is_identified(declared_fqn)?;
    for key in obj.keys() {
        if model_util::is_system_property(key) {
            continue;
        }
        if all_properties.iter().any(|(_, prop)| prop.name() == key) {
            continue;
        }
        // `reportUndeclaredField(obj.getIdentifier(), ...)`: the *bare*
        // identifier value, not `getFullyQualifiedIdentifier()` (bug fix,
        // found from the P3-01 review's oracle evidence: the previous
        // version wrongly formatted this as `fqn#id`). `obj.getIdentifier()`
        // can genuinely be JS `undefined` (never set), which `${...}`
        // interpolates as the literal word `undefined` ([`js_id_display`]),
        // not an empty string.
        let resource_id = if declared_is_identified && key != "$identifier" {
            let id = identifier_field_name
                .as_deref()
                .and_then(|f| obj.get(f))
                .and_then(Value::as_str);
            js_id_display(id)
        } else {
            p.current_identifier
                .clone()
                .unwrap_or_else(|| "undefined".to_string())
        };
        return Err(undeclared_field(&resource_id, key, &to_be_assigned_fqn));
    }

    // `if(classDeclaration.isIdentified())`.
    if p.mm.is_identified(declared_fqn)? {
        let id_field = identifier_field_name
            .clone()
            .unwrap_or_else(|| "$identifier".to_string());
        let id = obj.get(&id_field).and_then(Value::as_str).unwrap_or("");
        if id.trim().is_empty() {
            return Err(empty_identifier(&p.root_resource_identifier));
        }
        p.current_identifier = Some(format!("{to_be_assigned_fqn}#{id}"));
    }

    // `const properties = toBeAssignedClassDeclaration.getProperties();`
    for (owner_fqn, property) in &all_properties {
        let value = obj.get(property.name());
        match value {
            Some(v) if !is_js_null(v) => {
                visit_property(p, owner_fqn, property, v)?;
            }
            _ => {
                if !property.is_optional() {
                    if property.name() == "$identifier"
                        && identifier_field_name.as_deref() != Some("$identifier")
                    {
                        continue;
                    }
                    if property_has_default_value(property) {
                        continue;
                    }
                    return Err(missing_required_property(
                        &p.root_resource_identifier,
                        property,
                    ));
                }
            }
        }
    }
    Ok(())
}

/// `Util.isNull`: `undefined` or `null` ([`UNDEFINED_TAG`] stands for
/// `undefined`).
fn is_js_null(value: &Value) -> bool {
    value.is_null() || is_js_undefined(value)
}

/// TS `Identifiable.getFullyQualifiedIdentifier`: `this.getIdentifier() ?
/// fqn + '#' + id : fqn` — `getIdentifier()`'s own truthiness check, so an
/// absent identifier and an empty-string one (both falsy in JS) fall back to
/// the bare fqn alike.
fn fully_qualified_identifier(fqn: &str, id: Option<&str>) -> String {
    match id {
        Some(id) if !id.is_empty() => format!("{fqn}#{id}"),
        _ => fqn.to_string(),
    }
}

/// The JS `${value}` template-literal spelling of a possibly-absent string:
/// `undefined` (the literal six-letter word, not an empty string) when the
/// value was never set at all, distinct from an explicit empty string. Used
/// wherever TS interpolates a value that can genuinely be `undefined` (as
/// opposed to [`fully_qualified_identifier`]'s falsy-id check, which folds
/// `undefined` and `""` together).
fn js_id_display(id: Option<&str>) -> String {
    id.map(str::to_string)
        .unwrap_or_else(|| "undefined".to_string())
}

/// Whether `value` stands for a real TS `Identifiable` (a `Resource` or
/// `Relationship`) in this port's tagged-value scheme (module doc "Scope"):
/// a `$class`-tagged object — never a bare [`DAYJS_TAG`]-tagged value, which
/// stands for a `Dayjs`, not an `Identifiable`. Returns `(fqn,
/// fully_qualified_identifier)`, resolving the identifier field TS's own
/// `getIdentifier()` would read (`p.mm.identifier_field_name`) and reading
/// its value straight off `value` — a nested Resource's wire form always
/// carries its own identifying field as an ordinary property, and a
/// `RELATIONSHIP_TAG`-tagged value carries it under that same key
/// (`tests/oracle/recipe.rs`'s `decode_typed_instance`).
fn identifiable_parts(p: &Params, value: &Value) -> Option<(String, String)> {
    let obj = value.as_object()?;
    let fqn = obj.get("$class")?.as_str()?.to_string();
    let id_field =
        p.mm.identifier_field_name(&fqn)
            .ok()
            .flatten()
            .unwrap_or_else(|| "$identifier".to_string());
    let id = obj.get(&id_field).and_then(Value::as_str);
    let fqi = fully_qualified_identifier(&fqn, id);
    Some((fqn, fqi))
}

/// TS `Resource.prototype.toString`/`Relationship.prototype.toString`:
/// `'Resource {id=' + this.getFullyQualifiedIdentifier() + '}'` (or
/// `'Relationship {...}'`), read off [`identifiable_parts`].
fn identifiable_to_string(p: &Params, value: &Value) -> Option<String> {
    let (_, fqi) = identifiable_parts(p, value)?;
    let ctor = if value
        .as_object()
        .is_some_and(|o| o.contains_key(RELATIONSHIP_TAG))
    {
        "Relationship"
    } else {
        "Resource"
    };
    Some(format!("{ctor} {{id={fqi}}}"))
}

/// Whether a property carries a non-null AST `defaultValue`
/// (`!Util.isNull(property?.defaultValue)`).
fn property_has_default_value(property: &Property) -> bool {
    match property {
        Property::Boolean(p) => p.default_value.is_some(),
        Property::String(p) => p.default_value.is_some(),
        Property::Integer(p) => p.default_value.is_some(),
        Property::Long(p) => p.default_value.is_some(),
        Property::Double(p) => p.default_value.is_some(),
        Property::Object(p) => p.default_value.is_some(),
        // DateTimeProperty, RelationshipProperty and EnumProperty carry no
        // `defaultValue` in the metamodel.
        Property::DateTime(_) | Property::Relationship(_) | Property::Enum(_) => false,
    }
}

// ---------------------------------------------------------------------
// Property dispatch: visitField / visitRelationshipDeclaration /
// isTypeEnum / isTypeScalar
// ---------------------------------------------------------------------

/// What an `Object`-typed property's referenced declaration turns out to
/// be, for the `isTypeEnum`/`isTypeScalar` dispatch `Property.accept` does
/// in TS by calling through to the referenced declaration.
enum ObjectTarget {
    /// TS: `field.isTypeEnum()` — the referenced declaration is an enum.
    Enum(String),
    /// TS: `field.isTypeScalar()` (`Field.getScalarField`, ledger P2-04, not
    /// ported there — implemented here, since P3-01 needs it for full
    /// scalar support): the referenced declaration is a scalar.
    Scalar(String),
    /// The referenced declaration is itself a map.
    Map(String),
    /// An ordinary concept-like reference.
    Class(String),
}

/// Resolves what an `Object`-typed property points at, in the namespace of
/// `owner_fqn` (the type that declares the property).
///
/// TS: `Field.isTypeEnum`/`isTypeScalar`/`Property.getFullyQualifiedTypeName`
/// (src/introspect/property.ts, field.ts).
fn resolve_object_target(
    mm: &ModelManager,
    owner_fqn: &str,
    ti: &mm::TypeIdentifier,
) -> Result<ObjectTarget> {
    let namespace = model_util::get_namespace(Some(owner_fqn))?;
    let fqn = mm.resolve_type_name(namespace, &ti.name, None)?;
    let decl = mm.get_declaration(&fqn)?;
    Ok(if decl.is_enum_declaration() {
        ObjectTarget::Enum(fqn)
    } else if decl.is_scalar_declaration() {
        ObjectTarget::Scalar(fqn)
    } else if decl.is_map_declaration() {
        ObjectTarget::Map(fqn)
    } else {
        ObjectTarget::Class(fqn)
    })
}

/// TS: `Property.accept`, dispatching to `visitField` (a `Field`) or
/// `visitRelationshipDeclaration` (a `RelationshipDeclaration`).
fn visit_property(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    value: &Value,
) -> Result<()> {
    if let Property::Relationship(rp) = property {
        return visit_relationship(p, owner_fqn, property, &rp.type_, value);
    }
    if let Property::Enum(_) = property {
        // A class declaration's own properties never include an enum
        // *value* member (only an `EnumDeclaration`'s do, reached through
        // `visit_enum_declaration` instead) — defensive, not TS-reachable.
        return Err(ContractError::pre_port(
            ErrorKind::Error,
            "an EnumProperty cannot be a class declaration's own field".to_string(),
            None,
        )
        .into());
    }

    // `field.isTypeEnum()`/`isTypeScalar()` only ever apply to an
    // `Object`-typed field; every primitive kind is always `isPrimitive()`.
    if let Property::Object(op) = property {
        match resolve_object_target(p.mm, owner_fqn, &op.type_)? {
            ObjectTarget::Enum(enum_fqn) => {
                return visit_field(p, owner_fqn, property, value, &Kind::Enum(enum_fqn));
            }
            ObjectTarget::Scalar(scalar_fqn) => {
                return visit_field(p, owner_fqn, property, value, &Kind::Scalar(scalar_fqn));
            }
            ObjectTarget::Map(map_fqn) => {
                return visit_field(p, owner_fqn, property, value, &Kind::MapTyped(map_fqn));
            }
            ObjectTarget::Class(class_fqn) => {
                return visit_field(p, owner_fqn, property, value, &Kind::Class(class_fqn));
            }
        }
    }

    visit_field(p, owner_fqn, property, value, &Kind::Primitive)
}

/// What a field's declared type turns out to be, once `isTypeEnum`/
/// `isTypeScalar` have been resolved (`resolve_object_target`), so that
/// [`visit_field`]/[`check_item`] can share one body across every kind, the
/// same way `checkItem`'s `if(field.isPrimitive())`/`else` does over the
/// underlying declaration TS looks up separately at each call site.
enum Kind {
    Primitive,
    Enum(String),
    Scalar(String),
    MapTyped(String),
    Class(String),
}

/// TS: `ResourceValidator.visitField` (resourcevalidator.ts:300), folding in
/// the `isTypeEnum`/`isTypeScalar` dispatch [`resolve_object_target`]
/// already ran once (TS re-reads `field.isTypeEnum()` and
/// `field.getScalarField()` fresh at each call, which always agrees with
/// the same dispatch run once here, since the model does not change mid-walk).
fn visit_field(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    value: &Value,
    kind: &Kind,
) -> Result<()> {
    // `if (dataType === 'undefined' || dataType === 'symbol')`. Not reached
    // from `visit_class_declaration`, which skips an `undefined` field
    // (`Util.isNull`), but ported as TS has it.
    if is_js_undefined(value) {
        return Err(field_type_violation(p, owner_fqn, property, value));
    }
    if let Kind::Enum(enum_fqn) = kind {
        return check_enum(p, owner_fqn, property, enum_fqn, value);
    }

    if property.is_array() {
        return check_array(p, owner_fqn, property, kind, value);
    }

    // `if(field.getSizeValidator() && obj instanceof Map)`: only reachable
    // when the field's own declared type is itself a map (`Kind::MapTyped`).
    if let (Some(sv), Kind::MapTyped(_)) = (property.size_validator(), kind)
        && let Some(entries) = map_entries(value)
    {
        let elem = FieldElement::new(p.mm, owner_fqn, property);
        CollectionSizeValidator::new(&elem, sv)?.validate(
            &elem,
            Some(p.root_resource_identifier.as_str()),
            entries.len() as f64,
        )?;
    }

    check_item(p, owner_fqn, property, kind, value)
}

/// TS: `ResourceValidator.checkEnum` (resourcevalidator.ts:335).
fn check_enum(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    enum_fqn: &str,
    value: &Value,
) -> Result<()> {
    if property.is_array() && !value.is_array() {
        return Err(field_type_violation(p, owner_fqn, property, value));
    }
    if property.is_array() {
        let items = value.as_array().expect("just checked is_array");
        if let Some(sv) = property.size_validator() {
            let elem = FieldElement::new(p.mm, owner_fqn, property);
            CollectionSizeValidator::new(&elem, sv)?.validate(
                &elem,
                Some(p.root_resource_identifier.as_str()),
                items.len() as f64,
            )?;
        }
        for item in items {
            visit_enum_declaration(p, enum_fqn, item)?;
        }
    } else {
        visit_enum_declaration(p, enum_fqn, value)?;
    }
    Ok(())
}

/// TS: `ResourceValidator.checkArray` (resourcevalidator.ts:365).
fn check_array(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    kind: &Kind,
    value: &Value,
) -> Result<()> {
    if !value.is_array() {
        return Err(field_type_violation(p, owner_fqn, property, value));
    }
    let items = value.as_array().expect("just checked is_array");
    if let Some(sv) = property.size_validator() {
        let elem = FieldElement::new(p.mm, owner_fqn, property);
        CollectionSizeValidator::new(&elem, sv)?.validate(
            &elem,
            Some(p.root_resource_identifier.as_str()),
            items.len() as f64,
        )?;
    }
    for item in items {
        check_item(p, owner_fqn, property, kind, item)?;
    }
    Ok(())
}

/// TS: `ResourceValidator.checkItem` (resourcevalidator.ts:386).
fn check_item(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    kind: &Kind,
    value: &Value,
) -> Result<()> {
    // `if (dataType === 'undefined' || dataType === 'symbol')`: an
    // `undefined` array element (`["a", undefined, "b"]`) is reported here,
    // with value and type both `undefined`.
    if is_js_undefined(value) {
        return Err(field_type_violation(p, owner_fqn, property, value));
    }
    match kind {
        Kind::Primitive => check_primitive_item(p, owner_fqn, property, value),
        Kind::Scalar(scalar_fqn) => check_scalar_item(p, owner_fqn, property, scalar_fqn, value),
        Kind::MapTyped(map_fqn) => visit_map_declaration(p, map_fqn, value),
        Kind::Class(class_fqn) => check_object_item(p, owner_fqn, property, class_fqn, value),
        Kind::Enum(_) => unreachable!("check_enum handles the Enum kind before check_item"),
    }
}

/// `field.isPrimitive()` branch of `checkItem`, for the six primitive
/// property kinds.
fn check_primitive_item(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    value: &Value,
) -> Result<()> {
    let type_name = property.type_name().unwrap_or_default();
    if !primitive_type_matches(type_name, value) {
        return Err(field_type_violation(p, owner_fqn, property, value));
    }
    // `if(field.getValidator() !== null) { field.getValidator().validate(...) }`.
    let elem = FieldElement::new(p.mm, owner_fqn, property);
    let identifier = p.current_identifier.clone();
    match property {
        Property::String(sp) => {
            if sp.validator.is_some() || sp.length_validator.is_some() {
                StringValidator::new(&elem, sp.validator.as_ref(), sp.length_validator.as_ref())?
                    .validate(&elem, identifier.as_deref(), value.as_str())?;
            }
        }
        Property::Integer(ip) => {
            if let Some(v) = &ip.validator {
                let ast = number_validator_ast(v.lower, v.upper);
                NumberValidator::new(&elem, &ast)?.validate(
                    &elem,
                    identifier.as_deref(),
                    value.as_f64(),
                )?;
            }
        }
        Property::Long(lp) => {
            if let Some(v) = &lp.validator {
                let ast = number_validator_ast(v.lower, v.upper);
                NumberValidator::new(&elem, &ast)?.validate(
                    &elem,
                    identifier.as_deref(),
                    value.as_f64(),
                )?;
            }
        }
        Property::Double(dp) => {
            if let Some(v) = &dp.validator {
                let ast = number_validator_ast(v.lower, v.upper);
                NumberValidator::new(&elem, &ast)?.validate(
                    &elem,
                    identifier.as_deref(),
                    value.as_f64(),
                )?;
            }
        }
        Property::Boolean(_) | Property::DateTime(_) => {}
        _ => unreachable!("check_primitive_item is only reached for primitive properties"),
    }
    Ok(())
}

/// TS `checkItem`'s primitive `switch(field.getType())`, over the *value*
/// (JSONPopulator's wire representation, module doc "Scope").
fn primitive_type_matches(type_name: &str, value: &Value) -> bool {
    match type_name {
        "String" => value.is_string(),
        "Long" | "Integer" | "Double" => value.as_f64().is_some_and(f64::is_finite),
        "Boolean" => value.is_boolean(),
        "DateTime" => is_populated_datetime(value),
        _ => false,
    }
}

/// A `DateTime` value that has already gone through `JSONPopulator`'s
/// coercion into a `Dayjs` instance (module doc "Scope"): the oracle harness
/// (`tests/oracle/recipe.rs`) tags a replayed `dayjs` field value this way
/// when it decodes an oracle `"typed"` receiver into this validator's wire
/// form, so `is_populated_datetime` below can tell a real (possibly
/// invalid-but-still-a-`Dayjs`) instance from an un-coerced wire string --
/// mirroring TS's own post-population check, `typeof obj === 'object' &&
/// typeof obj.isBefore === 'function'` (resourcevalidator.ts:420), which
/// does *not* itself re-validate the date's shape or calendar range: a
/// `Dayjs` built from a nonsense string is still a `Dayjs` object, so TS
/// accepts it at this point regardless (`dayjs.isValid()` is never called
/// here). A bare `Value::String`/`Value::Number` reaching this check was
/// never coerced, so it is always rejected here, exactly as a raw string
/// left on a `Resource` field (for example by `setPropertyValue`, bypassing
/// `JSONPopulator`) would be in TS.
pub const DAYJS_TAG: &str = "$$dayjs";

/// A value that has already been populated as a `Relationship` instance
/// (see [`DAYJS_TAG`]'s doc for why the tag exists): mirrors TS's `obj
/// instanceof Relationship` (resourcevalidator.ts:492), as opposed to a
/// `$class`-tagged plain object, which stands for `obj instanceof Resource`.
pub const RELATIONSHIP_TAG: &str = "$$relationship";

/// A JS `undefined` held inside a value: an array element or a map value
/// (module doc "Scope"). The value is the one-key object
/// `{UNDEFINED_TAG: true}` that [`js_undefined`] builds. JSON has no
/// `undefined`, and writing `null` instead would change what TS reports:
/// `typeof undefined` is `'undefined'` and `${undefined}` is `undefined`,
/// where `null` gives `'object'` and `null`.
pub const UNDEFINED_TAG: &str = "$$undefined";

/// A JS number that JSON cannot hold (`NaN`, `Infinity`, `-Infinity`), as
/// the one-key object `{NUMBER_TAG: "<its JS spelling>"}` that
/// [`js_special_number`] builds (task P3-01b): `typeof` is `'number'`, and
/// `reportFieldTypeViolation` prints it with `value.toString()`.
pub const NUMBER_TAG: &str = "$$number";

/// A JS `Map` (a populated `MapDeclaration` value), as the one-key object
/// `{MAP_TAG: [[key, value], ...]}` that [`js_map`] builds (task P3-01b):
/// its keys keep their JS type (a number key is not a string), and a plain
/// object is told apart from a `Map` (`obj instanceof Map`).
pub const MAP_TAG: &str = "$$map";

/// The value that stands for a non-finite JS number ([`NUMBER_TAG`]).
pub fn js_special_number(text: &str) -> Value {
    serde_json::json!({ NUMBER_TAG: text })
}

/// The value that stands for a JS `Map` ([`MAP_TAG`]).
pub fn js_map(entries: Vec<(Value, Value)>) -> Value {
    serde_json::json!({
        MAP_TAG: entries.into_iter().map(|(k, v)| Value::Array(vec![k, v])).collect::<Vec<_>>()
    })
}

/// The JS spelling of a [`NUMBER_TAG`] value.
fn special_number(value: &Value) -> Option<&str> {
    let o = value.as_object()?;
    if o.len() != 1 {
        return None;
    }
    o.get(NUMBER_TAG)?.as_str()
}

/// The entries of a [`MAP_TAG`] value.
fn map_entries(value: &Value) -> Option<Vec<(&Value, &Value)>> {
    let o = value.as_object()?;
    if o.len() != 1 {
        return None;
    }
    let entries = o.get(MAP_TAG)?.as_array()?;
    Some(
        entries
            .iter()
            .filter_map(|e| {
                let pair = e.as_array()?;
                Some((pair.first()?, pair.get(1)?))
            })
            .collect(),
    )
}

/// The value that stands for a JS `undefined` ([`UNDEFINED_TAG`]).
pub fn js_undefined() -> Value {
    serde_json::json!({ UNDEFINED_TAG: true })
}

/// Whether `value` stands for a JS `undefined` ([`UNDEFINED_TAG`]).
pub fn is_js_undefined(value: &Value) -> bool {
    value
        .as_object()
        .is_some_and(|o| o.len() == 1 && o.contains_key(UNDEFINED_TAG))
}

/// `value` as a JS object, which a JS `undefined` ([`UNDEFINED_TAG`]) is not.
///
/// Provably unreachable through any caller (P5-06: cargo-mutants found the
/// `||`->`&&` mutant survived; this is the proof, not a repeat of the
/// assertion): [`is_js_undefined`] and [`special_number`] each require the
/// object to have *exactly one* key — [`UNDEFINED_TAG`] or [`NUMBER_TAG`]
/// respectively — and those are two different keys, so no `Value` can
/// satisfy both at once. Under the `&&` mutant the combined condition is
/// therefore always `false`, for every possible `value`, making the mutant
/// equivalent to `value.as_object()` alone (never explicitly `None` for a
/// tagged value). This still cannot be observed at any of this function's
/// three call sites ([`visit_class_declaration`] x2, [`check_relationship`]):
/// each one reads only `$class` or [`RELATIONSHIP_TAG`] off the returned
/// map, and every legitimately single-key-tagged `value` — the *only* shape
/// this mutant's `false` can ever differ on — has neither, by the same
/// single-key argument, so `obj.get("$class")`/`o.contains_key(
/// RELATIONSHIP_TAG)` fail identically whether `obj` is `None` (the real
/// rejection) or `Some(&{one untagged-relevant key})` (the mutant, holding
/// the value's own single tag key back unused) — every caller's error path
/// reports the same original `value` regardless, never `obj` itself.
fn as_js_object(value: &Value) -> Option<&serde_json::Map<String, Value>> {
    if is_js_undefined(value) || special_number(value).is_some() {
        None
    } else {
        value.as_object()
    }
}

/// `JSON.stringify(value)`: `None` for a top-level `undefined` (which
/// `JSON.stringify` returns as `undefined`, not a string); an `undefined`
/// array element is written as `null` and an `undefined` object member is
/// left out, as `JSON.stringify` does.
fn js_json_stringify(value: &Value) -> Option<String> {
    fn plain(value: &Value) -> Value {
        // A non-finite number is `null`; a `Map` has no own enumerable
        // properties (`{}`).
        if special_number(value).is_some() {
            return Value::Null;
        }
        if map_entries(value).is_some() {
            return Value::Object(serde_json::Map::new());
        }
        match value {
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| {
                        if is_js_undefined(item) {
                            Value::Null
                        } else {
                            plain(item)
                        }
                    })
                    .collect(),
            ),
            Value::Object(map) => Value::Object(
                map.iter()
                    .filter(|(_, v)| !is_js_undefined(v))
                    .map(|(k, v)| (k.clone(), plain(v)))
                    .collect(),
            ),
            other => other.clone(),
        }
    }
    if is_js_undefined(value) {
        return None;
    }
    let plain = plain(value);
    Some(serde_json::to_string(&plain).unwrap_or_else(|_| plain.to_string()))
}

/// TS `typeof obj === 'object' && typeof obj.isBefore === 'function'`
/// (resourcevalidator.ts `checkItem`): true exactly when `value` is a
/// [`DAYJS_TAG`]-tagged object, i.e. reached this check as an already-typed
/// `Dayjs` (see the constant's doc). Used for a class declaration's own
/// `DateTime` fields (and scalar fields whose base type is `DateTime`),
/// which is what `Resource`'s properties always hold post-population.
fn is_populated_datetime(value: &Value) -> bool {
    value.as_object().is_some_and(|o| o.contains_key(DAYJS_TAG))
}

/// TS `dayjs.utc(value).isValid()` (`checkMapType`, resourcevalidator.ts:156):
/// unlike a class declaration's own fields, a `MapDeclaration`'s primitive
/// values are *not* run through `JSONPopulator.convertToObject` (module doc
/// on `visitMapDeclaration`/`processMapType`: only non-primitive map values
/// are converted), so a `DateTime`-valued map entry really is still the raw
/// wire value here, and TS really does re-parse it with `dayjs.utc` at this
/// point. `dayjs.utc(string)` (no explicit format) first tries dayjs core's
/// own lenient `REGEX_PARSE`, which requires only a four-digit year and
/// accepts any digits (with any separators) after it -- out-of-range
/// month/day/hour/minute/second values are *not* rejected there, they
/// overflow-normalise the same way `Date.UTC` does with excess numeric
/// constructor arguments, so `.isValid()` stays true; a string that
/// `REGEX_PARSE` does not match falls back to native `Date` parsing, which
/// this port cannot reproduce exactly. This is therefore a best-effort,
/// *not byte-verified*, approximation of `dayjs`'s real leniency (documented
/// here rather than silently passed off as exact, PORTING.md 7.2): a
/// four-digit year, optionally followed by more digits/separators, is
/// accepted without further range checking; anything else (including the
/// native-`Date`-parsing fallback's own accepted formats, e.g. `"May 1,
/// 2020"`) is rejected, which is stricter than TS in that one corner.
fn parses_as_dayjs(value: &Value) -> bool {
    // `dayjs.utc(undefined)` is the current time, which is valid (task
    // P3-01b: a `Map` `DateTime` value that is `undefined`).
    if is_js_undefined(value) {
        return true;
    }
    match value {
        Value::Number(_) => value.as_f64().is_some_and(f64::is_finite),
        Value::String(s) => {
            let re = regress::Regex::new(r"^\d{4}([^0-9].*)?$").expect("static pattern");
            re.find(s).is_some()
        }
        _ => false,
    }
}

/// `{lower, upper}`, as `NumberValidator::new` reads it (`bound`,
/// validators.rs): a bound the typed metamodel struct already collapsed
/// "absent" and "explicit null" into `None` for alike (OD-3), so this
/// serialises back to exactly what `bound()` expects either way.
fn number_validator_ast(lower: Option<f64>, upper: Option<f64>) -> Value {
    serde_json::json!({ "lower": lower, "upper": upper })
}

/// `isTypeScalar()`: the field's declared type is a scalar, so it is
/// checked as the scalar's own underlying primitive type, with the
/// scalar's own validator (TS `Field.getScalarField()`).
fn check_scalar_item(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    scalar_fqn: &str,
    value: &Value,
) -> Result<()> {
    let decl = p.mm.get_declaration(scalar_fqn)?;
    let scalar = decl
        .as_scalar()
        .expect("resolve_object_target only returns Scalar for a scalar declaration");
    let type_name = scalar.scalar_type().unwrap_or_default();
    if !primitive_type_matches(type_name, value) {
        return Err(field_type_violation(p, owner_fqn, property, value));
    }
    let elem = FieldElement::new(p.mm, owner_fqn, property);
    let identifier = p.current_identifier.clone();
    match scalar.validator() {
        Some(ScalarValidator::Number(nv)) => {
            nv.validate(&elem, identifier.as_deref(), value.as_f64())?;
        }
        Some(ScalarValidator::String {
            validator,
            length_validator,
        }) => {
            let bad = |e: serde_json::Error| {
                ConcertoError::from(ContractError::pre_port(
                    ErrorKind::Error,
                    format!("invalid string validator: {e}"),
                    None,
                ))
            };
            let validator = validator
                .as_ref()
                .map(|v| serde_json::from_value(v.clone()).map_err(bad))
                .transpose()?;
            let length_validator = length_validator
                .as_ref()
                .map(|v| serde_json::from_value(v.clone()).map_err(bad))
                .transpose()?;
            StringValidator::new(&elem, validator.as_ref(), length_validator.as_ref())?.validate(
                &elem,
                identifier.as_deref(),
                value.as_str(),
            )?;
        }
        None => {}
    }
    Ok(())
}

/// The `else` branch of `checkItem`: a field pointing at a transaction,
/// asset, participant, concept... (a concept-like reference).
fn check_object_item(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    declared_class_fqn: &str,
    value: &Value,
) -> Result<()> {
    // TS resolves `classDeclaration` from `obj.getFullyQualifiedType()` when
    // `obj` is `Identifiable`, and reports a field type violation if that
    // type cannot be resolved (`try { ... } catch`); it otherwise keeps the
    // field's own declared type. Since every candidate here is a plain JSON
    // object (never a `Relationship`), the object's own `$class`, if it has
    // one, always takes over — `visit_class_declaration` re-resolves and
    // re-checks it (abstract; assignability is handled just below, since
    // `visit_class_declaration`'s recursive call validates the object
    // against its *own* resolved type, never against the field's declared
    // one).
    //
    // `if(obj instanceof Identifiable) { ... isAssignableTo check ... }`
    // (bug fix, found from the P3-01 review's oracle evidence: this branch
    // was missing entirely). Every `$class`-tagged object reaching this
    // point is `Identifiable` (`Resource` extends it unconditionally in TS,
    // module doc "Scope"), so the check always runs, exactly the way it
    // does for a value that happens to have an identifier field and one
    // that does not alike — TS's own `instanceof` check does not
    // distinguish them either.
    if let Some(own_fqn) = value
        .as_object()
        .and_then(|o| o.get("$class"))
        .and_then(Value::as_str)
        && !p.mm.is_assignable_to(own_fqn, declared_class_fqn)?
    {
        return Err(invalid_field_assignment(p, owner_fqn, property, own_fqn));
    }
    visit_class_declaration(
        p,
        owner_fqn_for_object(property, declared_class_fqn).as_str(),
        value,
    )
    .map_err(|e| retarget_not_resource(e, p, owner_fqn, property, value))
}

/// TS passes `classDeclaration` itself (the field's declared type) into the
/// recursive `accept` call, so a `reportNotResouceViolation` names the
/// *declared* type, not the value's own `$class`. `own_fqn_for_object` gives
/// `visit_class_declaration` that same declared type.
fn owner_fqn_for_object(_property: &Property, declared_class_fqn: &str) -> String {
    declared_class_fqn.to_string()
}

/// `not_resource_violation` raised one recursion level down already carries
/// the right `classFQN` (the declared type it was called with), so this is
/// only a passthrough — kept as a named step so the intent at the call site
/// (module doc: "recurse") stays clear against `checkItem`'s TS body, which
/// has no separate remapping of its own either.
fn retarget_not_resource(
    e: ConcertoError,
    _p: &Params,
    _owner_fqn: &str,
    _property: &Property,
    _value: &Value,
) -> ConcertoError {
    e
}

// ---------------------------------------------------------------------
// visitEnumDeclaration
// ---------------------------------------------------------------------

/// TS: `ResourceValidator.visitEnumDeclaration` (resourcevalidator.ts:94).
///
/// TS passes the *enum declaration* as `reportInvalidEnumValue`'s `field`
/// argument, so the message's `fieldName` is the enum's own short name
/// (`enumDeclaration.getName()`, e.g. `Color`), not the name of the property
/// holding the value; and `value` is the raw `obj`, which the formatter's
/// `String.prototype.replace` converts with `String()` (`1`, `undefined`).
fn visit_enum_declaration(p: &Params, enum_fqn: &str, value: &Value) -> Result<()> {
    let decl = p.mm.get_declaration(enum_fqn)?;
    let Declaration::Enum(enum_decl) = decl else {
        return Err(ContractError::pre_port(
            ErrorKind::Error,
            format!("'{enum_fqn}' is not an enum declaration"),
            None,
        )
        .into());
    };
    // `property.getName() === obj`: only a string can match.
    let found = value
        .as_str()
        .is_some_and(|obj| enum_decl.values().iter().any(|v| v.name() == obj));
    if !found {
        return Err(invalid_enum_value(
            &p.root_resource_identifier,
            enum_decl.name(),
            &identifiable_to_string(p, value).unwrap_or_else(|| js_to_string(value)),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------
// visitRelationshipDeclaration
// ---------------------------------------------------------------------

/// TS: `ResourceValidator.visitRelationshipDeclaration` (resourcevalidator.ts:463).
fn visit_relationship(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    type_id: &mm::TypeIdentifier,
    value: &Value,
) -> Result<()> {
    if property.is_array() {
        if !value.is_array() {
            return Err(invalid_field_assignment_shape(
                p, owner_fqn, property, value,
            ));
        }
        let items = value.as_array().expect("just checked is_array");
        if let Some(sv) = property.size_validator() {
            let elem = FieldElement::new(p.mm, owner_fqn, property);
            CollectionSizeValidator::new(&elem, sv)?.validate(
                &elem,
                Some(p.root_resource_identifier.as_str()),
                items.len() as f64,
            )?;
        }
        for item in items {
            check_relationship(p, owner_fqn, property, type_id, item)?;
        }
    } else {
        check_relationship(p, owner_fqn, property, type_id, value)?;
    }
    Ok(())
}

/// TS: `ResourceValidator.checkRelationship` (resourcevalidator.ts:491).
fn check_relationship(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    type_id: &mm::TypeIdentifier,
    value: &Value,
) -> Result<()> {
    // `obj instanceof Relationship`: a [`RELATIONSHIP_TAG`]-tagged object
    // (see its doc), carrying the pointed-at type as `$class`.
    let obj = as_js_object(value);
    let is_relationship_instance = obj.is_some_and(|o| o.contains_key(RELATIONSHIP_TAG));
    // `obj instanceof Resource && (convertResourcesToRelationships ||
    // permitResourcesForRelationships)`: a nested (untagged) object standing
    // in for the relationship.
    let resource_target = obj
        .filter(|_| !is_relationship_instance)
        .filter(|_| {
            p.options.convert_resources_to_relationships
                || p.options.permit_resources_for_relationships
        })
        .and_then(|obj| obj.get("$class"))
        .and_then(Value::as_str);

    let target_fqn = match (is_relationship_instance, resource_target) {
        (true, _) => obj
            .and_then(|o| o.get("$class"))
            .and_then(Value::as_str)
            .map(str::to_string),
        (false, Some(class)) => Some(class.to_string()),
        (false, None) => None,
    };
    let Some(target_fqn) = target_fqn else {
        return Err(not_relationship_violation(p, owner_fqn, property, value));
    };

    let relationship_type =
        p.mm.get_declaration(&target_fqn)
            .map_err(|e| remap_type_not_found(e, &target_fqn, "modelmanager-gettype-notypeinns"))?;
    let Some(target_class) = relationship_type.as_class() else {
        return Err(not_relationship_violation(p, owner_fqn, property, value));
    };
    let _ = target_class;

    if p.mm.identifier_field_name(&target_fqn)?.is_none() {
        return Err(ContractError::new(
            ErrorKind::Error,
            "resourcevalidator-checkrelationship-notidentifiable",
            Vec::new(),
        )
        .into());
    }

    let namespace = model_util::get_namespace(Some(owner_fqn))?;
    let declared_fqn = p.mm.resolve_type_name(namespace, &type_id.name, None)?;
    if !p.mm.is_assignable_to(&target_fqn, &declared_fqn)? {
        return Err(invalid_field_assignment(
            p,
            owner_fqn,
            property,
            &target_fqn,
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------
// visitMapDeclaration / checkMapType
// ---------------------------------------------------------------------

/// TS: `ResourceValidator.visitMapDeclaration` (resourcevalidator.ts:178).
fn visit_map_declaration(p: &mut Params, map_fqn: &str, value: &Value) -> Result<()> {
    // `if (!((obj instanceof Map)))`: only a [`MAP_TAG`] value is a `Map`.
    let Some(entries) = map_entries(value) else {
        // `'Expected a Map, but found ' + JSON.stringify(obj)`:
        // `JSON.stringify(undefined)` is `undefined`, which `+` spells out.
        return Err(ContractError::new(
            ErrorKind::Error,
            "resourcevalidator-visitmapdeclaration-notamap",
            vec![(
                "obj",
                js_json_stringify(value).unwrap_or_else(|| "undefined".to_string()),
            )],
        )
        .into());
    };
    let decl = p.mm.get_declaration(map_fqn)?;
    let Some(map) = decl.as_map() else {
        return Err(ContractError::pre_port(
            ErrorKind::Error,
            format!("'{map_fqn}' is not a map declaration"),
            None,
        )
        .into());
    };
    let key_is_scalar = map_key_is_scalar(p.mm, map_fqn, map)?;
    for (key, value) in entries {
        // `ModelUtil.isSystemProperty(key)`: an `includes`, so only a string
        // key can be one.
        if key.as_str().is_some_and(model_util::is_system_property) {
            continue;
        }
        check_map_type(
            p,
            map_fqn,
            map.key_kind(),
            map.key_type(),
            key_is_scalar,
            key,
        )?;
        check_map_type(
            p,
            map_fqn,
            map.value_kind(),
            map.value_type(),
            key_is_scalar,
            value,
        )?;
    }
    Ok(())
}

/// `ModelUtil.isScalar(mapDeclaration.getKey())`: ported verbatim, including
/// TS's own quirk of always asking about the *key*'s scalar-ness, even
/// while validating the *value* (PORTING.md: faithful port, no
/// improvements) — `checkMapType`'s own `if
/// (ModelUtil.isScalar(mapDeclaration.getKey())) { type = thing.getType(); }`
/// runs unconditionally for both the key and the value slot.
fn map_key_is_scalar(
    mm: &ModelManager,
    map_fqn: &str,
    map: &crate::introspect::declaration::MapDeclaration,
) -> Result<bool> {
    if map.key_kind() != "ObjectMapKeyType" {
        return Ok(false);
    }
    let Some(ti) = map.key_type() else {
        return Ok(false);
    };
    let namespace = model_util::get_namespace(Some(map_fqn))?;
    let fqn = mm.resolve_type_name(namespace, &ti.name, None)?;
    Ok(mm.get_declaration(&fqn)?.is_scalar_declaration())
}

/// TS: `ResourceValidator.checkMapType` (resourcevalidator.ts:123).
fn check_map_type(
    p: &mut Params,
    map_fqn: &str,
    kind: &str,
    type_id: Option<&mm::TypeIdentifier>,
    key_is_scalar: bool,
    value: &Value,
) -> Result<()> {
    let is_primitive_kind = !matches!(
        kind,
        "ObjectMapKeyType" | "ObjectMapValueType" | "RelationshipMapValueType"
    );

    let primitive_type_name: String = if !is_primitive_kind {
        let Some(ti) = type_id else {
            return Ok(());
        };
        let namespace = model_util::get_namespace(Some(map_fqn))?;
        let fqn = mm_resolve(p.mm, namespace, &ti.name)?;
        let decl = p.mm.get_declaration(&fqn)?;

        // `if (ModelUtil.isScalar(mapDeclaration.getKey())) { type =
        // thing.getType(); }` — ported verbatim (see `map_key_is_scalar`'s
        // doc): this only ever matters when `thing` actually is a scalar.
        if key_is_scalar && let Some(scalar) = decl.as_scalar() {
            scalar.scalar_type().unwrap_or_default().to_string()
        } else if decl.is_enum_declaration() {
            // `thing.accept(this, parameters)`, dispatched by TS's `visit()`
            // to `visitEnumDeclaration` (bug fix: relationship/enum map
            // values were previously rejected).
            return visit_enum_declaration_value(p, &fqn, value);
        } else if decl.is_class_declaration() {
            // `thing.accept(this, parameters)` -> `visitClassDeclaration`.
            // Ported faithfully: this is also how TS itself checks a
            // `RelationshipMapValueType` value (as a nested object, not a
            // relationship URI) — module doc "Scope". `value` may be a raw,
            // never-converted object (accordproject/concerto-rust#194): see
            // `visit_map_value_class_declaration`'s doc.
            return visit_map_value_class_declaration(p, &fqn, value);
        } else {
            return Ok(());
        }
    } else {
        kind_primitive_name(kind)
    };

    match primitive_type_name.as_str() {
        "String" if !value.is_string() => {
            return Err(ContractError::new(
                ErrorKind::Error,
                "resourcevalidator-checkmaptype-expectedstring",
                vec![
                    ("mapFqn", map_fqn.to_string()),
                    ("value", js_to_string(value)),
                ],
            )
            .into());
        }
        "DateTime" if !parses_as_dayjs(value) => {
            return Err(ContractError::new(
                ErrorKind::Error,
                "resourcevalidator-checkmaptype-expecteddatetime",
                vec![
                    ("mapFqn", map_fqn.to_string()),
                    ("value", js_to_string(value)),
                ],
            )
            .into());
        }
        "Boolean" if !value.is_boolean() => {
            return Err(ContractError::new(
                ErrorKind::Error,
                "resourcevalidator-checkmaptype-expectedboolean",
                vec![
                    ("mapFqn", map_fqn.to_string()),
                    ("type", js_typeof(value).to_string()),
                    ("value", js_to_string(value)),
                ],
            )
            .into());
        }
        // TS's `switch` has no `default` (`Integer`/`Long`/`Double` fall
        // through unchecked, and `String`/`DateTime`/`Boolean` fall through
        // here too when the value is already valid): a faithful port, not
        // an improvement.
        _ => {}
    }
    Ok(())
}

fn mm_resolve(mm: &ModelManager, namespace: &str, short: &str) -> Result<String> {
    mm.resolve_type_name(namespace, short, None)
}

/// The primitive name a primitive map key/value `$class` short kind
/// implies, e.g. `StringMapKeyType` -> `"String"`.
fn kind_primitive_name(kind: &str) -> String {
    kind.strip_suffix("MapKeyType")
        .or_else(|| kind.strip_suffix("MapValueType"))
        .unwrap_or(kind)
        .to_string()
}

/// A map key/value's resolved declaration is an enum: `thing.accept(this,
/// parameters)` dispatches to `visitEnumDeclaration`, the same as for a
/// field ([`visit_enum_declaration`]).
fn visit_enum_declaration_value(p: &Params, enum_fqn: &str, value: &Value) -> Result<()> {
    visit_enum_declaration(p, enum_fqn, value)
}

// ---------------------------------------------------------------------
// ValidatedElement: a Property in the context of its owning class, for the
// P2-02 validator types' `validate`/`new`.
// ---------------------------------------------------------------------

/// TS: the `field` a `NumberValidator`/`StringValidator`/
/// `CollectionSizeValidator` is attached to: a `Property`, read here for its
/// own AST `defaultValue` and for `getFullyQualifiedName()`
/// (`getParent().getFullyQualifiedName() + '.' + getName()`).
struct FieldElement<'a> {
    owner_fqn: &'a str,
    property: &'a Property,
}

impl<'a> FieldElement<'a> {
    fn new(_mm: &'a ModelManager, owner_fqn: &'a str, property: &'a Property) -> Self {
        Self {
            owner_fqn,
            property,
        }
    }
}

impl FullyQualified for FieldElement<'_> {
    type Error = ConcertoError;

    fn fully_qualified_name(&self) -> Result<String> {
        Ok(format!("{}.{}", self.owner_fqn, self.property.name()))
    }
}

impl ValidatedElement for FieldElement<'_> {
    fn default_value(&self) -> Result<Option<Value>> {
        Ok(match self.property {
            Property::Boolean(p) => p.default_value.map(Value::Bool),
            Property::String(p) => p.default_value.clone().map(Value::String),
            Property::Integer(p) => p
                .default_value
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number),
            Property::Long(p) => p
                .default_value
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number),
            Property::Double(p) => p
                .default_value
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number),
            _ => None,
        })
    }

    fn name(&self) -> Result<String> {
        Ok(self.property.name().to_string())
    }
}

// ---------------------------------------------------------------------
// Error reporting: ResourceValidator's static `report*` methods.
// ---------------------------------------------------------------------

fn js_typeof(value: &Value) -> &'static str {
    if is_js_undefined(value) {
        return "undefined";
    }
    if special_number(value).is_some() {
        return "number";
    }
    match value {
        Value::String(_) => "string",
        Value::Number(_) => "number",
        Value::Bool(_) => "boolean",
        // JS `typeof null === 'object'`; arrays and objects are also
        // `'object'`.
        Value::Null | Value::Array(_) | Value::Object(_) => "object",
    }
}

/// `value.toString()` for the two call sites that need it
/// (`reportNotResouceViolation`, `reportNotRelationshipViolation`): JS
/// `Array.prototype.toString` joins with `,`; a plain object's default
/// `toString` is `[object Object]`.
fn js_to_string(value: &Value) -> String {
    if is_js_undefined(value) {
        return "undefined".to_string();
    }
    if let Some(n) = special_number(value) {
        return n.to_string();
    }
    if map_entries(value).is_some() {
        return "[object Map]".to_string();
    }
    match value {
        Value::Array(items) => items
            .iter()
            .map(js_to_string_element)
            .collect::<Vec<_>>()
            .join(","),
        Value::Object(_) => "[object Object]".to_string(),
        other => ecma::to_js_string(other),
    }
}

/// An array element's `toString`: `null`/`undefined` join as `""`.
fn js_to_string_element(value: &Value) -> String {
    if is_js_null(value) {
        String::new()
    } else {
        js_to_string(value)
    }
}

/// The `value` param `reportFieldTypeViolation` passes: `JSON.stringify`
/// for a truthy value, the raw JS `ToString` for a falsy one (2.1: "Where TS
/// calls JSON.stringify(value) first ... the param is that JSON text"). A
/// [`DAYJS_TAG`]-tagged value is a `Dayjs` instance, which defines its own
/// `toJSON()` (the ISO string) that `JSON.stringify` calls, so it is
/// stringified as that quoted string, not as the tag object's own JSON.
fn field_value_param(value: &Value) -> String {
    if let Some(iso) = value.as_object().and_then(|o| o.get(DAYJS_TAG)) {
        return serde_json::to_string(iso).unwrap_or_else(|_| iso.to_string());
    }
    if is_js_undefined(value) {
        // Falsy: left as `undefined`, which the formatter spells out.
        return "undefined".to_string();
    }
    // `typeof value === 'number' && !isFinite(value)`: `value.toString()`
    // (`NaN` is falsy and left as it is, which prints the same).
    if let Some(n) = special_number(value) {
        return n.to_string();
    }
    if ecma::is_truthy(value) {
        js_json_stringify(value).unwrap_or_else(|| "undefined".to_string())
    } else {
        ecma::to_js_string(value)
    }
}

/// TS: `ResourceValidator.reportFieldTypeViolation` (resourcevalidator.ts:520).
fn field_type_violation(
    p: &Params,
    owner_fqn: &str,
    property: &Property,
    value: &Value,
) -> ConcertoError {
    let is_array = if property.is_array() { "[]" } else { "" };
    let _ = owner_fqn;
    // `if(value instanceof Identifiable) { typeOfValue =
    // value.getFullyQualifiedType(); value = value.getFullyQualifiedIdentifier(); }`
    // (bug fix, found from the P3-01 review's oracle evidence: this
    // `Identifiable` case is TS-reachable — a `Resource`/`Relationship`
    // reaching this point is exactly [`identifiable_parts`]'s tagged-value
    // scheme, module doc "Scope" — so it is no longer treated as
    // unreachable and JSON-stringified).
    let (value_param, type_of_value) = match identifiable_parts(p, value) {
        Some((fqn, fqi)) => (fqi, fqn),
        None => (field_value_param(value), js_typeof(value).to_string()),
    };
    ContractError::new(
        ErrorKind::Validation,
        "resourcevalidator-fieldtypeviolation",
        vec![
            ("resourceId", p.root_resource_identifier.clone()),
            ("propertyName", property.name().to_string()),
            (
                "fieldType",
                format!("{}{is_array}", property.type_name().unwrap_or_default()),
            ),
            ("value", value_param),
            ("typeOfValue", type_of_value),
        ],
    )
    .into()
}

/// TS: `ResourceValidator.reportNotResouceViolation` (resourcevalidator.ts:560).
/// `value.toString()` is a V8 `TypeError` for a `null` or `undefined` value
/// (DV-008), and `'Relationship {id=...}'` for a `Relationship`.
fn not_resource_violation(p: &Params, class_fqn: &str, value: &Value) -> ConcertoError {
    not_resource_violation_with(p, class_fqn, value, true)
}

/// [`not_resource_violation`], but never through [`identifiable_to_string`]
/// (`try_identifiable` gates it) — for `value`'s own `toString()` when
/// `value` is known to never have been a real `Identifiable` in the first
/// place, not merely a `Resource` in the wrong slot. TS's `identifiable_to_string`
/// stand-in only holds for a value `JSONPopulator` actually constructed
/// (module doc "Scope"): a raw, never-converted `$class`-tagged object
/// (`visit_map_value_class_declaration`'s doc, accordproject/concerto-rust#194)
/// is a plain JS object, whose real `toString()` is `Object.prototype`'s
/// (`js_to_string`'s `"[object Object]"`), not `Resource {id=...}`.
fn not_resource_violation_with(
    p: &Params,
    class_fqn: &str,
    value: &Value,
    try_identifiable: bool,
) -> ConcertoError {
    if is_js_null(value) {
        // DV-008
        return js_method_receiver_error(value, "value.toString", "toString");
    }
    let invalid_value = if try_identifiable {
        identifiable_to_string(p, value).unwrap_or_else(|| js_to_string(value))
    } else {
        js_to_string(value)
    };
    ContractError::new(
        ErrorKind::Validation,
        "resourcevalidator-notresourceorconcept",
        vec![
            ("resourceId", p.root_resource_identifier.clone()),
            ("classFQN", class_fqn.to_string()),
            ("invalidValue", invalid_value),
        ],
    )
    .into()
}

/// V8's `TypeError` for `expression()`, a call of `method` on `value`,
/// when `value` has no such method (PORTING.md 2.2 step 3): `Cannot read
/// properties of null (reading 'method')` when `value` is `null` or
/// `undefined`, and `expression is not a function` otherwise.
fn js_method_receiver_error(value: &Value, expression: &str, method: &str) -> ConcertoError {
    if is_js_null(value) {
        let receiver = if is_js_undefined(value) {
            "undefined"
        } else {
            "null"
        };
        return ContractError::new(
            ErrorKind::JsTypeError,
            "engine-typeerror-readproperties",
            vec![
                ("value", receiver.to_string()),
                ("property", method.to_string()),
            ],
        )
        .into();
    }
    ContractError::new(
        ErrorKind::JsTypeError,
        "engine-typeerror-notafunction",
        vec![("expression", expression.to_string())],
    )
    .into()
}

/// TS: `ResourceValidator.reportNotRelationshipViolation` (resourcevalidator.ts:576).
fn not_relationship_violation(
    p: &Params,
    owner_fqn: &str,
    property: &Property,
    value: &Value,
) -> ConcertoError {
    if is_js_null(value) {
        // DV-008: `value.toString()` on `null`/`undefined`.
        return js_method_receiver_error(value, "value.toString", "toString");
    }
    let type_name = property.type_name().unwrap_or_default();
    let namespace = model_util::get_namespace(Some(owner_fqn)).unwrap_or(owner_fqn);
    let class_fqn = model_util::get_fully_qualified_name(namespace, type_name);
    // `value.toString()`: a nested Resource or (wrongly, per this check)
    // Relationship-shaped value that reaches here is `Identifiable`, whose
    // own `toString()` is `'Resource {id=...}'`/`'Relationship {id=...}'`
    // (bug fix, found from the P3-01 review's oracle evidence: this case is
    // TS-reachable — it is exactly what a permitted-resource-in-place check
    // failing, or a plain object with no `convertResourcesToRelationships`,
    // produces), never the generic `[object Object]` fallback.
    let invalid_value = identifiable_to_string(p, value).unwrap_or_else(|| js_to_string(value));
    ContractError::new(
        ErrorKind::Validation,
        "resourcevalidator-notrelationship",
        vec![
            ("resourceId", p.root_resource_identifier.clone()),
            ("classFQN", class_fqn),
            ("invalidValue", invalid_value),
        ],
    )
    .into()
}

/// TS: `ResourceValidator.reportMissingRequiredProperty` (resourcevalidator.ts:591).
fn missing_required_property(resource_id: &str, property: &Property) -> ConcertoError {
    ContractError::new(
        ErrorKind::Validation,
        "resourcevalidator-missingrequiredproperty",
        vec![
            ("resourceId", resource_id.to_string()),
            ("fieldName", property.name().to_string()),
        ],
    )
    .into()
}

/// TS: `ResourceValidator.reportEmptyIdentifier` (resourcevalidator.ts:605).
fn empty_identifier(resource_id: &str) -> ConcertoError {
    ContractError::new(
        ErrorKind::Validation,
        "resourcevalidator-emptyidentifier",
        vec![("resourceId", resource_id.to_string())],
    )
    .into()
}

/// TS: `ResourceValidator.reportInvalidEnumValue` (resourcevalidator.ts:619).
/// `field_name` is the `field.getName()` TS reads, which is the enum
/// declaration's name ([`visit_enum_declaration`]).
/// `value` is the JS `String(obj)` of the value, which for a `Resource`
/// or a `Relationship` is its own `toString()`.
fn invalid_enum_value(resource_id: &str, field_name: &str, value: &str) -> ConcertoError {
    ContractError::new(
        ErrorKind::Validation,
        "resourcevalidator-invalidenumvalue",
        vec![
            ("resourceId", resource_id.to_string()),
            ("value", value.to_string()),
            ("fieldName", field_name.to_string()),
        ],
    )
    .into()
}

/// TS: `ResourceValidator.reportAbstractClass` (resourcevalidator.ts:634).
fn abstract_class(class_fqn: &str) -> ConcertoError {
    ContractError::new(
        ErrorKind::Validation,
        "resourcevalidator-abstractclass",
        vec![("className", class_fqn.to_string())],
    )
    .into()
}

/// TS: `ResourceValidator.reportUndeclaredField` (resourcevalidator.ts:649).
fn undeclared_field(resource_id: &str, property_name: &str, fqn: &str) -> ConcertoError {
    ContractError::new(
        ErrorKind::Validation,
        "resourcevalidator-undeclaredfield",
        vec![
            ("resourceId", resource_id.to_string()),
            ("propertyName", property_name.to_string()),
            ("fullyQualifiedTypeName", fqn.to_string()),
        ],
    )
    .into()
}

/// TS: `ResourceValidator.reportInvalidFieldAssignment` (resourcevalidator.ts:667).
fn invalid_field_assignment(
    p: &Params,
    owner_fqn: &str,
    property: &Property,
    object_type: &str,
) -> ConcertoError {
    let type_name = property.type_name().unwrap_or_default();
    let namespace = model_util::get_namespace(Some(owner_fqn)).unwrap_or(owner_fqn);
    let mut field_type = model_util::get_fully_qualified_name(namespace, type_name);
    if property.is_array() {
        field_type.push_str("[]");
    }
    ContractError::new(
        ErrorKind::Validation,
        "resourcevalidator-invalidfieldassignment",
        vec![
            ("resourceId", p.root_resource_identifier.clone()),
            ("propertyName", property.name().to_string()),
            ("objectType", object_type.to_string()),
            ("fieldType", field_type),
        ],
    )
    .into()
}

/// Same report as [`invalid_field_assignment`], for the shape mismatch at
/// the top of `visitRelationshipDeclaration` (`!(obj instanceof Array)`),
/// which TS reports through the same `reportInvalidFieldAssignment` call
/// (resourcevalidator.ts:468). That call reads `objectType:
/// obj.getFullyQualifiedType()` off the non-array value itself: an
/// `Identifiable` (a single `Relationship` or `Resource` on an array field)
/// answers its own type, and any other value has no such method, so V8
/// throws a `TypeError` instead of the `ValidationException` (DV-008;
/// fixture `d444ebcf0cf5a3c23e5ee6dd`, a string on a `--> Car[]` field).
fn invalid_field_assignment_shape(
    p: &Params,
    owner_fqn: &str,
    property: &Property,
    value: &Value,
) -> ConcertoError {
    match identifiable_parts(p, value) {
        Some((object_type, _)) => invalid_field_assignment(p, owner_fqn, property, &object_type),
        // DV-008
        None => {
            js_method_receiver_error(value, "obj.getFullyQualifiedType", "getFullyQualifiedType")
        }
    }
}

/// Remaps the generic [`ConcertoError::TypeNotFound`]
/// [`ModelManager::get_declaration`] raises into the catalogue's
/// `TypeNotFoundException` shape (table 2.3's default message), the same
/// way `model_manager.rs`'s own collaborator calls already do at their call
/// sites (`super_type_fqn`), since `get_declaration` itself is pre-port
/// (module doc on `error/mod.rs`, section 2.3).
fn remap_type_not_found(err: ConcertoError, fqn: &str, _hint: &str) -> ConcertoError {
    match err {
        ConcertoError::TypeNotFound { .. } => ContractError::type_not_found(
            "typenotfounderror-defaultmessage",
            Vec::new(),
            fqn.to_string(),
            None,
        )
        .into(),
        other => other,
    }
}

// ---------------------------------------------------------------------
// Collect-all diagnostics (task P3-03, accordproject/concerto-rust#58)
// ---------------------------------------------------------------------
//
// [`diagnostic`](crate::instance::diagnostic)'s module doc has the design.
// This section adds the walk itself: [`collect_diagnostics`] mirrors
// [`visit_class_declaration`]/[`visit_property`]/[`check_item`] above, but
// never returns early on a violation — it records a [`Diagnostic`] and keeps
// walking, so a value with several unrelated problems (two missing
// properties, an undeclared field, a bad enum value...) is reported in one
// pass. Where a leaf check already gives a single, TS-faithful verdict
// (a primitive/scalar/enum/relationship/map value, or a whole array of
// them), the walk reuses [`validate_property_value`] as-is rather than
// re-deriving its many branches, and turns its one [`ConcertoError`], if
// any, into one [`Diagnostic`] ([`classify_error`]); only the recursive,
// class-shaped part of the tree — where TS-faithful first-error would stop
// the *whole* walk at the first nested object's first problem — is walked
// here directly, so that sibling properties and sibling array elements each
// get their own chance to report.

use crate::instance::diagnostic::{Diagnostic, DiagnosticCode, ValidationResult};

/// Checks that `value`'s own `$class` (when present) is assignable to
/// `declared_fqn`. [`visit_class_declaration`]/[`collect_class`] both walk
/// by `value`'s own `$class`, regardless of what `declared_fqn` says (module
/// doc): the right behaviour for `Resource.validate`, which always validates
/// a resource against its own type, but not for
/// [`ClassDeclaration::validate_instance`]/`validate_instance_or_throw`
/// (crate::introspect::declaration::ClassDeclaration), whose whole point is
/// to validate against the declaration they were called on. Returns `Ok(())`
/// when `value` carries no `$class` (or isn't shaped like a Resource at
/// all): the ordinary walk that follows already reports that case
/// correctly, so there's nothing extra to check here; likewise `Ok(())` when
/// `declared_fqn` itself is `value`'s own `$class`, so a
/// [`ModelManager::validate_instance_or_throw`](crate::model_manager::ModelManager::validate_instance_or_throw)
/// call (which always passes `value`'s own `$class` as `declared_fqn`) never
/// pays for this check.
fn check_assignable_to_declaration(
    mm: &ModelManager,
    declared_fqn: &str,
    value: &Value,
) -> Result<()> {
    let Some(own_fqn) = value
        .as_object()
        .and_then(|o| o.get("$class"))
        .and_then(Value::as_str)
    else {
        return Ok(());
    };
    if own_fqn == declared_fqn {
        return Ok(());
    }
    match mm.is_assignable_to(own_fqn, declared_fqn) {
        Ok(true) => Ok(()),
        Ok(false) => Err(ContractError::pre_port(
            ErrorKind::Validation,
            format!("'{own_fqn}' is not assignable to '{declared_fqn}'"),
            None,
        )
        .into()),
        Err(_) => Err(ContractError::type_not_found(
            "typenotfounderror-defaultmessage",
            Vec::new(),
            own_fqn.to_string(),
            None,
        )
        .into()),
    }
}

/// [`validate_instance_from`], but validated against `declared_fqn` instead
/// of `value`'s own `$class` — what [`ClassDeclaration::validate_instance_or_throw`]
/// (crate::introspect::declaration::ClassDeclaration::validate_instance_or_throw)
/// needs to validate `value` against a specific declaration it already holds,
/// rather than whatever `value` claims to be.
pub(crate) fn validate_instance_against(
    mm: &ModelManager,
    declared_fqn: &str,
    value: &Value,
    options: &ValidateOptions,
) -> Result<()> {
    check_assignable_to_declaration(mm, declared_fqn, value)?;
    let mut params = Params {
        mm,
        options,
        root_resource_identifier: String::new(),
        current_identifier: None,
    };
    visit_class_declaration(&mut params, declared_fqn, value)
}

/// State threaded through the collect-all walk: the pieces
/// [`Params`] threads through the first-error walk, minus the
/// TS-message-only `root_resource_identifier`/`current_identifier` fields
/// (module doc: a collect-all diagnostic's `message` reuses whatever a leaf
/// check already renders; the walk's own diagnostics carry their own short
/// description instead, since they have no ported TS message to match).
struct Collector<'a> {
    mm: &'a ModelManager,
    options: &'a ValidateOptions,
    diagnostics: Vec<Diagnostic>,
}

impl Collector<'_> {
    fn push(&mut self, pointer: String, code: DiagnosticCode, message: String) {
        self.diagnostics
            .push(Diagnostic::error(pointer, code, message));
    }

    fn push_error(&mut self, pointer: String, err: ConcertoError) {
        let (code, message) = classify_error(&err);
        self.push(pointer, code, message);
    }
}

/// Maps a [`ConcertoError`] a leaf check raised to the [`DiagnosticCode`] it
/// reports as, keeping the check's own rendered message. A code this table
/// does not recognise (a JS-engine-shaped error, PORTING.md 2.2 step 3, or a
/// future check this table has not been updated for) falls back to
/// [`DiagnosticCode::TypeViolation`], the closest general-purpose code, so a
/// diagnostic is always produced rather than silently dropped.
fn classify_error(err: &ConcertoError) -> (DiagnosticCode, String) {
    match err {
        ConcertoError::TypeNotFound { type_name } => (
            DiagnosticCode::TypeNotFound,
            format!("type not found: {type_name}"),
        ),
        ConcertoError::IllegalModel { message, .. } => {
            (DiagnosticCode::TypeViolation, message.clone())
        }
        ConcertoError::Contract(ce) => {
            let message = ce.message();
            if ce.validator.is_some() {
                return (DiagnosticCode::ValidatorFailure, message);
            }
            let code = match ce.code {
                "resourcevalidator-missingrequiredproperty" => {
                    DiagnosticCode::MissingRequiredProperty
                }
                "resourcevalidator-undeclaredfield" => DiagnosticCode::UndeclaredField,
                "resourcevalidator-emptyidentifier" => DiagnosticCode::EmptyIdentifier,
                "resourcevalidator-invalidenumvalue" => DiagnosticCode::InvalidEnumValue,
                "resourcevalidator-abstractclass" => DiagnosticCode::AbstractClass,
                "resourcevalidator-invalidfieldassignment" => DiagnosticCode::NotAssignable,
                "resourcevalidator-notresourceorconcept" => DiagnosticCode::NotResource,
                "resourcevalidator-notrelationship"
                | "resourcevalidator-checkrelationship-notidentifiable" => {
                    DiagnosticCode::NotRelationship
                }
                "typenotfounderror-defaultmessage" => DiagnosticCode::TypeNotFound,
                _ => DiagnosticCode::TypeViolation,
            };
            (code, message)
        }
    }
}

/// A JSON Pointer (RFC 6901) one segment deeper than `base`, escaping `~`
/// and `/` in `segment` as the spec requires.
fn push_pointer(base: &str, segment: &str) -> String {
    format!("{base}/{}", segment.replace('~', "~0").replace('/', "~1"))
}

/// Collect-all instance validation (task P3-03, accordproject/concerto-rust#58):
/// walks `value` against `declared_fqn` in `mm`, gathering every
/// [`Diagnostic`] found instead of stopping at the first one (contrast
/// [`validate_instance`], TS `Resource.validate`'s first-error walk).
pub(crate) fn collect_diagnostics(
    mm: &ModelManager,
    declared_fqn: &str,
    value: &Value,
    options: &ValidateOptions,
) -> ValidationResult {
    // Same declared-vs-own-`$class` check [`check_assignable_to_declaration`]
    // makes for the first-error walk: [`collect_class`] otherwise walks by
    // `value`'s own `$class` regardless of `declared_fqn` (its own doc
    // comment), so a `ClassDeclaration::validate_instance` call would
    // silently validate a mismatched type as if it matched. Built inline
    // (rather than through [`classify_error`]) so the diagnostic keeps the
    // same [`DiagnosticCode`] [`collect_class_property_item`]'s own
    // assignability check uses for the same kind of mismatch, one level
    // down the tree.
    if let Some(own_fqn) = value
        .as_object()
        .and_then(|o| o.get("$class"))
        .and_then(Value::as_str)
        && own_fqn != declared_fqn
    {
        match mm.is_assignable_to(own_fqn, declared_fqn) {
            Ok(true) => {}
            Ok(false) => {
                return ValidationResult::new(vec![Diagnostic::error(
                    String::new(),
                    DiagnosticCode::NotAssignable,
                    format!("'{own_fqn}' is not assignable to '{declared_fqn}'"),
                )]);
            }
            Err(_) => {
                return ValidationResult::new(vec![Diagnostic::error(
                    String::new(),
                    DiagnosticCode::TypeNotFound,
                    format!("type not found: {own_fqn}"),
                )]);
            }
        }
    }
    let mut collector = Collector {
        mm,
        options,
        diagnostics: Vec::new(),
    };
    collect_class(&mut collector, declared_fqn, value, "");
    ValidationResult::new(collector.diagnostics)
}

/// [`collect_diagnostics`], resolving the declared type from `value`'s own
/// `$class`, the way [`validate_instance`] does for a root call.
pub(crate) fn collect_diagnostics_from_value(
    mm: &ModelManager,
    value: &Value,
    options: &ValidateOptions,
) -> ValidationResult {
    let Some(fqn) = value.get("$class").and_then(Value::as_str) else {
        return ValidationResult::new(vec![Diagnostic::error(
            String::new(),
            DiagnosticCode::NotResource,
            "cannot validate an instance with no $class".to_string(),
        )]);
    };
    collect_diagnostics(mm, fqn, value, options)
}

/// The collect-all counterpart of [`visit_class_declaration`]: same shape
/// (undeclared fields, abstractness, identity, own properties), but records
/// a [`Diagnostic`] and keeps going at every point [`visit_class_declaration`]
/// would return `Err` and stop.
fn collect_class(c: &mut Collector, declared_fqn: &str, value: &Value, pointer: &str) {
    let Some(obj) = as_js_object(value).filter(|o| !o.contains_key(RELATIONSHIP_TAG)) else {
        c.push(
            pointer.to_string(),
            DiagnosticCode::NotResource,
            format!(
                "expected a Resource at '{pointer}', found {}",
                js_to_string(value)
            ),
        );
        return;
    };
    let Some(own_fqn) = obj.get("$class").and_then(Value::as_str) else {
        c.push(
            pointer.to_string(),
            DiagnosticCode::NotResource,
            format!("expected a Resource with a $class at '{pointer}'"),
        );
        return;
    };
    let own_fqn = own_fqn.to_string();

    let to_be_assigned = match c.mm.get_declaration(&own_fqn) {
        Ok(d) => d,
        Err(_) => {
            c.push(
                pointer.to_string(),
                DiagnosticCode::TypeNotFound,
                format!("type not found: {own_fqn}"),
            );
            return;
        }
    };
    let Some(class) = to_be_assigned.as_class() else {
        c.push(
            pointer.to_string(),
            DiagnosticCode::NotResource,
            format!("'{own_fqn}' is not a class-like type and cannot back a Resource"),
        );
        return;
    };
    if class.is_abstract() {
        c.push(
            pointer.to_string(),
            DiagnosticCode::AbstractClass,
            format!("The class \"{own_fqn}\" is abstract and should not contain an instance."),
        );
    }

    let Ok(all_properties) = c.mm.get_all_properties(&own_fqn) else {
        c.push(
            pointer.to_string(),
            DiagnosticCode::TypeNotFound,
            format!("could not resolve the properties of '{own_fqn}'"),
        );
        return;
    };

    for key in obj.keys() {
        if model_util::is_system_property(key) {
            continue;
        }
        if all_properties.iter().any(|(_, prop)| prop.name() == key) {
            continue;
        }
        c.push(
            push_pointer(pointer, key),
            DiagnosticCode::UndeclaredField,
            format!("undeclared field '{key}' on '{own_fqn}'"),
        );
    }

    if c.mm.is_identified(declared_fqn).unwrap_or(false) {
        let id_field =
            c.mm.identifier_field_name(&own_fqn)
                .ok()
                .flatten()
                .unwrap_or_else(|| "$identifier".to_string());
        let id = obj.get(&id_field).and_then(Value::as_str).unwrap_or("");
        if id.trim().is_empty() {
            c.push(
                pointer.to_string(),
                DiagnosticCode::EmptyIdentifier,
                "an identifier must be provided".to_string(),
            );
        }
    }

    // `$identifier` is appended to every class's own properties whether or
    // not the type actually uses system identification (module doc on
    // `ClassDeclaration`), so a required, absent `$identifier` on an
    // explicitly-identified type (`identifier_field_name` names its own
    // field instead) is not really missing — [`visit_class_declaration`]
    // skips it the same way.
    let own_identifier_field_name = c.mm.identifier_field_name(&own_fqn).ok().flatten();
    for (owner_fqn, property) in &all_properties {
        let prop_pointer = push_pointer(pointer, property.name());
        match obj.get(property.name()) {
            Some(v) if !is_js_null(v) => {
                collect_property(c, owner_fqn, property, v, &prop_pointer);
            }
            _ => {
                if property.name() == "$identifier"
                    && own_identifier_field_name.as_deref() != Some("$identifier")
                {
                    continue;
                }
                if !property.is_optional() && !property_has_default_value(property) {
                    c.push(
                        prop_pointer,
                        DiagnosticCode::MissingRequiredProperty,
                        format!("the required field '{}' has not been set", property.name()),
                    );
                }
            }
        }
    }
}

/// The collect-all counterpart of [`visit_property`]/[`visit_field`]. A
/// class-typed `Object` property is the one case that recurses here directly
/// (so a nested object's own several problems are all collected, and so are
/// its siblings'); every other kind delegates to [`validate_property_value`],
/// the existing first-error check, since collect-all does not need to
/// distinguish *which* primitive/scalar/enum/relationship/map/validator
/// check failed within one field's own value, only *that* it did.
fn collect_property(
    c: &mut Collector,
    owner_fqn: &str,
    property: &Property,
    value: &Value,
    pointer: &str,
) {
    let class_target = match property {
        Property::Object(op) => match resolve_object_target(c.mm, owner_fqn, &op.type_) {
            Ok(ObjectTarget::Class(fqn)) => Some(fqn),
            _ => None,
        },
        _ => None,
    };

    let Some(class_fqn) = class_target else {
        if let Err(e) =
            validate_property_value(c.mm, owner_fqn, property, value, String::new(), c.options)
        {
            c.push_error(pointer.to_string(), e);
        }
        return;
    };

    if property.is_array() {
        let Some(items) = value.as_array() else {
            c.push(
                pointer.to_string(),
                DiagnosticCode::TypeViolation,
                format!("expected an array for field '{}'", property.name()),
            );
            return;
        };
        // Mirrors `check_array`'s size check (the first-error walk): a
        // class-typed array property can carry a `sizeValidator` too, and
        // collect-all must not silently skip it just because it recurses
        // into elements directly instead of going through
        // `validate_property_value`.
        if let Some(sv) = property.size_validator() {
            let elem = FieldElement::new(c.mm, owner_fqn, property);
            if let Ok(validator) = CollectionSizeValidator::new(&elem, sv)
                && let Err(e) = validator.validate(&elem, None, items.len() as f64)
            {
                c.push_error(pointer.to_string(), e);
            }
        }
        for (i, item) in items.iter().enumerate() {
            collect_class_property_item(c, &class_fqn, item, &format!("{pointer}/{i}"));
        }
        return;
    }

    collect_class_property_item(c, &class_fqn, value, pointer);
}

/// One value behind a class-typed `Object` property (or one of its array
/// elements): checks assignability, then recurses with
/// [`collect_class`] so the nested object's own diagnostics are collected
/// too.
fn collect_class_property_item(
    c: &mut Collector,
    declared_class_fqn: &str,
    value: &Value,
    pointer: &str,
) {
    if is_js_undefined(value) {
        c.push(
            pointer.to_string(),
            DiagnosticCode::TypeViolation,
            "value is undefined".to_string(),
        );
        return;
    }
    if let Some(own_fqn) = value
        .as_object()
        .and_then(|o| o.get("$class"))
        .and_then(Value::as_str)
    {
        match c.mm.is_assignable_to(own_fqn, declared_class_fqn) {
            Ok(true) => {}
            Ok(false) => {
                c.push(
                    pointer.to_string(),
                    DiagnosticCode::NotAssignable,
                    format!("'{own_fqn}' is not assignable to '{declared_class_fqn}'"),
                );
                return;
            }
            Err(_) => {
                c.push(
                    pointer.to_string(),
                    DiagnosticCode::TypeNotFound,
                    format!("type not found: {own_fqn}"),
                );
                return;
            }
        }
    }
    collect_class(c, declared_class_fqn, value, pointer);
}

#[cfg(test)]
mod tests {
    //! Exercises the confirmed `concerto-validate-rs` bug fixes (module doc),
    //! plus full type support: Long, DateTime, relationships, enums, maps and
    //! scalars, matching `ResourceValidator`'s checks and messages
    //! (`resourcevalidator.ts`, verified against its golden tests in
    //! `error/mod.rs`).

    use super::*;
    use crate::model_manager::ModelManager;
    use serde_json::json;

    /// One model exercising every kind this validator supports: multi-level
    /// inheritance (`Base` -> `Mid` -> `Leaf`, for the bug #1 fix), an
    /// abstract concept (`Animal`, bug #2), enums, relationships, maps
    /// (`String`, `DateTime`, `Boolean`, enum and object/relationship
    /// values), a regex-validated scalar, and Integer/Long/DateTime/String
    /// fields with validators.
    fn fixture() -> ModelManager {
        let mut mgr = ModelManager::new().unwrap();
        mgr.add_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Animal",
                      "isAbstract": true,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "name", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Dog",
                      "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Animal" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "breed", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Base",
                      "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "a", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Mid",
                      "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Base" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "b", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Leaf",
                      "isAbstract": false,
                      "superType": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Mid" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "c", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.EnumDeclaration", "name": "Color",
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "RED" },
                        { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "GREEN" },
                        { "$class": "concerto.metamodel@1.0.0.EnumProperty", "name": "BLUE" }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "Vehicle",
                      "isAbstract": false,
                      "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "vin" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "vin", "isArray": false, "isOptional": false },
                        { "$class": "concerto.metamodel@1.0.0.LongProperty", "name": "mileage", "isArray": false, "isOptional": false },
                        { "$class": "concerto.metamodel@1.0.0.DateTimeProperty", "name": "purchasedAt", "isArray": false, "isOptional": true },
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "color", "isArray": false, "isOptional": true,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Color" } },
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "pet", "isArray": false, "isOptional": true,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Animal" } },
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "tags", "isArray": true, "isOptional": true,
                          "sizeValidator": { "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator", "minSize": 1, "maxSize": 3 } },
                        { "$class": "concerto.metamodel@1.0.0.IntegerProperty", "name": "rating", "isArray": false, "isOptional": true,
                          "validator": { "$class": "concerto.metamodel@1.0.0.IntegerDomainValidator", "lower": 0, "upper": 5 } },
                        // Array-of-enum (P5-06: `check_enum`'s own
                        // `property.is_array()` guard, at the top of the
                        // function, had no test with an *array* enum
                        // property anywhere in this fixture — every
                        // existing enum test uses `color`, which is not an
                        // array).
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "colors", "isArray": true, "isOptional": true,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Color" } },
                        // A String field with its own (non-scalar)
                        // `validator`, no `length_validator` alongside it
                        // (P5-06: `check_primitive_item`'s `sp.validator
                        // .is_some() || sp.length_validator.is_some()`
                        // guard — every existing `String` field, including
                        // `vin`, carries neither, and the only other
                        // `StringValidator` exercise in this module is via
                        // a scalar, `VIN`, which runs through
                        // `check_scalar_item`, not `check_primitive_item`,
                        // so a validator on the *property* itself, alone,
                        // was never reached).
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "code", "isArray": false, "isOptional": true,
                          "validator": { "$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": "^[A-Z]+$", "flags": "" } },
                        // No `Double` property existed anywhere in this
                        // fixture (P5-06: cargo-mutants found
                        // `check_primitive_item`'s `Property::Double(dp)`
                        // match arm survived — deleting it falls through to
                        // the trailing `_ => unreachable!()`, which nothing
                        // exercised).
                        { "$class": "concerto.metamodel@1.0.0.DoubleProperty", "name": "weight", "isArray": false, "isOptional": true }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.AssetDeclaration", "name": "Owner",
                      "isAbstract": false,
                      "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "ownerId" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "ownerId", "isArray": false, "isOptional": false },
                        { "$class": "concerto.metamodel@1.0.0.RelationshipProperty", "name": "vehicle", "isArray": false, "isOptional": true,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Vehicle" } },
                        { "$class": "concerto.metamodel@1.0.0.RelationshipProperty", "name": "vehicles", "isArray": true, "isOptional": true,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Vehicle" } }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.StringScalar", "name": "VIN",
                      "validator": { "$class": "concerto.metamodel@1.0.0.StringRegexValidator", "pattern": "^[A-Z0-9]{5}$", "flags": "" } },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Item",
                      "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "name", "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Garage",
                      "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "vinField", "isArray": false, "isOptional": false,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "VIN" } },
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "items", "isArray": true, "isOptional": true,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Item" },
                          "sizeValidator": { "$class": "concerto.metamodel@1.0.0.CollectionSizeValidator", "minSize": 1, "maxSize": 2 } }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "StringMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.StringMapValueType" } },
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "ColorMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.ObjectMapValueType",
                                 "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Color" } } },
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "ItemMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.ObjectMapValueType",
                                 "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Item" } } },
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "VehicleMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.RelationshipMapValueType",
                                 "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Vehicle" } } },
                    // A scalar-typed map KEY (P5-06: `map_key_is_scalar` had
                    // no fixture where the map key itself is an
                    // `ObjectMapKeyType` referencing a scalar — every other
                    // map here keys on a plain `StringMapKeyType`, so
                    // `map_key_is_scalar` always took its early `key_kind()
                    // != "ObjectMapKeyType"` return and its real
                    // scalar-resolution logic was never reached).
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "ScalarKeyMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.ObjectMapKeyType",
                               "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "VIN" } },
                      "value": { "$class": "concerto.metamodel@1.0.0.ObjectMapValueType",
                                 "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "VIN" } } },
                    // A scalar-typed map VALUE whose KEY is *not* scalar
                    // (paired with `ScalarKeyMap` above): `checkMapType`
                    // reads `ModelUtil.isScalar(mapDeclaration.getKey())`
                    // unconditionally, even while validating the value slot
                    // (module doc, `map_key_is_scalar`'s own doc) — so this
                    // map's value type is never type-checked at all, by
                    // design (a faithful port of that quirk, not a bug).
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "PlainKeyScalarValueMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.ObjectMapValueType",
                                 "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "VIN" } } },
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "DateTimeMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.DateTimeMapValueType" } },
                    { "$class": "concerto.metamodel@1.0.0.MapDeclaration", "name": "BooleanMap",
                      "key": { "$class": "concerto.metamodel@1.0.0.StringMapKeyType" },
                      "value": { "$class": "concerto.metamodel@1.0.0.BooleanMapValueType" } }
                ]
            }),
            None,
        )
        .unwrap();
        mgr
    }

    fn err_of(result: Result<()>) -> ConcertoError {
        result.expect_err("expected a validation failure")
    }

    // ---- Bug #1: only the direct super type's properties were merged ----

    #[test]
    fn a_three_level_inherited_field_is_recognised() {
        let mgr = fixture();
        let leaf = json!({ "$class": "org.acme@1.0.0.Leaf", "a": "1", "b": "2", "c": "3" });
        validate_instance(&mgr, &leaf, &ValidateOptions::default())
            .expect("all three levels' fields are known");
    }

    #[test]
    fn a_missing_field_from_the_grandparent_type_is_still_caught() {
        let mgr = fixture();
        // `a` (declared on `Base`, two levels up from `Leaf`) is missing.
        let leaf = json!({ "$class": "org.acme@1.0.0.Leaf", "b": "2", "c": "3" });
        let err = err_of(validate_instance(&mgr, &leaf, &ValidateOptions::default()));
        assert!(err.to_string().contains("\"a\""), "{err}");
    }

    // ---- Bug #2: abstract and nested $class values were not checked ----

    #[test]
    fn an_abstract_class_at_the_root_is_rejected() {
        let mgr = fixture();
        let animal = json!({ "$class": "org.acme@1.0.0.Animal", "name": "Rex" });
        let err = err_of(validate_instance(
            &mgr,
            &animal,
            &ValidateOptions::default(),
        ));
        assert_eq!(
            err.to_string(),
            "The class \"org.acme@1.0.0.Animal\" is abstract and should not contain an instance."
        );
    }

    #[test]
    fn an_abstract_class_nested_inside_another_resource_is_also_rejected() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 100,
            // `pet`'s declared type is `Animal`; assigning `Animal` itself
            // (not a concrete subtype like `Dog`) must be rejected exactly
            // as it would be at the root.
            "pet": { "$class": "org.acme@1.0.0.Animal", "name": "Rex" }
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(
            err.to_string(),
            "The class \"org.acme@1.0.0.Animal\" is abstract and should not contain an instance."
        );
    }

    #[test]
    fn a_concrete_subtype_nested_inside_another_resource_passes() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 100,
            "pet": { "$class": "org.acme@1.0.0.Dog", "name": "Rex", "breed": "Lab" }
        });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    // ---- Undeclared / missing / empty identifier ----

    #[test]
    fn an_undeclared_field_is_rejected() {
        let mgr = fixture();
        let leaf =
            json!({ "$class": "org.acme@1.0.0.Leaf", "a": "1", "b": "2", "c": "3", "d": "nope" });
        let err = err_of(validate_instance(&mgr, &leaf, &ValidateOptions::default()));
        assert!(err.to_string().contains("\"d\""), "{err}");
        assert!(err.to_string().contains("org.acme@1.0.0.Leaf"), "{err}");
    }

    /// [`js_id_display`] (P5-06: cargo-mutants found this return value was
    /// never asserted): `Leaf` above is not identified, so
    /// [`an_undeclared_field_is_rejected`] reaches `undeclared_field`
    /// through the *other* branch (`p.current_identifier`), never through
    /// `js_id_display`. `Vehicle` is identified (by `vin`), so an undeclared
    /// field on a `Vehicle` instance whose own `vin` is absent hits
    /// `js_id_display(None)`, which is the JS `${undefined}` literal
    /// `"undefined"` — never an empty string — as the reported resource id.
    #[test]
    fn an_undeclared_field_on_an_identified_resource_with_no_identifier_value_reports_undefined() {
        let mgr = fixture();
        let vehicle = json!({ "$class": "org.acme@1.0.0.Vehicle", "mileage": 5, "extra": "nope" });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(
            err.to_string().contains("\"undefined\""),
            "expected the undefined-identifier placeholder, got: {err}"
        );
        assert!(err.to_string().contains("\"extra\""), "{err}");
    }

    /// [`visit_class_declaration`]'s undeclared-field `resource_id`
    /// computation (P5-06: cargo-mutants found the `&&`->`||` and
    /// `!=`->`==` mutants at this line both survived). `key !=
    /// "$identifier"` is, on its own, always true at this point (an actual
    /// `"$identifier"` key is filtered out as a system property earlier in
    /// the same loop, so this branch never reaches it) — the `!=` mutant's
    /// `==` is thus always false there. Both mutants are made observable by
    /// giving the `&&`'s *other* operand (`declared_is_identified`) a true
    /// and a false case with genuinely different `resource_id` outputs:
    /// `inner` is nested inside an already-identified `Outer` (so
    /// `p.current_identifier` is `Some("Outer#O1")` by the time it is
    /// visited) but is itself declared as the *identified* `Inner`, so the
    /// real `js_id_display`-based id (`"I1"`) differs from the `&&`/`==`
    /// mutants' `current_identifier` fallback (`"Outer#O1"`); `loose` is
    /// declared as the *unidentified* `Loose` and visited after `inner`
    /// (properties are walked in declaration order), so by then the real
    /// fallback is `p.current_identifier` as `inner` itself left it,
    /// `"Inner#I1"` (every *identified* object visited overwrites it, `inner`
    /// included — not only `Outer`) — still a value the `||` mutant's
    /// wrongly-taken `js_id_display` branch cannot produce (`"undefined"`,
    /// since an unidentified type has
    /// no identifier field to read).
    #[test]
    fn an_undeclared_field_s_reported_resource_id_depends_on_whether_the_declared_type_is_identified()
     {
        let mut mgr = ModelManager::new().unwrap();
        mgr.add_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.nest@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Inner",
                      "isAbstract": false,
                      "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "iid" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "iid",
                          "isArray": false, "isOptional": false }
                      ] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Loose",
                      "isAbstract": false, "properties": [] },
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Outer",
                      "isAbstract": false,
                      "identified": { "$class": "concerto.metamodel@1.0.0.IdentifiedBy", "name": "oid" },
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "oid",
                          "isArray": false, "isOptional": false },
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "inner",
                          "isArray": false, "isOptional": false,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Inner" } },
                        { "$class": "concerto.metamodel@1.0.0.ObjectProperty", "name": "loose",
                          "isArray": false, "isOptional": false,
                          "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "Loose" } }
                      ] }
                ]
            }),
            None,
        )
        .unwrap();

        let with_bad_inner = json!({
            "$class": "org.nest@1.0.0.Outer", "oid": "O1",
            "inner": { "$class": "org.nest@1.0.0.Inner", "iid": "I1", "bogus": "nope" },
            "loose": { "$class": "org.nest@1.0.0.Loose" }
        });
        let err = err_of(validate_instance(
            &mgr,
            &with_bad_inner,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("\"I1\""), "{err}");

        // `p.current_identifier` is set (and left set) by every *identified*
        // object the walk visits, `inner` included — since properties are
        // visited in declaration order (`oid`, `inner`, `loose`) and `inner`
        // is processed first and is itself identified, it is `"Inner#I1"`,
        // not `"Outer#O1"`, that is on `p.current_identifier` by the time
        // `loose` is reached below.
        let with_bad_loose = json!({
            "$class": "org.nest@1.0.0.Outer", "oid": "O1",
            "inner": { "$class": "org.nest@1.0.0.Inner", "iid": "I1" },
            "loose": { "$class": "org.nest@1.0.0.Loose", "bogus2": "nope" }
        });
        let err = err_of(validate_instance(
            &mgr,
            &with_bad_loose,
            &ValidateOptions::default(),
        ));
        assert!(
            err.to_string().contains("\"org.nest@1.0.0.Inner#I1\""),
            "{err}"
        );
    }

    #[test]
    fn a_missing_required_property_is_rejected() {
        let mgr = fixture();
        let vehicle = json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12" });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(
            err.to_string(),
            "The instance \"org.acme@1.0.0.Vehicle#ABC12\" is missing the required field \"mileage\"."
        );
    }

    #[test]
    fn an_empty_identifier_is_rejected() {
        let mgr = fixture();
        let vehicle = json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "  ", "mileage": 1 });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("empty identifier"), "{err}");
    }

    // ---- Long, DateTime, String, Boolean primitives ----

    #[test]
    fn a_valid_long_and_datetime_pass() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 9_007_199_254_740_991_i64,
            "purchasedAt": { "$$dayjs": "2020-01-01T00:00:00.000Z" }
        });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    /// A `Dayjs` instance is still a `Dayjs` instance even when the string it
    /// was built from was nonsense (module doc "Scope", [`DAYJS_TAG`]'s
    /// doc): TS's `checkItem` never re-validates it, so this passes.
    #[test]
    fn a_populated_datetime_passes_even_when_its_own_string_is_nonsense() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "purchasedAt": { "$$dayjs": "not-a-date" }
        });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    /// A raw (un-coerced) string on a `DateTime` field is always a field
    /// type violation post-population (module doc "Scope"): TS's
    /// `JSONPopulator` is what turns a wire string into a `Dayjs`, and
    /// `ResourceValidator` only ever sees the result.
    #[test]
    fn an_uncoerced_string_on_a_datetime_field_is_a_field_type_violation() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "purchasedAt": "2020-01-01T00:00:00.000Z"
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("\"purchasedAt\""), "{err}");
    }

    #[test]
    fn a_string_value_for_a_long_field_is_a_field_type_violation() {
        let mgr = fixture();
        let vehicle =
            json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": "far" });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("\"mileage\""), "{err}");
        assert!(
            err.to_string().contains("Expected type of value: \"Long\""),
            "{err}"
        );
    }

    #[test]
    fn a_non_object_value_on_a_datetime_field_is_rejected() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "purchasedAt": "not-a-date"
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("\"purchasedAt\""), "{err}");
    }

    // ---- Enums ----

    #[test]
    fn a_valid_enum_value_passes() {
        let mgr = fixture();
        let vehicle = json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1, "color": "RED" });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    /// TS passes the enum declaration as `reportInvalidEnumValue`'s
    /// `field`, so the message names the enum type (`Color`), not the
    /// property (`color`); the corpus records the same wording (for example
    /// `Invalid enum value of "Purple" for the field "Color".`).
    #[test]
    fn an_invalid_enum_value_is_rejected() {
        let mgr = fixture();
        let vehicle = json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1, "color": "PURPLE" });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(
            err.to_string(),
            "Model violation in the \"org.acme@1.0.0.Vehicle#ABC12\" instance. Invalid enum value of \"PURPLE\" for the field \"Color\"."
        );
    }

    // ---- Relationships ----

    #[test]
    fn a_populated_relationship_to_an_identified_type_passes() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicle": { "$$relationship": true, "$class": "org.acme@1.0.0.Vehicle" }
        });
        validate_instance(&mgr, &owner, &ValidateOptions::default()).unwrap();
    }

    /// A raw wire URI string on a relationship field is a field type
    /// violation post-population, the same way an un-coerced `DateTime`
    /// string is (module doc "Scope"): `JSONPopulator.visitRelationshipDeclaration`
    /// is what turns a URI string into a `Relationship`, via
    /// `Relationship.fromURI`; `ResourceValidator` only ever sees the
    /// result.
    #[test]
    fn an_uncoerced_relationship_uri_string_is_rejected() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicle": "resource:org.acme@1.0.0.Vehicle#ABC12"
        });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        assert!(
            err.to_string().contains("Expected a \"Relationship\""),
            "{err}"
        );
    }

    #[test]
    fn a_plain_string_that_is_not_a_relationship_is_rejected() {
        let mgr = fixture();
        let owner =
            json!({ "$class": "org.acme@1.0.0.Owner", "ownerId": "O1", "vehicle": "not a uri" });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        assert!(
            err.to_string().contains("Expected a \"Relationship\""),
            "{err}"
        );
    }

    // ---- Maps: String/DateTime/Boolean values, and the enum/relationship
    //      fix. A Map is never itself a top-level `validate_instance` entry
    //      in TS (only a `Resource` is: module doc "Scope"), so these call
    //      the internal `visit_map_declaration` directly, exactly the way a
    //      `Vehicle`-typed field pointing at a `MapDeclaration` would reach
    //      it (`Kind::MapTyped`, `check_item`). ----

    fn validate_map(mgr: &ModelManager, map_fqn: &str, value: &Value) -> Result<()> {
        let options = ValidateOptions::default();
        let mut params = Params {
            mm: mgr,
            options: &options,
            root_resource_identifier: String::new(),
            current_identifier: None,
        };
        visit_map_declaration(&mut params, map_fqn, value)
    }

    #[test]
    fn a_string_map_with_string_values_passes() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!("1")), (json!("b"), json!("2"))]);
        validate_map(&mgr, "org.acme@1.0.0.StringMap", &map).unwrap();
    }

    #[test]
    fn a_string_map_with_a_non_string_value_is_rejected() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!(1))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.StringMap", &map));
        assert!(err.to_string().contains("Expected Type of String"), "{err}");
    }

    /// Task P3-01b: a `Map` key keeps its JS type, so a number key of a
    /// `String`-keyed map is reported (`found '1234'`).
    #[test]
    fn a_string_map_with_a_number_key_is_rejected() {
        let mgr = fixture();
        let map = js_map(vec![(json!(1234), json!("Lorem"))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.StringMap", &map));
        assert!(err.to_string().contains("but found '1234'"), "{err}");
    }

    /// Task P3-01b: a key spelt like the `undefined` marker is an ordinary
    /// key, not a JS `undefined`.
    #[test]
    fn a_map_key_spelt_like_the_undefined_marker_is_an_ordinary_key() {
        let mgr = fixture();
        let map = js_map(vec![(json!(UNDEFINED_TAG), json!("x"))]);
        validate_map(&mgr, "org.acme@1.0.0.StringMap", &map).unwrap();
    }

    /// Task P3-01b: `obj instanceof Map` — a plain object is not a `Map`.
    #[test]
    fn a_plain_object_is_not_a_map() {
        let mgr = fixture();
        let err = err_of(validate_map(
            &mgr,
            "org.acme@1.0.0.StringMap",
            &json!({ "a": "1" }),
        ));
        assert!(
            err.to_string()
                .contains("Expected a Map, but found {\"a\":\"1\"}"),
            "{err}"
        );
    }

    /// Task P3-01b: `dayjs.utc(undefined)` is the current time, so an
    /// `undefined` value of a `DateTime` map passes.
    #[test]
    fn an_undefined_datetime_map_value_passes() {
        assert!(parses_as_dayjs(&js_undefined()));
    }

    /// [`parses_as_dayjs`]'s `Value::Number` arm (P5-06: cargo-mutants found
    /// that arm's deletion survived): a finite number is a valid `DateTime`
    /// map value. Deleting the arm falls through to the wildcard `false`.
    #[test]
    fn a_finite_number_is_a_valid_datetime_map_value() {
        assert!(parses_as_dayjs(&json!(1_700_000_000_000.0)));
    }

    /// [`parses_as_dayjs`]'s `Value::String` arm (P5-06: cargo-mutants found
    /// that arm's deletion survived, the same way as the `Number` arm
    /// above): a four-digit-year string is a valid `DateTime` map value.
    #[test]
    fn a_four_digit_year_string_is_a_valid_datetime_map_value() {
        assert!(parses_as_dayjs(&json!("2024-05-01")));
    }

    /// [`parses_as_dayjs`]'s wildcard arm (P5-06: cargo-mutants found the
    /// whole function's body replaced with a constant `true` surviving): a
    /// value that is none of undefined, a number or a string is never a
    /// valid `DateTime` map value.
    #[test]
    fn a_boolean_is_not_a_valid_datetime_map_value() {
        assert!(!parses_as_dayjs(&json!(true)));
    }

    /// Task P3-01b: a non-finite number is printed with `toString()`.
    #[test]
    fn a_non_finite_number_is_printed_with_to_string() {
        assert_eq!(
            field_value_param(&js_special_number("-Infinity")),
            "-Infinity"
        );
        assert_eq!(js_typeof(&js_special_number("NaN")), "number");
    }

    /// Bug fix (plan §1.2 gap list): a map value whose declared type
    /// resolves to an enum used to be wrongly rejected.
    #[test]
    fn a_map_with_a_valid_enum_value_passes() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!("RED"))]);
        validate_map(&mgr, "org.acme@1.0.0.ColorMap", &map).unwrap();
    }

    #[test]
    fn a_map_with_an_invalid_enum_value_is_rejected() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!("PURPLE"))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.ColorMap", &map));
        assert!(err.to_string().contains("Invalid enum value"), "{err}");
    }

    /// Bug fix (plan §1.2 gap list): a `RelationshipMapValueType` map value
    /// used to be wrongly rejected. Ported faithfully (module doc "Scope"
    /// on `check_map_type`): TS itself validates it as a nested object, not
    /// a relationship URI.
    #[test]
    fn a_map_with_a_relationship_typed_value_accepts_a_nested_resource() {
        let mgr = fixture();
        let map = js_map(vec![(
            json!("a"),
            json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1 }),
        )]);
        validate_map(&mgr, "org.acme@1.0.0.VehicleMap", &map).unwrap();
    }

    /// Bug fix, accordproject/concerto-rust#194: a map value whose own
    /// `$class` does not resolve to a type is a `ValidationException`
    /// ("not a Resource"), not a `TypeNotFoundException` from re-resolving
    /// that `$class`. `JSONPopulator.processMapType`'s `try`/`catch` (the
    /// only place TS swallows a `getType` failure) leaves such a value
    /// exactly as parsed — never a `Resource` — so `obj instanceof
    /// Resource` is false in TS before it ever looks at the value's own
    /// `$class` again.
    #[test]
    fn a_map_value_with_an_unresolvable_class_is_rejected_as_not_a_resource() {
        let mgr = fixture();
        let map = js_map(vec![(
            json!("a"),
            json!({ "$class": "org.acme@1.0.0.Missing", "name": "x" }),
        )]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.ItemMap", &map));
        assert!(matches!(&err, ConcertoError::Contract(e) if e.kind == ErrorKind::Validation));
        let message = err.to_string();
        assert!(
            message.contains("Expected a \"Resource\" or a \"Concept\""),
            "{message}"
        );
        assert!(message.contains("org.acme@1.0.0.Item"), "{message}");
        assert!(!message.contains("Missing"), "{message}");
    }

    /// [`map_key_is_scalar`] (P5-06: never reached beyond its own early
    /// `key_kind() != "ObjectMapKeyType"` return — every other map fixture
    /// keys on a plain `StringMapKeyType`). `ScalarKeyMap` keys *and*
    /// values on `VIN` (a `StringScalar`): a real scalar key makes
    /// `checkMapType` substitute the scalar's own underlying type
    /// (`String`) for the value check.
    #[test]
    fn a_scalar_keyed_map_with_a_string_value_passes() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!("ABC12"))]);
        validate_map(&mgr, "org.acme@1.0.0.ScalarKeyMap", &map).unwrap();
    }

    #[test]
    fn a_scalar_keyed_map_with_a_non_string_value_is_rejected() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!(12345))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.ScalarKeyMap", &map));
        assert!(err.to_string().contains("Expected Type of String"), "{err}");
    }

    /// The other half of `map_key_is_scalar`'s pairing: `checkMapType`
    /// reads the *key*'s scalar-ness even while checking the *value* slot
    /// (module doc "Scope", `map_key_is_scalar`'s own doc) — a faithfully
    /// ported quirk, not a bug. `PlainKeyScalarValueMap` has a scalar
    /// (`VIN`) value type but a plain `StringMapKeyType` key, so the value
    /// is never type-checked at all: even a value of the wrong JS type
    /// passes.
    #[test]
    fn a_scalar_valued_map_with_a_non_scalar_key_skips_value_type_checking() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!(12345))]);
        validate_map(&mgr, "org.acme@1.0.0.PlainKeyScalarValueMap", &map)
            .expect("checkMapType only consults the key's scalar-ness, so an untyped value passes");
    }

    /// `checkMapType`'s `DateTime` primitive-kind arm (P5-06: cargo-mutants
    /// found its `!parses_as_dayjs(value)` guard survived every mutation —
    /// `parses_as_dayjs` itself was unit-tested directly, but no fixture
    /// had a `DateTimeMapValueType` map to reach this guard through
    /// `check_map_type` itself).
    #[test]
    fn a_datetime_map_with_a_parseable_value_passes() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!("2024-05-01"))]);
        validate_map(&mgr, "org.acme@1.0.0.DateTimeMap", &map).unwrap();
    }

    #[test]
    fn a_datetime_map_with_an_unparseable_value_is_rejected() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!(true))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.DateTimeMap", &map));
        assert!(
            err.to_string().contains("Expected Type of DateTime"),
            "{err}"
        );
    }

    /// `checkMapType`'s `Boolean` primitive-kind arm (P5-06: same gap as
    /// the `DateTime` arm above — no `BooleanMapValueType` map fixture
    /// existed to reach it).
    #[test]
    fn a_boolean_map_with_a_boolean_value_passes() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!(true))]);
        validate_map(&mgr, "org.acme@1.0.0.BooleanMap", &map).unwrap();
    }

    #[test]
    fn a_boolean_map_with_a_non_boolean_value_is_rejected() {
        let mgr = fixture();
        let map = js_map(vec![(json!("a"), json!("nope"))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.BooleanMap", &map));
        assert!(
            err.to_string().contains("Expected Type of Boolean"),
            "{err}"
        );
    }

    // ---- Scalars ----

    #[test]
    fn a_scalar_field_matching_its_regex_passes() {
        let mgr = fixture();
        let garage = json!({ "$class": "org.acme@1.0.0.Garage", "vinField": "ABC12" });
        validate_instance(&mgr, &garage, &ValidateOptions::default()).unwrap();
    }

    #[test]
    fn a_scalar_field_violating_its_regex_is_rejected() {
        let mgr = fixture();
        let garage = json!({ "$class": "org.acme@1.0.0.Garage", "vinField": "not-valid!" });
        let err = err_of(validate_instance(
            &mgr,
            &garage,
            &ValidateOptions::default(),
        ));
        assert!(
            err.to_string().contains("failed to match validation regex"),
            "{err}"
        );
    }

    // ---- Size and numeric domain validators ----

    #[test]
    fn an_array_field_within_its_size_bounds_passes() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "tags": ["a", "b"]
        });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    #[test]
    fn an_array_field_over_its_max_size_is_rejected() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "tags": ["a", "b", "c", "d"]
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("no more than 3"), "{err}");
    }

    #[test]
    fn a_numeric_field_outside_its_domain_is_rejected() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "rating": 9
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("upper bound"), "{err}");
    }

    /// [`check_enum`]'s `property.is_array() && !value.is_array()` guard
    /// (P5-06: cargo-mutants found the `&&`->`||` and `delete !` mutants at
    /// this line survived): a valid *array* of enum values, which no
    /// existing test builds (every other enum test uses the non-array
    /// `color`). Under either mutant, `property.is_array()` (`true`) alone
    /// already makes the guard true, wrongly reporting a field type
    /// violation on this well-formed array.
    #[test]
    fn an_array_of_valid_enum_values_passes() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "colors": ["RED", "GREEN"]
        });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    /// [`check_primitive_item`]'s `sp.validator.is_some() ||
    /// sp.length_validator.is_some()` guard (P5-06: cargo-mutants found the
    /// `||`->`&&` mutant survived): `code` carries a `validator` but no
    /// `length_validator`, so the real `||` runs `StringValidator` (and
    /// rejects a value its regex does not match) while the `&&` mutant
    /// would skip it (`length_validator` is `None`) and wrongly accept.
    #[test]
    fn a_string_field_s_own_validator_runs_without_a_length_validator_alongside_it() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "code": "not-uppercase"
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("code"), "{err}");
    }

    /// [`check_primitive_item`]'s `Property::Double(dp)` match arm (P5-06:
    /// cargo-mutants found deleting it survived): no `Double` property
    /// existed anywhere in this fixture, so a deleted arm's fallthrough to
    /// `_ => unreachable!()` was never exercised.
    #[test]
    fn a_double_field_passes() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "weight": 12.5
        });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    /// [`property_has_default_value`] (P5-06: cargo-mutants found the
    /// `-> false` mutant survived): every existing "missing required
    /// property" test omits a property with *no* default value, so the
    /// `true` branch (skip, rather than report missing) is never actually
    /// reached. `d` here has both `isOptional: false` and a `defaultValue`.
    #[test]
    fn a_missing_required_property_with_a_default_value_is_accepted() {
        let mut mgr = ModelManager::new().unwrap();
        mgr.add_model(
            &json!({
                "$class": "concerto.metamodel@1.0.0.Model",
                "namespace": "org.acme.defaults@1.0.0",
                "declarations": [
                    { "$class": "concerto.metamodel@1.0.0.ConceptDeclaration", "name": "Defaulted",
                      "isAbstract": false,
                      "properties": [
                        { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "d",
                          "isArray": false, "isOptional": false, "defaultValue": "fallback" }
                      ] }
                ]
            }),
            None,
        )
        .unwrap();
        let value = json!({ "$class": "org.acme.defaults@1.0.0.Defaulted" });
        validate_instance(&mgr, &value, &ValidateOptions::default())
            .expect("a required property with a default value may be omitted");
    }

    /// [`fully_qualified_identifier`]'s `!id.is_empty()` match guard (P5-06:
    /// cargo-mutants found the guard-true, guard-false and `delete !`
    /// mutants all survived): direct unit coverage of the pure function,
    /// distinguishing a present-but-empty id (falls back to the bare fqn,
    /// same as `None`) from a genuinely present one.
    #[test]
    fn fully_qualified_identifier_falls_back_to_the_bare_fqn_only_for_an_absent_or_empty_id() {
        assert_eq!(fully_qualified_identifier("ns.Foo", None), "ns.Foo");
        assert_eq!(fully_qualified_identifier("ns.Foo", Some("")), "ns.Foo");
        assert_eq!(
            fully_qualified_identifier("ns.Foo", Some("42")),
            "ns.Foo#42"
        );
    }

    /// [`identifiable_to_string`] (P5-06: cargo-mutants found all three
    /// `-> None`/`Some(...)` mutants survived) and, incidentally,
    /// [`visit_class_declaration`]'s `!o.contains_key(RELATIONSHIP_TAG)`
    /// filter (the `delete !` mutant there): every existing enum/invalid-
    /// value test passes a plain string or number, for which
    /// `identifiable_to_string` already returns `None` (falls through to
    /// `js_to_string`) — never a `$class`-tagged value, so its `Some(...)`
    /// arm was never exercised. Assigning a `$$relationship`-tagged value to
    /// `pet` (a plain `Object`-typed, non-relationship property) hits
    /// exactly that arm: `visit_class_declaration` rejects it as "not a
    /// Resource" (a `Relationship` is `Identifiable`, never a `Resource`,
    /// module doc "Scope"), and the reported invalid value is
    /// `identifiable_to_string`'s `"Relationship {id=...}"` form.
    #[test]
    fn a_relationship_tagged_value_on_a_plain_object_property_reports_its_relationship_string_form()
    {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "pet": { "$$relationship": true, "$class": "org.acme@1.0.0.Dog" }
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(
            err.to_string()
                .contains("Relationship {id=org.acme@1.0.0.Dog}"),
            "{err}"
        );
    }

    // ---- Wrong shape at the root (not a Resource / not a relationship) ----

    #[test]
    fn a_non_object_at_the_root_is_rejected() {
        let mgr = fixture();
        // No `$class` at all: `validate_instance` reports a harness-level
        // pre-port error, not a TS-reachable one (module doc: a real
        // `Resource` always has a `$class`).
        let err = err_of(validate_instance(
            &mgr,
            &json!(42),
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("$class"), "{err}");
    }

    #[test]
    fn convert_resources_to_relationships_permits_a_nested_resource_in_place_of_a_relationship() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicle": { "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1 }
        });
        let options = ValidateOptions {
            convert_resources_to_relationships: true,
            permit_resources_for_relationships: false,
        };
        validate_instance(&mgr, &owner, &options).unwrap();
    }

    /// The TS class and message of a failure, as the oracle records them.
    fn class_and_message(err: &ConcertoError) -> (&'static str, String) {
        let ConcertoError::Contract(contract) = err else {
            panic!("expected a contract error, got {err:?}");
        };
        (contract.kind.ts_class(), err.to_string())
    }

    // ---- A JS `undefined` is not `null` (fixture 642d743981a69328b04f1e33) ----

    /// `checkItem` reports an `undefined` array element with value and type
    /// both `undefined` (a `null` one would read `null`/`object`).
    #[test]
    fn an_undefined_array_element_is_reported_as_undefined() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "tags": ["a", js_undefined(), "b"]
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(
            class_and_message(&err),
            (
                "ValidationException",
                "Model violation in the \"org.acme@1.0.0.Vehicle#ABC12\" instance. The field \"tags\" has a value of \"undefined\" (type of value: \"undefined\"). Expected type of value: \"String[]\".".to_string()
            )
        );
    }

    /// An `undefined` field is `Util.isNull`, so an optional one is skipped.
    #[test]
    fn an_undefined_optional_field_is_skipped() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "tags": js_undefined()
        });
        validate_instance(&mgr, &vehicle, &ValidateOptions::default()).unwrap();
    }

    /// `JSON.stringify([1, undefined])` is `[1,null]`.
    #[test]
    fn an_undefined_element_inside_a_reported_value_is_stringified_as_null() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12",
            "mileage": [1, js_undefined()]
        });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(
            err.to_string()
                .contains("has a value of \"[1,null]\" (type of value: \"object\")"),
            "{err}"
        );
    }

    // ---- DV-008: V8 TypeErrors from `report*` (fixture d444ebcf0cf5a3c23e5ee6dd) ----

    /// `reportInvalidFieldAssignment` calls `obj.getFullyQualifiedType()` on
    /// a string that reached a relationship array field.
    #[test]
    fn a_non_array_non_identifiable_value_on_a_relationship_array_is_a_type_error() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicles": "not-an-array"
        });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        assert_eq!(
            class_and_message(&err),
            (
                "TypeError",
                "obj.getFullyQualifiedType is not a function".to_string()
            )
        );
    }

    /// A single `Relationship` on a relationship array field does have
    /// `getFullyQualifiedType()`, so TS reports the field assignment.
    #[test]
    fn a_single_relationship_on_a_relationship_array_is_an_invalid_field_assignment() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicles": { "$$relationship": true, "$class": "org.acme@1.0.0.Vehicle", "vin": "V1" }
        });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        let (class, message) = class_and_message(&err);
        assert_eq!(class, "ValidationException");
        assert!(
            message.contains("org.acme@1.0.0.Vehicle")
                && message.contains("org.acme@1.0.0.Vehicle[]"),
            "{message}"
        );
    }

    /// `reportNotRelationshipViolation` calls `value.toString()` on a `null`
    /// array element.
    #[test]
    fn a_null_relationship_array_element_is_a_type_error() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicles": [null]
        });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        assert_eq!(
            class_and_message(&err),
            (
                "TypeError",
                "Cannot read properties of null (reading 'toString')".to_string()
            )
        );
    }

    /// `reportInvalidEnumValue`'s value goes through `String()`: a number is
    /// written as its digits, not dropped.
    #[test]
    fn a_numeric_enum_value_is_reported_by_its_string_form() {
        let mgr = fixture();
        let vehicle =
            json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1, "color": 1 });
        let err = err_of(validate_instance(
            &mgr,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(
            err.to_string()
                .contains("Invalid enum value of \"1\" for the field \"Color\"."),
            "{err}"
        );
    }

    // ---- Collect-all diagnostics (task P3-03, accordproject/concerto-rust#58) ----
    //
    // One test per `DiagnosticCode` (the issue's exit condition), plus a test
    // that collect-all really does gather more than one diagnostic in a
    // single pass, which is the point of the mode.

    fn diag_of(result: ValidationResult) -> Diagnostic {
        let mut diagnostics = result.into_diagnostics();
        assert_eq!(
            diagnostics.len(),
            1,
            "expected exactly one diagnostic, found {diagnostics:?}"
        );
        diagnostics.remove(0)
    }

    #[test]
    fn a_valid_instance_collects_no_diagnostics() {
        let mgr = fixture();
        let leaf = json!({ "$class": "org.acme@1.0.0.Leaf", "a": "1", "b": "2", "c": "3" });
        let result = collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Leaf",
            &leaf,
            &ValidateOptions::default(),
        );
        assert!(result.is_valid());
        assert!(result.diagnostics().is_empty());
    }

    #[test]
    fn missing_required_property_is_diagnosed() {
        let mgr = fixture();
        let leaf = json!({ "$class": "org.acme@1.0.0.Leaf", "b": "2", "c": "3" });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Leaf",
            &leaf,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::MissingRequiredProperty);
        assert_eq!(diag.pointer, "/a");
    }

    #[test]
    fn undeclared_field_is_diagnosed() {
        let mgr = fixture();
        let leaf = json!({
            "$class": "org.acme@1.0.0.Leaf", "a": "1", "b": "2", "c": "3", "zzz": "extra"
        });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Leaf",
            &leaf,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::UndeclaredField);
        assert_eq!(diag.pointer, "/zzz");
    }

    #[test]
    fn type_violation_is_diagnosed() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": "not-a-number"
        });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::TypeViolation);
        assert_eq!(diag.pointer, "/mileage");
    }

    #[test]
    fn invalid_enum_value_is_diagnosed() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1, "color": "PURPLE"
        });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::InvalidEnumValue);
        assert_eq!(diag.pointer, "/color");
    }

    #[test]
    fn empty_identifier_is_diagnosed() {
        let mgr = fixture();
        let vehicle = json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "", "mileage": 1 });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::EmptyIdentifier);
        assert_eq!(diag.pointer, "");
    }

    #[test]
    fn abstract_class_is_diagnosed() {
        let mgr = fixture();
        let animal = json!({ "$class": "org.acme@1.0.0.Animal", "name": "Rex" });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Animal",
            &animal,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::AbstractClass);
        assert_eq!(diag.pointer, "");
    }

    #[test]
    fn not_assignable_is_diagnosed() {
        let mgr = fixture();
        // `pet`'s declared type is `Animal`; a `Vehicle` is not assignable to it.
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "pet": { "$class": "org.acme@1.0.0.Vehicle", "vin": "XYZ99", "mileage": 2 }
        });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::NotAssignable);
        assert_eq!(diag.pointer, "/pet");
    }

    #[test]
    fn not_resource_is_diagnosed() {
        let mgr = fixture();
        let not_a_resource = json!("just a string");
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &not_a_resource,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::NotResource);
        assert_eq!(diag.pointer, "");
    }

    #[test]
    fn not_relationship_is_diagnosed() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1", "vehicle": "not a relationship"
        });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Owner",
            &owner,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::NotRelationship);
        assert_eq!(diag.pointer, "/vehicle");
    }

    /// [`number_validator_ast`] (P5-06: cargo-mutants found its body
    /// replaced with `Default::default()`, i.e. JSON `null`, surviving):
    /// [`validator_failure_is_diagnosed`] below only ever gives `rating` an
    /// out-of-range value, so a construction error from a bounds-less ast
    /// (`NumberValidator::new`'s own "no bounds" rejection) reports the same
    /// `ValidatorFailure` diagnostic the real out-of-range check does — the
    /// mutant is invisible there. An in-bounds value tells them apart: the
    /// real ast lets `NumberValidator::validate` accept it; the mutant's
    /// bounds-less ast never gets that far, rejecting it at construction.
    #[test]
    fn an_in_bounds_rating_collects_no_diagnostics() {
        let mgr = fixture();
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1, "rating": 3
        });
        let result = collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &vehicle,
            &ValidateOptions::default(),
        );
        assert!(result.is_valid());
        assert!(result.diagnostics().is_empty());
    }

    #[test]
    fn validator_failure_is_diagnosed() {
        let mgr = fixture();
        // `rating`'s `IntegerDomainValidator` is `0..=5`.
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1, "rating": 10
        });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::ValidatorFailure);
        assert_eq!(diag.pointer, "/rating");
    }

    #[test]
    fn type_not_found_is_diagnosed() {
        let mgr = fixture();
        let unknown = json!({ "$class": "org.acme@1.0.0.NoSuchType" });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.NoSuchType",
            &unknown,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::TypeNotFound);
        assert_eq!(diag.pointer, "");
    }

    /// The point of collect-all: several unrelated problems on one instance
    /// are all reported from a single call, not just the first one a
    /// first-error walk would stop at.
    #[test]
    fn collect_all_gathers_every_diagnostic_in_one_pass() {
        let mgr = fixture();
        let leaf = json!({
            // `a` is missing (required, from `Base`), and `zzz` is
            // undeclared: two unrelated problems, neither of which is the
            // other's cause.
            "$class": "org.acme@1.0.0.Leaf", "b": "2", "c": "3", "zzz": "extra"
        });
        let result = collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Leaf",
            &leaf,
            &ValidateOptions::default(),
        );
        assert!(!result.is_valid());
        let codes: Vec<DiagnosticCode> = result.diagnostics().iter().map(|d| d.code).collect();
        assert!(
            codes.contains(&DiagnosticCode::MissingRequiredProperty),
            "{codes:?}"
        );
        assert!(
            codes.contains(&DiagnosticCode::UndeclaredField),
            "{codes:?}"
        );
        assert_eq!(result.diagnostics().len(), 2, "{:?}", result.diagnostics());

        // First-error, by contrast, only ever reports one.
        let err = err_of(validate_instance(&mgr, &leaf, &ValidateOptions::default()));
        let _ = err;
    }

    /// [`ModelManager::validate_instance`] resolves the declared type from
    /// the value's own `$class`, and [`ModelManager::validate_instance_or_throw`]
    /// is exactly the first-error [`validate_instance`] free function.
    #[test]
    fn model_manager_entry_points_agree_with_the_free_functions() {
        let mgr = fixture();
        let leaf = json!({ "$class": "org.acme@1.0.0.Leaf", "b": "2", "c": "3" });

        let result = mgr.validate_instance(&leaf, &ValidateOptions::default());
        assert_eq!(
            diag_of(result).code,
            DiagnosticCode::MissingRequiredProperty
        );

        let err = err_of(mgr.validate_instance_or_throw(&leaf, &ValidateOptions::default()));
        assert!(err.to_string().contains("\"a\""), "{err}");
    }

    /// [`ClassDeclaration::validate_instance`]/`validate_instance_or_throw`
    /// validate against the declaration's own `fqn`, not the value's `$class`.
    #[test]
    fn class_declaration_entry_points_validate_against_their_own_fqn() {
        let mgr = fixture();
        let fqn = "org.acme@1.0.0.Vehicle";
        let class = mgr
            .get_declaration(fqn)
            .expect("Vehicle is in the fixture")
            .as_class()
            .expect("Vehicle is a class-like declaration");
        let vehicle = json!({ "$class": fqn, "vin": "", "mileage": 1 });

        let result = class.validate_instance(&mgr, fqn, &vehicle, &ValidateOptions::default());
        assert_eq!(diag_of(result).code, DiagnosticCode::EmptyIdentifier);

        let err = err_of(class.validate_instance_or_throw(
            &mgr,
            fqn,
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("identifier"), "{err}");
    }

    /// Review finding (P3-03): `collect_property`'s class-typed-array branch
    /// (`collect_class_property_item` per element) must not skip the
    /// `sizeValidator` check `check_array` runs for the first-error walk —
    /// otherwise the two modes disagree on whether an over-size array of
    /// class-typed elements is valid.
    #[test]
    fn collect_all_reports_a_class_typed_array_over_its_max_size() {
        let mgr = fixture();
        let garage = json!({
            "$class": "org.acme@1.0.0.Garage", "vinField": "ABC12",
            "items": [
                { "$class": "org.acme@1.0.0.Item", "name": "a" },
                { "$class": "org.acme@1.0.0.Item", "name": "b" },
                { "$class": "org.acme@1.0.0.Item", "name": "c" }
            ]
        });

        // First-error already catches this (it goes through `check_array`).
        let err = err_of(validate_instance(
            &mgr,
            &garage,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("items"), "{err}");

        // Collect-all must agree: this is not a valid instance.
        let result = collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Garage",
            &garage,
            &ValidateOptions::default(),
        );
        assert!(!result.is_valid(), "{result:?}");
        let codes: Vec<DiagnosticCode> = result.diagnostics().iter().map(|d| d.code).collect();
        assert!(
            codes.contains(&DiagnosticCode::ValidatorFailure),
            "{codes:?}"
        );
    }

    /// Review finding (P3-03): `ClassDeclaration::validate_instance`/
    /// `validate_instance_or_throw` must check the value's own `$class`
    /// against the declaration's own `fqn`, not silently validate whatever
    /// `value` claims to be (which is what `collect_class`/
    /// `visit_class_declaration` do on their own, module doc). Unlike
    /// `class_declaration_entry_points_validate_against_their_own_fqn`
    /// above, this uses a value whose own `$class` differs from `fqn`, so it
    /// can actually distinguish the two behaviours.
    #[test]
    fn class_declaration_entry_points_reject_a_value_not_assignable_to_their_fqn() {
        let mgr = fixture();
        let dog_fqn = "org.acme@1.0.0.Dog";
        let dog_decl = mgr
            .get_declaration(dog_fqn)
            .expect("Dog is in the fixture")
            .as_class()
            .expect("Dog is a class-like declaration");
        // A `Base` instance (unrelated to `Dog`/`Animal`), passed against
        // `Dog`'s own fqn.
        let base_instance = json!({ "$class": "org.acme@1.0.0.Base", "a": "x" });

        let result =
            dog_decl.validate_instance(&mgr, dog_fqn, &base_instance, &ValidateOptions::default());
        assert!(!result.is_valid(), "{result:?}");
        assert_eq!(diag_of(result).code, DiagnosticCode::NotAssignable);

        let err = err_of(dog_decl.validate_instance_or_throw(
            &mgr,
            dog_fqn,
            &base_instance,
            &ValidateOptions::default(),
        ));
        assert!(err.to_string().contains("not assignable"), "{err}");
    }
}
