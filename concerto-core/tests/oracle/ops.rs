//! The op registry: replays one fixture's `op` against the Rust engine.
//!
//! `accordproject/concerto`'s `migration/oracle/lib/ops.js` is the full op
//! catalogue the oracle records (README "Ops"). An op gets a dispatch entry
//! here when the Rust engine has a counterpart for it; each Phase 2/3 task
//! adds the entries for the members it ports, in the same PR (PORTING.md
//! 6.2). Every other op is reported `unsupported`, with the task the seam
//! ledger plans for it, and never counted as a pass or a fail.
//!
//! Dispatched today:
//!
//! - **`ModelUtil`**, all 18 statics (ported by the P0-04b trial). The five
//!   that take collaborators (`isAssignableTo`, `isEnum`, `isMap`,
//!   `isScalar`, `isValidMapKeyScalar`) run over a replayed
//!   [`ModelManager`](concerto_core::model_manager::ModelManager) through
//!   its `ResolutionContext`, with the model file, declaration and property
//!   arguments as `Node` handles (PORTING.md 6.2).
//! - **`TypeNotFoundException.new`**, which needs no receiver.
//! - **The model manager's load path** (`recipe.rs` has the mapping):
//!   `ModelManager.new`, `BaseModelManager.new`, `AstModelManager.new`, and
//!   the ops `addCTOModel`, `addModel`, `addModelFile`, `addModelFiles`,
//!   `validateModelFiles`, `clearModelFiles`, `fromAst`, which are also the
//!   recipe steps every other model-manager fixture is rebuilt with; plus
//!   the queries with a direct Rust counterpart: `getNamespaces`,
//!   `getAst(false, …)` and `getType` (`get_declaration`). The Rust
//!   `ModelManager` is still pre-port (P2-08 ports it), so these fixtures
//!   report its differences from TS as per-rule failures, which is the
//!   point: they are what P2-08 and the introspection tasks have to close.
//! - **`ScalarDeclaration`** `toString`, `getType`, `getValidator` and
//!   `getDefaultValue`, over the trial's port of `ScalarDeclaration.process`,
//!   for a receiver loaded into a registered `ModelManager` (a `declref`
//!   handle) *and* for one built directly with `new ScalarDeclaration(modelFile,
//!   ast)` and never added to its model file (a `declnew` recipe, rebuilt by
//!   `ScalarDeclaration::build_standalone`, `recipe.rs`); and `new` itself,
//!   over the same `build_standalone` (P2-05), which replays the constructor
//!   without going through a registered `ModelManager` (the receiver is an
//!   `mfnew` recipe argument, never a `declref`, so it needs no arena
//!   handle). Every other declaration kind's `declnew` still needs P4-07,
//!   which closes this in general.
//! - **`Resource.validate`**, plus the read-only `Typed`/`Identifiable`/
//!   `Relationship`/`Resource` accessors ([`instance_op`]'s doc has the
//!   full list), over an oracle `"typed"` receiver decoded into
//!   [`recipe::DecodedInstance`] (`recipe.rs`'s `Session::typed`, task
//!   P3-01 review, `accordproject-concerto-rust#56` follow-up).
//!   `Factory`/`Serializer`/`JSONPopulator`/`JSONGenerator` and every
//!   receiver-mutating op (`setPropertyValue`, `addArrayValue`,
//!   `setIdentifier`) are not ported yet, so their fixtures stay
//!   `unsupported`.
//! - **`MapDeclaration`** `declarationKind`, `getKey`, `getValue`,
//!   `isMapDeclaration`, `toString` and `validate`; **`MapKeyType`**/
//!   **`MapValueType`** `getType`, `getNamespace`, `getParent`, `toString` and
//!   `validate` (P2-06), over a `recipe::Arg::MapPart` handle (`recipe.rs`)
//!   since this engine reads a map's key and value as plain accessors on
//!   `MapDeclaration` rather than as their own registered declarations;
//!   **`ModelManager.getMapDeclarations`**, a generic query any model
//!   manager already answers (`ClassDeclaration.isMapDeclaration` is
//!   dispatched with the rest of the `ClassDeclaration` family, P2-03). A
//!   fixture whose target is an
//!   unregistered `ModelFile` (`mfnew`, e.g. most of
//!   `MapDeclaration.validate`) stays `unsupported`, owned by P2-08's
//!   `ModelFile.new`.

use concerto_core::error::{ConcertoError, ErrorKind};
use concerto_core::introspect::declaration::ClassDeclaration;
use concerto_core::introspect::model_file::ModelFile;
use concerto_core::introspect::property::Property;
use concerto_core::introspect::scalar::ScalarValidator;
use concerto_core::introspect::validators::Validator;
use concerto_core::introspect::{
    Declaration, DeclarationKind, MapDeclaration, Named, Typed, Validate,
};
use concerto_core::model_manager::{DeclId, ModelManager, Node, PropId, ResolutionContext};
use concerto_core::model_util::{self, ParsedNamespace};
use concerto_core::validation;
use serde_json::{Value, json};

use super::Harness;
use super::decode::{self, Unsupported};
use super::fixture::Inputs;
use super::recipe::{self, Arg, Fault, Faulty, M, Replayed, Session};

/// An error outcome in the oracle's `outcome.error` shape (README "Fixture
/// schema"), built from a [`ConcertoError`] the same way the TS reference's
/// exception constructors would (PORTING.md section 2).
#[derive(Debug, Clone)]
pub struct OracleError {
    pub class: String,
    pub message: String,
    pub location: Option<Value>,
    pub component: Option<String>,
}

impl OracleError {
    /// An error recorded in the CTO cache, replayed as the outcome: Rust
    /// never parses CTO, so never raises a `ParseException` (PORTING.md
    /// 2.3). The cache keeps `{class, message, location}` for whatever
    /// `build-cto-cache.js`'s `parseOne` caught; an entry without a string
    /// `class` or `message` is a harness error.
    ///
    /// Two things the cache does not keep are restored, for a
    /// `ParseException` only: its `component`, which it inherits from
    /// concerto-util's `BaseException` because concerto-cto passes none
    /// (`component || packageJson.name`), and its location's
    /// `source: undefined` ([`restore_location_source`]). Any other class
    /// keeps the entry's own `component` (JS `null` when absent, as
    /// `codec.js`'s `encodeError` records an `Error` without one) and its
    /// location as cached.
    pub fn from_cached_error(error: &Value) -> Result<Self, String> {
        let text = |key: &str| {
            error
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("CTO cache error entry without a string `{key}`: {error}"))
        };
        let class = text("class")?;
        let message = text("message")?;
        let is_parse_exception = class == "ParseException";
        let location = error
            .get("location")
            .filter(|l| !l.is_null())
            .cloned()
            .map(|l| {
                if is_parse_exception {
                    restore_location_source(l)
                } else {
                    l
                }
            });
        let recorded_component = error
            .get("component")
            .and_then(Value::as_str)
            .map(str::to_string);
        let component = if is_parse_exception {
            recorded_component.or_else(|| Some(PARSE_EXCEPTION_COMPONENT.to_string()))
        } else {
            recorded_component
        };
        Ok(Self {
            class,
            message,
            location,
            component,
        })
    }

    pub fn to_value(&self) -> Value {
        json!({
            "class": self.class,
            "message": self.message,
            "location": self.location.clone().unwrap_or(Value::Null),
            "component": self.component,
        })
    }
}

/// The cache writes a `ParseException`'s location through `JSON.stringify`,
/// which drops a key whose value is `undefined`. Peggy's `location()` is
/// always `{source, start, end}`, and concerto-cto passes no grammar source,
/// so every recorded `ParseException` location has `source: undefined` (all
/// 225 in the corpus do): put the key back as the oracle encodes it.
fn restore_location_source(mut location: Value) -> Value {
    if let Value::Object(map) = &mut location
        && map.contains_key("start")
        && !map.contains_key("source")
    {
        map.insert("source".into(), recipe::undefined());
    }
    location
}

/// `@accordproject/concerto-util`'s package name, the default `component`
/// of every `BaseException` (`baseexception.js`: `component ||
/// packageJson.name`), which `concerto-cto`'s `ParseException` inherits.
const PARSE_EXCEPTION_COMPONENT: &str = "@accordproject/concerto-util";

/// The result of dispatching one fixture's op.
pub enum Dispatch {
    /// The op ran: `{"ok": …}` or `{"error": …}`, in the oracle's outcome
    /// shape.
    Ran(Value),
    Fault(Fault),
}

fn ran(outcome: recipe::Outcome) -> Dispatch {
    Dispatch::Ran(match outcome {
        Ok(value) => json!({ "ok": value }),
        Err(error) => json!({ "error": error.to_value() }),
    })
}

fn from_engine<T>(result: Result<T, ConcertoError>, encode: impl FnOnce(T) -> Value) -> Dispatch {
    ran(result.map(encode).map_err(|e| to_oracle_error(&e)))
}

fn unsupported(reason: impl Into<String>) -> Dispatch {
    Dispatch::Fault(Fault::Unsupported(reason.into()))
}

fn option_bool(value: Option<bool>) -> Value {
    value.map_or_else(recipe::undefined, Value::Bool)
}

/// Runs `op` against the Rust engine, if it has a dispatch entry.
pub fn exec(h: &Harness, op: &str, inputs: &Inputs) -> Dispatch {
    if let Some(dispatch) = exec_plain(op, inputs) {
        return dispatch;
    }
    match exec_handles(h, op, inputs) {
        Ok(dispatch) => dispatch,
        Err(fault) => Dispatch::Fault(fault),
    }
}

/// The ops whose arguments are plain data only. `None` for any other op.
fn exec_plain(op: &str, inputs: &Inputs) -> Option<Dispatch> {
    const PLAIN_OPS: [&str; 14] = [
        "ModelUtil.getShortName",
        "ModelUtil.getNamespace",
        "ModelUtil.parseNamespace",
        "ModelUtil.importFullyQualifiedNames",
        "ModelUtil.isPrimitiveType",
        "ModelUtil.capitalizeFirstLetter",
        "ModelUtil.isValidIdentifier",
        "ModelUtil.getFullyQualifiedName",
        "ModelUtil.removeNamespaceVersionFromFullyQualifiedName",
        "ModelUtil.isSystemProperty",
        "ModelUtil.isPrivateSystemProperty",
        "ModelUtil.isValidMapKey",
        "ModelUtil.isValidMapValue",
        "TypeNotFoundException.new",
    ];
    if !PLAIN_OPS.contains(&op) {
        return None;
    }
    let args = match decode::decode_args(&inputs.args) {
        Ok(a) => a,
        Err(Unsupported(reason)) => return Some(unsupported(reason)),
    };

    macro_rules! bad_args {
        () => {
            return Some(unsupported(format!(
                "{op}: arguments did not decode for this op"
            )))
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
        // `new TypeNotFoundException(typeName, message?, component?)`
        // (src/typenotfoundexception.ts): pure string handling over its own
        // arguments. The TS reference does not throw here: the constructed
        // exception is itself the `ok` value, which codec.js encodes as
        // `{"@@oracle": "error", "error": {...}}` (task P1-07 review;
        // `TypeNotFoundException #constructor`).
        "TypeNotFoundException.new" => {
            let arg0 = decode::arg(&args, 0);
            let Ok(type_name) = decode::as_str(&arg0) else {
                bad_args!()
            };
            let arg1 = decode::arg(&args, 1);
            let Ok(message) = decode::as_nullable_str(&arg1) else {
                bad_args!()
            };
            let arg2 = decode::arg(&args, 2);
            let Ok(component) = decode::as_nullable_str(&arg2) else {
                bad_args!()
            };
            // TS: `if (!message) { message = <default> }` and
            // `component || '@accordproject/concerto-core'`, both falsy
            // checks. The default is the `typenotfounderror-defaultmessage`
            // catalogue entry, whose one placeholder has no `$`-pattern
            // hazards, so this literal format matches it byte for byte.
            let message = message
                .filter(|m| !m.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("Type \"{type_name}\" not found."));
            let component = component
                .filter(|c| !c.is_empty())
                .unwrap_or("@accordproject/concerto-core");
            Ok(json!({
                M: "error",
                "error": {
                    "class": "TypeNotFoundException",
                    "message": message,
                    "location": Value::Null,
                    "component": component,
                }
            }))
        }
        _ => unreachable!("PLAIN_OPS lists every arm"),
    };
    Some(from_engine(outcome, |v| v))
}

/// Model-manager steps that are also ops (README "Ops").
const MM_STEP_OPS: [&str; 7] = [
    "addCTOModel",
    "addModel",
    "addModelFile",
    "addModelFiles",
    "validateModelFiles",
    "clearModelFiles",
    "fromAst",
];

/// The ops whose inputs hold model managers or their handles.
fn exec_handles(h: &Harness, op: &str, inputs: &Inputs) -> Faulty<Dispatch> {
    let (class, member) = op.split_once('.').unwrap_or((op, ""));

    // `Decorated`'s target may be an `mfref` (P2-07): the generic decode
    // below turns that into an `Arg::File`, which drops the model manager it
    // belongs to. Handled separately, ahead of the generic decode.
    if class == "Decorated" && matches!(member, "getDecorator" | "getDecorators") {
        return decorated_op(h, member, inputs);
    }

    let dispatched = match (class, member) {
        ("ModelManager" | "BaseModelManager" | "AstModelManager", "new") => true,
        ("ModelManager", m) => {
            MM_STEP_OPS.contains(&m)
                || matches!(
                    m,
                    "getNamespaces" | "getAst" | "getType" | "getMapDeclarations"
                )
        }
        ("ModelUtil", m) => matches!(
            m,
            "isAssignableTo" | "isEnum" | "isMap" | "isScalar" | "isValidMapKeyScalar"
        ),
        ("ScalarDeclaration", m) => {
            matches!(
                m,
                "new" | "toString" | "getType" | "getValidator" | "getDefaultValue"
            )
        }
        ("MapDeclaration", m) => matches!(
            m,
            "declarationKind"
                | "getKey"
                | "getValue"
                | "isMapDeclaration"
                | "toString"
                | "validate"
        ),
        ("NumberValidator" | "StringValidator", m) => matches!(m, "validate" | "compatibleWith"),
        ("CollectionSizeValidator", m) => {
            matches!(m, "compatibleWith" | "getMinSize" | "getMaxSize")
        }
        ("ClassDeclaration", m) => matches!(
            m,
            "isAbstract"
                | "isIdentified"
                | "isSystemIdentified"
                | "isExplicitlyIdentified"
                | "getIdentifierFieldName"
                | "getOwnProperties"
                | "getProperties"
                | "getProperty"
                | "getSuperType"
                | "getSuperTypeDeclaration"
                | "getAllSuperTypeDeclarations"
                | "getAssignableClassDeclarations"
                | "getDirectSubclasses"
                | "getNestedProperty"
                | "isEnum"
                | "isEvent"
                | "isMapDeclaration"
                | "toString"
                | "validate"
        ),
        ("MapKeyType" | "MapValueType", m) => {
            matches!(
                m,
                "getType" | "getNamespace" | "getParent" | "toString" | "validate"
            )
        }
        ("Declaration", m) => matches!(
            m,
            "getFullyQualifiedName" | "getName" | "getNamespace" | "getModelFile"
        ),
        ("Decorator", m) => matches!(m, "getArguments" | "validate"),
        // P2-04: `Property`'s own members (Field, RelationshipDeclaration and
        // EnumValueDeclaration all inherit these unchanged, module doc on
        // `property_op`), plus the `Field`-only members (getDefaultValue,
        // getValidator, isTypeScalar, getScalarField — never reached by a
        // relationship or an enum value, which are not `Field`s in TS) and
        // the two `toString` overrides.
        ("Property", m) => matches!(
            m,
            "getName"
                | "getType"
                | "isArray"
                | "isOptional"
                | "getFullyQualifiedTypeName"
                | "getFullyQualifiedName"
                | "getNamespace"
                | "getParent"
                | "getSizeValidator"
                | "isPrimitive"
                | "isTypeEnum"
        ),
        ("Field", m) => matches!(
            m,
            "getDefaultValue" | "getValidator" | "isTypeScalar" | "getScalarField"
        ),
        ("RelationshipDeclaration", "toString") => true,
        ("EnumDeclaration", "toString") => true,
        // P2-08: every `ModelFile` member the oracle corpus reaches
        // (`filter`, `getConceptDeclarations`, `getMapDeclarations` and
        // `getExternalImports` have no fixtures, module doc on `ops.rs`).
        ("ModelFile", m) => matches!(
            m,
            "new"
                | "getAllDeclarations"
                | "getAssetDeclaration"
                | "getAssetDeclarations"
                | "getAst"
                | "getClassDeclarations"
                | "getConcertoVersion"
                | "getDefinitions"
                | "getEnumDeclarations"
                | "getEventDeclaration"
                | "getEventDeclarations"
                | "getFullyQualifiedTypeName"
                | "getImportURI"
                | "getImportedType"
                | "getImports"
                | "getModelManager"
                | "getName"
                | "getNamespace"
                | "getParticipantDeclaration"
                | "getScalarDeclarations"
                | "getTransactionDeclaration"
                | "getTransactionDeclarations"
                | "getType"
                | "isDefined"
                | "isExternal"
                | "isImportedType"
                | "isLocalType"
                | "isModelFile"
                | "isSystemModelFile"
                | "resolveImport"
                | "validate"
        ),
        ("Introspector", m) => {
            matches!(
                m,
                "getClassDeclaration" | "getClassDeclarations" | "getModelManager"
            )
        }
        // P3-01 review (task `accordproject-concerto-rust#56` follow-up):
        // the instance ops dispatched over a `"typed"` receiver
        // ([`instance_op`]'s doc has the full list and its TS source).
        ("Resource", m) => matches!(
            m,
            "validate" | "toString" | "isResource" | "isConcept" | "isIdentifiable" | "instanceOf"
        ),
        ("Identifiable", m) => matches!(
            m,
            "getIdentifier"
                | "getFullyQualifiedIdentifier"
                | "getTimestamp"
                | "toURI"
                | "isRelationship"
                | "isResource"
        ),
        ("Relationship", m) => matches!(m, "toString" | "isRelationship" | "fromURI"),
        ("Typed", m) => matches!(
            m,
            "getType" | "getNamespace" | "getFullyQualifiedType" | "getClassDeclaration"
        ),
        _ => false,
    };
    if !dispatched {
        return Ok(unsupported(format!("{op} is not ported yet")));
    }

    let mut session = Session::new(h);
    let target = match &inputs.target {
        Some(t) => Some(session.decode(t, None)?),
        None => None,
    };
    let args = inputs
        .args
        .iter()
        .map(|a| session.decode(a, None))
        .collect::<Faulty<Vec<_>>>()?;

    // `new ScalarDeclaration(modelFile, ast)` (PORTING.md 6.2): the receiver
    // is built directly from a `ModelFile` recipe argument, never registered
    // with a model manager, so it never reaches `declref` (which has no
    // handle for an unregistered declaration; `Fault::Blocked` cites
    // `ScalarDeclaration.new` for exactly that case elsewhere).
    if class == "ScalarDeclaration" && member == "new" {
        let Some(Arg::File(file)) = args.first() else {
            return Ok(unsupported(
                "ScalarDeclaration.new with a model file argument that is not a model file",
            ));
        };
        let Some(Arg::Plain(ast)) = args.get(1) else {
            return Ok(unsupported(
                "ScalarDeclaration.new with a declaration AST that is not plain data",
            ));
        };
        let namespace = file
            .ast
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or("");
        return Ok(from_engine(
            concerto_core::introspect::ScalarDeclaration::validate_new(
                namespace,
                file.file_name.as_deref(),
                ast,
            ),
            |fqn| {
                json!({
                    M: "Declaration",
                    "ctor": "ScalarDeclaration",
                    "fqn": fqn,
                })
            },
        ));
    }

    // TS: `new ModelFile(modelManager, ast, definitions, fileName)` (P2-08):
    // args[0] (the model manager) has already been fully replayed by
    // `session.decode` above, for state-divergence parity, even though the
    // constructed `ModelFile` does not itself need it for these arguments —
    // only `getModelManager`/`getType` do, later, through `file.mm_index`.
    if class == "ModelFile" && member == "new" {
        return Ok(model_file_new(&args));
    }

    if member == "new" {
        let kind = match class {
            "ModelManager" => recipe::Kind::ModelManager,
            "BaseModelManager" => recipe::Kind::BaseModelManager,
            _ => recipe::Kind::AstModelManager,
        };
        let options = match args.first() {
            None => recipe::undefined(),
            Some(Arg::Plain(v)) => v.clone(),
            Some(_) => return Ok(unsupported("model manager options that are not plain data")),
        };
        if args.len() > 1 {
            return Ok(unsupported("a custom processFile argument"));
        }
        let r = Replayed::new(kind, &options)?;
        return Ok(ran(Ok(r.summary())));
    }

    match class {
        "ModelManager" => {
            let Some(Arg::Mm(index)) = target else {
                return Err(Fault::Unsupported(
                    "a model manager op whose receiver is not a model manager recipe".into(),
                ));
            };
            if MM_STEP_OPS.contains(&member) {
                let r = &mut session.pool[index];
                return Ok(ran(r.apply(h, member, &args)?));
            }
            let r = &session.pool[index];
            Ok(model_manager_query(r, member, &args))
        }
        "ModelUtil" => model_util_with_context(&session, member, &args),
        "ScalarDeclaration" => match target {
            Some(Arg::Decl(index, id)) => {
                let r = &session.pool[index];
                let Some(Declaration::Scalar(scalar)) = r.mm.declaration(id) else {
                    return Err(Fault::Divergence(
                        "state divergence: the declaration did not load as a scalar".into(),
                    ));
                };
                Ok(match member {
                    "toString" => from_engine(
                        r.mm.get_fully_qualified_name(&Node::Declaration(id)),
                        |fqn| {
                            Value::String(concerto_core::introspect::ScalarDeclaration::to_string(
                                &fqn,
                            ))
                        },
                    ),
                    "getType" => ran(Ok(scalar
                        .scalar_type()
                        .map_or(Value::Null, |t| Value::String(t.to_string())))),
                    "getDefaultValue" => {
                        ran(Ok(scalar.default_value().cloned().unwrap_or(Value::Null)))
                    }
                    _ => ran(Ok(scalar_validator_summary(scalar.validator()))),
                })
            }
            // `new ScalarDeclaration(modelFile, ast)`, never added to
            // `modelFile`: `recipe.rs`'s `declnew` decoding already ran
            // `ScalarDeclaration::build_standalone` (the receiver's
            // construction), so the four getters below just read its result.
            Some(Arg::DeclNew { fqn, processed }) => Ok(match member {
                "toString" => ran(Ok(Value::String(
                    concerto_core::introspect::ScalarDeclaration::to_string(&fqn),
                ))),
                "getType" => ran(Ok(processed
                    .scalar_type
                    .map_or(Value::Null, |t| Value::String(t.to_string())))),
                "getDefaultValue" => ran(Ok(processed.default_value.unwrap_or(Value::Null))),
                _ => ran(Ok(scalar_validator_summary(processed.validator.as_ref()))),
            }),
            _ => Err(Fault::Unsupported(
                "a ScalarDeclaration receiver that is not a declref or declnew".into(),
            )),
        },
        "MapDeclaration" => {
            let Some(Arg::Decl(index, id)) = target else {
                return Err(Fault::Unsupported(
                    "a MapDeclaration receiver that is not a declref".into(),
                ));
            };
            let r = &session.pool[index];
            let Some(Declaration::Map(map)) = r.mm.declaration(id) else {
                return Err(Fault::Divergence(
                    "state divergence: the declaration did not load as a map".into(),
                ));
            };
            Ok(match member {
                "declarationKind" => ran(Ok(Value::String(map.declaration_kind().to_string()))),
                "isMapDeclaration" => ran(Ok(Value::Bool(true))),
                "toString" => from_engine(
                    r.mm.get_fully_qualified_name(&Node::Declaration(id)),
                    |fqn| Value::String(MapDeclaration::to_string(&fqn)),
                ),
                "getKey" => ran(Ok(r.map_part_summary(id, true).unwrap_or(Value::Null))),
                "getValue" => ran(Ok(r.map_part_summary(id, false).unwrap_or(Value::Null))),
                _ => {
                    let Some(namespace) = map_namespace(r, id) else {
                        return Err(Fault::Divergence(
                            "state divergence: the map's model file is not registered".into(),
                        ));
                    };
                    let result = validation::validate_map_key(&r.mm, namespace, map)
                        .and_then(|()| validation::validate_map_value(&r.mm, namespace, map));
                    from_engine(result, |()| recipe::undefined())
                }
            })
        }
        "MapKeyType" | "MapValueType" => {
            let Some(Arg::MapPart(index, id, is_key)) = target else {
                return Err(Fault::Unsupported(
                    "a MapKeyType/MapValueType receiver that is not a map part".into(),
                ));
            };
            let r = &session.pool[index];
            let Some(Declaration::Map(map)) = r.mm.declaration(id) else {
                return Err(Fault::Divergence(
                    "state divergence: the declaration did not load as a map".into(),
                ));
            };
            let type_name = if is_key {
                map.key_type_name()
            } else {
                map.value_type_name()
            };
            let ctor = if is_key { "MapKeyType" } else { "MapValueType" };
            Ok(match member {
                "getType" => ran(Ok(Value::String(type_name.to_string()))),
                "toString" => ran(Ok(Value::String(format!("{ctor} {{id={type_name}}}")))),
                "getNamespace" => match map_namespace(r, id) {
                    Some(ns) => ran(Ok(Value::String(ns.to_string()))),
                    None => Dispatch::Fault(Fault::Divergence(
                        "state divergence: the map's model file is not registered".into(),
                    )),
                },
                "getParent" => ran(Ok(r.declaration_summary(id).unwrap_or(Value::Null))),
                _ => {
                    let Some(namespace) = map_namespace(r, id) else {
                        return Err(Fault::Divergence(
                            "state divergence: the map's model file is not registered".into(),
                        ));
                    };
                    let result = if is_key {
                        validation::validate_map_key(&r.mm, namespace, map)
                    } else {
                        validation::validate_map_value(&r.mm, namespace, map)
                    };
                    from_engine(result, |()| recipe::undefined())
                }
            })
        }
        "NumberValidator" => {
            let Some(Arg::Validator(mm_idx, prop_id, part)) = target else {
                return Err(Fault::Unsupported(
                    "a NumberValidator op whose receiver is not a validatorref".into(),
                ));
            };
            let (built, elem) = build_validator(&session, mm_idx, prop_id, &part)?;
            let Validator::Number(nv) = built else {
                return Err(Fault::Divergence(
                    "state divergence: the validatorref did not build a NumberValidator".into(),
                ));
            };
            Ok(match member {
                "validate" => {
                    let id = arg_nullable_str(&args, 0)?;
                    let value = arg_nullable_f64(&args, 1)?;
                    from_engine(nv.validate(&elem, id.as_deref(), value), |()| {
                        recipe::undefined()
                    })
                }
                "compatibleWith" => {
                    let other = arg_other_validator(&session, &args, 0)?;
                    ran(Ok(Value::Bool(nv.compatible_with(other.as_ref()))))
                }
                _ => unreachable!("`dispatched` lists every member"),
            })
        }
        "StringValidator" => {
            let Some(Arg::Validator(mm_idx, prop_id, part)) = target else {
                return Err(Fault::Unsupported(
                    "a StringValidator op whose receiver is not a validatorref".into(),
                ));
            };
            let (built, elem) = build_validator(&session, mm_idx, prop_id, &part)?;
            let Validator::String(sv) = built else {
                return Err(Fault::Divergence(
                    "state divergence: the validatorref did not build a StringValidator".into(),
                ));
            };
            Ok(match member {
                "validate" => {
                    let id = arg_nullable_str(&args, 0)?;
                    let value = arg_nullable_str(&args, 1)?;
                    from_engine(sv.validate(&elem, id.as_deref(), value.as_deref()), |()| {
                        recipe::undefined()
                    })
                }
                "compatibleWith" => {
                    let other = arg_other_validator(&session, &args, 0)?;
                    ran(Ok(Value::Bool(sv.compatible_with(other.as_ref()))))
                }
                _ => unreachable!("`dispatched` lists every member"),
            })
        }
        "CollectionSizeValidator" => {
            let Some(Arg::Validator(mm_idx, prop_id, part)) = target else {
                return Err(Fault::Unsupported(
                    "a CollectionSizeValidator op whose receiver is not a validatorref".into(),
                ));
            };
            let (built, _elem) = build_validator(&session, mm_idx, prop_id, &part)?;
            let Validator::CollectionSize(cv) = built else {
                return Err(Fault::Divergence(
                    "state divergence: the validatorref did not build a CollectionSizeValidator"
                        .into(),
                ));
            };
            Ok(match member {
                "compatibleWith" => {
                    let other = arg_other_validator(&session, &args, 0)?;
                    ran(Ok(Value::Bool(cv.compatible_with(other.as_ref()))))
                }
                "getMinSize" => ran(Ok(cv
                    .min_size()
                    .map_or_else(recipe::undefined, |v| json!(v)))),
                "getMaxSize" => ran(Ok(cv
                    .max_size()
                    .map_or_else(recipe::undefined, |v| json!(v)))),
                _ => unreachable!("`dispatched` lists every member"),
            })
        }
        "ClassDeclaration" => {
            let Some(Arg::Decl(index, id)) = target else {
                return Err(Fault::Unsupported(
                    "a ClassDeclaration receiver that is not a declref".into(),
                ));
            };
            let r = &session.pool[index];
            let Some(declaration) = r.mm.declaration(id) else {
                return Err(Fault::Divergence(
                    "state divergence: the declaration handle does not resolve".into(),
                ));
            };
            if !matches!(declaration, Declaration::Class(_) | Declaration::Enum(_)) {
                // TS: every one of these members is defined on
                // `ClassDeclaration`, inherited unchanged by `EnumDeclaration`
                // (module doc); a scalar or map declaration has no such
                // method at all — not a fixture this op family owns.
                return Ok(unsupported(
                    "ClassDeclaration op on a receiver that is neither class-like nor an enum",
                ));
            }
            let fqn =
                r.mm.get_fully_qualified_name(&Node::Declaration(id))
                    .expect("a resolved declref always names a loaded declaration");
            Ok(class_declaration_op(r, id, &fqn, member, &args))
        }
        "Declaration" => {
            let Some(Arg::Decl(index, id)) = target else {
                return Err(Fault::Unsupported(
                    "a Declaration receiver that is not a declref".into(),
                ));
            };
            let r = &session.pool[index];
            Ok(declaration_op(r, id, member))
        }
        "Decorator" => {
            let Some(Arg::Deco(index, parent, position)) = target else {
                return Err(Fault::Unsupported(
                    "a Decorator receiver that is not a decoref".into(),
                ));
            };
            let r = &session.pool[index];
            let Some(decorator) = parent.decorators(r).and_then(|ds| ds.get(position)) else {
                return Err(Fault::Divergence(
                    "state divergence: the decorator was not found at its recorded position".into(),
                ));
            };
            Ok(match member {
                "getArguments" => ran(Ok(Value::Array(
                    decorator
                        .arguments()
                        .iter()
                        .map(encode_decorator_argument)
                        .collect(),
                ))),
                _ => {
                    // TS default: `decoratorValidation` unset, so `validate`
                    // is a no-op (module doc on `Decorator::validate`); this
                    // harness does not yet replay a `decoratorValidation`
                    // option (`UNMODELLED_OPTIONS`), so every fixture it
                    // reaches runs with the manager's default (disabled).
                    let namespace = decorated_namespace(r, &parent).unwrap_or_default();
                    ran(match decorator.validate(&r.mm, &namespace, None) {
                        Ok(()) => Ok(recipe::undefined()),
                        Err(e) => Err(to_oracle_error(&e)),
                    })
                }
            })
        }
        "Property" | "Field" => {
            let Some(Arg::Prop(index, id)) = target else {
                return Err(Fault::Unsupported(
                    "a Property/Field receiver that is not a propref".into(),
                ));
            };
            let r = &session.pool[index];
            let Some(property) = r.mm.property(id) else {
                return Err(Fault::Divergence(
                    "state divergence: the property handle does not resolve".into(),
                ));
            };
            if class == "Field" && property_ctor(property) != "Field" {
                // TS: `getDefaultValue`/`getValidator`/`isTypeScalar`/
                // `getScalarField` are defined only on `Field`, which
                // `RelationshipDeclaration` and `EnumValueDeclaration` do not
                // extend (module doc on `property_op`) — not a fixture this
                // op family owns.
                return Ok(unsupported(
                    "Field op on a receiver that is not a Field (relationship or enum value)",
                ));
            }
            Ok(property_op(r, id, property, member))
        }
        "RelationshipDeclaration" => {
            let Some(Arg::Prop(index, id)) = target else {
                return Err(Fault::Unsupported(
                    "a RelationshipDeclaration receiver that is not a propref".into(),
                ));
            };
            let r = &session.pool[index];
            let Some(property) = r.mm.property(id) else {
                return Err(Fault::Divergence(
                    "state divergence: the property handle does not resolve".into(),
                ));
            };
            if !property.is_relationship() {
                return Ok(unsupported(
                    "RelationshipDeclaration.toString on a receiver that is not a relationship",
                ));
            }
            Ok(relationship_to_string(r, id, property))
        }
        "EnumDeclaration" => {
            let Some(Arg::Decl(index, id)) = target else {
                return Err(Fault::Unsupported(
                    "an EnumDeclaration receiver that is not a declref".into(),
                ));
            };
            let r = &session.pool[index];
            let Some(Declaration::Enum(_)) = r.mm.declaration(id) else {
                return Err(Fault::Divergence(
                    "state divergence: the declaration did not load as an enum".into(),
                ));
            };
            let fqn =
                r.mm.get_fully_qualified_name(&Node::Declaration(id))
                    .expect("a resolved declref always names a loaded declaration");
            // TS: `EnumDeclaration.toString` (enumdeclaration.ts): `'EnumDeclaration
            // {id=' + this.getFullyQualifiedName() + '}'`.
            Ok(ran(Ok(Value::String(format!(
                "EnumDeclaration {{id={fqn}}}"
            )))))
        }
        "ModelFile" => {
            let Some(Arg::File(file)) = target else {
                return Err(Fault::Unsupported(
                    "a ModelFile op whose receiver is not a model file (mfref/mfnew)".into(),
                ));
            };
            Ok(model_file_op(&session, &file, member, &args))
        }
        "Introspector" => {
            let Some(Arg::Mm(index)) = target else {
                return Err(Fault::Unsupported(
                    "an Introspector op whose receiver is not an introspector recipe".into(),
                ));
            };
            let r = &session.pool[index];
            Ok(introspector_op(r, member, &args))
        }
        "Relationship" if member == "fromURI" => Ok(relationship_from_uri(&session, &args)),
        "Resource" | "Identifiable" | "Relationship" | "Typed" => {
            let Some(Arg::Typed(index, inst)) = target else {
                return Err(Fault::Unsupported(format!(
                    "{class}.{member} with a receiver that is not a typed instance"
                )));
            };
            let r = &session.pool[index];
            Ok(instance_op(r, &inst, class, member, &args))
        }
        _ => unreachable!("`dispatched` lists every class"),
    }
}

/// The oracle's summary of `ScalarDeclaration.getValidator`'s result, shared
/// by a `declref` receiver's loaded [`ScalarValidator`] and a `declnew`
/// receiver's [`ProcessedScalar`](concerto_core::introspect::scalar::ProcessedScalar) one.
fn scalar_validator_summary(validator: Option<&ScalarValidator>) -> Value {
    match validator {
        None => Value::Null,
        Some(ScalarValidator::Number(_)) => {
            json!({ M: "Validator", "ctor": "NumberValidator" })
        }
        Some(ScalarValidator::String { .. }) => {
            json!({ M: "Validator", "ctor": "StringValidator" })
        }
    }
}

/// A property, captured just enough to rebuild one of its validators for
/// this harness's own replay: only what `FullyQualified`/`ValidatedElement`
/// need to report a validator error, snapshotted at the point the validator
/// is rebuilt. No production `Property`/`Field` API returns a built
/// validator yet — that wiring is P2-04/P2-05 (PORTING.md 6.2) — so the
/// harness rebuilds one straight from the property's own AST fields, which
/// are already public, each time a `NumberValidator`/`StringValidator`/
/// `CollectionSizeValidator` fixture needs one.
struct PropertyElement {
    fqn: String,
    name: String,
    default_value: Option<Value>,
}

impl concerto_core::introspect::FullyQualified for PropertyElement {
    type Error = ConcertoError;

    fn fully_qualified_name(&self) -> Result<String, ConcertoError> {
        Ok(self.fqn.clone())
    }
}

impl concerto_core::model_manager::ValidatedElement for PropertyElement {
    fn default_value(&self) -> Result<Option<Value>, ConcertoError> {
        Ok(self.default_value.clone())
    }

    fn name(&self) -> Result<String, ConcertoError> {
        Ok(self.name.clone())
    }
}

/// `property.ast.defaultValue`: the property kinds that carry one (`String`,
/// `Integer`, `Long`, `Double`), or `None` (JS `null`/`undefined`) for a kind
/// that has none.
fn property_default_value(prop: &concerto_core::introspect::Property) -> Option<Value> {
    use concerto_core::introspect::Property;
    match prop {
        Property::String(p) => p.default_value.clone().map(Value::String),
        Property::Integer(p) => p.default_value.map(|v| json!(v)),
        Property::Long(p) => p.default_value.map(|v| json!(v)),
        Property::Double(p) => p.default_value.map(|v| json!(v)),
        _ => None,
    }
}

/// Rebuilds the validator a `validatorref` names (`part`: `"validator"`, the
/// regex/length or numeric-domain one; `"size"`, the collection-size one)
/// from its owning property (`mm_idx`/`prop_id`), plus the [`PropertyElement`]
/// the property's own AST fields feed it with.
///
/// A build failure here is a real [`Fault::Divergence`], not an
/// [`Fault::Unsupported`]: the property already loaded successfully (its own
/// ad hoc `check_pattern`/`check_length`/`check_domain`/`check_size` checks
/// passed when its model manager was rebuilt), so rebuilding the equivalent
/// `Validator` from the same AST must succeed too, or the two disagree.
fn build_validator(
    session: &Session,
    mm_idx: usize,
    prop_id: concerto_core::model_manager::PropId,
    part: &str,
) -> Faulty<(Validator, PropertyElement)> {
    use concerto_core::introspect::Named;
    use concerto_core::introspect::Property;

    let r = session.pool.get(mm_idx).ok_or_else(|| {
        Fault::Divergence("state divergence: dangling model manager index in a validatorref".into())
    })?;
    let prop = r.mm.property(prop_id).ok_or_else(|| {
        Fault::Divergence("state divergence: the validatorref's property was not found".into())
    })?;
    let fqn =
        r.mm.get_fully_qualified_name(&Node::Property(prop_id))
            .map_err(|e| {
                Fault::Divergence(format!(
                    "computing the validatorref property's fully qualified name: {e}"
                ))
            })?;
    let elem = PropertyElement {
        fqn,
        name: prop.name().to_string(),
        default_value: property_default_value(prop),
    };

    let validator = match part {
        "size" => {
            let ast = prop.size_validator().ok_or_else(|| {
                Fault::Divergence(
                    "state divergence: the validatorref's property has no size validator".into(),
                )
            })?;
            let built =
                concerto_core::introspect::validators::CollectionSizeValidator::new(&elem, ast)
                    .map_err(|e| {
                        Fault::Divergence(format!("rebuilding CollectionSizeValidator: {e}"))
                    })?;
            Validator::CollectionSize(built)
        }
        "validator" => match prop {
            Property::String(p) => {
                let built = concerto_core::introspect::validators::StringValidator::new(
                    &elem,
                    p.validator.as_ref(),
                    p.length_validator.as_ref(),
                )
                .map_err(|e| Fault::Divergence(format!("rebuilding StringValidator: {e}")))?;
                Validator::String(built)
            }
            Property::Integer(p) => {
                let ast = p.validator.as_ref().map_or(Value::Null, |v| {
                    serde_json::to_value(v).expect("domain validator serializes")
                });
                let built =
                    concerto_core::introspect::validators::NumberValidator::new(&elem, &ast)
                        .map_err(|e| {
                            Fault::Divergence(format!("rebuilding NumberValidator: {e}"))
                        })?;
                Validator::Number(built)
            }
            Property::Long(p) => {
                let ast = p.validator.as_ref().map_or(Value::Null, |v| {
                    serde_json::to_value(v).expect("domain validator serializes")
                });
                let built =
                    concerto_core::introspect::validators::NumberValidator::new(&elem, &ast)
                        .map_err(|e| {
                            Fault::Divergence(format!("rebuilding NumberValidator: {e}"))
                        })?;
                Validator::Number(built)
            }
            Property::Double(p) => {
                let ast = p.validator.as_ref().map_or(Value::Null, |v| {
                    serde_json::to_value(v).expect("domain validator serializes")
                });
                let built =
                    concerto_core::introspect::validators::NumberValidator::new(&elem, &ast)
                        .map_err(|e| {
                            Fault::Divergence(format!("rebuilding NumberValidator: {e}"))
                        })?;
                Validator::Number(built)
            }
            _ => {
                return Err(Fault::Unsupported(
                    "a `validator` part on a property kind with no ported validator".into(),
                ));
            }
        },
        other => {
            return Err(Fault::Unsupported(format!(
                "a validatorref part {other:?} with no Rust counterpart"
            )));
        }
    };
    Ok((validator, elem))
}

/// `NumberValidator.validate`/`StringValidator.validate`'s first argument
/// (`identifier`): a nullable string.
fn arg_nullable_str(args: &[Arg], index: usize) -> Faulty<Option<String>> {
    match args.get(index) {
        None | Some(Arg::Plain(Value::Null)) => Ok(None),
        Some(Arg::Plain(Value::String(s))) => Ok(Some(s.clone())),
        _ => Err(Fault::Unsupported(
            "expected a nullable string argument".into(),
        )),
    }
}

/// `NumberValidator.validate`'s value argument: a nullable number.
fn arg_nullable_f64(args: &[Arg], index: usize) -> Faulty<Option<f64>> {
    match args.get(index) {
        None | Some(Arg::Plain(Value::Null)) => Ok(None),
        Some(Arg::Plain(v)) if v.is_number() => Ok(v.as_f64()),
        _ => Err(Fault::Unsupported(
            "expected a nullable number argument".into(),
        )),
    }
}

/// `compatibleWith`'s `other` argument: `null`, or another `validatorref`,
/// rebuilt the same way the receiver was.
fn arg_other_validator(session: &Session, args: &[Arg], index: usize) -> Faulty<Option<Validator>> {
    match args.get(index) {
        None | Some(Arg::Plain(Value::Null)) => Ok(None),
        Some(Arg::Validator(mm_idx, prop_id, part)) => {
            build_validator(session, *mm_idx, *prop_id, part).map(|(v, _)| Some(v))
        }
        _ => Err(Fault::Unsupported(
            "expected a validator or null argument".into(),
        )),
    }
}

/// The `ctor` an oracle `Property` summary carries for `p` (README "Value
/// encoding", `codec.js`'s `encodable.js`): the TS class that would have
/// constructed it, from its own kind (a property's kind, unlike a
/// declaration's, never needs a receiver's model file to tell, PORTING.md
/// 6.2).
fn property_ctor(p: &Property) -> &'static str {
    if p.is_enum_value() {
        "EnumValueDeclaration"
    } else if p.is_relationship() {
        "RelationshipDeclaration"
    } else {
        "Field"
    }
}

/// The outcome-only `Property` summary (`makeOutputEncoder`): `{ctor, fqn}`.
/// `owner_fqn` is the fully-qualified name of the declaration that actually
/// declares `p` — its `Property.getParent().getFullyQualifiedName()` — which
/// for an inherited property is its super type's, not the type the walk
/// started from ([`ModelManager::get_all_properties`]'s doc comment).
fn property_summary(owner_fqn: &str, p: &Property) -> Value {
    json!({
        M: "Property",
        "ctor": property_ctor(p),
        "fqn": format!("{owner_fqn}.{}", p.name()),
    })
}

/// Every fully-qualified super type name `get_all_super_type_names` (or
/// `get_assignable_class_declarations`/`get_direct_subclasses`) gives, each
/// resolved back to its declaration and encoded the outcome-only way
/// (`declaration_summary`).
fn declaration_summaries(r: &Replayed, fqns: &[String]) -> Value {
    Value::Array(
        fqns.iter()
            .map(|fqn| {
                let id =
                    r.mm.declaration_id(fqn)
                        .expect("a name this manager's own walk produced is always loaded");
                r.declaration_summary(id)
                    .expect("declaration_id just resolved it")
            })
            .collect(),
    )
}

/// `ClassDeclaration.*`/`EnumDeclaration`-inherited ops (module doc): every
/// member TS defines once on `ClassDeclaration`, which `EnumDeclaration`
/// (src/introspect/enumdeclaration.ts) inherits unchanged except `toString`
/// and `declarationKind` — so `fqn` here may equally be a concept-like
/// declaration or an enum (`exec_handles`'s own gate on `declaration`).
fn class_declaration_op(
    r: &Replayed,
    id: DeclId,
    fqn: &str,
    member: &str,
    args: &[Arg],
) -> Dispatch {
    let declaration =
        r.mm.declaration(id)
            .expect("the caller just resolved this handle");
    let arg_str = |i: usize| match args.get(i) {
        Some(Arg::Plain(Value::String(s))) => Some(s.as_str()),
        _ => None,
    };
    match member {
        "isAbstract" => {
            let value = match declaration {
                Declaration::Class(c) => c.is_abstract(),
                Declaration::Enum(e) => e.is_abstract(),
                _ => unreachable!("the caller already gated to class-like or enum"),
            };
            ran(Ok(Value::Bool(value)))
        }
        "isIdentified" => from_engine(r.mm.is_identified(fqn), Value::Bool),
        "isSystemIdentified" => from_engine(r.mm.is_system_identified(fqn), Value::Bool),
        "isExplicitlyIdentified" => {
            let value = match declaration {
                Declaration::Class(c) => c.is_explicitly_identified(),
                // An enum's AST never carries `identified` (own_identifier_field_name).
                Declaration::Enum(_) => false,
                _ => unreachable!("the caller already gated to class-like or enum"),
            };
            ran(Ok(Value::Bool(value)))
        }
        "getIdentifierFieldName" => from_engine(r.mm.identifier_field_name(fqn), |name| {
            name.map_or(Value::Null, Value::String)
        }),
        "getOwnProperties" => from_engine(r.mm.get_own_properties(fqn), |props| {
            Value::Array(props.iter().map(|p| property_summary(fqn, p)).collect())
        }),
        "getProperties" => from_engine(r.mm.get_all_properties(fqn), |props| {
            Value::Array(
                props
                    .iter()
                    .map(|(owner, p)| property_summary(owner, p))
                    .collect(),
            )
        }),
        "getProperty" => {
            let Some(name) = arg_str(0) else {
                return unsupported("getProperty with a name that is not a string");
            };
            from_engine(r.mm.get_property(fqn, name), |found| {
                found.map_or(Value::Null, |(owner, p)| property_summary(&owner, &p))
            })
        }
        "getSuperType" => from_engine(r.mm.get_super_type(fqn), |name| {
            name.map_or(Value::Null, Value::String)
        }),
        "getSuperTypeDeclaration" => from_engine(r.mm.get_super_type_declaration(fqn), |found| {
            found
                .and_then(|id| r.declaration_summary(id))
                .unwrap_or(Value::Null)
        }),
        "getAllSuperTypeDeclarations" => from_engine(r.mm.get_all_super_type_names(fqn), |names| {
            declaration_summaries(r, &names)
        }),
        "getAssignableClassDeclarations" => {
            from_engine(r.mm.get_assignable_class_declarations(fqn), |names| {
                declaration_summaries(r, &names)
            })
        }
        "getDirectSubclasses" => from_engine(r.mm.get_direct_subclasses(fqn), |names| {
            declaration_summaries(r, &names)
        }),
        "getNestedProperty" => {
            let Some(path) = arg_str(0) else {
                return unsupported("getNestedProperty with a path that is not a string");
            };
            from_engine(r.mm.get_nested_property(fqn, path), |(owner, p)| {
                property_summary(&owner, &p)
            })
        }
        "isEnum" => ran(Ok(Value::Bool(declaration.is_enum_declaration()))),
        "isEvent" => {
            let value = match declaration {
                Declaration::Class(c) => c.is_event(),
                Declaration::Enum(_) => false,
                _ => unreachable!("the caller already gated to class-like or enum"),
            };
            ran(Ok(Value::Bool(value)))
        }
        "isMapDeclaration" => ran(Ok(Value::Bool(declaration.is_map_declaration()))),
        "toString" => {
            let Declaration::Class(class) = declaration else {
                // `EnumDeclaration` overrides `toString` (module doc): the
                // oracle records that override as its own op
                // (`EnumDeclaration.toString`), never reaching here.
                return unsupported("ClassDeclaration.toString on an enum receiver");
            };
            let super_name = class.super_type().map(|ti| ti.name.as_str());
            ran(Ok(Value::String(ClassDeclaration::to_string(
                fqn,
                super_name,
                class.is_abstract(),
            ))))
        }
        "validate" => {
            let namespace =
                r.mm.model_file_of(id)
                    .and_then(|file| r.mm.file(file))
                    .map(ModelFile::namespace)
                    .expect("a resolved declref's declaration always has a model file");
            // TS: `validate()` returns nothing (`undefined`), never `null`.
            from_engine(declaration.validate(&r.mm, namespace), |()| {
                recipe::undefined()
            })
        }
        _ => unreachable!("`dispatched` lists every ClassDeclaration member"),
    }
}

/// `Declaration.*` ops that reach every kind of declaration unchanged
/// (module doc): none of them is overridden anywhere in the hierarchy.
fn declaration_op(r: &Replayed, id: DeclId, member: &str) -> Dispatch {
    let Some(declaration) = r.mm.declaration(id) else {
        return Dispatch::Fault(Fault::Divergence(
            "state divergence: the declaration handle does not resolve".into(),
        ));
    };
    match member {
        "getFullyQualifiedName" => from_engine(
            r.mm.get_fully_qualified_name(&Node::Declaration(id)),
            Value::String,
        ),
        "getName" => ran(Ok(Value::String(declaration.name().to_string()))),
        "getNamespace" => {
            let Some(namespace) =
                r.mm.model_file_of(id)
                    .and_then(|file| r.mm.file(file))
                    .map(ModelFile::namespace)
            else {
                return Dispatch::Fault(Fault::Divergence(
                    "state divergence: the declaration's model file does not resolve".into(),
                ));
            };
            ran(Ok(Value::String(namespace.to_string())))
        }
        "getModelFile" => {
            let Some(namespace) =
                r.mm.model_file_of(id)
                    .and_then(|file| r.mm.file(file))
                    .map(ModelFile::namespace)
            else {
                return Dispatch::Fault(Fault::Divergence(
                    "state divergence: the declaration's model file does not resolve".into(),
                ));
            };
            ran(Ok(r.model_file_summary(namespace).unwrap_or(Value::Null)))
        }
        _ => unreachable!("`dispatched` lists every Declaration member"),
    }
}

/// TS: `new ModelFile(modelManager, ast, definitions, fileName)` (P2-08),
/// for a receiver never added to a manager via `addModelFile` (that is
/// `ModelManager.addModelFile`, already dispatched).
fn model_file_new(args: &[Arg]) -> Dispatch {
    // Each constructor argument as plain data, `None` for JS `undefined`.
    let mut plain = [None, None, None];
    for (slot, (index, what)) in
        plain
            .iter_mut()
            .zip([(1, "ast"), (2, "definitions"), (3, "fileName")])
    {
        match args.get(index) {
            None => {}
            Some(Arg::Plain(v)) if recipe::is_undefined(v) => {}
            Some(Arg::Plain(v)) => *slot = Some(v),
            Some(_) => {
                return unsupported(format!(
                    "ModelFile.new with a {what} argument that is not plain data"
                ));
            }
        }
    }
    let [ast, definitions, file_name_arg] = plain;
    // TS's own argument checks come first (a plain `Error` each).
    if let Err(e) = ModelFile::check_constructor_arguments(ast, definitions, file_name_arg) {
        return ran(Err(to_oracle_error(&e)));
    }
    let ast = ast.expect("check_constructor_arguments rejects a missing ast");
    // A falsy non-string `definitions` passed those checks; TS keeps it as
    // given, but nothing the oracle compares reads it back (the harness's
    // `getDefinitions` answers only for a string).
    let definitions = match definitions {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    };
    let file_name_value = file_name_arg.cloned().unwrap_or_else(recipe::undefined);
    let file_name = match &file_name_value {
        Value::String(s) => Some(s.clone()),
        _ => None,
    };
    from_engine(
        ModelFile::from_json_with_definitions(ast, definitions, file_name),
        |mf| {
            json!({
                M: "ModelFile",
                "namespace": mf.namespace(),
                "name": mf
                    .file_name()
                    .map_or_else(|| file_name_value.clone(), |n| Value::String(n.to_string())),
                "ast": mf.ast(),
            })
        },
    )
}

/// The outcome-only `Declaration` summary `{ctor, fqn}` (the same shape
/// [`Replayed::declaration_summary`] builds from a [`DeclId`]), for a
/// declaration read straight off a rebuilt [`ModelFile`] — every `ModelFile`
/// getter that hands back a declaration (`getAssetDeclaration` and its
/// siblings, `getAllDeclarations` and the other `get*Declarations` lists)
/// already has it in hand, with no arena lookup needed.
fn declaration_value(namespace: &str, declaration: &Declaration) -> Value {
    let ctor = match declaration {
        Declaration::Class(class) => class.declaration_kind(),
        Declaration::Enum(_) => "EnumDeclaration",
        Declaration::Scalar(_) => "ScalarDeclaration",
        Declaration::Map(_) => "MapDeclaration",
    };
    json!({
        M: "Declaration",
        "ctor": ctor,
        "fqn": format!("{namespace}.{}", declaration.name()),
    })
}

/// A [`Node`] `ModelFile.getType` resolved to, in its outcome shape: a
/// primitive's bare name, a declaration's summary, or `null`.
fn node_type_value(r: &Replayed, node: Option<Node>) -> Value {
    match node {
        None => Value::Null,
        Some(Node::Primitive(name)) => Value::String(name.to_string()),
        Some(Node::Declaration(id)) => r.declaration_summary(id).unwrap_or(Value::Null),
        // `ModelFile.getType` never resolves to a model file or a property.
        Some(_) => Value::Null,
    }
}

/// `ModelFile.*` ops (P2-08), other than `new` (handled directly in
/// `exec_handles`, since the receiver does not exist yet). `file` is the
/// `mfref`/`mfnew` receiver.
///
/// Every getter below runs against `mf`, rebuilt fresh from
/// `file.ast`/`file.definitions`/`file.file_name`: a `ModelFile` is a pure
/// function of what it was built from (module doc on
/// [`concerto_core::introspect::model_file::ModelFile`]), and
/// `Session::file` already proved this same construction succeeds once,
/// while decoding the receiver — so rebuilding it here for a second time
/// gives back an equivalent object. `getModelManager`, `getType` and
/// `validate` are the three members that need the owning manager for
/// something beyond that (`getModelManager`/`getType`'s cross-file lookups,
/// `validate`'s import checks): `file.mm_index`, via `session`.
///
/// Every string argument these members take is required to already be a
/// plain string; TS itself is not always this strict (`isLocalType`,
/// `isImportedType` and `getType`/`getFullyQualifiedTypeName` degrade
/// gracefully on a nullish argument, while the `get*Declaration` getters
/// throw a `TypeError` calling `.startsWith` on one) — the oracle corpus
/// exercises every one of these members with a real string name only, so
/// this port does not need to reproduce either behaviour, and instead
/// reports a non-string argument `unsupported` uniformly.
fn model_file_op(
    session: &Session,
    file: &recipe::FileArg,
    member: &str,
    args: &[Arg],
) -> Dispatch {
    let mf = match ModelFile::from_json_with_definitions(
        &file.ast,
        file.definitions.clone(),
        file.file_name.clone(),
    ) {
        Ok(mf) => mf,
        Err(e) => {
            return Dispatch::Fault(Fault::Divergence(format!(
                "state divergence: a model file that decoded successfully failed to rebuild: {}",
                to_oracle_error(&e).message
            )));
        }
    };
    let namespace = mf.namespace().to_string();
    let decl =
        |d: Option<&Declaration>| d.map_or(Value::Null, |d| declaration_value(&namespace, d));
    let decls = |ds: Vec<&Declaration>| {
        Value::Array(
            ds.into_iter()
                .map(|d| declaration_value(&namespace, d))
                .collect(),
        )
    };
    let arg_str = |i: usize| -> Result<Option<&str>, ()> {
        match args.get(i) {
            None => Ok(None),
            Some(Arg::Plain(v)) if v.is_null() || recipe::is_undefined(v) => Ok(None),
            Some(Arg::Plain(Value::String(s))) => Ok(Some(s.as_str())),
            _ => Err(()),
        }
    };
    macro_rules! str_arg {
        ($i:expr) => {
            match arg_str($i) {
                Ok(Some(s)) => s,
                Ok(None) | Err(()) => {
                    return unsupported(format!("ModelFile.{member} with a non-string argument"));
                }
            }
        };
    }

    match member {
        "getNamespace" => ran(Ok(Value::String(namespace))),
        "getName" => ran(Ok(mf.file_name().map_or_else(
            || file.nullish_name.clone(),
            |n| Value::String(n.to_string()),
        ))),
        "getAst" => ran(Ok(mf.ast().clone())),
        // TS `this.definitions = definitions` (the constructor parameter),
        // which defaults to JS `undefined` when omitted — never observed as
        // an explicit `null` in the oracle corpus (module doc above).
        "getDefinitions" => ran(Ok(mf
            .definitions()
            .map_or_else(recipe::undefined, |d| Value::String(d.to_string())))),
        "getConcertoVersion" => ran(Ok(mf
            .concerto_version()
            .map_or(Value::Null, |v| Value::String(v.to_string())))),
        "isExternal" => ran(Ok(Value::Bool(mf.is_external()))),
        "isModelFile" => ran(Ok(Value::Bool(true))),
        "isSystemModelFile" => ran(Ok(Value::Bool(mf.is_system_namespace()))),
        "getAllDeclarations" => ran(Ok(decls(mf.declarations().iter().collect()))),
        "getClassDeclarations" => ran(Ok(decls(mf.get_class_declarations()))),
        "getEnumDeclarations" => ran(Ok(decls(mf.get_enum_declarations()))),
        "getScalarDeclarations" => ran(Ok(decls(mf.get_scalar_declarations()))),
        "getAssetDeclarations" => ran(Ok(decls(mf.get_asset_declarations()))),
        "getTransactionDeclarations" => ran(Ok(decls(mf.get_transaction_declarations()))),
        "getEventDeclarations" => ran(Ok(decls(mf.get_event_declarations()))),
        "getAssetDeclaration" => ran(Ok(decl(mf.get_asset_declaration(str_arg!(0))))),
        "getTransactionDeclaration" => ran(Ok(decl(mf.get_transaction_declaration(str_arg!(0))))),
        "getEventDeclaration" => ran(Ok(decl(mf.get_event_declaration(str_arg!(0))))),
        "getParticipantDeclaration" => ran(Ok(decl(mf.get_participant_declaration(str_arg!(0))))),
        "getImports" => ran(Ok(Value::Array(
            mf.get_imports().into_iter().map(Value::String).collect(),
        ))),
        "getImportURI" => ran(Ok(mf
            .get_import_uri(str_arg!(0))
            .map_or(Value::Null, |u| Value::String(u.to_string())))),
        "isImportedType" => ran(Ok(Value::Bool(mf.is_imported_type(str_arg!(0))))),
        "isLocalType" => ran(Ok(Value::Bool(mf.is_local_type(str_arg!(0))))),
        "isDefined" => ran(Ok(Value::Bool(mf.is_defined(str_arg!(0))))),
        "resolveImport" => from_engine(mf.resolve_import(str_arg!(0)), Value::String),
        "getImportedType" => from_engine(mf.get_imported_type(str_arg!(0)), Value::String),
        "getFullyQualifiedTypeName" => ran(Ok(mf
            .get_fully_qualified_type_name(str_arg!(0))
            .map_or(Value::Null, Value::String))),
        "getModelManager" => match registered_file(session, file) {
            Ok(r) => ran(Ok(r.summary())),
            Err(Fault::Unsupported(reason)) => unsupported(reason),
            Err(other) => Dispatch::Fault(other),
        },
        "getType" => {
            let r = match registered_file(session, file) {
                Ok(r) => r,
                Err(Fault::Unsupported(reason)) => return unsupported(reason),
                Err(other) => return Dispatch::Fault(other),
            };
            let type_name = match arg_str(0) {
                Ok(v) => v,
                Err(()) => {
                    return unsupported("ModelFile.getType with a type name that is not a string");
                }
            };
            let file_id = r
                .file_id(&namespace)
                .expect("registered_file just confirmed this namespace is registered");
            from_engine(
                r.mm.get_type(&Node::ModelFile(file_id), type_name),
                |node| node_type_value(r, node),
            )
        }
        // TS `modelFile.validate()` needs only `this.getModelManager()`, not
        // that the manager has registered `this`: a file built with `new
        // ModelFile(mm, ast)` and validated straight away is the common case
        // (P2-08). `validate_detached_model_file` validates such a file
        // against its manager without changing it.
        "validate" => match owning_manager(session, file) {
            Ok(r) => from_engine(r.mm.validate_detached_model_file(&mf), |()| {
                recipe::undefined()
            }),
            Err(Fault::Unsupported(reason)) => unsupported(reason),
            Err(other) => Dispatch::Fault(other),
        },
        _ => unreachable!("`dispatched` lists every ModelFile member"),
    }
}

/// `file`'s owning model manager, only when `file` is genuinely the file
/// registered there under its namespace — the same check
/// [`recipe::model_file_node`] makes for a receiver that needs a `Node`
/// handle. `getModelManager` and `getType` use this: `getType`'s
/// `ModelManager::resolve_type_name` looks a namespace's file up *through
/// the manager*, not through `file` directly, so an `mfnew` receiver that
/// was never added via `addModelFile` would resolve with the wrong verdict
/// (P2-08 review). `validate` does not need it: it uses
/// `ModelManager::validate_detached_model_file` through [`owning_manager`].
fn registered_file<'s>(
    session: &'s Session,
    file: &recipe::FileArg,
) -> Result<&'s Replayed, Fault> {
    let r = owning_manager(session, file)?;
    // Discard the `Node`: only its existence (this namespace resolves to
    // exactly `file`'s own AST) is wanted here.
    recipe::model_file_node(r, file)?;
    Ok(r)
}

/// `file`'s owning model manager (TS `this.getModelManager()`), whether or
/// not it has registered `file`.
fn owning_manager<'s>(session: &'s Session, file: &recipe::FileArg) -> Result<&'s Replayed, Fault> {
    let index = file.mm_index.ok_or_else(|| {
        Fault::Unsupported(
            "a ModelFile op needing the owning model manager, decoded without its pool index"
                .into(),
        )
    })?;
    Ok(&session.pool[index])
}

/// `Introspector.*` ops (P2-08): a thin wrapper over the receiver's own
/// `ModelManager` (module doc on `Session::decode`'s `"introspector"` arm) —
/// `getModelManager` returns it directly, and the other two members
/// delegate to it exactly as `Introspector`'s TS methods do
/// (src/introspect/introspector.ts).
fn introspector_op(r: &Replayed, member: &str, args: &[Arg]) -> Dispatch {
    match member {
        "getModelManager" => ran(Ok(r.summary())),
        // TS: `getClassDeclarations` concatenates
        // `modelFile.getAllDeclarations().filter(d => !isMap && !isScalar)`
        // over `modelManager.getModelFiles()`, i.e. every loaded file but
        // the system and decorator ones — `ModelManager::class_declarations`
        // (P2-08 review: fixed to leave those out, matching `EXCLUDE_NS`).
        "getClassDeclarations" => ran(Ok(Value::Array(
            r.mm.class_declarations()
                .filter_map(|id| r.declaration_summary(id))
                .collect(),
        ))),
        // TS: `getClassDeclaration(fqn)` is `this.modelManager.getType(fqn)`
        // outright — the same lookup `ModelManager.getType`'s own dispatch
        // (`model_manager_query`) already uses.
        "getClassDeclaration" => {
            let Some(Arg::Plain(Value::String(fqn))) = args.first() else {
                return unsupported(
                    "Introspector.getClassDeclaration with a type name that is not a string",
                );
            };
            match r.mm.get_declaration(fqn) {
                Err(e) => ran(Err(to_oracle_error(&e))),
                Ok(_) => {
                    let id = r.mm.declaration_id(fqn).expect("get_declaration found it");
                    ran(Ok(r.declaration_summary(id).unwrap_or(Value::Null)))
                }
            }
        }
        _ => unreachable!("`dispatched` lists every Introspector member"),
    }
}

/// `Property.*` ops (P2-04, issue #48), plus the `Field`-only members
/// (`getDefaultValue`, `getValidator`, `isTypeScalar`, `getScalarField`):
/// every member `Field`, `RelationshipDeclaration` and
/// `EnumValueDeclaration` inherit unchanged from `Property`
/// (src/introspect/property.ts), so `id`/`property` may be any one of the
/// three kinds for the base members; the caller (`exec_handles`) already
/// gates the `Field`-only members to a receiver that is not a relationship
/// or an enum value.
fn property_op(r: &Replayed, id: PropId, property: &Property, member: &str) -> Dispatch {
    let node = Node::Property(id);
    match member {
        "getName" => ran(Ok(Value::String(property.name().to_string()))),
        "getType" => ran(Ok(property
            .type_name()
            .map_or(Value::Null, |t| Value::String(t.to_string())))),
        "isArray" => ran(Ok(Value::Bool(property.is_array()))),
        "isOptional" => ran(Ok(Value::Bool(property.is_optional()))),
        "getFullyQualifiedTypeName" => {
            from_engine(r.mm.get_fully_qualified_type_name(&node), Value::String)
        }
        "getFullyQualifiedName" => from_engine(r.mm.get_fully_qualified_name(&node), Value::String),
        "getNamespace" => {
            // TS: `getParent().getNamespace()`, `ClassDeclaration`'s own
            // (inherited from `Declaration`): its model file's namespace.
            let Some(namespace) =
                r.mm.parent_of(id)
                    .and_then(|parent| r.mm.model_file_of(parent))
                    .and_then(|file| r.mm.file(file))
                    .map(ModelFile::namespace)
            else {
                return Dispatch::Fault(Fault::Divergence(
                    "state divergence: the property's parent model file does not resolve".into(),
                ));
            };
            ran(Ok(Value::String(namespace.to_string())))
        }
        "getParent" => {
            let Some(parent) = r.mm.parent_of(id) else {
                return Dispatch::Fault(Fault::Divergence(
                    "state divergence: the property's parent does not resolve".into(),
                ));
            };
            ran(Ok(r.declaration_summary(parent).unwrap_or(Value::Null)))
        }
        "getSizeValidator" => ran(Ok(match property.size_validator() {
            None => Value::Null,
            Some(_) => json!({ M: "Validator", "ctor": "CollectionSizeValidator" }),
        })),
        "isPrimitive" => ran(Ok(Value::Bool(property.is_primitive()))),
        "isTypeEnum" => from_engine(is_type_enum(r, id, property), Value::Bool),
        "getDefaultValue" => ran(Ok(r
            .mm
            .property_default_value(id)
            .cloned()
            .unwrap_or(Value::Null))),
        "getValidator" => ran(Ok(field_validator_summary(property))),
        "isTypeScalar" => from_engine(is_type_scalar(r, id, property), Value::Bool),
        "getScalarField" => get_scalar_field(r, id, property),
        _ => unreachable!("`dispatched` lists every Property/Field member"),
    }
}

/// `RelationshipDeclaration.toString` (P2-04): the one override besides
/// `getName` et al. (`Property.toString` does not exist; TS's own default
/// `Object.prototype.toString` is never called by any op this harness
/// dispatches).
///
/// TS: `'RelationshipDeclaration {name=' + this.name + ', type=' +
/// this.getFullyQualifiedTypeName() + ', array=' + this.array + ',
/// optional=' + this.optional + '}'` (relationshipdeclaration.ts).
fn relationship_to_string(r: &Replayed, id: PropId, property: &Property) -> Dispatch {
    from_engine(
        r.mm.get_fully_qualified_type_name(&Node::Property(id)),
        |fqn| {
            Value::String(format!(
                "RelationshipDeclaration {{name={}, type={}, array={}, optional={}}}",
                property.name(),
                fqn,
                property.is_array(),
                property.is_optional(),
            ))
        },
    )
}

/// Dispatches an op whose receiver is a `"typed"` `Resource`,
/// `ValidatedResource` or `Relationship` (P3-01 review, task
/// `accordproject-concerto-rust#56` follow-up), decoded into a
/// [`recipe::DecodedInstance`] by `recipe.rs`'s `Session::typed`. Covers:
///
/// - `Resource.validate` (TS `ValidatedResource.validate`,
///   `src/model/validatedresource.ts`, over
///   [`concerto_core::instance::validate::validate_instance`], task P3-01's
///   own port of `ResourceValidator`), `toString`, `isResource`,
///   `isConcept`, `isIdentifiable` (`src/model/resource.ts`), and
///   `instanceOf` (inherited from `Typed`);
/// - `Identifiable.getIdentifier`, `getFullyQualifiedIdentifier`,
///   `getTimestamp`, `toURI`, `isRelationship`, `isResource`
///   (`src/model/identifiable.ts`);
/// - `Relationship.toString`, `isRelationship` (`src/model/relationship.ts`;
///   the static `fromURI` is [`relationship_from_uri`]);
/// - `Typed.getType`, `getNamespace`, `getFullyQualifiedType`,
///   `getClassDeclaration` (`src/model/typed.ts`).
///
/// The members left (`setPropertyValue`, `addArrayValue`, `setIdentifier`,
/// `toJSON`) mutate the receiver or serialize it: they need a
/// `Serializer`/`Factory` port and an `effects`-comparing receiver, so
/// `dispatched` (`exec_handles`) does not list them and they stay
/// `unsupported`, owned by P3-01b (`ledger.rs`, `PLAN_OWNER_OVERRIDES`).
fn instance_op(
    r: &Replayed,
    inst: &recipe::DecodedInstance,
    class: &str,
    member: &str,
    args: &[Arg],
) -> Dispatch {
    match (class, member) {
        ("Resource", "validate") => ran(
            match concerto_core::instance::validate::validate_instance(
                &r.mm,
                &inst.wire,
                &concerto_core::instance::validate::ValidateOptions::default(),
            ) {
                Ok(()) => Ok(recipe::undefined()),
                Err(e) => Err(to_oracle_error(&e)),
            },
        ),
        ("Resource", "toString") => ran(Ok(Value::String(format!(
            "Resource {{id={}}}",
            fully_qualified_identifier(inst)
        )))),
        ("Resource", "isResource") => ran(Ok(Value::Bool(true))),
        ("Resource", "isConcept") => from_engine(instance_is_concept(r, inst), Value::Bool),
        ("Resource", "isIdentifiable") => from_engine(r.mm.is_identified(&inst.fqn), Value::Bool),
        ("Identifiable", "getIdentifier") => ran(Ok(inst
            .identifier
            .clone()
            .map(Value::String)
            .unwrap_or_else(recipe::undefined))),
        ("Identifiable", "getFullyQualifiedIdentifier") => {
            ran(Ok(Value::String(fully_qualified_identifier(inst))))
        }
        ("Identifiable", "toURI") => ran(match instance_resource_id(inst) {
            Ok(id) => Ok(Value::String(id.to_uri())),
            Err(e) => Err(to_oracle_error(&e)),
        }),
        ("Identifiable" | "Relationship", "isRelationship") => {
            ran(Ok(Value::Bool(inst.ctor == "Relationship")))
        }
        ("Identifiable", "isResource") => ran(Ok(Value::Bool(inst.ctor != "Relationship"))),
        ("Relationship", "toString") => ran(Ok(Value::String(format!(
            "Relationship {{id={}}}",
            fully_qualified_identifier(inst)
        )))),
        ("Typed", "getType") => ran(Ok(Value::String(inst.type_name.clone()))),
        ("Typed", "getNamespace") => ran(Ok(Value::String(inst.namespace.clone()))),
        ("Typed", "getFullyQualifiedType") => ran(Ok(Value::String(inst.fqn.clone()))),
        // TS `Identifiable.getTimestamp`: `return this.$timestamp`, as
        // recorded (a `dayjs` node, `null` or `undefined`).
        ("Identifiable", "getTimestamp") => ran(Ok(inst.timestamp.clone())),
        // TS `Typed.getClassDeclaration`: `return this.$classDeclaration`.
        ("Typed", "getClassDeclaration") => match inst.class_declaration {
            Some(id) => ran(Ok(r.declaration_summary(id).unwrap_or(Value::Null))),
            None => unregistered_class_declaration(),
        },
        ("Resource", "instanceOf") => {
            let Some(id) = inst.class_declaration else {
                return unregistered_class_declaration();
            };
            let fqt = match args.first() {
                Some(Arg::Plain(v)) => v.clone(),
                None => recipe::undefined(),
                _ => return unsupported("instanceOf with a type name that is not plain data"),
            };
            from_engine(instance_of(r, id, &fqt), Value::Bool)
        }
        _ => unreachable!("`dispatched` lists every (class, member) this function handles"),
    }
}

/// A receiver whose `$classDeclaration` has no Rust handle
/// ([`recipe::DecodedInstance::class_declaration`]): a declaration never
/// registered with its model manager.
fn unregistered_class_declaration() -> Dispatch {
    Dispatch::Fault(Fault::Blocked(
        "the receiver's class declaration is not registered with a model manager".into(),
        recipe::Blocker::Member("ModelFile.new".into()),
    ))
}

/// TS `Typed.instanceOf(fqt)` (src/model/typed.ts): whether the receiver's
/// own `$classDeclaration`, or any declaration up its
/// `getSuperTypeDeclaration()` chain, has the fully qualified name `fqt`
/// (compared with `===`, so only a string can match).
fn instance_of(r: &Replayed, id: DeclId, fqt: &Value) -> Result<bool, ConcertoError> {
    let fqt = fqt.as_str();
    let mut current = r.mm.get_fully_qualified_name(&Node::Declaration(id))?;
    if fqt == Some(current.as_str()) {
        return Ok(true);
    }
    while let Some(super_id) = r.mm.get_super_type_declaration(&current)? {
        current =
            r.mm.get_fully_qualified_name(&Node::Declaration(super_id))?;
        if fqt == Some(current.as_str()) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// TS `Relationship.fromURI(modelManager, uriAsString, defaultNamespace?,
/// defaultType?)` (src/model/relationship.ts): parses the URI
/// (`ResourceId.fromURI`), looks the type up (`modelManager.getType`), and
/// builds a `Relationship`. The result is written the way the oracle
/// encodes a `Typed` value (`codec.js`): `$identifierFieldName` is the
/// type's identifying field or `$identifier` (`Identifiable`'s
/// constructor, via `modelFile.getType(fqt)?.getIdentifierFieldName()`,
/// `null` for a declaration that is not class-like), `setIdentifier` writes
/// the id under that name, `$timestamp` is `undefined`, and the
/// `Relationship` constructor adds `$class: 'Relationship'`.
fn relationship_from_uri(session: &Session, args: &[Arg]) -> Dispatch {
    let Some(Arg::Mm(index)) = args.first() else {
        return unsupported("Relationship.fromURI with a model manager argument that is not one");
    };
    let plain = |i: usize| match args.get(i) {
        None => Some(None),
        Some(Arg::Plain(v)) if recipe::is_undefined(v) => Some(None),
        Some(Arg::Plain(Value::String(s))) => Some(Some(s.as_str())),
        _ => None,
    };
    let (Some(Some(uri)), Some(default_namespace), Some(default_type)) =
        (plain(1), plain(2), plain(3))
    else {
        return unsupported("Relationship.fromURI with arguments that are not strings");
    };
    let r = &session.pool[*index];
    let built = (|| {
        let resource_id = concerto_core::instance::resource_id::ResourceId::from_uri(
            uri,
            default_namespace,
            default_type,
        )?;
        let fqt =
            model_util::get_fully_qualified_name(&resource_id.namespace, &resource_id.type_name);
        let id = r.mm.get_type_declaration(&fqt)?;
        let fqn = r.mm.get_fully_qualified_name(&Node::Declaration(id))?;
        let identifier_field_name = match r.mm.declaration(id) {
            Some(Declaration::Class(_) | Declaration::Enum(_)) => {
                r.mm.identifier_field_name(&fqn)?
            }
            _ => None,
        }
        .unwrap_or_else(|| "$identifier".to_string());
        let mut fields = serde_json::Map::new();
        fields.insert("$class".into(), json!("Relationship"));
        if identifier_field_name != "$identifier" {
            fields.insert(identifier_field_name, json!(resource_id.id));
        }
        Ok::<_, ConcertoError>(json!({
            M: "typed",
            "ctor": "Relationship",
            "fqn": fqn,
            "ns": resource_id.namespace,
            "type": resource_id.type_name,
            "id": resource_id.id,
            "timestamp": recipe::undefined(),
            "fields": fields,
        }))
    })();
    ran(built.map_err(|e| to_oracle_error(&e)))
}

/// TS `Identifiable.getFullyQualifiedIdentifier`: `this.getIdentifier() ?
/// fqn + '#' + id : fqn` — `getIdentifier()`'s truthiness check, so an empty
/// string identifier (like `undefined`/absent) falls back to the bare fqn.
fn fully_qualified_identifier(inst: &recipe::DecodedInstance) -> String {
    match inst.identifier.as_deref() {
        Some(id) if !id.is_empty() => format!("{}#{id}", inst.fqn),
        _ => inst.fqn.clone(),
    }
}

/// TS `Identifiable.toURI`: `new ResourceId(ns, type,
/// this.getIdentifier()).toURI()` — the `ResourceId` constructor itself
/// throws when the identifier is empty or absent, exactly as
/// [`concerto_core::instance::resource_id::ResourceId::new`] does.
fn instance_resource_id(
    inst: &recipe::DecodedInstance,
) -> Result<concerto_core::instance::resource_id::ResourceId, ConcertoError> {
    concerto_core::instance::resource_id::ResourceId::new(
        inst.namespace.clone(),
        inst.type_name.clone(),
        inst.identifier.clone().unwrap_or_default(),
    )
}

/// TS `Resource.isConcept`: `this.getClassDeclaration().isConcept()`.
fn instance_is_concept(
    r: &Replayed,
    inst: &recipe::DecodedInstance,
) -> Result<bool, ConcertoError> {
    let decl = r.mm.get_declaration(&inst.fqn)?;
    Ok(decl.as_class().is_some_and(ClassDeclaration::is_concept))
}

/// TS: `Property.isTypeEnum` (src/introspect/property.ts): `this.isPrimitive()
/// ? false : this.getParent().getModelFile().getType(this.getType()).isEnum()`.
fn is_type_enum(r: &Replayed, id: PropId, property: &Property) -> Result<bool, ConcertoError> {
    if property.is_primitive() {
        return Ok(false);
    }
    let type_node = resolve_property_type(r, id, property)?;
    r.mm.is_enum(&type_node)
}

/// TS: `Field.isTypeScalar` (src/introspect/field.ts): `this.isPrimitive() ?
/// false : (…resolveType…, type.isScalarDeclaration?.())`. The `resolveType`
/// call only re-checks what `getType` below already needs to resolve to
/// answer, so it is not replayed separately (PORTING.md 6.2).
fn is_type_scalar(r: &Replayed, id: PropId, property: &Property) -> Result<bool, ConcertoError> {
    if property.is_primitive() {
        return Ok(false);
    }
    let type_node = resolve_property_type(r, id, property)?;
    Ok(r.mm.is_scalar_declaration(&type_node)?.unwrap_or(false))
}

/// `getParent().getModelFile().getType(getType())`, the resolution both
/// `isTypeEnum` and `isTypeScalar` (and `getScalarField`) build on: a
/// non-primitive property's declared type, resolved in its parent
/// declaration's own model file.
fn resolve_property_type(
    r: &Replayed,
    id: PropId,
    property: &Property,
) -> Result<Node, ConcertoError> {
    let unknown_parent = || ConcertoError::IllegalModel {
        message: "property has no resolvable parent".into(),
        file_name: None,
        location: None,
    };
    let parent = r.mm.parent_of(id).ok_or_else(unknown_parent)?;
    let file = r.mm.model_file_of(parent).ok_or_else(unknown_parent)?;
    let type_name = property.type_name();
    r.mm.get_type(&Node::ModelFile(file), type_name)?
        .ok_or_else(|| ConcertoError::TypeNotFound {
            type_name: type_name.unwrap_or("null").to_string(),
        })
}

/// TS: `Field.getValidator` (src/introspect/field.ts): a `NumberValidator` for
/// Integer/Long/Double, a `StringValidator` for String (from either its regex
/// or its length validator, or both), `null` otherwise.
fn field_validator_summary(property: &Property) -> Value {
    let has_number_validator = match property {
        Property::Integer(p) => p.validator.is_some(),
        Property::Long(p) => p.validator.is_some(),
        Property::Double(p) => p.validator.is_some(),
        _ => false,
    };
    if has_number_validator {
        return json!({ M: "Validator", "ctor": "NumberValidator" });
    }
    if let Property::String(p) = property
        && (p.validator.is_some() || p.length_validator.is_some())
    {
        return json!({ M: "Validator", "ctor": "StringValidator" });
    }
    Value::Null
}

/// TS: `Field.getScalarField` (src/introspect/field.ts): unboxes a field
/// whose type is a scalar declaration into a synthetic `Field` built from the
/// scalar's own AST — `JSON.parse(JSON.stringify(type.ast))` with `$class`
/// swapped for the matching `*Property` class and `name` set back to this
/// field's own name — then `array` overwritten from this field's own
/// `isArray()`. The synthetic field is never registered (it has no `PropId`
/// of its own, `mm::ScalarDeclaration`'s own doc comment), so its outcome is
/// its `{ctor, fqn}` summary the same way a `getProperty` result is encoded;
/// its `fqn` is identical to the original field's own (same parent, same
/// name), since neither changes.
fn get_scalar_field(r: &Replayed, id: PropId, property: &Property) -> Dispatch {
    match is_type_scalar(r, id, property) {
        Ok(true) => {}
        Ok(false) => {
            // TS: `throw new Error(\`Field ${this.name} is not a scalar property.\`)`.
            return ran(Err(OracleError {
                class: "Error".into(),
                message: format!("Field {} is not a scalar property.", property.name()),
                location: None,
                component: None,
            }));
        }
        Err(e) => return ran(Err(to_oracle_error(&e))),
    }
    let type_node = match resolve_property_type(r, id, property) {
        Ok(node) => node,
        Err(e) => return ran(Err(to_oracle_error(&e))),
    };
    let Node::Declaration(scalar_id) = type_node else {
        return unsupported("getScalarField: the resolved type is not a declaration");
    };
    let Some(Declaration::Scalar(scalar)) = r.mm.declaration(scalar_id) else {
        return Dispatch::Fault(Fault::Divergence(
            "state divergence: getScalarField's resolved type did not load as a scalar".into(),
        ));
    };
    let scalar_ast = match serde_json::to_value(scalar.ast()) {
        Ok(v) => v,
        Err(e) => {
            return Dispatch::Fault(Fault::Harness(format!(
                "getScalarField: the scalar's own AST did not re-serialise: {e}"
            )));
        }
    };
    let scalar_class = scalar_ast.get("$class").and_then(Value::as_str);
    let property_class = match scalar_class {
        Some("concerto.metamodel@1.0.0.BooleanScalar") => {
            "concerto.metamodel@1.0.0.BooleanProperty"
        }
        Some("concerto.metamodel@1.0.0.IntegerScalar") => {
            "concerto.metamodel@1.0.0.IntegerProperty"
        }
        Some("concerto.metamodel@1.0.0.LongScalar") => "concerto.metamodel@1.0.0.LongProperty",
        Some("concerto.metamodel@1.0.0.DoubleScalar") => "concerto.metamodel@1.0.0.DoubleProperty",
        Some("concerto.metamodel@1.0.0.StringScalar") => "concerto.metamodel@1.0.0.StringProperty",
        Some("concerto.metamodel@1.0.0.DateTimeScalar") => {
            "concerto.metamodel@1.0.0.DateTimeProperty"
        }
        other => {
            return Dispatch::Fault(Fault::Divergence(format!(
                "state divergence: getScalarField's resolved type has an unrecognized scalar $class {other:?}"
            )));
        }
    };
    let mut field_ast = scalar_ast;
    field_ast["$class"] = Value::String(property_class.to_string());
    field_ast["name"] = Value::String(property.name().to_string());
    field_ast["isArray"] = Value::Bool(property.is_array());
    let synthetic = match Property::try_from(&field_ast) {
        Ok(p) => p,
        Err(e) => return ran(Err(to_oracle_error(&e))),
    };
    let Some(parent) = r.mm.parent_of(id) else {
        return Dispatch::Fault(Fault::Divergence(
            "state divergence: the property's parent does not resolve".into(),
        ));
    };
    let Ok(owner_fqn) = r.mm.get_fully_qualified_name(&Node::Declaration(parent)) else {
        return Dispatch::Fault(Fault::Divergence(
            "state divergence: the property's parent has no fully-qualified name".into(),
        ));
    };
    ran(Ok(property_summary(&owner_fqn, &synthetic)))
}

/// `Decorated.getDecorator`/`getDecorators` (P2-07): the target is a
/// `declref`, `propref` or `mfref` directly, decoded through
/// [`Session::decorated_target`] rather than the generic [`Session::decode`]
/// (module doc on [`exec_handles`]'s early return), since a plain `Arg::File`
/// would drop the model manager an `mfref` target belongs to.
fn decorated_op(h: &Harness, member: &str, inputs: &Inputs) -> Faulty<Dispatch> {
    let mut session = Session::new(h);
    let Some(target) = &inputs.target else {
        return Ok(unsupported(format!("Decorated.{member} without a target")));
    };
    let (index, parent) = session.decorated_target(target)?;
    let r = &session.pool[index];
    let Some(decorators) = parent.decorators(r) else {
        return Err(Fault::Divergence(
            "state divergence: the decorated element was not found".into(),
        ));
    };
    if member == "getDecorators" {
        return Ok(ran(Ok(Value::Array(
            decorators.iter().map(encode_decorator).collect(),
        ))));
    }
    let args = decode::decode_args(&inputs.args)
        .map_err(|decode::Unsupported(reason)| Fault::Unsupported(reason))?;
    let name_arg = decode::arg(&args, 0);
    let Ok(name) = decode::as_str(&name_arg) else {
        return Ok(unsupported("Decorated.getDecorator with a non-string name"));
    };
    let found = decorators.iter().find(|d| d.name() == name);
    Ok(ran(Ok(found.map_or(Value::Null, encode_decorator))))
}

/// The namespace a decorated element's model file was loaded under, needed
/// only to build [`concerto_core::error::ConcertoError`] messages that name
/// where a decorator's own name failed to resolve — never exercised by a
/// fixture in this scope, since every one of them runs with the manager's
/// default (disabled) `decoratorValidation` (module doc on the `"Decorator"`
/// arm of [`exec_handles`]'s class match).
fn decorated_namespace(r: &Replayed, parent: &recipe::DecoParent) -> Option<String> {
    match parent {
        recipe::DecoParent::File(ns) => Some(ns.clone()),
        recipe::DecoParent::Decl(id) => {
            r.mm.model_file_of(*id)
                .and_then(|f| r.mm.file(f))
                .map(|f| f.namespace().to_string())
        }
        recipe::DecoParent::Prop(id) => {
            r.mm.parent_of(*id)
                .and_then(|d| r.mm.model_file_of(d))
                .and_then(|f| r.mm.file(f))
                .map(|f| f.namespace().to_string())
        }
    }
}

/// A [`concerto_core::Decorator`] in the oracle's encoding, matching a
/// recorded `{"@@oracle":"Decorator","name":…,"arguments":[…]}` value.
fn encode_decorator(d: &concerto_core::Decorator) -> Value {
    json!({
        M: "Decorator",
        "name": d.name(),
        "arguments": d.arguments().iter().map(encode_decorator_argument).collect::<Vec<_>>(),
    })
}

/// One [`concerto_core::DecoratorArgument`] in the oracle's encoding: a
/// literal as it is, or a type reference as the plain object TS builds
/// (`{type, name, array}` in `Decorator.process`).
fn encode_decorator_argument(arg: &concerto_core::DecoratorArgument) -> Value {
    use concerto_core::DecoratorArgument;
    match arg {
        DecoratorArgument::String(s) => Value::String(s.clone()),
        DecoratorArgument::Number(n) => json!(n),
        DecoratorArgument::Boolean(b) => Value::Bool(*b),
        DecoratorArgument::TypeReference(t) => json!({
            "type": "Identifier",
            "name": t.name,
            "array": t.array,
        }),
    }
}

fn model_manager_query(r: &Replayed, member: &str, args: &[Arg]) -> Dispatch {
    let plain = |i: usize| match args.get(i) {
        None => Some(recipe::undefined()),
        Some(Arg::Plain(v)) => Some(v.clone()),
        Some(_) => None,
    };
    match member {
        "getNamespaces" => ran(Ok(json!(r.namespaces()))),
        "getAst" => {
            let (Some(resolve), Some(include)) = (plain(0), plain(1)) else {
                return unsupported("getAst with non-plain arguments");
            };
            if recipe::truthy(&resolve) {
                return Dispatch::Fault(Fault::Blocked(
                    "getAst(resolve = true) needs resolveMetaModel, not ported yet".into(),
                    recipe::Blocker::Member("BaseModelManager.resolveMetaModel".into()),
                ));
            }
            ran(Ok(r.ast(recipe::truthy(&include))))
        }
        "getType" => {
            let Some(Value::String(fqn)) = plain(0) else {
                return unsupported("getType with a type name that is not a string");
            };
            match r.mm.get_declaration(&fqn) {
                Err(e) => ran(Err(to_oracle_error(&e))),
                Ok(_) => {
                    let id = r.mm.declaration_id(&fqn).expect("get_declaration found it");
                    ran(Ok(r.declaration_summary(id).unwrap_or(Value::Null)))
                }
            }
        }
        "getMapDeclarations" => {
            // TS `BaseModelManager.getMapDeclarations` (basemodelmanager.js):
            // every model file's map declarations, concatenated in
            // registration order.
            let declarations: Vec<Value> =
                r.mm.model_files()
                    .flat_map(|mf| {
                        let file = r.mm.model_file_id(mf.namespace());
                        file.into_iter().flat_map(|f| r.mm.declaration_ids(f))
                    })
                    .filter(|id| matches!(r.mm.declaration(*id), Some(Declaration::Map(_))))
                    .filter_map(|id| r.declaration_summary(id))
                    .collect();
            ran(Ok(Value::Array(declarations)))
        }
        _ => unreachable!("`dispatched` lists every query"),
    }
}

/// The namespace of the model file a declaration was loaded into, for the
/// map-part ops (`MapKeyType`/`MapValueType.getNamespace` and the shared
/// `validate_map_key`/`validate_map_value` calls, which both need it to
/// resolve a referenced type).
fn map_namespace(r: &Replayed, id: DeclId) -> Option<&str> {
    let file_id = r.mm.model_file_of(id)?;
    Some(r.mm.file(file_id)?.namespace())
}

/// The `ModelUtil` statics that take model-manager collaborators.
fn model_util_with_context(session: &Session, member: &str, args: &[Arg]) -> Faulty<Dispatch> {
    let prop = |arg: Option<&Arg>| -> Faulty<(usize, Node)> {
        match arg {
            Some(Arg::Prop(index, id)) => Ok((*index, Node::Property(*id))),
            _ => Err(Fault::Unsupported(
                "a field argument that is not a property of a registered declaration".into(),
            )),
        }
    };
    match member {
        "isAssignableTo" => {
            let Some(Arg::File(file)) = args.first() else {
                return Err(Fault::Unsupported(
                    "isAssignableTo with a model file argument that is not a model file".into(),
                ));
            };
            let Some(Arg::Plain(Value::String(type_name))) = args.get(1) else {
                return Err(Fault::Unsupported(
                    "isAssignableTo with a type name that is not a string".into(),
                ));
            };
            let (index, property) = prop(args.get(2))?;
            let r = &session.pool[index];
            let model_file = recipe::model_file_node(r, file)?;
            Ok(from_engine(
                model_util::is_assignable_to(&r.mm, &model_file, type_name, &property),
                Value::Bool,
            ))
        }
        "isEnum" | "isMap" | "isScalar" => {
            let (index, field) = prop(args.first())?;
            let r = &session.pool[index];
            let result = match member {
                "isEnum" => model_util::is_enum(&r.mm, &field),
                "isMap" => model_util::is_map(&r.mm, &field),
                _ => model_util::is_scalar(&r.mm, &field),
            };
            Ok(from_engine(result, option_bool))
        }
        "isValidMapKeyScalar" => match args.first() {
            // `decl?.…`: a nullish declaration never reaches the context.
            None => Ok(from_engine(
                model_util::is_valid_map_key_scalar(&fresh_context()?, None),
                option_bool,
            )),
            Some(Arg::Plain(v)) if v.is_null() || recipe::is_undefined(v) => Ok(from_engine(
                model_util::is_valid_map_key_scalar(&fresh_context()?, None),
                option_bool,
            )),
            Some(Arg::Decl(index, id)) => {
                let r = &session.pool[*index];
                Ok(from_engine(
                    model_util::is_valid_map_key_scalar(&r.mm, Some(&Node::Declaration(*id))),
                    option_bool,
                ))
            }
            Some(_) => Err(Fault::Unsupported(
                "isValidMapKeyScalar with an argument that is not a declaration".into(),
            )),
        },
        _ => unreachable!("`dispatched` lists every ModelUtil collaborator op"),
    }
}

/// A context for a call that is given no handle at all
/// (`isValidMapKeyScalar(undefined)`), which never consults it.
fn fresh_context() -> Faulty<ModelManager> {
    ModelManager::new().map_err(|e| {
        Fault::Divergence(format!(
            "ModelManager::new failed: {}",
            to_oracle_error(&e).message
        ))
    })
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
            // model specially (a node-semver `SemVer`) is recorded as the
            // generic summary `{"@@oracle":"object","ctor":"<constructor>"}`
            // (confirmed against a recorded fixture, task P1-07). This
            // checks that a `SemVer` was returned at all when `version` is
            // present; node-semver's own fields have no Rust port to check.
            "versionParsed": version_parsed.as_ref().map(|_| json!({
                M: "object",
                "ctor": "SemVer",
            })),
        }),
    }
}

/// Builds the oracle's `outcome.error` shape from a [`ConcertoError`], the
/// same fields `ContractError` carries (PORTING.md section 2.1):
/// `kind.ts_class()` for `class`, [`concerto_core::error::ContractError::final_message`]
/// for `message` (its doc comment: "Used by the native oracle harness
/// only"), the AST `location` verbatim, and `component`. The two pre-port
/// variants (`TypeNotFound`, `IllegalModel`) carry no catalogue key; they
/// map to their TS class with their own text, so a fixture that reaches one
/// fails on its message until the owning task ports the throw site.
pub fn to_oracle_error(err: &ConcertoError) -> OracleError {
    match err {
        ConcertoError::Contract(ce) => OracleError {
            class: ce.kind.ts_class().to_string(),
            message: ce.final_message(),
            location: ce.location.clone(),
            component: ce.component().map(str::to_string),
        },
        ConcertoError::TypeNotFound { type_name } => OracleError {
            class: ErrorKind::TypeNotFound.ts_class().to_string(),
            message: format!("Type \"{type_name}\" not found."),
            location: None,
            component: Some("@accordproject/concerto-core".into()),
        },
        ConcertoError::IllegalModel {
            message, location, ..
        } => OracleError {
            class: ErrorKind::IllegalModel.ts_class().to_string(),
            message: message.clone(),
            location: location.clone(),
            component: Some("@accordproject/concerto-core".into()),
        },
    }
}
