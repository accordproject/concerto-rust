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
//!   `getDefaultValue`, over the trial's port of `ScalarDeclaration.process`.
//! - **`MapDeclaration`** `declarationKind`, `getKey`, `getValue`,
//!   `isMapDeclaration`, `toString` and `validate`; **`MapKeyType`**/
//!   **`MapValueType`** `getType`, `getNamespace`, `getParent`, `toString` and
//!   `validate` (P2-06), over a `recipe::Arg::MapPart` handle (`recipe.rs`)
//!   since this engine reads a map's key and value as plain accessors on
//!   `MapDeclaration` rather than as their own registered declarations;
//!   **`ClassDeclaration.isMapDeclaration`** and
//!   **`ModelManager.getMapDeclarations`**, generic queries any declaration
//!   or model manager already answers. A fixture whose target is an
//!   unregistered `ModelFile` (`mfnew`, e.g. most of
//!   `MapDeclaration.validate`) stays `unsupported`, owned by P2-08's
//!   `ModelFile.new`.

use concerto_core::error::{ConcertoError, ErrorKind};
use concerto_core::introspect::scalar::ScalarValidator;
use concerto_core::introspect::{Declaration, DeclarationKind, MapDeclaration};
use concerto_core::model_manager::{DeclId, ModelManager, Node, ResolutionContext};
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
                "toString" | "getType" | "getValidator" | "getDefaultValue"
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
        ("MapKeyType" | "MapValueType", m) => {
            matches!(
                m,
                "getType" | "getNamespace" | "getParent" | "toString" | "validate"
            )
        }
        ("ClassDeclaration", "isMapDeclaration") => true,
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
        "ScalarDeclaration" => {
            let Some(Arg::Decl(index, id)) = target else {
                return Err(Fault::Unsupported(
                    "a ScalarDeclaration receiver that is not a declref".into(),
                ));
            };
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
                _ => ran(Ok(match scalar.validator() {
                    None => Value::Null,
                    Some(ScalarValidator::Number(_)) => {
                        json!({ M: "Validator", "ctor": "NumberValidator" })
                    }
                    Some(ScalarValidator::String { .. }) => {
                        json!({ M: "Validator", "ctor": "StringValidator" })
                    }
                })),
            })
        }
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
        "ClassDeclaration" => {
            let Some(Arg::Decl(index, id)) = target else {
                return Err(Fault::Unsupported(
                    "a ClassDeclaration receiver that is not a declref".into(),
                ));
            };
            let r = &session.pool[index];
            let Some(declaration) = r.mm.declaration(id) else {
                return Err(Fault::Divergence(
                    "state divergence: the declaration is no longer registered".into(),
                ));
            };
            Ok(ran(Ok(Value::Bool(declaration.is_map_declaration()))))
        }
        _ => unreachable!("`dispatched` lists every class"),
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
