//! The op registry: replays one fixture's `op` against the Rust engine.
//!
//! `accordproject/concerto`'s `migration/oracle/lib/ops.js` is the full op
//! catalogue the oracle records (README "Ops"). This harness (task P1-07)
//! implements the family that is both pure (needs no `ModelManager` or
//! other collaborator reconstruction — OD not yet settled for those, see
//! `decode.rs`) and already ported to Rust per `PORTING.md`: the
//! `ModelUtil` statics that take only plain-data arguments —
//! `getShortName`, `getNamespace`, `parseNamespace`,
//! `importFullyQualifiedNames`, `isPrimitiveType`, `capitalizeFirstLetter`,
//! `isValidIdentifier`, `getFullyQualifiedName`,
//! `removeNamespaceVersionFromFullyQualifiedName`, `isSystemProperty`,
//! `isPrivateSystemProperty`, `isValidMapKey`, `isValidMapValue`.
//!
//! The other five `ModelUtil` statics (`isAssignableTo`, `isEnum`, `isMap`,
//! `isScalar`, `isValidMapKeyScalar`) take a model manager collaborator
//! (`ResolutionContext` in the Rust port) and every other op family needs a
//! loaded `ModelManager`, a `Factory`/`Serializer` instance, or both: none
//! of that reconstruction from a fixture's `mm`/`mfref`/`mfnew`/`typed`
//! encoding is implemented yet (`decode.rs`), so those ops fall through to
//! [`Dispatch::Unsupported`]. A later task extends this registry rather than
//! replacing it (the corpus and the comparison machinery in `fixture.rs`,
//! `compare.rs` and `report.rs` do not change).

use concerto_core::error::{ConcertoError, ErrorKind};
use concerto_core::model_util::{self, ParsedNamespace};
use serde_json::{Value, json};

use super::decode::{self, Unsupported};
use super::fixture::Inputs;

/// An error outcome in the oracle's `outcome.error` shape (README "Fixture
/// schema"), built from a [`ConcertoError`] the same way the TS reference's
/// exception constructors would (PORTING.md section 2).
pub struct OracleError {
    pub class: &'static str,
    pub message: String,
    pub location: Option<Value>,
    pub component: Option<&'static str>,
}

/// What running a *supported* op produced.
pub enum ExecOutcome {
    Ok(Value),
    Err(OracleError),
}

/// The result of dispatching one fixture's op.
pub enum Dispatch {
    Ran(ExecOutcome),
    /// This op, or this fixture's particular arguments, are not implemented
    /// by the harness yet (see the module doc). Carries a short reason.
    Unsupported(String),
}

/// Runs `op` with `inputs.args` against the Rust engine, if the harness
/// implements it.
pub fn exec(op: &str, inputs: &Inputs) -> Dispatch {
    let args = match decode::decode_args(&inputs.args) {
        Ok(a) => a,
        Err(Unsupported(reason)) => return Dispatch::Unsupported(reason),
    };

    macro_rules! bad_args {
        () => {
            return Dispatch::Unsupported(format!("{op}: arguments did not decode for this op"))
        };
    }

    let outcome: Result<Value, ConcertoError> = match op {
        "ModelUtil.getShortName" => {
            let arg0 = decode::arg(&args, 0);
            let Ok(fqn) = decode::as_str(&arg0) else {
                bad_args!()
            };
            Ok(Value::String(model_util::get_short_name(fqn).to_string()))
        }
        "ModelUtil.getNamespace" => {
            let arg0 = decode::arg(&args, 0);
            let Ok(fqn) = decode::as_nullable_str(&arg0) else {
                bad_args!()
            };
            model_util::get_namespace(fqn).map(|s| Value::String(s.to_string()))
        }
        "ModelUtil.parseNamespace" => {
            let arg0 = decode::arg(&args, 0);
            let Ok(ns) = decode::as_nullable_str(&arg0) else {
                bad_args!()
            };
            let Ok(disable) = decode::disable_version_parsing(args.get(1)) else {
                bad_args!()
            };
            model_util::parse_namespace(ns, disable).map(encode_parsed_namespace)
        }
        "ModelUtil.importFullyQualifiedNames" => {
            let arg0 = decode::arg(&args, 0);
            let imp = decode::as_value(&arg0).cloned();
            model_util::import_fully_qualified_names(imp.as_ref())
                .map(|names| Value::Array(names.into_iter().map(Value::String).collect()))
        }
        "ModelUtil.isPrimitiveType" => {
            let arg0 = decode::arg(&args, 0);
            let Ok(type_name) = decode::as_str(&arg0) else {
                bad_args!()
            };
            Ok(Value::Bool(model_util::is_primitive_type(type_name)))
        }
        "ModelUtil.capitalizeFirstLetter" => {
            let arg0 = decode::arg(&args, 0);
            let Ok(s) = decode::as_str(&arg0) else {
                bad_args!()
            };
            Ok(Value::String(model_util::capitalize_first_letter(s)))
        }
        "ModelUtil.isValidIdentifier" => {
            let arg0 = decode::arg(&args, 0);
            let Ok(name) = decode::as_str(&arg0) else {
                bad_args!()
            };
            Ok(Value::Bool(model_util::is_valid_identifier(name)))
        }
        "ModelUtil.getFullyQualifiedName" => {
            let arg0 = decode::arg(&args, 0);
            let arg1 = decode::arg(&args, 1);
            let Ok(ns) = decode::as_str(&arg0) else {
                bad_args!()
            };
            let Ok(type_name) = decode::as_str(&arg1) else {
                bad_args!()
            };
            Ok(Value::String(model_util::get_fully_qualified_name(
                ns, type_name,
            )))
        }
        "ModelUtil.removeNamespaceVersionFromFullyQualifiedName" => {
            let arg0 = decode::arg(&args, 0);
            let Ok(fqn) = decode::as_nullable_str(&arg0) else {
                bad_args!()
            };
            model_util::remove_namespace_version_from_fully_qualified_name(fqn).map(Value::String)
        }
        "ModelUtil.isSystemProperty" => {
            let arg0 = decode::arg(&args, 0);
            let Ok(name) = decode::as_str(&arg0) else {
                bad_args!()
            };
            Ok(Value::Bool(model_util::is_system_property(name)))
        }
        "ModelUtil.isPrivateSystemProperty" => {
            let arg0 = decode::arg(&args, 0);
            let Ok(name) = decode::as_str(&arg0) else {
                bad_args!()
            };
            Ok(Value::Bool(model_util::is_private_system_property(name)))
        }
        "ModelUtil.isValidMapKey" => {
            let arg0 = decode::arg(&args, 0);
            let key = decode::as_value(&arg0).cloned();
            model_util::is_valid_map_key(key.as_ref()).map(Value::Bool)
        }
        "ModelUtil.isValidMapValue" => {
            let arg0 = decode::arg(&args, 0);
            let value = decode::as_value(&arg0).cloned();
            model_util::is_valid_map_value(value.as_ref()).map(Value::Bool)
        }
        _ => return Dispatch::Unsupported(format!("op not implemented: {op}")),
    };

    match outcome {
        Ok(value) => Dispatch::Ran(ExecOutcome::Ok(value)),
        Err(err) => Dispatch::Ran(ExecOutcome::Err(to_oracle_error(&err))),
    }
}

/// `ModelUtil.parseNamespace`'s return value, in the shape TS returns it
/// (`{name}`, or `{name, escapedNamespace, version, versionParsed}`).
fn encode_parsed_namespace(parsed: ParsedNamespace) -> Value {
    match parsed {
        ParsedNamespace::NameOnly { name } => json!({ "name": name }),
        ParsedNamespace::Full {
            name,
            escaped_namespace,
            version,
            version_parsed,
        } => json!({
            "name": name,
            "escapedNamespace": escaped_namespace,
            "version": version,
            // README "Value encoding": a returned handle codec.js does not
            // model specially (no `mm`/`typed`/... kind fits a node-semver
            // `SemVer` instance) is recorded as the generic outcome-only
            // summary `{"@@oracle":"object","ctor":"<constructor name>"}`,
            // not a deep field-by-field encoding — confirmed against a real
            // recorded fixture (`ModelUtil.parseNamespace`, task P1-07).
            // This harness only checks that a `SemVer` was returned at all
            // when `version` is present; it does not compare node-semver's
            // own fields (major/minor/patch/prerelease/...), which have no
            // Rust port to check against yet.
            "versionParsed": version_parsed.as_ref().map(|_| json!({
                "@@oracle": "object",
                "ctor": "SemVer",
            })),
        }),
    }
}

/// Builds the oracle's `outcome.error` shape from a [`ConcertoError`], the
/// same fields `ContractError` carries (PORTING.md section 2.1):
/// `kind.ts_class()` for `class`, [`concerto_core::error::ContractError::final_message`]
/// for `message` (its doc comment: "Used by the native oracle harness
/// only"), the AST `location` verbatim, and `component`.
fn to_oracle_error(err: &ConcertoError) -> OracleError {
    match err {
        ConcertoError::Contract(ce) => OracleError {
            class: ce.kind.ts_class(),
            message: ce.final_message(),
            location: ce.location.clone(),
            component: ce.component(),
        },
        ConcertoError::TypeNotFound { type_name } => OracleError {
            class: ErrorKind::TypeNotFound.ts_class(),
            message: format!("Type \"{type_name}\" not found."),
            location: None,
            component: Some("@accordproject/concerto-core"),
        },
        ConcertoError::IllegalModel { message, .. } => OracleError {
            class: ErrorKind::IllegalModel.ts_class(),
            message: message.clone(),
            location: None,
            component: Some("@accordproject/concerto-core"),
        },
    }
}
