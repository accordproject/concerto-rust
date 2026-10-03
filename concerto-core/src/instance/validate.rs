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
//!   a valid multi-level type (fixed: the validation plan's property table
//!   holds the whole super type chain's properties);
//! - abstract and nested `$class` values were not checked (fixed: every
//!   object, at any depth, is re-resolved by its own `$class` and checked
//!   for `isAbstract`);
//! - Long, DateTime, relationships, enums, maps and scalars had no support
//!   (all six are implemented below);
//! - errors were stringly typed (fixed: every error is a `ContractError`
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
//! runtime object; [`super::from_json`] populates plain JSON into the value
//! shape this module reads, and the JS layer (`concerto-core-js`) converts a
//! live JS `Resource` into it. That shape is wire JSON (`$class`-tagged,
//! primitive fields as plain JSON) with two reserved markers standing in
//! for the two non-JSON runtime types `JSONPopulator` produces, so this
//! validator's checks are `instanceof`-shaped, not shape/parse-shaped,
//! exactly like TS's own post-population checks:
//!
//! - `DAYJS_TAG` (`"$$dayjs"`) on an object marks an already-coerced
//!   `DateTime` value (its doc comment has the detail);
//! - `RELATIONSHIP_TAG` (`"$$relationship"`) on an object marks an
//!   already-coerced `Relationship` value, carrying the pointed-at type as
//!   `$class` (its doc comment on `check_relationship` has the detail).
//!
//! An untagged `DateTime`/relationship value — one that was never run
//! through the coercion step — is always rejected here as a field type
//! violation, which is the TS-faithful verdict for that case (an uncoerced
//! value on a `Resource` field is exactly what `checkItem`'s
//! `instanceof`-style check rejects in TS too).
//!
//! A third marker, `UNDEFINED_TAG` (`js_undefined`), stands for a JS
//! `undefined` held *inside* a value, such as an array element
//! (`["a", undefined, "b"]`) or a map value. JSON has no `undefined`, and
//! `null` is a different JS value (`typeof null` is `'object'`,
//! `${null}` is `null`), so collapsing one into the other changes the
//! words TS reports (`checkItem` reports an `undefined` item as a field type
//! violation of value `undefined`, type `undefined`).
//!
//! # Values the `report*` helpers cannot describe
//!
//! TS 5.0.0's `reportInvalidFieldAssignment` calls `obj.getFullyQualifiedType()`,
//! and `reportNotResouceViolation`/`reportNotRelationshipViolation` call
//! `value.toString()`, on whatever value reached them, so a value that is not
//! `Identifiable` (or a `null`/`undefined` element) made V8 throw a
//! `TypeError` in place of the `ValidationException` (DV-008). Since BC-06
//! (R1) the report is the `ValidationException` itself: the field assignment
//! names the value's JS type (`invalid_field_assignment_shape`), and a
//! `null` or `undefined` value is written as `null`/`undefined`.
//!
//! # Walk
//!
//! [`validate_instance`] is the entry point (TS `Resource.validate`): it
//! resolves the root value's own `$class` and calls
//! `visit_class_declaration`, which is the port of
//! `ResourceValidator.visitClassDeclaration` and recurses through
//! `visit_property` (`Property.accept`/`visitField`/
//! `visitRelationshipDeclaration`) and `visit_map_declaration`
//! (`MapDeclaration.accept`), mirroring the TS visitor one function per
//! method, in the same order, so that the first error raised matches
//! (PORTING.md 2.4).
//!
//! There is one walk (P5-99, accordproject/concerto-rust#453), over the
//! declaration's validation plan ([`super::plan`]): every model fact it
//! needs (the property table, each property's resolved type, its
//! validators, a map's key and value kinds) is read from the plan, and a
//! plan-build failure is raised from the plan at the point the walk needs
//! that fact.
//!
//! # Stop or collect
//!
//! The walk reports each violation to a [`Sink`] in its parameters. With
//! [`Sink::Stop`] (TS `Resource.validate`, and every first-error caller) the
//! first violation ends the walk as the error it returns. With
//! [`Sink::Collect`] (accordproject/concerto#1239's collect-all
//! diagnostics) each violation is recorded, with the JSON Pointer (RFC 6901)
//! of the value it was found at, and the walk goes on with the next key,
//! property, array element or map entry. Both modes run the same checks in
//! the same order, so the first violation collected is the error the
//! first-error walk returns (class, code and message).

use std::borrow::Cow;

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use serde_json::Value;

use crate::ecma;
use crate::error::{ContractError, Error, ErrorKind, Result};
use crate::introspect::scalar::ScalarValidator;
use crate::introspect::{Declaration, FullyQualified, Property};
use crate::model_manager::{DeclId, ModelManager, ValidatedElement};
use crate::model_util;

use super::plan::{self, ClassPlan, EnumPlan, MapPlan, MapSlot, PlanKind, PlanProp, Prepared, ValueValidator};

/// TS `SerializerOptions`, the two fields `ResourceValidator`'s constructor
/// reads (`resourcevalidator.ts` lines 53-58).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ValidateOptions {
    /// TS `options.convertResourcesToRelationships`.
    pub convert_resources_to_relationships: bool,
    /// TS `options.permitResourcesForRelationships`.
    pub permit_resources_for_relationships: bool,
}

/// Where the walk's violations go (P5-99, module doc "Stop or collect").
pub(crate) enum Sink {
    /// The first violation ends the walk, as the error it returns.
    Stop,
    /// Each violation is recorded with the JSON Pointer of the value it was
    /// found at. With `all`, the walk goes on after each; without, it ends
    /// after recording the first.
    Collect {
        /// The violations, in the order the walk met them.
        found: Vec<(String, Error)>,
        /// Whether to go on after the first.
        all: bool,
    },
}

/// TS `parameters`: the mutable state threaded through the whole visit.
struct Params<'a> {
    mm: &'a ModelManager,
    options: &'a ValidateOptions,
    /// TS `parameters.rootResourceIdentifier`.
    root_resource_identifier: String,
    /// TS `parameters.currentIdentifier`, written into one buffer (P5-99)
    /// once [`Params::has_current_identifier`] is set.
    current_identifier: String,
    /// Whether `parameters.currentIdentifier` has been set.
    has_current_identifier: bool,
    /// Where the violations go.
    sink: Sink,
    /// The JSON Pointer of the value being checked, kept only while
    /// collecting.
    pointer: String,
}

impl<'a> Params<'a> {
    fn new(
        mm: &'a ModelManager,
        options: &'a ValidateOptions,
        root_resource_identifier: String,
        sink: Sink,
    ) -> Self {
        Self {
            mm,
            options,
            root_resource_identifier,
            current_identifier: String::new(),
            has_current_identifier: false,
            sink,
            pointer: String::new(),
        }
    }

    /// TS `parameters.currentIdentifier`.
    fn current_identifier(&self) -> Option<&str> {
        self.has_current_identifier
            .then_some(self.current_identifier.as_str())
    }

    /// `parameters.currentIdentifier = fqn + '#' + id`, into the buffer.
    fn set_current_identifier(&mut self, fqn: &str, id: &str) {
        self.current_identifier.clear();
        self.current_identifier.push_str(fqn);
        self.current_identifier.push('#');
        self.current_identifier.push_str(id);
        self.has_current_identifier = true;
    }

    fn collecting(&self) -> bool {
        matches!(self.sink, Sink::Collect { .. })
    }

    /// The outcome of a check of the value at the current pointer. Stopping,
    /// a violation is returned as it is. Collecting, it is recorded and the
    /// walk goes on (`Ok`); or, when only the first is wanted, the walk ends
    /// once it is recorded, and it passes back up unrecorded.
    fn absorb(&mut self, outcome: Result<()>) -> Result<()> {
        let Err(err) = outcome else {
            return Ok(());
        };
        match &mut self.sink {
            Sink::Stop => Err(err),
            Sink::Collect { found, all: false } if !found.is_empty() => Err(err),
            Sink::Collect { found, all } => {
                if *all {
                    found.push((self.pointer.clone(), err));
                    Ok(())
                } else {
                    found.push((self.pointer.clone(), err.clone()));
                    Err(err)
                }
            }
        }
    }

    /// [`Params::absorb`] of a violation at the child `key` of the current
    /// value.
    fn report_at(&mut self, key: &str, err: Error) -> Result<()> {
        let mark = self.enter_key(key);
        let outcome = self.absorb(Err(err));
        self.leave(mark);
        outcome
    }

    /// Moves the pointer to the child `key` (escaped as RFC 6901 says);
    /// returns where to move it back to. Only while collecting.
    fn enter_key(&mut self, key: &str) -> usize {
        let mark = self.pointer.len();
        if self.collecting() {
            self.pointer.push('/');
            if key.contains(['~', '/']) {
                self.pointer
                    .push_str(&key.replace('~', "~0").replace('/', "~1"));
            } else {
                self.pointer.push_str(key);
            }
        }
        mark
    }

    /// [`Params::enter_key`] for an array index.
    fn enter_index(&mut self, index: usize) -> usize {
        let mark = self.pointer.len();
        if self.collecting() {
            use std::fmt::Write as _;
            // Writing to a `String` cannot fail.
            let _ = write!(self.pointer, "/{index}");
        }
        mark
    }

    /// [`Params::enter_key`] for a map key, which keeps its JS type: a
    /// string as itself, any other key as its JS `String()`.
    fn enter_map_key<V: ValidatorInput>(&mut self, key: &V) -> usize {
        if !self.collecting() {
            return self.pointer.len();
        }
        match key.as_str() {
            Some(k) => self.enter_key(k),
            None => self.enter_key(&js_to_string(&key.to_value())),
        }
    }

    /// Moves the pointer back to `mark`.
    fn leave(&mut self, mark: usize) {
        self.pointer.truncate(mark);
    }

    /// The violations recorded.
    fn into_found(self) -> Vec<(String, Error)> {
        match self.sink {
            Sink::Stop => Vec::new(),
            Sink::Collect { found, .. } => found,
        }
    }
}

/// Validates `value` (a JSON instance, `$class`-tagged the way a Resource
/// serializes) against the model loaded into `mm`.
///
/// TS: `Resource.validate` (src/model/resource.ts), which resolves its own
/// type (`this.getModelManager().getType(this.getFullyQualifiedType())`,
/// always its own `$class`, since `this` supplies both) and hands it to
/// `ResourceValidator` as `classDeclaration`, with `[this]` as the visitor
/// stack's only entry.
#[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
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
///
/// Generic over the value it reads ([`ValidatorInput`], P5-102): plain
/// JSON here, or the JS layer's own values, read in place.
pub fn validate_instance_from<V: ValidatorInput>(
    mm: &ModelManager,
    value: &V,
    options: &ValidateOptions,
    root_resource_identifier: String,
) -> Result<()> {
    let mut params = Params::new(mm, options, root_resource_identifier, Sink::Stop);
    visit_root(&mut params, value)
}

/// [`validate_instance_from`], collecting (module doc "Stop or collect"):
/// every violation, or (without `all`) the first, each with the JSON
/// Pointer of the value it was found at.
pub(crate) fn collect_instance_violations(
    mm: &ModelManager,
    value: &Value,
    options: &ValidateOptions,
    root_resource_identifier: String,
    all: bool,
) -> Vec<(String, Error)> {
    let sink = Sink::Collect {
        found: Vec::new(),
        all,
    };
    let mut params = Params::new(mm, options, root_resource_identifier, sink);
    let outcome = visit_root(&mut params, value);
    // A violation of the root value itself, recorded at the root pointer;
    // or, collecting the first only, the one already recorded.
    let _ = params.absorb(outcome);
    params.into_found()
}

/// The root value: its own `$class` is the declared type.
fn visit_root<V: ValidatorInput>(p: &mut Params, value: &V) -> Result<()> {
    let declared_fqn = value
        .as_object()
        .and_then(|o| o.class())
        .ok_or_else(|| {
            // Not a TS-reachable path: a real `Resource` always has a
            // `$class` (it is how `getFullyQualifiedType()` answers at
            // all). A JSON document with none has no declared type to
            // report a violation against, so this is a harness-level
            // error, not a ported TS message.
            ContractError::pre_port(
                ErrorKind::InvalidArgument,
                "cannot validate an instance with no $class".to_string(),
                None,
            )
        })?;
    visit_class_declaration(p, declared_fqn, value)
}

/// Validates one property value, as `ValidatedResource.setPropertyValue`
/// and `addArrayValue` do before they assign it: `field.accept(this.$validator,
/// parameters)` with `value` alone on the stack and the instance's
/// `getFullyQualifiedIdentifier()` as `rootResourceIdentifier` (task P3-01b,
/// accordproject/concerto-rust#124). The property is the one at `index`
/// of the instance's declaration's [`ClassPlan`] (P5-88; the only form
/// since P5-99).
///
/// TS: `field.accept(this.$validator, parameters)` in
/// `ValidatedResource.setPropertyValue`/`addArrayValue`
/// (src/model/validatedresource.ts), which dispatches to
/// `ResourceValidator.visitField` or `visitRelationshipDeclaration`.
pub fn validate_property_value<V: ValidatorInput>(
    mm: &ModelManager,
    class_plan: &ClassPlan,
    index: usize,
    value: &V,
    root_resource_identifier: String,
    options: &ValidateOptions,
) -> Result<()> {
    let mut params = Params::new(mm, options, root_resource_identifier, Sink::Stop);
    let (owner_fqn, property) = class_plan.property(mm, index);
    visit_property(&mut params, owner_fqn, property, &class_plan.props[index], value)
}

// ---------------------------------------------------------------------
// visitClassDeclaration
// ---------------------------------------------------------------------

/// TS: `ResourceValidator.visitClassDeclaration` (resourcevalidator.ts:205).
/// `declared_fqn` is TS's `classDeclaration` argument: the *declared* type
/// in scope (the field's declared type, or the root's own type), which may
/// differ from `value`'s own, more specific `$class`.
fn visit_class_declaration<V: ValidatorInput>(
    p: &mut Params,
    declared_fqn: &str,
    value: &V,
) -> Result<()> {
    visit_class_declaration_dispatch(p, declared_fqn, value, false)
}

/// [`visit_class_declaration`], reached through [`check_map_slot`] for a map
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
fn visit_map_value_class_declaration<V: ValidatorInput>(
    p: &mut Params,
    declared_fqn: &str,
    value: &V,
) -> Result<()> {
    visit_class_declaration_dispatch(p, declared_fqn, value, true)
}

fn visit_class_declaration_dispatch<V: ValidatorInput>(
    p: &mut Params,
    declared_fqn: &str,
    value: &V,
    is_map_value: bool,
) -> Result<()> {
    // `obj instanceof Resource`: a `Relationship` ([`RELATIONSHIP_TAG`]) is
    // `Identifiable` but not a `Resource`.
    let Some(obj) = value.as_object().filter(|o| !o.is_relationship()) else {
        return Err(not_resource_violation(p, declared_fqn, &value.to_value()));
    };
    let Some(own_fqn) = obj.class() else {
        return Err(not_resource_violation(p, declared_fqn, &value.to_value()));
    };

    // `toBeAssignedClassDeclaration = modelManager.getType(obj.getFullyQualifiedType())`
    // — bug fix (nested/abstract `$class` unchecked): every object's own
    // `$class`, at any depth, is resolved and checked here, not only the
    // outermost one.
    let Some(id) = p.mm.declaration_id(own_fqn) else {
        // See `visit_map_value_class_declaration`'s doc.
        if is_map_value {
            return Err(not_resource_violation_with(
                p,
                declared_fqn,
                &value.to_value(),
                false,
            ));
        }
        return Err(type_not_found(own_fqn));
    };
    if p.mm.declaration(id).and_then(Declaration::as_class).is_none() {
        // `obj` resolves to an enum/scalar/map `$class`: not a TS-reachable
        // path (a Resource is never constructed with one of those types),
        // so this is a harness error, not a ported message.
        return Err(ContractError::pre_port(
            ErrorKind::InvalidArgument,
            format!("'{own_fqn}' is not a class-like type and cannot back a Resource"),
            None,
        )
        .into());
    }
    // The validation plan (P5-88); its chain's error, when it does not
    // resolve, is the one `getIdentifierFieldName()` raises.
    let class_plan = plan::class_plan(p.mm, id)?;
    visit_class::<V>(p, declared_fqn, own_fqn, obj, &class_plan)
}

/// [`visit_class_declaration_dispatch`] after its `$class` lookup, over the
/// declaration's [`ClassPlan`].
fn visit_class<'v, V: ValidatorInput + 'v>(
    p: &mut Params,
    declared_fqn: &str,
    own_fqn: &str,
    obj: V::Object<'v>,
    class_plan: &ClassPlan,
) -> Result<()> {
    // `if(obj instanceof Identifiable) { parameters.rootResourceIdentifier =
    // obj.getFullyQualifiedIdentifier(); }`. Every `obj` reaching this point
    // is a `Resource`, and every `Resource` extends `Identifiable`
    // unconditionally in TS (`resource.ts`) — this does *not* depend on
    // whether `own_fqn`'s declared type happens to have an identifier field.
    // `getFullyQualifiedIdentifier()`'s own truthiness check on
    // `getIdentifier()` (`identifiable.ts`) is what decides whether the
    // `#id` suffix appears ([`write_fully_qualified_identifier`]: an absent
    // or empty identifier both fall back to the bare fqn). Written into the
    // existing buffer rather than a new string (P5-13).
    let identifier_field_name = class_plan.identifier_field(p.mm);
    let own_id = obj
        .get(identifier_field_name.unwrap_or("$identifier"))
        .and_then(V::as_str);
    write_fully_qualified_identifier(&mut p.root_resource_identifier, own_fqn, own_id);

    // `if(toBeAssignedClassDeclaration.isAbstract())` — bug fix (abstract
    // `$class` unchecked): this runs for every nested object, not only the
    // root.
    if class_plan.is_abstract {
        p.absorb(Err(abstract_class(own_fqn)))?;
    }

    // `if(classDeclaration.isIdentified())`, of the declared type.
    let declared_is_identified = if declared_fqn == own_fqn {
        identifier_field_name.is_some()
    } else {
        plan::class_plan_by_name(p.mm, declared_fqn)?
            .identifier_field(p.mm)
            .is_some()
    };

    // `let props = Object.getOwnPropertyNames(obj)` — bug fix (only the
    // direct super type was merged): the plan's table holds the whole
    // chain's properties, so a property declared two or more levels up is
    // found.
    for key in obj.keys() {
        if model_util::is_system_property(key) || class_plan.contains(key) {
            continue;
        }
        // `reportUndeclaredField(obj.getIdentifier(), ...)`: the *bare*
        // identifier value, not `getFullyQualifiedIdentifier()`.
        // `obj.getIdentifier()` can genuinely be JS `undefined` (never
        // set), which `${...}` interpolates as the literal word `undefined`
        // ([`js_id_display`]), not an empty string.
        let resource_id = if declared_is_identified && key != "$identifier" {
            let id = identifier_field_name
                .and_then(|f| obj.get(f))
                .and_then(V::as_str);
            Cow::Borrowed(js_id_display(id))
        } else {
            Cow::Owned(p.current_identifier().unwrap_or("undefined").to_string())
        };
        p.report_at(key, undeclared_field(&resource_id, key, own_fqn))?;
    }

    if declared_is_identified {
        let id = obj
            .get(identifier_field_name.unwrap_or("$identifier"))
            .and_then(V::as_str)
            .unwrap_or("");
        if id.trim().is_empty() {
            let err = empty_identifier(&p.root_resource_identifier);
            p.absorb(Err(err))?;
        }
        p.set_current_identifier(own_fqn, id);
    }

    // `const properties = toBeAssignedClassDeclaration.getProperties();`
    for (i, pp) in class_plan.props.iter().enumerate() {
        let (owner_fqn, property) = class_plan.property(p.mm, i);
        match obj.get(property.name()) {
            Some(v) if !is_js_null(v) => {
                let mark = p.enter_key(property.name());
                let outcome = visit_property(p, owner_fqn, property, pp, v);
                let outcome = p.absorb(outcome);
                p.leave(mark);
                outcome?;
            }
            _ => {
                if !property.is_optional() {
                    if property.name() == "$identifier"
                        && identifier_field_name != Some("$identifier")
                    {
                        continue;
                    }
                    if property_has_default_value(property) {
                        continue;
                    }
                    let err = missing_required_property(&p.root_resource_identifier, property);
                    p.report_at(property.name(), err)?;
                }
            }
        }
    }
    Ok(())
}

/// `Util.isNull`: `undefined` or `null` ([`UNDEFINED_TAG`] stands for
/// `undefined`).
fn is_js_null<V: ValidatorInput>(value: &V) -> bool {
    value.is_null() || value.is_undefined()
}

/// TS `Identifiable.getFullyQualifiedIdentifier`: `this.getIdentifier() ?
/// fqn + '#' + id : fqn` — `getIdentifier()`'s own truthiness check, so an
/// absent identifier and an empty-string one (both falsy in JS) fall back to
/// the bare fqn alike.
fn fully_qualified_identifier(fqn: &str, id: Option<&str>) -> String {
    let mut out = String::new();
    write_fully_qualified_identifier(&mut out, fqn, id);
    out
}

/// [`fully_qualified_identifier`], replacing `out`'s contents.
fn write_fully_qualified_identifier(out: &mut String, fqn: &str, id: Option<&str>) {
    out.clear();
    out.push_str(fqn);
    if let Some(id) = id.filter(|id| !id.is_empty()) {
        out.push('#');
        out.push_str(id);
    }
}

/// The JS `${value}` template-literal spelling of a possibly-absent string:
/// `undefined` (the literal six-letter word, not an empty string) when the
/// value was never set at all, distinct from an explicit empty string. Used
/// wherever TS interpolates a value that can genuinely be `undefined` (as
/// opposed to [`fully_qualified_identifier`]'s falsy-id check, which folds
/// `undefined` and `""` together).
fn js_id_display(id: Option<&str>) -> &str {
    id.unwrap_or("undefined")
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
    let fqn = obj.get("$class")?.as_str()?;
    let id_field =
        p.mm.identifier_field(fqn)
            .ok()
            .flatten()
            .unwrap_or("$identifier");
    let id = obj.get(id_field).and_then(Value::as_str);
    let fqi = fully_qualified_identifier(fqn, id);
    Some((fqn.to_string(), fqi))
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

/// TS: `Property.accept`, dispatching to `visitField` (a `Field`) or
/// `visitRelationshipDeclaration` (a `RelationshipDeclaration`), with the
/// property's type already resolved by the plan (`isTypeEnum`,
/// `isTypeScalar`, `getFullyQualifiedTypeName`), and the error resolving it
/// raised here when it does not resolve.
fn visit_property<V: ValidatorInput>(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    value: &V,
) -> Result<()> {
    match (&pp.kind, property) {
        (PlanKind::Relationship(declared), Property::Relationship(rp)) => {
            visit_relationship(p, owner_fqn, property, &rp.type_, value, pp, Ok(declared))
        }
        // `checkRelationship` resolves the declared type only once the
        // value's own target has checked out.
        (PlanKind::Unresolved(err), Property::Relationship(rp)) => {
            visit_relationship(p, owner_fqn, property, &rp.type_, value, pp, Err(err))
        }
        (PlanKind::Unresolved(err), _) => Err(err.clone()),
        // A class declaration's own properties never include an enum
        // *value* member (only an `EnumDeclaration`'s do, and an enum never
        // backs a Resource) — defensive, not TS-reachable.
        (PlanKind::EnumValue, _) => Err(ContractError::pre_port(
            ErrorKind::InvalidArgument,
            "an EnumProperty cannot be a class declaration's own field".to_string(),
            None,
        )
        .into()),
        _ => visit_field(p, owner_fqn, property, pp, value),
    }
}

/// TS: `ResourceValidator.visitField` (resourcevalidator.ts:300).
fn visit_field<V: ValidatorInput>(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    value: &V,
) -> Result<()> {
    // `if (dataType === 'undefined' || dataType === 'symbol')`. Not reached
    // from `visit_class_declaration`, which skips an `undefined` field
    // (`Util.isNull`), but ported as TS has it.
    if value.is_undefined() {
        return Err(field_type_violation(p, property, &value.to_value()));
    }
    if let PlanKind::Enum(enum_plan) = &pp.kind {
        return check_enum(p, owner_fqn, property, pp, enum_plan, value);
    }

    if property.is_array() {
        return check_array(p, owner_fqn, property, pp, value);
    }

    // `if(field.getSizeValidator() && obj instanceof Map)`: only reachable
    // when the field's own declared type is itself a map.
    if let (Some(_), PlanKind::Map { .. }) = (property.size_validator(), &pp.kind)
        && let Some(entries) = value.map_entries()
    {
        check_size(p, owner_fqn, property, pp, entries.count())?;
    }

    check_item(p, owner_fqn, property, pp, value)
}

/// The field's `CollectionSizeValidator` over `len` items, built once by
/// the plan (or the error building it raises).
fn check_size(
    p: &Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    len: usize,
) -> Result<()> {
    match &pp.size {
        Prepared::Built(v) => v.validate(
            &FieldElement::new(owner_fqn, property),
            Some(p.root_resource_identifier.as_str()),
            len as f64,
        ),
        Prepared::None => Ok(()),
        Prepared::Failed(err) => Err(err.clone()),
    }
}

/// Each element of an array value: checked at its own pointer, collecting.
fn check_elements<V: ValidatorInput>(
    p: &mut Params,
    items: &[V],
    mut check: impl FnMut(&mut Params, &V) -> Result<()>,
) -> Result<()> {
    for (i, item) in items.iter().enumerate() {
        let mark = p.enter_index(i);
        let outcome = check(p, item);
        let outcome = p.absorb(outcome);
        p.leave(mark);
        outcome?;
    }
    Ok(())
}

/// TS: `ResourceValidator.checkEnum` (resourcevalidator.ts:335).
fn check_enum<V: ValidatorInput>(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    enum_plan: &EnumPlan,
    value: &V,
) -> Result<()> {
    if property.is_array() && value.as_array().is_none() {
        return Err(field_type_violation(p, property, &value.to_value()));
    }
    if let Some(items) = value.as_array().filter(|_| property.is_array()) {
        if property.size_validator().is_some() {
            let outcome = check_size(p, owner_fqn, property, pp, items.len());
            p.absorb(outcome)?;
        }
        check_elements(p, items, |p, item| visit_enum(p, enum_plan, item))
    } else {
        visit_enum(p, enum_plan, value)
    }
}

/// TS: `ResourceValidator.checkArray` (resourcevalidator.ts:365).
fn check_array<V: ValidatorInput>(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    value: &V,
) -> Result<()> {
    let Some(items) = value.as_array() else {
        return Err(field_type_violation(p, property, &value.to_value()));
    };
    if property.size_validator().is_some() {
        let outcome = check_size(p, owner_fqn, property, pp, items.len());
        p.absorb(outcome)?;
    }
    check_elements(p, items, |p, item| check_item(p, owner_fqn, property, pp, item))
}

/// TS: `ResourceValidator.checkItem` (resourcevalidator.ts:386).
fn check_item<V: ValidatorInput>(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    value: &V,
) -> Result<()> {
    // `if (dataType === 'undefined' || dataType === 'symbol')`: an
    // `undefined` array element (`["a", undefined, "b"]`) is reported here,
    // with value and type both `undefined`.
    if value.is_undefined() {
        return Err(field_type_violation(p, property, &value.to_value()));
    }
    match &pp.kind {
        PlanKind::Primitive(_) => check_primitive(p, owner_fqn, property, pp, value),
        PlanKind::Scalar { decl, primitive } => {
            check_scalar(p, owner_fqn, property, pp, *decl, *primitive, value)
        }
        PlanKind::Map { decl, entries } => visit_map_declaration(p, *decl, entries, value),
        PlanKind::Class(decl) => {
            let declared_class_fqn = p.mm.decl_fqn(*decl)?;
            check_object_item(p, owner_fqn, property, declared_class_fqn, value)
        }
        PlanKind::Enum(_)
        | PlanKind::Relationship(_)
        | PlanKind::EnumValue
        | PlanKind::Unresolved(_) => {
            unreachable!("visit_property and visit_field handle these kinds before check_item")
        }
    }
}

/// `field.isPrimitive()` branch of `checkItem`, for the six primitive
/// property kinds, with the field's validator built once by the plan.
fn check_primitive<V: ValidatorInput>(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    value: &V,
) -> Result<()> {
    let type_name = property.type_name().unwrap_or_default();
    if !primitive_type_matches(type_name, value) {
        return Err(field_type_violation(p, property, &value.to_value()));
    }
    // `if(field.getValidator() !== null) { field.getValidator().validate(...) }`.
    check_value_validator(p, owner_fqn, property, pp, None, value)
}

/// `isTypeScalar()`: the field's declared type is a scalar, so it is
/// checked as the scalar's own underlying primitive type, with the
/// scalar's own validator (TS `Field.getScalarField()`).
#[allow(clippy::too_many_arguments)]
fn check_scalar<V: ValidatorInput>(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    decl: DeclId,
    primitive: Option<&'static str>,
    value: &V,
) -> Result<()> {
    if !primitive_type_matches(primitive.unwrap_or_default(), value) {
        return Err(field_type_violation(p, property, &value.to_value()));
    }
    check_value_validator(p, owner_fqn, property, pp, Some(decl), value)
}

/// The field's (or its scalar's) value validator, over `value`.
fn check_value_validator<V: ValidatorInput>(
    p: &Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    scalar: Option<DeclId>,
    value: &V,
) -> Result<()> {
    let elem = FieldElement::new(owner_fqn, property);
    let identifier = p.current_identifier();
    match &pp.validator {
        Prepared::Built(ValueValidator::String(sv)) => {
            sv.validate(&elem, identifier, value.as_str())
        }
        Prepared::Built(ValueValidator::Number(nv)) => {
            nv.validate(&elem, identifier, value.as_f64())
        }
        Prepared::Built(ValueValidator::ScalarNumber) => {
            match scalar.and_then(|id| p.mm.declaration(id)) {
                Some(Declaration::Scalar(s)) => match s.validator() {
                    Some(ScalarValidator::Number(nv)) => {
                        nv.validate(&elem, identifier, value.as_f64())
                    }
                    _ => Ok(()),
                },
                _ => Ok(()),
            }
        }
        Prepared::None => Ok(()),
        Prepared::Failed(err) => Err(err.clone()),
    }
}

/// TS `checkItem`'s primitive `switch(field.getType())`, over the *value*
/// (JSONPopulator's wire representation, module doc "Scope").
fn primitive_type_matches<V: ValidatorInput>(type_name: &str, value: &V) -> bool {
    match type_name {
        "String" => value.as_str().is_some(),
        "Long" | "Integer" | "Double" => value.as_f64().is_some_and(f64::is_finite),
        "Boolean" => value.is_boolean(),
        "DateTime" => value.is_dayjs(),
        _ => false,
    }
}

/// The `else` branch of `checkItem`: a field pointing at a transaction,
/// asset, participant, concept... (a concept-like reference).
fn check_object_item<V: ValidatorInput>(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    declared_class_fqn: &str,
    value: &V,
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
    // `if(obj instanceof Identifiable) { ... isAssignableTo check ... }`.
    // Every `$class`-tagged object reaching this point is `Identifiable`
    // (`Resource` extends it unconditionally in TS, module doc "Scope"), so
    // the check always runs.
    if let Some(own_fqn) = value.as_object().and_then(|o| o.class())
        && !is_assignable(p.mm, own_fqn, declared_class_fqn)?
    {
        return Err(invalid_field_assignment(p, owner_fqn, property, own_fqn));
    }
    // TS passes `classDeclaration` itself (the field's declared type) into
    // the recursive `accept` call, so a `reportNotResouceViolation` names
    // the *declared* type, not the value's own `$class`.
    visit_class_declaration(p, declared_class_fqn, value)
}

/// [`ModelManager::is_assignable_to`], from `sub_fqn`'s plan when it is a
/// class declaration (P5-88): the same answer and errors.
fn is_assignable(mm: &ModelManager, sub_fqn: &str, super_fqn: &str) -> Result<bool> {
    if sub_fqn == super_fqn {
        return Ok(true);
    }
    let Some(id) = mm.declaration_id(sub_fqn) else {
        return Err(Error::type_not_found(sub_fqn.to_string()));
    };
    if mm.declaration(id).and_then(Declaration::as_class).is_none() {
        return Ok(false);
    }
    Ok(plan::class_plan(mm, id)?.is_assignable_to(mm, super_fqn))
}

// ---------------------------------------------------------------------
// visitEnumDeclaration
// ---------------------------------------------------------------------

/// TS: `ResourceValidator.visitEnumDeclaration` (resourcevalidator.ts:94),
/// with the enum's value names in a set (P5-88).
///
/// TS passes the *enum declaration* as `reportInvalidEnumValue`'s `field`
/// argument, so the message's `fieldName` is the enum's own short name
/// (`enumDeclaration.getName()`, e.g. `Color`), not the name of the property
/// holding the value; and `value` is the raw `obj`, which the formatter's
/// `String.prototype.replace` converts with `String()` (`1`, `undefined`).
fn visit_enum<V: ValidatorInput>(p: &Params, enum_plan: &EnumPlan, value: &V) -> Result<()> {
    // `property.getName() === obj`: only a string can match.
    if value
        .as_str()
        .is_some_and(|obj| enum_plan.values.contains(obj))
    {
        return Ok(());
    }
    let name = p
        .mm
        .declaration(enum_plan.decl)
        .map(Declaration::name)
        .unwrap_or_default();
    let value = value.to_value();
    Err(invalid_enum_value(
        &p.root_resource_identifier,
        name,
        &identifiable_to_string(p, &value).unwrap_or_else(|| js_to_string(&value)),
    ))
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

/// A JS `BigInt`, as the one-key object `{BIGINT_TAG: "<decimal digits>"}`
/// that [`js_bigint`] builds (task P2-11b-U6): `typeof` is `'bigint'`, and
/// `reportFieldTypeViolation` prints it with `value.toString()` because
/// `JSON.stringify` throws on a `BigInt`.
pub const BIGINT_TAG: &str = "$$bigint";

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

/// The value that stands for a JS `BigInt` ([`BIGINT_TAG`]).
#[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
pub fn js_bigint(text: &str) -> Value {
    serde_json::json!({ BIGINT_TAG: text })
}

/// The JS spelling of a [`NUMBER_TAG`] value.
fn special_number(value: &Value) -> Option<&str> {
    let o = value.as_object()?;
    if o.len() != 1 {
        return None;
    }
    o.get(NUMBER_TAG)?.as_str()
}

/// The decimal digit string of a [`BIGINT_TAG`] value.
fn bigint_value(value: &Value) -> Option<&str> {
    let o = value.as_object()?;
    if o.len() != 1 {
        return None;
    }
    o.get(BIGINT_TAG)?.as_str()
}

/// The entries of a [`MAP_TAG`] value.
fn map_entries(value: &Value) -> Option<impl Iterator<Item = (&Value, &Value)>> {
    let o = value.as_object()?;
    if o.len() != 1 {
        return None;
    }
    let entries = o.get(MAP_TAG)?.as_array()?;
    Some(entries.iter().filter_map(|e| {
        let pair = e.as_array()?;
        Some((pair.first()?, pair.get(1)?))
    }))
}

/// ECMAScript `Number::toString` (radix 10): `1` not `1.0`, `1e+21`,
/// `NaN`, `Infinity`, and `-0` gives `"0"`.
#[cfg_attr(not(feature = "js-compat"), expect(dead_code, reason = "js-compat seam only"))]
pub fn js_number_to_string(n: f64) -> String {
    ecma::number_to_string(n)
}

/// A JS number in the validator's value shape: an integral one as a JSON
/// integer, so that the messages that print it (`JSON.stringify`,
/// `String`) read `1`, not `1.0`; a non-finite one as
/// [`js_special_number`].
pub fn js_number(n: f64) -> Value {
    if !n.is_finite() {
        return js_special_number(&ecma::number_to_string(n));
    }
    if n.trunc() == n && n.abs() < 9_007_199_254_740_992.0 {
        return Value::Number(serde_json::Number::from(n as i64));
    }
    serde_json::Number::from_f64(n).map_or(Value::Null, Value::Number)
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

/// What the instance validator reads of the value it walks (P5-102,
/// accordproject/concerto-rust#456, C-6): the walk is generic over it,
/// so the JS layer (`concerto-core-js`) can have its own values
/// validated in place, rather than first deep-copying the whole object
/// graph into this module's tagged plain-JSON shape (module doc,
/// "Scope") on every call.
///
/// Each method answers what that tagged shape would answer, so a value
/// gives the same verdict and the same error either way. The
/// [`serde_json::Value`] implementation is that shape itself. The
/// messages that print a value read it through [`Self::to_value`],
/// which may build the tagged shape for that one value: only an error
/// pays for it.
pub trait ValidatorInput: Sized {
    /// A JS object's view ([`ValidatorObject`]).
    type Object<'a>: ValidatorObject<'a, Self>
    where
        Self: 'a;
    /// JS `undefined` ([`UNDEFINED_TAG`]).
    fn is_undefined(&self) -> bool;
    /// JS `null`.
    fn is_null(&self) -> bool;
    /// The value as a JS object: never a JS `undefined` or a non-finite
    /// number (each one a one-key tagged object in the plain-JSON
    /// shape), and never a value without own properties.
    fn as_object(&self) -> Option<Self::Object<'_>>;
    /// The value as a JS array.
    fn as_array(&self) -> Option<&[Self]>;
    /// The value as a JS string.
    fn as_str(&self) -> Option<&str>;
    /// The value as a finite JS number.
    fn as_f64(&self) -> Option<f64>;
    /// Whether the value is a JS boolean.
    fn is_boolean(&self) -> bool;
    /// Whether the value is a `Dayjs` ([`DAYJS_TAG`]).
    fn is_dayjs(&self) -> bool;
    /// A JS `Map`'s entries ([`MAP_TAG`]), in order, read in place
    /// (B-15: no `Vec` per map).
    fn map_entries(&self) -> Option<impl Iterator<Item = (&Self, &Self)>>;
    /// The value in the plain-JSON shape (module doc, "Scope"), for the
    /// messages that print it.
    fn to_value(&self) -> Cow<'_, Value>;
}

/// A JS object, as [`ValidatorInput::as_object`] gives it.
pub trait ValidatorObject<'a, V: 'a>: Copy {
    /// Its own `$class`, when that is a string.
    fn class(&self) -> Option<&'a str>;
    /// Whether it is a `Relationship` ([`RELATIONSHIP_TAG`]).
    fn is_relationship(&self) -> bool;
    /// Its own property `key`. Read only of an object with a
    /// [`Self::class`], and never for `$class` itself.
    fn get(&self, key: &str) -> Option<&'a V>;
    /// Its own property names, in order (`Object.getOwnPropertyNames`),
    /// `$class` among them or not.
    fn keys(&self) -> impl Iterator<Item = &'a str>;
}

impl ValidatorInput for Value {
    type Object<'a> = &'a serde_json::Map<String, Value>;

    fn is_undefined(&self) -> bool {
        is_js_undefined(self)
    }

    fn is_null(&self) -> bool {
        self.is_null()
    }

    fn as_object(&self) -> Option<<Value as ValidatorInput>::Object<'_>> {
        as_js_object(self)
    }

    fn as_array(&self) -> Option<&[Value]> {
        self.as_array().map(Vec::as_slice)
    }

    fn as_str(&self) -> Option<&str> {
        self.as_str()
    }

    fn as_f64(&self) -> Option<f64> {
        self.as_f64()
    }

    fn is_boolean(&self) -> bool {
        self.is_boolean()
    }

    fn is_dayjs(&self) -> bool {
        is_populated_datetime(self)
    }

    fn map_entries(&self) -> Option<impl Iterator<Item = (&Value, &Value)>> {
        map_entries(self)
    }

    fn to_value(&self) -> Cow<'_, Value> {
        Cow::Borrowed(self)
    }
}

impl<'a> ValidatorObject<'a, Value> for &'a serde_json::Map<String, Value> {
    fn class(&self) -> Option<&'a str> {
        self.get("$class").and_then(Value::as_str)
    }

    fn is_relationship(&self) -> bool {
        self.contains_key(RELATIONSHIP_TAG)
    }

    fn get(&self, key: &str) -> Option<&'a Value> {
        serde_json::Map::get(self, key)
    }

    fn keys(&self) -> impl Iterator<Item = &'a str> {
        serde_json::Map::keys(self).map(String::as_str)
    }
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

/// Whether a `DateTime` map value is valid (`checkMapType`,
/// resourcevalidator.ts; TS: `dayjs.utc(value).isValid()`). A map's
/// primitive values are *not* run through `JSONPopulator.convertToObject`
/// (only non-primitive map values are converted), so a `DateTime` map value
/// is still the raw wire value here.
///
/// P5-24 (BC-43, R1; accordproject/concerto-rust#328): the same strict
/// rule as a `DateTime` field (`Dayjs::utc_parse`: the
/// `strictQualifiedDateTimes` format, naming a real instant). The dayjs
/// approximation this replaces accepted any string starting with a
/// four-digit year and any finite number, and differed from TS both ways
/// (P5-23's D7; DIVERGENCES.md DV-020). A number is no longer a valid
/// `DateTime` map value, as it is not one for a field. `undefined` still
/// passes: it is the absence of a value, not a form of one (task P3-01b).
fn parses_as_dayjs<V: ValidatorInput>(value: &V) -> bool {
    if value.is_undefined() {
        return true;
    }
    value
        .as_str()
        .is_some_and(|s| super::dayjs::Dayjs::utc_parse(s).is_valid())
}

// ---------------------------------------------------------------------
// visitRelationshipDeclaration
// ---------------------------------------------------------------------

/// TS: `ResourceValidator.visitRelationshipDeclaration` (resourcevalidator.ts:463).
/// `declared` is the declared target type the plan resolved, or the error
/// resolving it.
fn visit_relationship<V: ValidatorInput>(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    type_id: &mm::TypeIdentifier,
    value: &V,
    pp: &PlanProp,
    declared: std::result::Result<&str, &Error>,
) -> Result<()> {
    let holder = RelationshipHolder {
        owner_fqn,
        name: property.name(),
        type_name: &type_id.name,
        is_array: property.is_array(),
        declared,
    };
    if !property.is_array() {
        return check_relationship(p, &holder, value);
    }
    let Some(items) = value.as_array() else {
        return Err(invalid_field_assignment_shape(
            p,
            owner_fqn,
            property,
            &value.to_value(),
        ));
    };
    if property.size_validator().is_some() {
        let outcome = check_size(p, owner_fqn, property, pp, items.len());
        p.absorb(outcome)?;
    }
    check_elements(p, items, |p, item| check_relationship(p, &holder, item))
}

/// What `checkRelationship` reads of the relationship it checks: a
/// relationship property (`--> T field`), or, since P5-58 (BC-05, R1;
/// DV-007), a map's relationship-typed value (`map M { o String --> T }`),
/// so that both go through [`check_relationship`] under the same
/// `convertResourcesToRelationships`/`permitResourcesForRelationships`
/// options.
struct RelationshipHolder<'a> {
    /// The fully-qualified name of the declaring class, or of the map.
    owner_fqn: &'a str,
    /// The property's name, or the map's name.
    name: &'a str,
    /// The declared target type, as written.
    type_name: &'a str,
    /// TS `isArray()`: `false` for a map value.
    is_array: bool,
    /// The declared target type, resolved by the plan in `owner_fqn`'s
    /// namespace, or the error resolving it.
    declared: std::result::Result<&'a str, &'a Error>,
}

/// TS: `ResourceValidator.checkRelationship` (resourcevalidator.ts:491).
fn check_relationship<V: ValidatorInput>(
    p: &mut Params,
    holder: &RelationshipHolder,
    value: &V,
) -> Result<()> {
    // `obj instanceof Relationship`: a [`RELATIONSHIP_TAG`]-tagged object
    // (see its doc), carrying the pointed-at type as `$class`; or `obj
    // instanceof Resource && (convertResourcesToRelationships ||
    // permitResourcesForRelationships)`: a nested (untagged) object standing
    // in for the relationship. Either way the target type is the object's
    // own `$class`, borrowed from the value (P5-99).
    let obj = value.as_object();
    let is_relationship_instance = obj.is_some_and(|o| o.is_relationship());
    let stands_in = is_relationship_instance
        || p.options.convert_resources_to_relationships
        || p.options.permit_resources_for_relationships;
    let target_fqn = obj.filter(|_| stands_in).and_then(|o| o.class());
    let Some(target_fqn) = target_fqn else {
        return Err(not_relationship_violation(p, holder, &value.to_value()));
    };

    // `modelManager.getType(obj.getFullyQualifiedType())`.
    let Some(target_id) = p.mm.declaration_id(target_fqn) else {
        return Err(type_not_found(target_fqn));
    };
    if p.mm.declaration(target_id).and_then(Declaration::as_class).is_none() {
        return Err(not_relationship_violation(p, holder, &value.to_value()));
    }
    // The target's plan (P5-88); its chain's error, when it does not
    // resolve, is the one `isIdentified()` raises.
    let target = plan::class_plan(p.mm, target_id)?;
    if target.identifier_field(p.mm).is_none() {
        return Err(ContractError::new(
            ErrorKind::InvalidArgument,
            "resourcevalidator-checkrelationship-notidentifiable",
            Vec::new(),
        )
        .into());
    }

    let declared = holder.declared.map_err(Clone::clone)?;
    if target_fqn != declared && !target.is_assignable_to(p.mm, declared) {
        return Err(invalid_assignment(
            p,
            holder.owner_fqn,
            holder.name,
            holder.type_name,
            holder.is_array,
            target_fqn,
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------
// visitMapDeclaration / checkMapType
// ---------------------------------------------------------------------

/// TS: `ResourceValidator.visitMapDeclaration` (resourcevalidator.ts:178),
/// with the map's key and value kinds resolved once by the plan
/// ([`MapPlan`]), or the error `ModelUtil.isScalar(mapDeclaration.getKey())`
/// raises when the key's type does not resolve.
fn visit_map_declaration<V: ValidatorInput>(
    p: &mut Params,
    map_id: DeclId,
    map_plan: &std::result::Result<MapPlan, Error>,
    value: &V,
) -> Result<()> {
    // `if (!((obj instanceof Map)))`: only a [`MAP_TAG`] value is a `Map`.
    let Some(entries) = value.map_entries() else {
        return Err(not_a_map(&value.to_value()));
    };
    let map_plan = map_plan.as_ref().map_err(Clone::clone)?;
    let map_fqn = p.mm.decl_fqn(map_id)?;
    let decl = p
        .mm
        .declaration(map_id)
        .expect("a plan's declaration handle is live");
    for (key, value) in entries {
        // `ModelUtil.isSystemProperty(key)`: an `includes`, so only a string
        // key can be one.
        if key.as_str().is_some_and(model_util::is_system_property) {
            continue;
        }
        let mark = p.enter_map_key(key);
        let outcome = check_map_entry(p, map_fqn, decl, map_plan, key, value);
        let outcome = p.absorb(outcome);
        p.leave(mark);
        outcome?;
    }
    Ok(())
}

/// One entry of [`visit_map_declaration`]: its key, then its value.
fn check_map_entry<V: ValidatorInput>(
    p: &mut Params,
    map_fqn: &str,
    decl: &Declaration,
    map_plan: &MapPlan,
    key: &V,
    value: &V,
) -> Result<()> {
    check_map_slot(p, map_fqn, &map_plan.key, key)?;
    // P5-58 (BC-05, R1; DV-007): a relationship-typed value is checked as a
    // relationship property is (`checkRelationship`), not as an embedded
    // object.
    if let MapSlot::Relationship(declared) = &map_plan.value
        && let Some(type_id) = decl.as_map().and_then(|m| m.value_type())
    {
        let holder = RelationshipHolder {
            owner_fqn: map_fqn,
            name: decl.name(),
            type_name: &type_id.name,
            is_array: false,
            declared: declared.as_deref(),
        };
        return check_relationship(p, &holder, value);
    }
    check_map_slot(p, map_fqn, &map_plan.value, value)
}

/// `'Expected a Map, but found ' + JSON.stringify(obj)`:
/// `JSON.stringify(undefined)` is `undefined`, which `+` spells out.
fn not_a_map(value: &Value) -> Error {
    ContractError::new(
        ErrorKind::InvalidArgument,
        "resourcevalidator-visitmapdeclaration-notamap",
        vec![(
            "obj",
            js_json_stringify(value).unwrap_or_else(|| "undefined".to_string()),
        )],
    )
    .into()
}

/// TS: `ResourceValidator.checkMapType` (resourcevalidator.ts:123), for a
/// slot the plan resolved.
fn check_map_slot<V: ValidatorInput>(
    p: &mut Params,
    map_fqn: &str,
    slot: &MapSlot,
    value: &V,
) -> Result<()> {
    match slot {
        MapSlot::Primitive(name) => check_map_primitive(map_fqn, name, value),
        // `thing.accept(this, parameters)`, dispatched by TS's `visit()` to
        // `visitEnumDeclaration`.
        MapSlot::Enum(enum_plan) => visit_enum(p, enum_plan, value),
        // `thing.accept(this, parameters)` -> `visitClassDeclaration`. A
        // `RelationshipMapValueType` value does not reach here: it goes
        // through `check_relationship` (P5-58, BC-05). `value` may be a raw,
        // never-converted object (accordproject/concerto-rust#194): see
        // `visit_map_value_class_declaration`'s doc.
        MapSlot::Class(id) => {
            let declared_fqn = p.mm.decl_fqn(*id)?;
            visit_map_value_class_declaration(p, declared_fqn, value)
        }
        MapSlot::Relationship(_) | MapSlot::Skip => Ok(()),
        MapSlot::Unresolved(err) => Err(err.clone()),
    }
}

/// `ModelUtil.isScalar(mapDeclaration.getKey())`: ported verbatim, including
/// TS's own quirk of always asking about the *key*'s scalar-ness, even
/// while validating the *value* (PORTING.md: faithful port, no
/// improvements) — `checkMapType`'s own `if
/// (ModelUtil.isScalar(mapDeclaration.getKey())) { type = thing.getType(); }`
/// runs unconditionally for both the key and the value slot.
pub(super) fn map_key_is_scalar(
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
    let fqn = mm.resolve_type_name_at(namespace, &ti.name, None)?;
    Ok(mm.get_declaration(&fqn)?.is_scalar_declaration())
}

/// Whether a map key/value `$class` short kind names a declared type.
pub(super) fn is_object_map_kind(kind: &str) -> bool {
    matches!(
        kind,
        "ObjectMapKeyType" | "ObjectMapValueType" | "RelationshipMapValueType"
    )
}

/// `checkMapType`'s `switch` over the primitive type name.
fn check_map_primitive<V: ValidatorInput>(
    map_fqn: &str,
    primitive_type_name: &str,
    value: &V,
) -> Result<()> {
    match primitive_type_name {
        "String" if value.as_str().is_none() => {
            return Err(ContractError::new(
                ErrorKind::InvalidArgument,
                "resourcevalidator-checkmaptype-expectedstring",
                vec![
                    ("mapFqn", map_fqn.to_string()),
                    ("value", js_to_string(&value.to_value())),
                ],
            )
            .into());
        }
        "DateTime" if !parses_as_dayjs(value) => {
            return Err(ContractError::new(
                ErrorKind::InvalidArgument,
                "resourcevalidator-checkmaptype-expecteddatetime",
                vec![
                    ("mapFqn", map_fqn.to_string()),
                    ("value", js_to_string(&value.to_value())),
                ],
            )
            .into());
        }
        "Boolean" if !value.is_boolean() => {
            let value = value.to_value();
            return Err(ContractError::new(
                ErrorKind::InvalidArgument,
                "resourcevalidator-checkmaptype-expectedboolean",
                vec![
                    ("mapFqn", map_fqn.to_string()),
                    ("type", js_typeof(&value).to_string()),
                    ("value", js_to_string(&value)),
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

/// The primitive name a primitive map key/value `$class` short kind
/// implies, e.g. `StringMapKeyType` -> `"String"`; `""` (which checks
/// nothing) for a kind that names no primitive.
pub(super) fn kind_primitive_name(kind: &str) -> &'static str {
    let name = kind
        .strip_suffix("MapKeyType")
        .or_else(|| kind.strip_suffix("MapValueType"))
        .unwrap_or(kind);
    ["String", "DateTime", "Boolean", "Integer", "Long", "Double"]
        .into_iter()
        .find(|primitive| *primitive == name)
        .unwrap_or_default()
}

// ---------------------------------------------------------------------
// ValidatedElement: a Property in the context of its owning class, for the
// P2-02 validator types' `validate`/`new`.
// ---------------------------------------------------------------------

/// TS: the `field` a `NumberValidator`/`StringValidator`/
/// `CollectionSizeValidator` is attached to: a `Property`, read here for its
/// own AST `defaultValue` and for `getFullyQualifiedName()`
/// (`getParent().getFullyQualifiedName() + '.' + getName()`).
pub(crate) struct FieldElement<'a> {
    owner_fqn: &'a str,
    property: &'a Property,
}

impl<'a> FieldElement<'a> {
    pub(crate) fn new(owner_fqn: &'a str, property: &'a Property) -> Self {
        Self {
            owner_fqn,
            property,
        }
    }
}

impl FullyQualified for FieldElement<'_> {
    type Error = Error;

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
    if bigint_value(value).is_some() {
        return "bigint";
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
    // `JSON.stringify` throws a `TypeError` on a `BigInt` ("Do not know how
    // to serialize a BigInt"); TS's `try { JSON.stringify(value) } catch
    // (err) { value = value.toString() }` falls back to `toString()`, which
    // is exactly the decimal digit string already held here.
    if let Some(n) = bigint_value(value) {
        return n.to_string();
    }
    if ecma::is_truthy(value) {
        js_json_stringify(value).unwrap_or_else(|| "undefined".to_string())
    } else {
        ecma::to_js_string(value)
    }
}

/// TS: `ResourceValidator.reportFieldTypeViolation` (resourcevalidator.ts:520).
fn field_type_violation(p: &Params, property: &Property, value: &Value) -> Error {
    let is_array = if property.is_array() { "[]" } else { "" };
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
/// `value.toString()` is `'Relationship {id=...}'` for a `Relationship`, and a
/// `null` or `undefined` value is written as `null`/`undefined` (BC-06, R1;
/// TS 5.0.0's `value.toString()` threw a V8 `TypeError` for one, DV-008).
fn not_resource_violation(p: &Params, class_fqn: &str, value: &Value) -> Error {
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
) -> Error {
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

/// TS: `ResourceValidator.reportNotRelationshipViolation` (resourcevalidator.ts:576).
/// A `null` or `undefined` value is written as `null`/`undefined` (BC-06, R1;
/// TS 5.0.0's `value.toString()` threw a V8 `TypeError` for one, DV-008).
fn not_relationship_violation(p: &Params, holder: &RelationshipHolder, value: &Value) -> Error {
    let namespace = model_util::get_namespace(Some(holder.owner_fqn)).unwrap_or(holder.owner_fqn);
    let class_fqn = model_util::qualify(namespace, holder.type_name);
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
fn missing_required_property(resource_id: &str, property: &Property) -> Error {
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
fn empty_identifier(resource_id: &str) -> Error {
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
fn invalid_enum_value(resource_id: &str, field_name: &str, value: &str) -> Error {
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
fn abstract_class(class_fqn: &str) -> Error {
    ContractError::new(
        ErrorKind::Validation,
        "resourcevalidator-abstractclass",
        vec![("className", class_fqn.to_string())],
    )
    .into()
}

/// TS: `ResourceValidator.reportUndeclaredField` (resourcevalidator.ts:649).
fn undeclared_field(resource_id: &str, property_name: &str, fqn: &str) -> Error {
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
) -> Error {
    invalid_assignment(
        p,
        owner_fqn,
        property.name(),
        property.type_name().unwrap_or_default(),
        property.is_array(),
        object_type,
    )
}

/// [`invalid_field_assignment`] for a holder named `name`, declared as
/// `type_name` (an array when `is_array`) in `owner_fqn`: a property, or a
/// relationship-typed map value (P5-58).
fn invalid_assignment(
    p: &Params,
    owner_fqn: &str,
    name: &str,
    type_name: &str,
    is_array: bool,
    object_type: &str,
) -> Error {
    let namespace = model_util::get_namespace(Some(owner_fqn)).unwrap_or(owner_fqn);
    let mut field_type = model_util::qualify(namespace, type_name);
    if is_array {
        field_type.push_str("[]");
    }
    ContractError::new(
        ErrorKind::Validation,
        "resourcevalidator-invalidfieldassignment",
        vec![
            ("resourceId", p.root_resource_identifier.clone()),
            ("propertyName", name.to_string()),
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
/// answers its own type. Any other value is reported by its JS type
/// (`string`, `number`, `null`, ...; BC-06, R1): TS 5.0.0 found no such
/// method on it, so V8 threw a `TypeError` instead of the
/// `ValidationException` (DV-008; fixture `d444ebcf0cf5a3c23e5ee6dd`, a
/// string on a `--> Car[]` field).
fn invalid_field_assignment_shape(
    p: &Params,
    owner_fqn: &str,
    property: &Property,
    value: &Value,
) -> Error {
    match identifiable_parts(p, value) {
        Some((object_type, _)) => invalid_field_assignment(p, owner_fqn, property, &object_type),
        None => {
            let js_type = if value.is_null() { "null" } else { js_typeof(value) };
            invalid_field_assignment(p, owner_fqn, property, js_type)
        }
    }
}

/// The catalogue's `TypeNotFoundException` for `fqn` (table 2.3's default
/// message), which `modelManager.getType` throws for a type that is not
/// declared.
fn type_not_found(fqn: &str) -> Error {
    ContractError::type_not_found(
        "typenotfounderror-defaultmessage",
        Vec::new(),
        fqn.to_string(),
        None,
    )
    .into()
}


// ---------------------------------------------------------------------
// The named type of the `_as` entry points
// ---------------------------------------------------------------------

/// Checks that `value`'s own `$class` (when present) is assignable to
/// `declared_fqn`. [`visit_class_declaration`] walks
/// by `value`'s own `$class`, regardless of what `declared_fqn` says (module
/// doc): the right behaviour for `Resource.validate`, which always validates
/// a resource against its own type, but not for
/// [`ModelManager::validate_instance_as`], whose whole point is to validate
/// against the type it names. Returns `Ok(())`
/// when `value` carries no `$class` (or isn't shaped like a Resource at
/// all): the ordinary walk that follows already reports that case
/// correctly, so there's nothing extra to check here; likewise `Ok(())` when
/// `declared_fqn` itself is `value`'s own `$class`.
pub(crate) fn check_assignable_to_declaration(
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

#[cfg(test)]
mod tests;
