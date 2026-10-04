//! Instance validation in one engine call per resource.
//!
//! `ValidatedResource.validate()`, `setPropertyValue` and `addArrayValue`
//! (concerto-core src/model/validatedresource.ts) hand the value they check
//! across once, and the instance validator
//! ([`concerto_core::instance::validate`]) runs over it.
//!
//! # Transport
//!
//! The TS side (src/engine/validate-resource.ts) writes the value straight
//! from the live object into the compact binary layout below, already in
//! the shape the validator reads (the `validate.rs` module doc, "Scope"),
//! exactly what `Instance::to_validator_value` builds from the Serializer's
//! wire value. So there is no JSON text, no `decode_wire` and no
//! `to_validator_value` on this path.
//!
//! The layout is concerto-core `introspect::compact`'s, the model AST's too,
//! with one TS writer (src/engine/wire.ts) and one reader
//! ([`compact_validator_value`]). One tag byte, then:
//!
//! | tag | value |
//! |---|---|
//! | 0 | `null` |
//! | 1 | `false` |
//! | 2 | `true` |
//! | 3 | a double, 8 bytes LE |
//! | 4 | an `i32`, 4 bytes LE |
//! | 5 | a string: `u32` LE byte length, then UTF-8 |
//! | 6 | an array: `u32` LE count, then the items |
//! | 7 | an object: `u32` LE count, then (`u32` LE key length, UTF-8 key, value) per entry |
//!
//! The validator options cross as a bit set (bit 0
//! `convertResourcesToRelationships`, bit 1
//! `permitResourcesForRelationships`) and the root identifier as a string.
//!
//! # Result codes
//!
//! Each binding returns a code instead of throwing, so the common outcomes
//! cost no exception object built in WASM:
//!
//! - [`CODE_VALID`]: the value is valid;
//! - [`CODE_VALIDATION`]: a `Validation` error that is not a validator's
//!   (a validator's carries an `errorType`, BC-39, so it is a
//!   [`CODE_ERROR`]). TS reads its message with
//!   [`validate_error_message`] and throws `new ValidationException(message)`
//!   itself, which is exactly what the error factory builds for that kind
//!   (src/engine/errors.ts);
//! - [`CODE_ERROR`]: any other error. TS throws what [`validate_take_error`]
//!   returns: the exception the unchanged `throw`/error-factory path builds,
//!   so its class is the one every other binding would throw;
//! - [`CODE_UNSUPPORTED`]: the bytes are not a value this layout can carry,
//!   or the engine's model does not have the property TS found. TS throws
//!   `EngineFastPathUnsupported` and runs the `ResourceValidator` visitor
//!   instead, as it does for a value it cannot encode at all.
//!
//! The error stays in a thread-local slot until TS takes it, or until the
//! next call replaces it.

use concerto_core::error::ErrorKind;
use concerto_core::instance::ValidateOptions;
use concerto_core::instance::validate::{validate_instance_from, validate_property_value};
use concerto_core::introspect::compact_validator_value;
use concerto_core::json::Value;
use wasm_bindgen::prelude::*;

use super::{Error, JsResult, ModelManagerHandle, Result, throw, wire_error};

/// The value is valid.
const CODE_VALID: u32 = 0;
/// A `Validation` error (module doc).
const CODE_VALIDATION: u32 = 1;
/// Any other error (module doc).
const CODE_ERROR: u32 = 2;
/// The transport cannot carry the value (module doc).
const CODE_UNSUPPORTED: u32 = 3;
/// A [`ModelManagerHandle::validate_property_by_id`] slot of another epoch.
const CODE_STALE: u32 = 4;

/// A failure of the transport itself, not of the validator.
struct Unsupported(Error);

/// The code for a validation outcome, keeping the error for
/// [`validate_error_message`] or [`validate_take_error`].
fn code_of(result: std::result::Result<Result<()>, Unsupported>) -> u32 {
    let (code, err) = match result {
        Ok(Ok(())) => return CODE_VALID,
        Ok(Err(err)) => {
            let code = match &err {
                // A validator error (BC-39) is a `Validation` error too, but
                // it carries an `errorType`, which only the full `throw` path
                // sets on the exception.
                Error::Contract(c) if c.kind == ErrorKind::Validation && c.validator.is_none() => {
                    CODE_VALIDATION
                }
                _ => CODE_ERROR,
            };
            (code, err)
        }
        Err(Unsupported(err)) => (CODE_UNSUPPORTED, err),
    };
    crate::caches::LAST_ERROR.with(|l| *l.borrow_mut() = Some(err));
    code
}

/// The rendered message of the error behind the last non-zero code, and
/// drops it. Empty when there is none.
#[wasm_bindgen(js_name = validateErrorMessage)]
pub fn validate_error_message() -> String {
    crate::caches::LAST_ERROR.with(|l| match l.borrow_mut().take() {
        Some(Error::Contract(c) | Error::Unsupported(c)) => c.message(),
        _ => String::new(),
    })
}

/// The error behind the last non-zero code, as the exception every other
/// binding throws for it (`run`'s mapping), and drops it. `undefined` when
/// there is none.
#[wasm_bindgen(js_name = validateTakeError)]
pub fn validate_take_error() -> JsValue {
    // The error is taken out, and the borrow released, before `throw`
    // calls the JS error factory, so a re-entrant engine call from the
    // factory cannot find `LAST_ERROR` still borrowed.
    let err = crate::caches::LAST_ERROR.with(|l| l.borrow_mut().take());
    match err {
        Some(err) => throw(err, None),
        None => JsValue::UNDEFINED,
    }
}

fn options_from_flags(flags: u32) -> ValidateOptions {
    ValidateOptions {
        convert_resources_to_relationships: flags & 1 != 0,
        permit_resources_for_relationships: flags & 2 != 0,
    }
}

fn unsupported(reason: &str) -> Unsupported {
    Unsupported(wire_error(format!("a binary wire value: {reason}")))
}

/// The whole of `bytes` as one value, in the validator's shape, through
/// concerto-core's one reader of the layout ([`compact_validator_value`]).
/// Bytes not in the layout cannot cross.
fn decode(bytes: &[u8]) -> std::result::Result<Value, Unsupported> {
    compact_validator_value(bytes).map_err(|e| unsupported(&e.to_string()))
}

#[wasm_bindgen]
impl ModelManagerHandle {
    /// `ValidatedResource.validate()`: the resource in the binary layout
    /// (module doc), validated as `Resource.validate` does
    /// (`validate_instance_from` with the instance's
    /// `getFullyQualifiedIdentifier()` as `root_id`). Returns a result code
    /// (module doc). The `$identifier` write-back of `visitClassDeclaration`
    /// is left to the caller, which owns the live object.
    #[wasm_bindgen(js_name = validateResourceBinary)]
    pub fn validate_resource_binary(&self, bytes: &[u8], root_id: &str, flags: u32) -> u32 {
        code_of(decode(bytes).map(|value| {
            validate_instance_from(
                &self.manager,
                &value,
                &options_from_flags(flags),
                root_id.to_string(),
            )
            .map_err(Error::from)
        }))
    }

    /// `field.accept(this.$validator, parameters)` in
    /// `ValidatedResource.setPropertyValue` and `addArrayValue`: `bytes` is
    /// the value (for `addArrayValue`, the whole new array) in the binary
    /// layout, `class_fqn` the instance's type and `prop_name` the property
    /// TS already found on it. Returns a result code (module doc);
    /// `CODE_UNSUPPORTED` when this manager has no such property.
    #[wasm_bindgen(js_name = validatePropertyBinary)]
    pub fn validate_property_binary(
        &self,
        bytes: &[u8],
        class_fqn: &str,
        prop_name: &str,
        root_id: &str,
        flags: u32,
    ) -> u32 {
        code_of(decode(bytes).and_then(|value| {
            // Validation plan: the property from the plan's name index,
            // validated over the plan. A type whose plan (its chain) does
            // not resolve, or that has no such property, is not one this
            // manager can check.
            let Ok(class_plan) =
                concerto_core::instance::plan::class_plan_by_name(&self.manager, class_fqn)
            else {
                return Err(unsupported("no such property in the engine's model"));
            };
            let Some(index) = class_plan.find(prop_name) else {
                return Err(unsupported("no such property in the engine's model"));
            };
            Ok(validate_property_value(
                &self.manager,
                &class_plan,
                index,
                &value,
                root_id.to_string(),
                &options_from_flags(flags),
            )
            .map_err(Error::from))
        }))
    }

    /// The `[declId, propIndex, epoch]` slot [`Self::validate_property_by_id`]
    /// takes for `class_fqn.prop_name` (epoch's low 32 bits), which TS keeps
    /// per model version. `undefined` where
    /// [`Self::validate_property_binary`] would answer `CODE_UNSUPPORTED`.
    /// Reads only.
    #[wasm_bindgen(js_name = validationPropertySlot)]
    pub fn validation_property_slot(&self, class_fqn: &str, prop_name: &str) -> Option<Vec<u32>> {
        let decl = self.manager.type_declaration(class_fqn).ok()?;
        let class_plan = concerto_core::instance::plan::class_plan(&self.manager, decl).ok()?;
        let index = u32::try_from(class_plan.find(prop_name)?).ok()?;
        Some(vec![decl.index(), index, self.epoch_low()])
    }

    /// [`Self::validate_property_binary`] by the slot
    /// [`Self::validation_property_slot`] gave (`decl_id`, `prop_index`, at
    /// `epoch`), so neither the type's name nor the property's crosses and
    /// neither is looked up by name, with the outcome in the same call:
    ///
    /// - `0` (`CODE_VALID`): the value is valid;
    /// - a string: a `CODE_VALIDATION` error's message, which TS throws
    ///   as `new ValidationException(message)`;
    /// - `3` (`CODE_UNSUPPORTED`): the transport cannot carry the value
    ///   (the caller runs the visitor);
    /// - `4` (`CODE_STALE`): the slot is not one of this epoch's (the
    ///   caller looks it up again);
    /// - any other (`CODE_ERROR`) error is thrown, as the exception every
    ///   other binding throws for it.
    ///
    /// Nothing is kept for `validate_error_message` or
    /// `validate_take_error`.
    #[wasm_bindgen(js_name = validatePropertyById)]
    pub fn validate_property_by_id(
        &self,
        bytes: &[u8],
        decl_id: u32,
        prop_index: u32,
        epoch: u32,
        root_id: &str,
        flags: u32,
    ) -> JsResult<JsValue> {
        if epoch != self.epoch_low() {
            return Ok(JsValue::from(CODE_STALE));
        }
        let decl = concerto_core::model_manager::DeclId::from_index(decl_id);
        let outcome = decode(bytes).and_then(|value| {
            let class_plan = concerto_core::instance::plan::class_plan(&self.manager, decl)
                .map_err(|_| unsupported("no such property in the engine's model"))?;
            let index = usize::try_from(prop_index)
                .ok()
                .filter(|index| *index < class_plan.props.len())
                .ok_or_else(|| unsupported("no such property in the engine's model"))?;
            Ok(validate_property_value(
                &self.manager,
                &class_plan,
                index,
                &value,
                root_id.to_string(),
                &options_from_flags(flags),
            )
            .map_err(Error::from))
        });
        match outcome {
            Ok(Ok(())) => Ok(JsValue::from(CODE_VALID)),
            Ok(Err(err)) => match err {
                Error::Contract(c) if c.kind == ErrorKind::Validation && c.validator.is_none() => {
                    Ok(JsValue::from(c.message()))
                }
                err => Err(throw(err, None)),
            },
            Err(Unsupported(_)) => Ok(JsValue::from(CODE_UNSUPPORTED)),
        }
    }

    /// This handle's epoch, low 32 bits: the stamp of a
    /// [`Self::validation_property_slot`].
    #[allow(clippy::cast_possible_truncation)]
    fn epoch_low(&self) -> u32 {
        self.epoch as u32
    }
}

#[cfg(test)]
mod tests;
