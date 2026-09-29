//! Instance-level Concerto: validating data against a model, as opposed to
//! the model itself.
//!
//! # The stable API
//!
//! [`ModelManager::validate_instance`](crate::ModelManager::validate_instance)
//! (first error) and
//! [`ModelManager::check_instance`](crate::ModelManager::check_instance)
//! (every violation, accordproject/concerto#1239), with their `_as` forms,
//! validate a plain JSON document under [`ValidationOptions`], which carry
//! the accordproject/concerto#1273 options. The collect-all mode reports a
//! [`ValidationReport`] of [`Diagnostic`]s (docs/public-api.md section 5.7).
//!
//! # The JS-compatibility layer
//!
//! Everything else here is public only with the `js-compat` feature, and
//! carries no stability promise (docs/public-api.md section 4):
//!
//! - `validate` (task P3-01, accordproject/concerto-rust#56): the
//!   `ResourceValidator` port, over a value in the validator's shape, where
//!   the `$$` tags stand for the JS values a live `Resource` holds;
//! - `from_json`: `Serializer.fromJSON` over plain JSON, the route the stable
//!   API and the metamodel checks take (P6-01 step 5), with the `Factory`
//!   checks the JS layer shares;
//! - `metamodel` (task P3-04, accordproject/concerto-rust#59):
//!   `BaseModelManager.validateAst` and the `introspect/metamodel.ts`
//!   functions, whose stable names are in [`crate::metamodel`];
//! - `model`: the model queries the instance layer makes (`getType`, then
//!   `isX()`), with TS's semantics for each declaration kind;
//! - `dayjs` and `resource_id`: the `DateTime` and relationship URI
//!   semantics `from_json` reads plain JSON with, which the JS object model
//!   shares.
//!
//! The JS object model itself (task P3-01b, accordproject/concerto-rust#124:
//! the TS `Resource` objects, the JS values they hold, and the `Serializer`,
//! `Factory`, `JSONPopulator` and `JSONGenerator`, with #1273's
//! `DeserializeOptions` from task P3-02) is the `concerto-core-js` crate,
//! built on this seam for the WASM binding (P6-01 step 5).
//!
//! Without the feature these modules are crate-private. Crate-private, they
//! lose clippy's exemption for exported names, which the JS-facing names
//! (`from_json::from_json`) keep through the `allow`s below.

/// Declares a module that is `pub` with the `js-compat` feature and
/// crate-private without it (docs/public-api.md section 4.6).
macro_rules! js_compat_mod {
    ($name:ident) => {
        #[cfg(feature = "js-compat")]
        pub mod $name;
        #[cfg(not(feature = "js-compat"))]
        #[allow(dead_code, unused_imports)]
        #[allow(clippy::enum_variant_names, clippy::wrong_self_convention)]
        pub(crate) mod $name;
    };
}

js_compat_mod!(dayjs);
mod diagnostic;
js_compat_mod!(from_json);
js_compat_mod!(metamodel);
js_compat_mod!(model);
mod options;
js_compat_mod!(resource_id);
js_compat_mod!(validate);

#[allow(deprecated)]
pub use diagnostic::ValidationResult;
pub use diagnostic::{Diagnostic, DiagnosticCode, Severity, ValidationReport};
pub use options::ValidationOptions;

#[cfg(feature = "js-compat")]
pub use from_json::InstanceEnv;
#[cfg(feature = "js-compat")]
pub use metamodel::{
    METAMODEL_NAMESPACE, check_ast_shape, model_manager_from_meta_model, validate_ast,
    validate_meta_model_instance, validate_metamodel,
};
#[cfg(feature = "js-compat")]
pub use validate::{ValidateOptions, validate_instance};
