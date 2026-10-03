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
    fn enter_map_key(&mut self, key: &Value) -> usize {
        if !self.collecting() {
            return self.pointer.len();
        }
        match key.as_str() {
            Some(k) => self.enter_key(k),
            None => self.enter_key(&js_to_string(key)),
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
pub fn validate_instance(
    mm: &ModelManager,
    value: &Value,
    options: &ValidateOptions,
) -> Result<()> {
    validate_instance_from(mm, value, options, String::new())
}

js_compat_pub! {
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
        let mut params = Params::new(mm, options, root_resource_identifier, Sink::Stop);
        visit_root(&mut params, value)
    }
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
fn visit_root(p: &mut Params, value: &Value) -> Result<()> {
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
                ErrorKind::InvalidArgument,
                "cannot validate an instance with no $class".to_string(),
                None,
            )
        })?;
    visit_class_declaration(p, declared_fqn, value)
}

js_compat_pub! {
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
    pub fn validate_property_value(
        mm: &ModelManager,
        class_plan: &ClassPlan,
        index: usize,
        value: &Value,
        root_resource_identifier: String,
        options: &ValidateOptions,
    ) -> Result<()> {
        let mut params = Params::new(mm, options, root_resource_identifier, Sink::Stop);
        let (owner_fqn, property) = class_plan.property(mm, index);
        visit_property(&mut params, owner_fqn, property, &class_plan.props[index], value)
    }
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

    // `toBeAssignedClassDeclaration = modelManager.getType(obj.getFullyQualifiedType())`
    // — bug fix (nested/abstract `$class` unchecked): every object's own
    // `$class`, at any depth, is resolved and checked here, not only the
    // outermost one.
    let Some(id) = p.mm.declaration_id(own_fqn) else {
        // See `visit_map_value_class_declaration`'s doc.
        if is_map_value {
            return Err(not_resource_violation_with(p, declared_fqn, value, false));
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
    visit_class(p, declared_fqn, own_fqn, obj, &class_plan)
}

/// [`visit_class_declaration_dispatch`] after its `$class` lookup, over the
/// declaration's [`ClassPlan`].
fn visit_class(
    p: &mut Params,
    declared_fqn: &str,
    own_fqn: &str,
    obj: &serde_json::Map<String, Value>,
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
        .and_then(Value::as_str);
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
                .and_then(Value::as_str);
            js_id_display(id)
        } else {
            p.current_identifier()
                .unwrap_or("undefined")
                .to_string()
        };
        p.report_at(key, undeclared_field(&resource_id, key, own_fqn))?;
    }

    if declared_is_identified {
        let id = obj
            .get(identifier_field_name.unwrap_or("$identifier"))
            .and_then(Value::as_str)
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
fn is_js_null(value: &Value) -> bool {
    value.is_null() || is_js_undefined(value)
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
fn visit_property(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    value: &Value,
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
fn visit_field(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    value: &Value,
) -> Result<()> {
    // `if (dataType === 'undefined' || dataType === 'symbol')`. Not reached
    // from `visit_class_declaration`, which skips an `undefined` field
    // (`Util.isNull`), but ported as TS has it.
    if is_js_undefined(value) {
        return Err(field_type_violation(p, property, value));
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
        && let Some(entries) = map_entries(value)
    {
        check_size(p, owner_fqn, property, pp, entries.len())?;
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
fn check_elements(
    p: &mut Params,
    items: &[Value],
    mut check: impl FnMut(&mut Params, &Value) -> Result<()>,
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
fn check_enum(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    enum_plan: &EnumPlan,
    value: &Value,
) -> Result<()> {
    if property.is_array() && !value.is_array() {
        return Err(field_type_violation(p, property, value));
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
fn check_array(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    value: &Value,
) -> Result<()> {
    let Some(items) = value.as_array() else {
        return Err(field_type_violation(p, property, value));
    };
    if property.size_validator().is_some() {
        let outcome = check_size(p, owner_fqn, property, pp, items.len());
        p.absorb(outcome)?;
    }
    check_elements(p, items, |p, item| check_item(p, owner_fqn, property, pp, item))
}

/// TS: `ResourceValidator.checkItem` (resourcevalidator.ts:386).
fn check_item(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    value: &Value,
) -> Result<()> {
    // `if (dataType === 'undefined' || dataType === 'symbol')`: an
    // `undefined` array element (`["a", undefined, "b"]`) is reported here,
    // with value and type both `undefined`.
    if is_js_undefined(value) {
        return Err(field_type_violation(p, property, value));
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
fn check_primitive(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    value: &Value,
) -> Result<()> {
    let type_name = property.type_name().unwrap_or_default();
    if !primitive_type_matches(type_name, value) {
        return Err(field_type_violation(p, property, value));
    }
    // `if(field.getValidator() !== null) { field.getValidator().validate(...) }`.
    check_value_validator(p, owner_fqn, property, pp, None, value)
}

/// `isTypeScalar()`: the field's declared type is a scalar, so it is
/// checked as the scalar's own underlying primitive type, with the
/// scalar's own validator (TS `Field.getScalarField()`).
fn check_scalar(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    decl: DeclId,
    primitive: Option<&'static str>,
    value: &Value,
) -> Result<()> {
    if !primitive_type_matches(primitive.unwrap_or_default(), value) {
        return Err(field_type_violation(p, property, value));
    }
    check_value_validator(p, owner_fqn, property, pp, Some(decl), value)
}

/// The field's (or its scalar's) value validator, over `value`.
fn check_value_validator(
    p: &Params,
    owner_fqn: &str,
    property: &Property,
    pp: &PlanProp,
    scalar: Option<DeclId>,
    value: &Value,
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
fn primitive_type_matches(type_name: &str, value: &Value) -> bool {
    match type_name {
        "String" => value.is_string(),
        "Long" | "Integer" | "Double" => value.as_f64().is_some_and(f64::is_finite),
        "Boolean" => value.is_boolean(),
        "DateTime" => is_populated_datetime(value),
        _ => false,
    }
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
    // `if(obj instanceof Identifiable) { ... isAssignableTo check ... }`.
    // Every `$class`-tagged object reaching this point is `Identifiable`
    // (`Resource` extends it unconditionally in TS, module doc "Scope"), so
    // the check always runs.
    if let Some(own_fqn) = value
        .as_object()
        .and_then(|o| o.get("$class"))
        .and_then(Value::as_str)
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
fn visit_enum(p: &Params, enum_plan: &EnumPlan, value: &Value) -> Result<()> {
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
    Err(invalid_enum_value(
        &p.root_resource_identifier,
        name,
        &identifiable_to_string(p, value).unwrap_or_else(|| js_to_string(value)),
    ))
}

js_compat_pub! {
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
}

js_compat_pub! {
    /// A value that has already been populated as a `Relationship` instance
    /// (see [`DAYJS_TAG`]'s doc for why the tag exists): mirrors TS's `obj
    /// instanceof Relationship` (resourcevalidator.ts:492), as opposed to a
    /// `$class`-tagged plain object, which stands for `obj instanceof Resource`.
    pub const RELATIONSHIP_TAG: &str = "$$relationship";
}

js_compat_pub! {
    /// A JS `undefined` held inside a value: an array element or a map value
    /// (module doc "Scope"). The value is the one-key object
    /// `{UNDEFINED_TAG: true}` that [`js_undefined`] builds. JSON has no
    /// `undefined`, and writing `null` instead would change what TS reports:
    /// `typeof undefined` is `'undefined'` and `${undefined}` is `undefined`,
    /// where `null` gives `'object'` and `null`.
    pub const UNDEFINED_TAG: &str = "$$undefined";
}

js_compat_pub! {
    /// A JS number that JSON cannot hold (`NaN`, `Infinity`, `-Infinity`), as
    /// the one-key object `{NUMBER_TAG: "<its JS spelling>"}` that
    /// [`js_special_number`] builds (task P3-01b): `typeof` is `'number'`, and
    /// `reportFieldTypeViolation` prints it with `value.toString()`.
    pub const NUMBER_TAG: &str = "$$number";
}

js_compat_pub! {
    /// A JS `BigInt`, as the one-key object `{BIGINT_TAG: "<decimal digits>"}`
    /// that [`js_bigint`] builds (task P2-11b-U6): `typeof` is `'bigint'`, and
    /// `reportFieldTypeViolation` prints it with `value.toString()` because
    /// `JSON.stringify` throws on a `BigInt`.
    pub const BIGINT_TAG: &str = "$$bigint";
}

js_compat_pub! {
    /// A JS `Map` (a populated `MapDeclaration` value), as the one-key object
    /// `{MAP_TAG: [[key, value], ...]}` that [`js_map`] builds (task P3-01b):
    /// its keys keep their JS type (a number key is not a string), and a plain
    /// object is told apart from a `Map` (`obj instanceof Map`).
    pub const MAP_TAG: &str = "$$map";
}

js_compat_pub! {
    /// The value that stands for a non-finite JS number ([`NUMBER_TAG`]).
    pub fn js_special_number(text: &str) -> Value {
        serde_json::json!({ NUMBER_TAG: text })
    }
}

js_compat_pub! {
    /// The value that stands for a JS `Map` ([`MAP_TAG`]).
    pub fn js_map(entries: Vec<(Value, Value)>) -> Value {
        serde_json::json!({
            MAP_TAG: entries.into_iter().map(|(k, v)| Value::Array(vec![k, v])).collect::<Vec<_>>()
        })
    }
}

js_compat_pub! {
    /// The value that stands for a JS `BigInt` ([`BIGINT_TAG`]).
    pub fn js_bigint(text: &str) -> Value {
        serde_json::json!({ BIGINT_TAG: text })
    }
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

js_compat_pub! {
    /// ECMAScript `Number::toString` (radix 10): `1` not `1.0`, `1e+21`,
    /// `NaN`, `Infinity`, and `-0` gives `"0"`.
    pub fn js_number_to_string(n: f64) -> String {
        ecma::number_to_string(n)
    }
}

js_compat_pub! {
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
}

js_compat_pub! {
    /// The value that stands for a JS `undefined` ([`UNDEFINED_TAG`]).
    pub fn js_undefined() -> Value {
        serde_json::json!({ UNDEFINED_TAG: true })
    }
}

js_compat_pub! {
    /// Whether `value` stands for a JS `undefined` ([`UNDEFINED_TAG`]).
    pub fn is_js_undefined(value: &Value) -> bool {
        value
            .as_object()
            .is_some_and(|o| o.len() == 1 && o.contains_key(UNDEFINED_TAG))
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
fn parses_as_dayjs(value: &Value) -> bool {
    if is_js_undefined(value) {
        return true;
    }
    match value {
        Value::String(s) => super::dayjs::Dayjs::utc_parse(s).is_valid(),
        _ => false,
    }
}

// ---------------------------------------------------------------------
// visitRelationshipDeclaration
// ---------------------------------------------------------------------

/// TS: `ResourceValidator.visitRelationshipDeclaration` (resourcevalidator.ts:463).
/// `declared` is the declared target type the plan resolved, or the error
/// resolving it.
fn visit_relationship(
    p: &mut Params,
    owner_fqn: &str,
    property: &Property,
    type_id: &mm::TypeIdentifier,
    value: &Value,
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
            p, owner_fqn, property, value,
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
fn check_relationship(p: &mut Params, holder: &RelationshipHolder, value: &Value) -> Result<()> {
    // `obj instanceof Relationship`: a [`RELATIONSHIP_TAG`]-tagged object
    // (see its doc), carrying the pointed-at type as `$class`; or `obj
    // instanceof Resource && (convertResourcesToRelationships ||
    // permitResourcesForRelationships)`: a nested (untagged) object standing
    // in for the relationship. Either way the target type is the object's
    // own `$class`, borrowed from the value (P5-99).
    let obj = as_js_object(value);
    let is_relationship_instance = obj.is_some_and(|o| o.contains_key(RELATIONSHIP_TAG));
    let stands_in = is_relationship_instance
        || p.options.convert_resources_to_relationships
        || p.options.permit_resources_for_relationships;
    let target_fqn = obj
        .filter(|_| stands_in)
        .and_then(|o| o.get("$class"))
        .and_then(Value::as_str);
    let Some(target_fqn) = target_fqn else {
        return Err(not_relationship_violation(p, holder, value));
    };

    // `modelManager.getType(obj.getFullyQualifiedType())`.
    let Some(target_id) = p.mm.declaration_id(target_fqn) else {
        return Err(type_not_found(target_fqn));
    };
    if p.mm.declaration(target_id).and_then(Declaration::as_class).is_none() {
        return Err(not_relationship_violation(p, holder, value));
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
fn visit_map_declaration(
    p: &mut Params,
    map_id: DeclId,
    map_plan: &std::result::Result<MapPlan, Error>,
    value: &Value,
) -> Result<()> {
    // `if (!((obj instanceof Map)))`: only a [`MAP_TAG`] value is a `Map`.
    let Some(entries) = map_entries(value) else {
        return Err(not_a_map(value));
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
fn check_map_entry(
    p: &mut Params,
    map_fqn: &str,
    decl: &Declaration,
    map_plan: &MapPlan,
    key: &Value,
    value: &Value,
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
fn check_map_slot(p: &mut Params, map_fqn: &str, slot: &MapSlot, value: &Value) -> Result<()> {
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
fn check_map_primitive(map_fqn: &str, primitive_type_name: &str, value: &Value) -> Result<()> {
    match primitive_type_name {
        "String" if !value.is_string() => {
            return Err(ContractError::new(
                ErrorKind::InvalidArgument,
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
                ErrorKind::InvalidArgument,
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
                ErrorKind::InvalidArgument,
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
mod tests {
    //! Exercises the confirmed `concerto-validate-rs` bug fixes (module doc),
    //! plus full type support: Long, DateTime, relationships, enums, maps and
    //! scalars, matching `ResourceValidator`'s checks and messages
    //! (`resourcevalidator.ts`, verified against its golden tests in
    //! `error/mod.rs`).

    use super::*;
    use crate::instance::{Diagnostic, DiagnosticCode, ValidationReport};
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
        mgr.load_model(
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

    fn err_of(result: Result<()>) -> Error {
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
        mgr.load_model(
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
        validate_map_with(mgr, map_fqn, value, ValidateOptions::default())
    }

    fn validate_map_with(
        mgr: &ModelManager,
        map_fqn: &str,
        value: &Value,
        options: ValidateOptions,
    ) -> Result<()> {
        let id = mgr.declaration_id(map_fqn).unwrap();
        let map_plan = plan::map_plan(mgr, id);
        let mut params = Params::new(mgr, &options, String::new(), Sink::Stop);
        visit_map_declaration(&mut params, id, &map_plan, value)
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

    /// P5-24 (BC-43, R1): a number is not a valid `DateTime` map value,
    /// as it is not a valid `DateTime` field value.
    #[test]
    fn a_number_is_not_a_valid_datetime_map_value() {
        assert!(!parses_as_dayjs(&json!(1_700_000_000_000.0)));
    }

    /// [`parses_as_dayjs`]'s `Value::String` arm (P5-06: cargo-mutants found
    /// that arm's deletion survived): a strict date-time string is a valid
    /// `DateTime` map value, and (P5-24, BC-43) nothing looser is.
    #[test]
    fn only_a_strict_string_is_a_valid_datetime_map_value() {
        assert!(parses_as_dayjs(&json!("2024-05-01T00:00:00Z")));
        assert!(parses_as_dayjs(&json!("2024-05-01T10:00:00.5+02:00")));
        for s in [
            "2024-05-01",
            "2024xyz",
            " 2024-05-01",
            "20240102",
            "May 1, 2020",
            "1",
            "2024-02-30T00:00:00Z",
            "2024-05-01T24:00:00Z",
        ] {
            assert!(!parses_as_dayjs(&json!(s)), "{s:?}");
        }
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

    /// Validation plan (P5-88): every map in the fixture resolves to a
    /// [`MapPlan`], with each slot of the kind `checkMapType` gives it.
    #[test]
    fn every_map_in_the_fixture_has_a_map_plan() {
        let mgr = fixture();
        for (name, key, value) in [
            ("StringMap", "Primitive", "Primitive"),
            ("ColorMap", "Primitive", "Enum"),
            ("ItemMap", "Primitive", "Class"),
            ("VehicleMap", "Primitive", "Relationship"),
            ("ScalarKeyMap", "Primitive", "Primitive"),
            ("PlainKeyScalarValueMap", "Primitive", "Skip"),
            ("DateTimeMap", "Primitive", "Primitive"),
            ("BooleanMap", "Primitive", "Primitive"),
        ] {
            let id = mgr
                .declaration_id(&format!("org.acme@1.0.0.{name}"))
                .unwrap();
            let map_plan = plan::map_plan(&mgr, id).unwrap_or_else(|e| panic!("{name}: {e}"));
            let kind = |slot: &MapSlot| match slot {
                MapSlot::Primitive(_) => "Primitive",
                MapSlot::Enum(_) => "Enum",
                MapSlot::Class(_) => "Class",
                MapSlot::Relationship(Ok(_)) => "Relationship",
                MapSlot::Skip => "Skip",
                MapSlot::Relationship(Err(_)) | MapSlot::Unresolved(_) => "Unresolved",
            };
            assert_eq!((kind(&map_plan.key), kind(&map_plan.value)), (key, value), "{name}");
        }
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

    /// P5-58 (BC-05, R1; DV-007): a `RelationshipMapValueType` map value is
    /// checked as a relationship property is (`checkRelationship`): a
    /// relationship to the declared type, or a subtype, passes.
    #[test]
    fn a_map_with_a_relationship_typed_value_accepts_a_relationship() {
        let mgr = fixture();
        let map = js_map(vec![(
            json!("a"),
            json!({ "$$relationship": true, "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12" }),
        )]);
        validate_map(&mgr, "org.acme@1.0.0.VehicleMap", &map).unwrap();
    }

    /// P5-58 (BC-05, R1; DV-007): an embedded resource in a relationship
    /// map is rejected by default, as in a relationship property (TS 5.0.0
    /// required it), and accepted exactly when
    /// `permitResourcesForRelationships` or `convertResourcesToRelationships`
    /// allows it for a property.
    #[test]
    fn a_map_with_a_relationship_typed_value_takes_a_nested_resource_only_with_the_options() {
        let mgr = fixture();
        let map = js_map(vec![(
            json!("a"),
            json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1 }),
        )]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.VehicleMap", &map));
        assert!(matches!(err.ported(), Some(e) if e.kind == ErrorKind::Validation));
        assert!(
            err.to_string().contains("Expected a \"Relationship\""),
            "{err}"
        );
        for options in [
            ValidateOptions {
                permit_resources_for_relationships: true,
                ..ValidateOptions::default()
            },
            ValidateOptions {
                convert_resources_to_relationships: true,
                ..ValidateOptions::default()
            },
        ] {
            validate_map_with(&mgr, "org.acme@1.0.0.VehicleMap", &map, options).unwrap();
        }
    }

    /// P5-58: a relationship map value of the wrong type, or a string that
    /// was never populated into a relationship, fails as a relationship
    /// property does.
    #[test]
    fn a_map_with_a_relationship_typed_value_rejects_what_a_relationship_property_rejects() {
        let mgr = fixture();
        let wrong_type = js_map(vec![(
            json!("a"),
            json!({ "$$relationship": true, "$class": "org.acme@1.0.0.Owner", "ownerId": "O1" }),
        )]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.VehicleMap", &wrong_type));
        assert!(matches!(err.ported(), Some(e) if e.kind == ErrorKind::Validation));
        assert!(err.to_string().contains("org.acme@1.0.0.Owner"), "{err}");
        let uri = js_map(vec![(json!("a"), json!("resource:org.acme@1.0.0.Vehicle#V1"))]);
        let err = err_of(validate_map(&mgr, "org.acme@1.0.0.VehicleMap", &uri));
        assert!(
            err.to_string().contains("Expected a \"Relationship\""),
            "{err}"
        );
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
        assert!(matches!(err.ported(), Some(e) if e.kind == ErrorKind::Validation));
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
        let map = js_map(vec![(json!("a"), json!("2024-05-01T00:00:00Z"))]);
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
        mgr.load_model(
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
    fn class_and_message(err: &Error) -> (&'static str, String) {
        let Some(contract) = err.ported().cloned() else {
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

    // ---- BC-06 (R1; DV-008 was a V8 TypeError; fixture d444ebcf0cf5a3c23e5ee6dd) ----

    /// A string that reached a relationship array field is reported by its JS
    /// type (TS 5.0.0 called `obj.getFullyQualifiedType()` on it).
    #[test]
    fn a_non_array_non_identifiable_value_on_a_relationship_array_is_a_validation_error() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicles": "not-an-array"
        });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        let (class, message) = class_and_message(&err);
        assert_eq!(class, "ValidationException");
        assert!(
            message.contains("\"vehicles\" with type \"string\"")
                && message.contains("org.acme@1.0.0.Vehicle[]"),
            "{message}"
        );
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicles": 5
        });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        let (class, message) = class_and_message(&err);
        assert_eq!(class, "ValidationException");
        assert!(message.contains("with type \"number\""), "{message}");
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

    /// A `null` relationship array element is reported as `null` (TS 5.0.0
    /// called `value.toString()` on it).
    #[test]
    fn a_null_relationship_array_element_is_a_validation_error() {
        let mgr = fixture();
        let owner = json!({
            "$class": "org.acme@1.0.0.Owner", "ownerId": "O1",
            "vehicles": [null]
        });
        let err = err_of(validate_instance(&mgr, &owner, &ValidateOptions::default()));
        let (class, message) = class_and_message(&err);
        assert_eq!(class, "ValidationException");
        assert!(
            message.contains("has a value of \"null\". Expected a \"Relationship\""),
            "{message}"
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
    // single pass, which is the point of the mode. Since P5-99 it is the
    // one walk, collecting (module doc "Stop or collect").

    /// The walk over `value`, collecting every violation, as diagnostics.
    fn collect_diagnostics(
        mgr: &ModelManager,
        _declared_fqn: &str,
        value: &Value,
        options: &ValidateOptions,
    ) -> ValidationReport {
        ValidationReport::new(
            collect_instance_violations(mgr, value, options, String::new(), true)
                .iter()
                .map(|(pointer, err)| super::super::diagnostic::walk_diagnostic(pointer, err))
                .collect(),
        )
    }

    fn diag_of(result: ValidationReport) -> Diagnostic {
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
        let vehicle = json!({
            "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": 1,
            "pet": "just a string"
        });
        let diag = diag_of(collect_diagnostics(
            &mgr,
            "org.acme@1.0.0.Vehicle",
            &vehicle,
            &ValidateOptions::default(),
        ));
        assert_eq!(diag.code, DiagnosticCode::NotResource);
        assert_eq!(diag.pointer, "/pet");
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

    /// The plan's `NumberValidator::from_bounds` (P5-06 found the old
    /// `{lower, upper}` AST builder's body replaced with JSON `null`
    /// surviving): [`validator_failure_is_diagnosed`] below only ever gives
    /// `rating` an out-of-range value, so a construction error from lost
    /// bounds (the constructor's own "no bounds" rejection) reports the same
    /// `ValidatorFailure` diagnostic the real out-of-range check does. An
    /// in-bounds value tells them apart.
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

    /// [`ModelManager::check_instance`] resolves the declared type from the
    /// value's own `$class`, and [`ModelManager::validate_instance`] reports
    /// the first error, as the free [`validate_instance`] does.
    #[test]
    fn model_manager_entry_points_agree_with_the_free_functions() {
        let mgr = fixture();
        let leaf = json!({ "$class": "org.acme@1.0.0.Leaf", "b": "2", "c": "3" });
        let options = crate::instance::ValidationOptions::default();

        let result = mgr.check_instance(&leaf, &options);
        assert_eq!(
            diag_of(result).code,
            DiagnosticCode::MissingRequiredProperty
        );

        let err = err_of(mgr.validate_instance(&leaf, &options));
        assert!(err.to_string().contains("\"a\""), "{err}");
        let free = err_of(validate_instance(&mgr, &leaf, &ValidateOptions::default()));
        assert_eq!(err.kind(), free.kind());
        assert_eq!(err.code(), free.code());
    }

    /// The `_as` entry points check against the named type: an empty
    /// identifier is the `Factory` error `Serializer.fromJSON` raises.
    #[test]
    fn the_as_entry_points_validate_against_the_named_type() {
        let mgr = fixture();
        let fqn = "org.acme@1.0.0.Vehicle";
        let vehicle = json!({ "vin": "", "mileage": 1 });
        let options = crate::instance::ValidationOptions::default();

        let result = mgr.check_instance_as(fqn, &vehicle, &options);
        assert_eq!(diag_of(result).code, DiagnosticCode::EmptyIdentifier);

        let err = err_of(mgr.validate_instance_as(fqn, &vehicle, &options));
        assert_eq!(err.code(), "factory-newinstance-missingidentifier");
    }

    /// Review finding (P3-03): collecting must not skip the `sizeValidator`
    /// check `check_array` runs for the first-error walk — otherwise the two
    /// modes disagree on whether an over-size array of class-typed elements
    /// is valid. (Since P5-99 both modes are the one walk.)
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

    /// Review finding (P3-03): the `_as` entry points must check the value's
    /// own `$class` against the named type, not silently validate whatever
    /// `value` claims to be (which is what `visit_class_declaration` does on
    /// its own, module doc).
    #[test]
    fn the_as_entry_points_reject_a_value_not_assignable_to_the_named_type() {
        let mgr = fixture();
        let dog_fqn = "org.acme@1.0.0.Dog";
        // A `Base` instance (unrelated to `Dog`/`Animal`), passed against
        // `Dog`'s own fqn.
        let base_instance = json!({ "$class": "org.acme@1.0.0.Base", "a": "x" });
        let options = crate::instance::ValidationOptions::default();

        let result = mgr.check_instance_as(dog_fqn, &base_instance, &options);
        assert!(!result.is_valid(), "{result:?}");
        assert_eq!(diag_of(result).code, DiagnosticCode::NotAssignable);

        let err = err_of(mgr.validate_instance_as(dog_fqn, &base_instance, &options));
        assert!(err.to_string().contains("not assignable"), "{err}");
    }

    // ---- One walk, stop or collect (P5-99, accordproject/concerto-rust#453) ----

    /// The first violation collected is the error the first-error walk
    /// returns (class, code and message), and every collected diagnostic's
    /// message is its error's own, from the catalogue: the same instance
    /// reads the same in both modes.
    #[test]
    fn the_first_violation_collected_is_the_error_thrown() {
        let mgr = fixture();
        let options = ValidateOptions::default();
        for value in [
            json!({ "$class": "org.acme@1.0.0.Leaf", "b": "2", "c": "3", "zzz": "extra" }),
            json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": "ABC12", "mileage": "far", "color": "PURPLE" }),
            json!({ "$class": "org.acme@1.0.0.Vehicle", "vin": " ", "mileage": 1, "tags": ["a", 1, "b", 2] }),
            json!({
                "$class": "org.acme@1.0.0.Garage", "vinField": "bad!",
                "items": [ { "$class": "org.acme@1.0.0.Item" }, { "$class": "org.acme@1.0.0.Item", "name": 1 } ]
            }),
            json!({ "$class": "org.acme@1.0.0.Animal" }),
        ] {
            let thrown = validate_instance(&mgr, &value, &options).unwrap_err();
            let all = collect_instance_violations(&mgr, &value, &options, String::new(), true);
            let first = collect_instance_violations(&mgr, &value, &options, String::new(), false);
            assert_eq!(first.len(), 1, "{value}");
            assert_eq!(all[0], first[0], "{value}");
            let (_, collected) = &first[0];
            assert_eq!(collected.kind(), thrown.kind(), "{value}");
            assert_eq!(collected.code(), thrown.code(), "{value}");
            assert_eq!(collected.to_string(), thrown.to_string(), "{value}");
            assert_eq!(*collected, thrown, "{value}");
            let report = collect_diagnostics(&mgr, "", &value, &options);
            for (diagnostic, (_, err)) in report.diagnostics().iter().zip(&all) {
                assert_eq!(diagnostic.message, err.to_string());
            }
        }
    }

    /// Collecting goes on past each violation with the next key, property,
    /// array element and nested object, each at its own pointer, in walk
    /// order, with TS's wording.
    #[test]
    fn collecting_reports_each_violation_at_its_own_pointer() {
        let mgr = fixture();
        let garage = json!({
            "$class": "org.acme@1.0.0.Garage", "vinField": "bad!",
            "items": [
                { "$class": "org.acme@1.0.0.Item" },
                { "$class": "org.acme@1.0.0.Item", "name": 1, "a/b": true }
            ]
        });
        let found: Vec<(String, String)> = collect_instance_violations(
            &mgr,
            &garage,
            &ValidateOptions::default(),
            String::new(),
            true,
        )
        .into_iter()
        .map(|(pointer, err)| (pointer, err.code().to_string()))
        .collect();
        assert_eq!(
            found,
            [
                ("/vinField", "stringvalidator-validate-regexmismatch"),
                ("/items/0/name", "resourcevalidator-missingrequiredproperty"),
                ("/items/1/a~1b", "resourcevalidator-undeclaredfield"),
                ("/items/1/name", "resourcevalidator-fieldtypeviolation"),
            ]
            .map(|(p, c)| (p.to_string(), c.to_string()))
        );
        let report = collect_diagnostics(&mgr, "", &garage, &ValidateOptions::default());
        assert_eq!(
            report.diagnostics()[1].message,
            "The instance \"org.acme@1.0.0.Item\" is missing the required field \"name\"."
        );
    }

    /// A map's entries are collected one by one, at their keys (an entry
    /// whose key fails is not checked further).
    #[test]
    fn collecting_reports_each_map_entry_at_its_key() {
        let mgr = fixture();
        let map = js_map(vec![
            (json!("a"), json!(1)),
            (json!("b"), json!("ok")),
            (json!(7), json!(true)),
        ]);
        let id = mgr.declaration_id("org.acme@1.0.0.StringMap").unwrap();
        let map_plan = plan::map_plan(&mgr, id);
        let options = ValidateOptions::default();
        let sink = Sink::Collect {
            found: Vec::new(),
            all: true,
        };
        let mut params = Params::new(&mgr, &options, String::new(), sink);
        visit_map_declaration(&mut params, id, &map_plan, &map).unwrap();
        let pointers: Vec<String> = params.into_found().into_iter().map(|(p, _)| p).collect();
        assert_eq!(pointers, ["/a", "/7"]);
    }
}
