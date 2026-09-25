//! Rebuilds a fixture's model managers, model files, declarations and
//! properties on the Rust engine, from their recipes (README "Value
//! encoding"; `makeDecoder` in `migration/oracle/lib/codec.js` is the JS
//! original this follows step by step).
//!
//! # Model managers (`mm`, `mmref`, `self`)
//!
//! A recipe is `{id, kind, options, steps}`. The harness builds a fresh
//! [`ModelManager`] (which, since P1-07b, preloads `concerto.decorator@1.0.0`
//! and then `concerto@1.0.0` as the TS constructor does) and replays each
//! step through the Rust engine. A step whose status (`ok`/`error`) or error
//! class differs from the recorded one is a **state divergence**, a failure
//! (README; PORTING.md 6.2). A `derived` recipe (a model manager returned by
//! another op, such as `MetaModel.modelManagerFromMetaModel`) is replayed
//! only when that op is; none is yet, so it is `unsupported`.
//!
//! The steps map onto the Rust `ModelManager` as follows. `add_model` is
//! the Rust counterpart of TS `new ModelFile(...)` plus `addModelFile`'s
//! registration (its version and namespace checks), and
//! `validate_models` of `validateModelFiles`; CTO text becomes an AST through
//! the P1-07a cache ([`super::cto_cache`]), never a Rust parser.
//!
//! | TS step | Replayed as |
//! |---|---|
//! | `addCTOModel`, `addModel` (CTO text or AST), `addModelFile` | `add_model`, then, unless validation is disabled, the new file's validation (below) |
//! | `addModelFiles` | `add_model` per file, then `validate_models` unless validation is disabled; any error restores the files that were there before, as TS does |
//! | `validateModelFiles` | `validate_models` |
//! | `clearModelFiles` | a fresh `ModelManager::new()` (TS: `modelFiles = {}`, then the decorator and root models again) |
//! | `fromAst` | `clearModelFiles`, `add_model` per non-system model, then `validate_models` unless disabled |
//! | `updateModelFile`, `deleteModelFile`, `addDecoratorFactory` | `unsupported`: the Rust engine has no counterpart yet |
//!
//! **Validation on add.** TS `addModelFile` validates *only the new file*
//! (`modelFile.validate()`) before registering it. The Rust engine has no
//! single-file validation yet (`ModelFile.validate` is P2-08's); it only
//! validates every file (`validate_models`). The two agree exactly when every
//! file already registered is known to validate on the Rust engine (it was
//! added with validation, or a later `validateModelFiles` passed): adding a
//! namespace cannot make another file invalid, so the first error
//! `validate_models` reports is then the new file's. The harness therefore
//! replays a validating add as "register, `validate_models`, and on an error
//! restore the files that were there before" when that holds, and reports the
//! fixture `unsupported` when it does not, rather than risk blaming the new
//! file for an older one's error. Removal has no Rust counterpart either, so
//! restoring rebuilds the manager from the surviving files (all of which
//! loaded before).
//!
//! **Options.** Only `skipLocationNodes` (it selects the cache entry) is
//! replayed. Any other option with a truthy value changes TS behaviour the
//! Rust engine does not model yet (`metamodelValidation`, `addMetamodel`,
//! `decoratorValidation`, ...), so such a recipe is `unsupported`.
//!
//! # Model files, declarations, properties
//!
//! `mfref` is the Rust model file registered under `ns`; `mfnew` is built
//! with `ModelFile::from_json` (TS `new ModelFile(mm, ast, definitions,
//! fileName)`), a failure being "input construction failed" (a failure).
//! `declref` and `propref` become [`Node`] handles by position, checked by
//! name as `codec.js` checks them. `decoref` (P2-07) becomes a
//! [`DecoParent`] plus its position, resolved against its parent's processed
//! decorators at dispatch time (`ops.rs`); its `parent` must itself be a
//! `declref`, `propref` or `mfref`. A map key/value `propref` (one with a
//! `part`) becomes an [`Arg::MapPart`] (P2-06). A `declnew` (a declaration built
//! directly via `new Cls(modelFile, ast)`, never added to `modelFile`) is
//! rebuilt with `ScalarDeclaration::build_standalone` when `cls` is
//! `ScalarDeclaration` (P2-05); any other `cls` is `unsupported`, for its own
//! owner. `validatorref`, `factory`, `serializer`,
//! `introspector`, `predicate` and `decoratorfactory` have no Rust
//! counterpart yet: `unsupported`. `typed` (P3-01 review, task
//! `accordproject-concerto-rust#56` follow-up) is decoded directly into
//! [`DecodedInstance`] by [`Session::typed`], from the node's own `fields`
//! object rather than a `Factory`/`JSONPopulator` replay — see
//! [`Arg::Typed`]'s doc.

use std::collections::HashMap;

use concerto_core::instance::validate::{DAYJS_TAG, RELATIONSHIP_TAG, js_undefined};
use concerto_core::introspect::scalar::ProcessedScalar;
use concerto_core::introspect::{
    Declaration, DeclarationKind, ModelFile, Named, ScalarDeclaration,
};
use concerto_core::model_manager::{DeclId, ModelFileId, ModelManager, Node, PropId};
use concerto_core::model_util;
use serde_json::{Value, json};

use super::Harness;
use super::cto_cache::CacheEntry;
use super::ledger::UNOWNED;
use super::ops::{OracleError, to_oracle_error};

pub const M: &str = "@@oracle";

/// TS `EXCLUDE_NS` (src/basemodelmanager.ts): the namespaces `fromAst` and
/// `getModelFiles()` leave out.
const EXCLUDE_NS: [&str; 3] = ["concerto@1.0.0", "concerto", "concerto.decorator@1.0.0"];

/// Why a fixture could not be judged on its outcome.
#[derive(Debug)]
pub enum Fault {
    /// The fixture's op cannot be replayed with these inputs yet; the op's
    /// own owner is responsible.
    Unsupported(String),
    /// Something other than the fixture's op blocks it: a recipe step, an
    /// option, an input kind. The [`Blocker`] names who owns that.
    Blocked(String, Blocker),
    /// The fixture itself is broken, or its CTO text is missing from the
    /// cache: never a pass (plan §2.6).
    Harness(String),
    /// The Rust engine behaved differently while the inputs were being
    /// rebuilt (a step's status, a missing model file, a construction
    /// failure): a failure (README "Verdicts").
    Divergence(String),
}

pub type Faulty<T> = Result<T, Fault>;

/// What blocks an unsupported fixture, for its owner (`ledger.rs`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Blocker {
    /// A TS member, `<Class>.<member>`, whose owner the ledger gives.
    Member(String),
    /// An owner named directly: a task, or `ledger::UNOWNED`.
    Owner(String),
}

fn blocked(reason: impl Into<String>, member: impl Into<String>) -> Fault {
    Fault::Blocked(reason.into(), Blocker::Member(member.into()))
}

/// The TS member an input kind this harness cannot rebuild stands for.
/// `"typed"` is not listed here any more: [`Session::decode`] now decodes it
/// directly (P3-01 review), ahead of this fallback.
fn member_of_kind(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "declnew" => "ScalarDeclaration.new",
        "decoref" => "Decorator.new",
        "validatorref" => "Validator.new",
        "factory" => "Factory.new",
        "serializer" => "Serializer.new",
        "introspector" => "Introspector.new",
        "predicate" => "BaseModelManager.filter",
        "decoratorfactory" => "BaseModelManager.addDecoratorFactory",
        _ => return None,
    })
}

/// What an engine call produced: a value in the oracle's output encoding,
/// or an error in its `outcome.error` shape.
pub type Outcome = Result<Value, OracleError>;

pub fn undefined() -> Value {
    json!({ M: "undefined" })
}

pub fn is_undefined(v: &Value) -> bool {
    v.get(M).and_then(Value::as_str) == Some("undefined")
}

/// JS truthiness of a decoded plain value.
pub fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) => true,
        Value::Object(_) => match v.get(M).and_then(Value::as_str) {
            Some("undefined") => false,
            Some("number") => v
                .get("value")
                .and_then(Value::as_str)
                .is_some_and(|n| n != "NaN" && n != "-0"),
            _ => true,
        },
    }
}

/// The kind of a model manager recipe.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // the TS class names, as recipes spell them
pub enum Kind {
    ModelManager,
    BaseModelManager,
    AstModelManager,
}

impl Kind {
    fn parse(kind: &str) -> Faulty<Self> {
        match kind {
            "ModelManager" => Ok(Self::ModelManager),
            "BaseModelManager" => Ok(Self::BaseModelManager),
            "AstModelManager" => Ok(Self::AstModelManager),
            other => Err(Fault::Harness(format!(
                "unknown model manager kind {other}"
            ))),
        }
    }

    pub fn ctor(self) -> &'static str {
        match self {
            Self::ModelManager => "ModelManager",
            Self::BaseModelManager => "BaseModelManager",
            Self::AstModelManager => "AstModelManager",
        }
    }
}

/// A user model file as the harness tracks it: enough to rebuild the Rust
/// manager without it (restoring after a failed add), and the JS nullish
/// spelling of its file name, which the Rust engine's `Option<String>`
/// does not keep (`getName()` returns `undefined` or `null` as passed).
#[derive(Debug, Clone)]
struct Entry {
    ast: Value,
    file_name: Option<String>,
    nullish_name: Value,
    known_valid: bool,
}

/// A replayed model manager.
pub struct Replayed {
    pub kind: Kind,
    skip_location_nodes: Value,
    files: Vec<Entry>,
    pub mm: ModelManager,
}

/// A model file argument, rebuilt from `mfref` or `mfnew`.
#[derive(Debug, Clone)]
pub struct FileArg {
    pub(crate) ast: Value,
    pub(crate) file_name: Option<String>,
    nullish_name: Value,
}

/// One decoded argument.
#[derive(Debug)]
pub enum Arg {
    /// Plain JSON, or the `{"@@oracle":"undefined"}` marker.
    Plain(Value),
    File(FileArg),
    /// A model manager, by its index in the session's pool.
    Mm(usize),
    /// The model manager a step runs on.
    SelfMm,
    Decl(usize, DeclId),
    Prop(usize, PropId),
    /// A `MapKeyType` (`is_key = true`) or `MapValueType` (`is_key = false`)
    /// belonging to the `MapDeclaration` at `(pool index, DeclId)`. TS gives
    /// these their own class, but this engine reads a map's key and value as
    /// plain accessors on `MapDeclaration` (README "Ops";
    /// `introspect::declaration::MapDeclaration`), so there is no separate
    /// handle to register — the recorder's `propref` with a `part` field
    /// (`"key"` or `"value"`) decodes straight to this variant instead of a
    /// `PropId`.
    MapPart(usize, DeclId, bool),
    /// A declaration built directly via `new` (`declnew`), never added to its
    /// model file: `ScalarDeclaration::build_standalone`'s result, computed
    /// eagerly here as TS runs the constructor while decoding the receiver.
    DeclNew {
        fqn: String,
        processed: ProcessedScalar,
    },
    /// A validator reached through a property (`validatorref` with a
    /// `propref` owner): the property's model manager pool index, the
    /// property itself, and which validator it names (`"validator"`, the
    /// regex/length or numeric-domain one; `"size"`, the collection-size
    /// one) — `ops.rs` rebuilds the actual validator from these, since no
    /// production `Property`/`Field` API returns one yet (P2-04/P2-05).
    Validator(usize, PropId, String),
    /// A decorator: the pool index of its model manager, which of its
    /// declaration/property/model-file's decorators it is, and its position
    /// (P2-07).
    Deco(usize, DecoParent, usize),
    /// An array that holds encoded values (a list of model files).
    List(Vec<Arg>),
    /// A `Resource`, `ValidatedResource` or `Relationship` (`"typed"`,
    /// README "Value encoding"), decoded into [`DecodedInstance`]: the
    /// model manager it belongs to (pool index) plus the instance itself.
    /// P3-01 review (task `accordproject-concerto-rust#56` follow-up):
    /// `Resource.validate` and the `Identifiable`/`Typed`/`Relationship`
    /// accessors are dispatched from this.
    Typed(usize, DecodedInstance),
}

/// A decoded oracle `"typed"` value: enough of a `Resource`, `ValidatedResource`
/// or `Relationship` to dispatch `Resource.validate` and the read-only
/// `Typed`/`Identifiable`/`Relationship` accessors (`ops.rs`), without a
/// `JSONPopulator`/`Factory` port (`instance/validate.rs`'s module doc
/// "Scope" — this is the decode side of the same scope decision: it reads
/// an oracle `"typed"` node's own `fields` object directly, never a `decl`
/// handle, so it needs no `ModelManager` lookup of its own beyond the one
/// `mm` index every field that points to another instance already carries).
///
/// TS: this is `Resource`/`ValidatedResource`/`Relationship`
/// (`src/model/resource.ts`, `validatedresource.ts`, `relationship.ts`),
/// read back from the oracle's own recorded encoding
/// (`migration/oracle/lib/codec.js`'s `"typed"` kind) rather than rebuilt
/// through a Rust `Factory`, which does not exist yet.
///
/// The one exception is the receiver's own `$classDeclaration`
/// ([`Self::class_declaration`]), which `Typed.getClassDeclaration` and
/// `instanceOf` return or walk: [`Session::typed`] resolves the node's `decl`
/// handle for that, when it is a `declref`.
#[derive(Debug, Clone)]
pub struct DecodedInstance {
    /// `"Resource"`, `"ValidatedResource"` or `"Relationship"`.
    pub ctor: String,
    /// TS `$namespace`.
    pub namespace: String,
    /// TS `$type` (short name, no namespace).
    pub type_name: String,
    /// TS `$namespace + '.' + $type` (`getFullyQualifiedType()`).
    pub fqn: String,
    /// TS `$identifierFieldName`. Not yet read by any dispatched op
    /// ([`super::ops::instance_op`]); kept for a future one (`setIdentifier`,
    /// say) rather than dropped, the same way [`super::fixture::Fixture`]
    /// keeps `source_test`.
    #[allow(dead_code)]
    pub identifier_field_name: Option<String>,
    /// TS `$identifier` (`getIdentifier()`/`getFullyQualifiedIdentifier()`).
    pub identifier: Option<String>,
    /// TS `$timestamp` (`Identifiable.getTimestamp()`), in the oracle's own
    /// encoding (a `dayjs` node, `null`, or the `undefined` marker when the
    /// field was never set).
    pub timestamp: Value,
    /// TS `$classDeclaration`, when the node's `decl` is a `declref` this
    /// session could resolve (only a top-level receiver or argument; a
    /// nested field value is decoded without a session, so `None`).
    pub class_declaration: Option<DeclId>,
    /// The instance in [`crate::instance::validate::validate_instance`]'s
    /// input shape (module doc "Scope"): for a `Resource`/`ValidatedResource`,
    /// a `$class`-tagged wire-shaped object with every own (non-system)
    /// field recursively decoded the same way, `DateTime`/`Relationship`
    /// values tagged per that module's `DAYJS_TAG`/`RELATIONSHIP_TAG`; for a
    /// `Relationship`, a `RELATIONSHIP_TAG`-tagged `{"$class": <pointed-at
    /// fqn>}` object (never a URI string — module doc "Scope" — since this
    /// is already a *populated* `Relationship`, not wire JSON).
    pub wire: Value,
}

/// What a `decoref`'s `parent` names (P2-07).
#[derive(Debug, Clone)]
pub enum DecoParent {
    Decl(DeclId),
    Prop(PropId),
    /// A model file's own decorators, by namespace.
    File(String),
}

/// One decoding session: `dctx` in `codec.js`.
pub struct Session<'h> {
    h: &'h Harness,
    ids: HashMap<String, usize>,
    pub pool: Vec<Replayed>,
}

fn nullish_or_string(v: &Value) -> Faulty<(Option<String>, Value)> {
    match v {
        Value::String(s) => Ok((Some(s.clone()), v.clone())),
        Value::Null => Ok((None, Value::Null)),
        _ if is_undefined(v) => Ok((None, undefined())),
        _ => Err(Fault::Unsupported(
            "a file name that is neither a string nor nullish".into(),
        )),
    }
}

fn divergence_from(err: &OracleError, what: &str) -> Fault {
    Fault::Divergence(format!(
        "input construction failed: {what}: {}: {}",
        err.class, err.message
    ))
}

impl<'h> Session<'h> {
    pub fn new(h: &'h Harness) -> Self {
        Self {
            h,
            ids: HashMap::new(),
            pool: Vec::new(),
        }
    }

    /// Decodes one argument. `self_mm` is the model manager a step runs on,
    /// for `self` and `mfref`s of it (steps are decoded in their own
    /// session, as `codec.js` decodes them with a fresh `sctx`).
    pub fn decode(&mut self, v: &Value, self_mm: Option<&Replayed>) -> Faulty<Arg> {
        let Some(kind) = v.get(M).and_then(Value::as_str) else {
            if !contains_marker(v) {
                return Ok(Arg::Plain(v.clone()));
            }
            if let Value::Array(items) = v {
                return items
                    .iter()
                    .map(|x| self.decode(x, self_mm))
                    .collect::<Faulty<Vec<_>>>()
                    .map(Arg::List);
            }
            return Err(Fault::Unsupported(
                "an encoded value nested inside a plain object".into(),
            ));
        };
        match kind {
            "undefined" => Ok(Arg::Plain(v.clone())),
            "self" => match self_mm {
                Some(_) => Ok(Arg::SelfMm),
                None => Err(Fault::Harness(
                    "\"self\" outside a model manager step".into(),
                )),
            },
            "mm" => self.replay(v).map(Arg::Mm),
            "mmref" => self.mmref(v).map(Arg::Mm),
            "mfref" | "mfnew" => self.file(v, self_mm).map(Arg::File),
            "declref" => {
                let (mm, id) = self.declref(v)?;
                Ok(Arg::Decl(mm, id))
            }
            "propref" if v.get("part").and_then(Value::as_str).is_some() => {
                let (mm, id, is_key) = self.map_part(v)?;
                Ok(Arg::MapPart(mm, id, is_key))
            }
            "declnew" => self.declnew(v, self_mm),
            "typed" => self.typed(v),
            "propref" => {
                let (mm, id) = self.propref(v)?;
                Ok(Arg::Prop(mm, id))
            }
            "validatorref" => {
                let (mm, id, part) = self.validatorref(v)?;
                Ok(Arg::Validator(mm, id, part))
            }
            "decoref" => {
                let (mm, parent, index) = self.decoref(v)?;
                Ok(Arg::Deco(mm, parent, index))
            }
            "blob" => Err(Fault::Harness("unresolved blob".into())),
            other => {
                let reason = format!("@@oracle:{other} has no Rust counterpart yet");
                Err(match member_of_kind(other) {
                    Some(member) => blocked(reason, member),
                    None => Fault::Unsupported(reason),
                })
            }
        }
    }

    fn mmref(&self, v: &Value) -> Faulty<usize> {
        let id = v.get("id").map(Value::to_string).unwrap_or_default();
        self.ids
            .get(&id)
            .copied()
            .ok_or_else(|| Fault::Harness(format!("dangling mmref {id}")))
    }

    /// A model manager argument (`mm` or `mmref`) as a pool index.
    pub fn mm_index(&mut self, v: &Value) -> Faulty<usize> {
        match v.get(M).and_then(Value::as_str) {
            Some("mm") => self.replay(v),
            Some("mmref") => self.mmref(v),
            _ => Err(Fault::Unsupported(
                "a receiver that is not a model manager recipe".into(),
            )),
        }
    }

    /// Rebuilds one `mm` recipe on the Rust engine.
    fn replay(&mut self, node: &Value) -> Faulty<usize> {
        let kind = Kind::parse(node.get("kind").and_then(Value::as_str).unwrap_or(""))?;
        let mut r = match node.get("derived") {
            Some(derived) => {
                let op = derived.get("op").and_then(Value::as_str).unwrap_or("?");
                let inputs: super::fixture::Inputs =
                    serde_json::from_value(derived.get("inputs").cloned().unwrap_or(Value::Null))
                        .map_err(|e| Fault::Harness(format!("derived inputs of {op}: {e}")))?;
                let Some(derived_mm) =
                    super::ops::derive_model_manager(self.h, op, &inputs, derived.get("path"))?
                else {
                    return Err(blocked(
                        format!(
                            "a model manager derived from {op}, which is not replayed natively"
                        ),
                        op,
                    ));
                };
                Replayed::from_derived(kind, derived_mm)
            }
            None => {
                let options = node.get("options").cloned().unwrap_or_else(undefined);
                Replayed::new(kind, &options)?
            }
        };
        for step in node
            .get("steps")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let method = step
                .get("method")
                .and_then(Value::as_str)
                .ok_or_else(|| Fault::Harness("a step without a method".into()))?;
            let raw_args = step
                .get("args")
                .and_then(Value::as_array)
                .ok_or_else(|| Fault::Harness("a step without args".into()))?;
            let args = {
                let mut sub = Session::new(self.h);
                raw_args
                    .iter()
                    .map(|a| sub.decode(a, Some(&r)))
                    .collect::<Faulty<Vec<_>>>()?
            };
            let result = r.apply(self.h, method, &args)?;
            let recorded = step.get("status").and_then(Value::as_str).unwrap_or("?");
            let recorded_class = step.get("errorClass").and_then(Value::as_str);
            let (status, class, message) = match &result {
                Ok(_) => ("ok", None, None),
                Err(e) => ("error", Some(e.class.as_str()), Some(e.message.as_str())),
            };
            if status != recorded || (status == "error" && class != recorded_class) {
                return Err(Fault::Divergence(format!(
                    "state divergence: step {method} recorded {recorded}{}, replayed {status}{}{}",
                    recorded_class.map(|c| format!("({c})")).unwrap_or_default(),
                    class.map(|c| format!("({c})")).unwrap_or_default(),
                    message.map(|m| format!(": {m}")).unwrap_or_default(),
                )));
            }
        }
        let index = self.pool.len();
        self.pool.push(r);
        if let Some(id) = node.get("id") {
            self.ids.insert(id.to_string(), index);
        }
        Ok(index)
    }

    fn file(&mut self, v: &Value, self_mm: Option<&Replayed>) -> Faulty<FileArg> {
        let mm_node = v
            .get("mm")
            .ok_or_else(|| Fault::Harness("a model file without its model manager".into()))?;
        let owner: Option<usize> = match mm_node.get(M).and_then(Value::as_str) {
            Some("self") if self_mm.is_some() => None,
            Some("self") => {
                return Err(Fault::Harness(
                    "\"self\" outside a model manager step".into(),
                ));
            }
            _ => Some(self.mm_index(mm_node)?),
        };
        if v.get(M).and_then(Value::as_str) == Some("mfref") {
            let ns = v
                .get("ns")
                .and_then(Value::as_str)
                .ok_or_else(|| Fault::Harness("mfref without ns".into()))?;
            let r = match owner {
                Some(index) => &self.pool[index],
                None => self_mm.expect("checked above"),
            };
            return r.file_arg(ns).ok_or_else(|| {
                Fault::Divergence(format!(
                    "state divergence: model file {ns} not registered after replay"
                ))
            });
        }
        let ast = v.get("ast").cloned().unwrap_or(Value::Null);
        let (file_name, nullish_name) =
            nullish_or_string(v.get("fileName").unwrap_or(&undefined()))?;
        // TS `new ModelFile(mm, ast, definitions, fileName)` runs while the
        // input is decoded, so a failure here is "input construction failed".
        ModelFile::from_json(&ast, file_name.clone())
            .map_err(|e| divergence_from(&to_oracle_error(&e), "new ModelFile"))?;
        Ok(FileArg {
            ast,
            file_name,
            nullish_name,
        })
    }

    /// A declaration built directly via `new` (`{cls, mf, ast}`, never added
    /// to `mf`): `codec.js`'s `decodeDecl` runs `new Cls(mf, ast)` while
    /// decoding, so a constructor failure here is "input construction
    /// failed", the same convention as `file()`'s `new ModelFile`. Only
    /// `ScalarDeclaration` is a recorded `declnew` class so far (P2-05); any
    /// other is `unsupported` for its own owner.
    fn declnew(&mut self, v: &Value, self_mm: Option<&Replayed>) -> Faulty<Arg> {
        let cls = v.get("cls").and_then(Value::as_str).unwrap_or_default();
        if cls != "ScalarDeclaration" {
            return Err(blocked(
                format!("a declaration built directly via `new {cls}(...)`, not ported yet"),
                format!("{cls}.new"),
            ));
        }
        let mf = v
            .get("mf")
            .ok_or_else(|| Fault::Harness("declnew without mf".into()))?;
        let file = self.file(mf, self_mm)?;
        let ast = v.get("ast").cloned().unwrap_or(Value::Null);
        let namespace = file
            .ast
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or("");
        let (fqn, processed) =
            ScalarDeclaration::build_standalone(namespace, file.file_name.as_deref(), &ast)
                .map_err(|e| divergence_from(&to_oracle_error(&e), "new ScalarDeclaration"))?;
        Ok(Arg::DeclNew { fqn, processed })
    }

    /// Decodes an oracle `"typed"` value (README "Value encoding": "a
    /// Resource, ValidatedResource or Relationship: its handles plus every
    /// own property in order") into a [`DecodedInstance`] ([`Arg::Typed`]'s
    /// doc has the rationale for reading `fields` directly rather than the
    /// `decl`/`mm` handles a full `Factory`/`JSONPopulator` port would use).
    ///
    /// `fields` always carries `$namespace`/`$type`/`$identifierFieldName`/
    /// `$identifier` (recorded straight from the TS instance's own private
    /// fields, `identifiable.ts`), so those need no `ModelManager` lookup;
    /// this method resolves only the `mm` handle every instance carries (so
    /// `Resource.validate` and friends have a real [`ModelManager`] to run
    /// against) and recurses into the non-system fields via
    /// [`Self::typed_field_value`], which needs no further `mm` resolution
    /// of its own (a nested `"typed"`/`"dayjs"` field value carries its own
    /// data, not a fresh handle to look up).
    fn typed(&mut self, v: &Value) -> Faulty<Arg> {
        let mm_node = v
            .get("mm")
            .ok_or_else(|| Fault::Harness("typed value without mm".into()))?;
        let mm_index = self.mm_index(mm_node)?;
        let mut inst = decode_typed_instance(v)?;
        // `$classDeclaration`: only a registered declaration has a handle. A
        // `decl` this session cannot resolve leaves it `None`, which only
        // the ops that read it (`getClassDeclaration`, `instanceOf`) report.
        if let Some(decl) = v.get("decl")
            && decl.get(M).and_then(Value::as_str) == Some("declref")
            && let Ok((owner, id)) = self.declref(decl)
            && owner == mm_index
        {
            inst.class_declaration = Some(id);
        }
        Ok(Arg::Typed(mm_index, inst))
    }

    fn declref(&mut self, v: &Value) -> Faulty<(usize, DeclId)> {
        let mf = v
            .get("mf")
            .ok_or_else(|| Fault::Harness("declref without mf".into()))?;
        if mf.get(M).and_then(Value::as_str) != Some("mfref") {
            return Err(blocked(
                "a declaration of a model file that is not registered (mfnew) has no Rust handle",
                "ModelFile.new",
            ));
        }
        let owner = self.mm_index(
            mf.get("mm")
                .ok_or_else(|| Fault::Harness("mfref without mm".into()))?,
        )?;
        let ns = mf.get("ns").and_then(Value::as_str).unwrap_or_default();
        let index = v.get("index").and_then(Value::as_u64).unwrap_or(u64::MAX);
        let name = v.get("name").and_then(Value::as_str).unwrap_or_default();
        let mm = &self.pool[owner].mm;
        let not_found =
            || Fault::Divergence(format!("state divergence: declaration {name} not found"));
        let file = mm.model_file_id(ns).ok_or_else(|| {
            Fault::Divergence(format!(
                "state divergence: model file {ns} not registered after replay"
            ))
        })?;
        let id = mm
            .declaration_ids(file)
            .nth(usize::try_from(index).unwrap_or(usize::MAX))
            .ok_or_else(not_found)?;
        match mm.declaration(id) {
            Some(d) if d.name() == name => Ok((owner, id)),
            _ => Err(not_found()),
        }
    }

    /// A `MapKeyType`/`MapValueType` target: `{decl: <declref>, part: "key" |
    /// "value"}` (`migration/oracle/lib/codec.js`). The referenced
    /// declaration is checked to be a `MapDeclaration` here, once, rather
    /// than by every op that takes an [`Arg::MapPart`].
    fn map_part(&mut self, v: &Value) -> Faulty<(usize, DeclId, bool)> {
        let part = v.get("part").and_then(Value::as_str).unwrap_or_default();
        let is_key = match part {
            "key" => true,
            "value" => false,
            other => {
                return Err(Fault::Unsupported(format!(
                    "a map part that is neither \"key\" nor \"value\": {other:?}"
                )));
            }
        };
        let decl = v
            .get("decl")
            .ok_or_else(|| Fault::Harness("propref without decl".into()))?;
        if decl.get(M).and_then(Value::as_str) != Some("declref") {
            return Err(blocked(
                "a map key or value of a declaration that is not in its model file (declnew)",
                "MapDeclaration.new",
            ));
        }
        let (owner, id) = self.declref(decl)?;
        match self.pool[owner].mm.declaration(id) {
            Some(Declaration::Map(_)) => Ok((owner, id, is_key)),
            _ => Err(Fault::Divergence(
                "state divergence: the declaration did not load as a map".into(),
            )),
        }
    }

    fn propref(&mut self, v: &Value) -> Faulty<(usize, PropId)> {
        let decl = v
            .get("decl")
            .ok_or_else(|| Fault::Harness("propref without decl".into()))?;
        if decl.get(M).and_then(Value::as_str) != Some("declref") {
            return Err(blocked(
                "a property of a declaration that is not in its model file (declnew)",
                "ScalarDeclaration.new",
            ));
        }
        let (owner, decl) = self.declref(decl)?;
        let mm = &self.pool[owner].mm;
        let index = v.get("index").and_then(Value::as_u64).unwrap_or(u64::MAX);
        let name = v.get("name").and_then(Value::as_str).unwrap_or_default();
        let not_found =
            || Fault::Divergence(format!("state divergence: property {name} not found"));
        let id = mm
            .property_ids(decl)
            .nth(usize::try_from(index).unwrap_or(usize::MAX))
            .ok_or_else(not_found)?;
        match mm.property(id) {
            Some(p) if p.name() == name => Ok((owner, id)),
            _ => Err(not_found()),
        }
    }

    /// A `validatorref`: which validator (`part`) of which property
    /// (`owner`, a `propref`). A `declref`-owned `validatorref` (a scalar
    /// declaration's own validator) has no fixture in the corpus today and
    /// is reported the same way any other unhandled `@@oracle` kind is.
    fn validatorref(&mut self, v: &Value) -> Faulty<(usize, PropId, String)> {
        let part = v
            .get("part")
            .and_then(Value::as_str)
            .ok_or_else(|| Fault::Harness("validatorref without part".into()))?
            .to_string();
        let owner = v
            .get("owner")
            .ok_or_else(|| Fault::Harness("validatorref without owner".into()))?;
        match owner.get(M).and_then(Value::as_str) {
            Some("propref") => {
                let (mm, id) = self.propref(owner)?;
                Ok((mm, id, part))
            }
            _ => Err(blocked(
                "a validator whose owner is not a property has no Rust handle yet",
                "Validator.new",
            )),
        }
    }
    /// A `decoref`: `{parent, index}`, `parent` being a `declref`, `propref`
    /// or `mfref` (P2-07).
    fn decoref(&mut self, v: &Value) -> Faulty<(usize, DecoParent, usize)> {
        let parent = v
            .get("parent")
            .ok_or_else(|| Fault::Harness("decoref without parent".into()))?;
        let index = usize::try_from(v.get("index").and_then(Value::as_u64).unwrap_or(u64::MAX))
            .unwrap_or(usize::MAX);
        let (owner, target) = self.decorated_target(parent)?;
        Ok((owner, target, index))
    }

    /// An element that can carry decorators, targeted directly (the receiver
    /// of `Decorated.getDecorator`/`getDecorators`) or as a `decoref`'s
    /// `parent` (P2-07): a `declref`, `propref` or `mfref`.
    pub fn decorated_target(&mut self, v: &Value) -> Faulty<(usize, DecoParent)> {
        match v.get(M).and_then(Value::as_str) {
            Some("declref") => {
                let (owner, id) = self.declref(v)?;
                Ok((owner, DecoParent::Decl(id)))
            }
            Some("propref") => {
                let (owner, id) = self.propref(v)?;
                Ok((owner, DecoParent::Prop(id)))
            }
            Some("mfref") => {
                let mm_node = v
                    .get("mm")
                    .ok_or_else(|| Fault::Harness("mfref without mm".into()))?;
                let owner = self.mm_index(mm_node)?;
                let ns = v
                    .get("ns")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                Ok((owner, DecoParent::File(ns)))
            }
            _ => Err(Fault::Unsupported(
                "a decorated element this harness cannot rebuild (mfnew/declnew)".into(),
            )),
        }
    }
}

impl DecoParent {
    /// The decorators this names, read from a replayed model manager.
    pub fn decorators<'a>(&self, r: &'a Replayed) -> Option<&'a [concerto_core::Decorator]> {
        use concerto_core::Decorated;
        match self {
            Self::Decl(id) => r.mm.declaration(*id).map(Decorated::get_decorators),
            Self::Prop(id) => r.mm.property(*id).map(Decorated::get_decorators),
            Self::File(ns) => r.mm.model_file(ns).map(Decorated::get_decorators),
        }
    }
}

/// Whether plain data holds an encoded value somewhere inside it (a
/// `ModelFile` inside an array, say), which this harness would otherwise
/// pass through as plain JSON.
fn contains_marker(v: &Value) -> bool {
    match v {
        Value::Array(items) => items
            .iter()
            .any(|x| x.get(M).is_some() || contains_marker(x)),
        Value::Object(map) => map
            .values()
            .any(|x| x.get(M).is_some() || contains_marker(x)),
        _ => false,
    }
}

/// Decodes an oracle `"typed"` node's `fields` object into a
/// [`DecodedInstance`] ([`Session::typed`] resolves its `mm` handle first;
/// this part needs none, see that method's doc).
fn decode_typed_instance(v: &Value) -> Faulty<DecodedInstance> {
    let ctor = v
        .get("ctor")
        .and_then(Value::as_str)
        .ok_or_else(|| Fault::Harness("typed value without ctor".into()))?
        .to_string();
    let fields = v
        .get("fields")
        .and_then(Value::as_object)
        .ok_or_else(|| Fault::Harness("typed value without a fields object".into()))?;
    let namespace = fields
        .get("$namespace")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let type_name = fields
        .get("$type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let fqn = model_util::get_fully_qualified_name(&namespace, &type_name);
    let identifier_field_name = fields
        .get("$identifierFieldName")
        .and_then(Value::as_str)
        .map(str::to_string);
    // TS `getIdentifier()`: `this[this.$identifierFieldName]`, not always
    // `this.$identifier` (bug fix: a shadowed identifying field, TS's own
    // wording in `identifiable.ts`, can genuinely diverge from `$identifier`
    // — for example after `setIdentifier` sets both at construction but a
    // later direct field write changes only the named one).
    let identifier = fields
        .get(identifier_field_name.as_deref().unwrap_or("$identifier"))
        .and_then(Value::as_str)
        .map(str::to_string);
    // `this.$timestamp`: recorded only when the key exists on the instance.
    let timestamp = fields.get("$timestamp").cloned().unwrap_or_else(undefined);

    if ctor == "Relationship" {
        // `RELATIONSHIP_TAG`'s doc (`instance/validate.rs`): the wire form
        // this validator expects for an already-populated `Relationship` is
        // `{RELATIONSHIP_TAG: true, "$class": <pointed-at fqn>}`, plus its
        // identifying field under its *own* name (not a URI string), so
        // that `instance/validate.rs`'s `identifiable_parts` — which looks
        // an `Identifiable` value's id up by `ModelManager::identifier_field_name`,
        // the same way for a `Relationship` or a nested `Resource` alike —
        // finds it.
        let mut wire = serde_json::Map::new();
        wire.insert(RELATIONSHIP_TAG.to_string(), Value::Bool(true));
        wire.insert("$class".to_string(), Value::String(fqn.clone()));
        if let (Some(id_field), Some(id)) = (&identifier_field_name, &identifier) {
            wire.insert(id_field.clone(), Value::String(id.clone()));
        }
        return Ok(DecodedInstance {
            ctor,
            namespace,
            type_name,
            fqn,
            identifier_field_name,
            identifier,
            timestamp,
            class_declaration: None,
            wire: Value::Object(wire),
        });
    }

    // `Resource`/`ValidatedResource`: TS `JSONGenerator.visitClassDeclaration`
    // writes `$class` plus each of `classDeclaration.getProperties()`'s own
    // values (module doc on `DecodedInstance::wire`). `getProperties()`
    // includes a synthetic `$identifier`/`$timestamp` entry for a
    // system-identified/transaction-or-event type (plan §1.2's "implicit
    // `Concept` super type and the `$identifier`/`$timestamp` fields" gap;
    // `instance/validate.rs`'s `get_all_properties` already resolves these),
    // so `$identifier` and `$timestamp` are kept here too, alongside every
    // ordinary (non-`$`) field — every *other* `$`-prefixed key is TS's
    // `isPrivateSystemProperty` list (`modelutil.ts`), never a real Concerto
    // property (a declared name never starts with `$` in this corpus), so
    // skipping them is exact, not an approximation.
    const PRIVATE_ONLY_KEYS: [&str; 9] = [
        "$modelManager",
        "$classDeclaration",
        "$namespace",
        "$type",
        "$identifierFieldName",
        "$validator",
        "$imports",
        "$superTypes",
        "$id",
    ];
    let mut wire = serde_json::Map::new();
    wire.insert("$class".to_string(), Value::String(fqn.clone()));
    for (key, value) in fields {
        if PRIVATE_ONLY_KEYS.contains(&key.as_str()) {
            continue;
        }
        wire.insert(key.clone(), typed_field_value(value)?);
    }
    Ok(DecodedInstance {
        ctor,
        namespace,
        type_name,
        fqn,
        identifier_field_name,
        identifier,
        timestamp,
        class_declaration: None,
        wire: Value::Object(wire),
    })
}

/// Decodes one field value found inside an oracle `"typed"` node's `fields`
/// object (recursively: an array, a nested `"typed"` instance, a `"dayjs"`
/// timestamp, a `"map"` (a JS `Map`-backed `MapDeclaration` value) or a
/// `"number"` (`NaN`/`Infinity`/`-Infinity`/`-0`) — every other plain JSON
/// value needs no decoding.
fn typed_field_value(v: &Value) -> Faulty<Value> {
    if let Value::Array(items) = v {
        return items
            .iter()
            .map(typed_field_value)
            .collect::<Faulty<Vec<_>>>()
            .map(Value::Array);
    }
    let Value::Object(map) = v else {
        return Ok(v.clone());
    };
    let Some(Value::String(kind)) = map.get(M) else {
        return Ok(v.clone());
    };
    match kind.as_str() {
        // `DAYJS_TAG`'s doc (`instance/validate.rs`): marks a value that
        // really did go through `JSONPopulator`'s `DateTime` coercion in TS,
        // as every `"dayjs"`-encoded field here did (it is how the TS
        // recorder itself found this value, README "Value encoding":
        // `dayjs | {iso, offset, utc, valid}`) — the ISO string itself is
        // kept only for readability in a failing assertion, not read by the
        // validator, which only checks the tag's presence.
        "dayjs" => {
            let iso = map.get("iso").and_then(Value::as_str).unwrap_or_default();
            Ok(json!({ DAYJS_TAG: iso }))
        }
        "typed" => Ok(decode_typed_instance(v)?.wire),
        // A JS `undefined`, kept distinct from `null` (`UNDEFINED_TAG`'s doc,
        // `instance/validate.rs`): `["a", undefined, "b"]` is reported by
        // TS as a value `undefined` of type `undefined`, not `null`/`object`.
        "undefined" => Ok(js_undefined()),
        // A JS `Map` (a `MapDeclaration` value): `{"@@oracle":"map",
        // "entries": [[key, value], ...]}` -> the plain object
        // `visitMapDeclaration`'s `Object.fromEntries(map)` would produce.
        "map" => {
            let entries = map
                .get("entries")
                .and_then(Value::as_array)
                .ok_or_else(|| Fault::Harness("map value without entries".into()))?;
            let mut obj = serde_json::Map::new();
            for entry in entries {
                let pair = entry
                    .as_array()
                    .ok_or_else(|| Fault::Harness("a map entry that is not [key, value]".into()))?;
                let key = pair
                    .first()
                    .and_then(Value::as_str)
                    .ok_or_else(|| Fault::Harness("a map entry with a non-string key".into()))?;
                let value = typed_field_value(pair.get(1).unwrap_or(&Value::Null))?;
                obj.insert(key.to_string(), value);
            }
            Ok(Value::Object(obj))
        }
        // `{"@@oracle":"number","value":"NaN"|"Infinity"|"-Infinity"|"-0"}`:
        // a JSON number cannot hold any of these; encoded here as a JSON
        // string, which is not itself a valid Concerto primitive value for
        // any field type, so a validator check reaching it correctly
        // reports a field type violation (never a silent pass) the same way
        // TS's own `!isFinite(NaN)` does for the numeric kinds.
        "number" => Ok(map.get("value").cloned().unwrap_or(Value::Null)),
        other => Err(Fault::Unsupported(format!(
            "a typed field value of kind {other} is not decoded"
        ))),
    }
}

/// Model manager options that concerto-core 5.0.0 reads on the paths this
/// harness replays and the Rust engine does not model yet: metamodel
/// validation (`basemodelmanager.ts` `addModelFile`), adding the metamodel
/// (constructor), decorator validation (`Decorated.validate`), reserved
/// system type names (`declaration.ts`) and a custom `RegExp`
/// (`stringvalidator.ts`).
const UNMODELLED_OPTIONS: [&str; 5] = [
    "metamodelValidation",
    "addMetamodel",
    "decoratorValidation",
    "dangerouslyAllowReservedSystemTypeNamesInUserModels",
    "regExp",
];

/// Options concerto-core 5.0.0 never reads on these paths: `strict`,
/// `enableMapType` and `importAliasing` are v3/v4 flags no 5.0.0 source file
/// reads, `utcOffset` only reaches the `Serializer` the constructor builds,
/// which no dispatched op uses, and `offline` is a `ModelLoader` option
/// (`modelloader.ts`) that the model manager itself never reads. Replaying
/// without them is exact.
const INERT_OPTIONS: [&str; 5] = [
    "strict",
    "enableMapType",
    "importAliasing",
    "utcOffset",
    "offline",
];

/// The TS member that reads an unmodelled option, whose owner ports it.
fn option_reader(key: &str) -> Option<&'static str> {
    Some(match key {
        // `addModelFile` -> `validateAst` (src/basemodelmanager.ts).
        "metamodelValidation" => "BaseModelManager.validateAst",
        // The constructor adds the metamodel file.
        "addMetamodel" => "BaseModelManager.new",
        // `Decorator.validate` reads `mm.getDecoratorValidation()`.
        "decoratorValidation" => "Decorator.validate",
        // `Declaration.validate` (src/introspect/declaration.ts).
        "dangerouslyAllowReservedSystemTypeNamesInUserModels" => "Declaration.validate",
        // `StringValidator`'s constructor builds the custom RegExp.
        "regExp" => "StringValidator.new",
        _ => return None,
    })
}

/// Checks a recipe's options, returning its `skipLocationNodes` (which only
/// selects the cache entry, i.e. the AST's shape). An unmodelled option with
/// a truthy value, or an option this harness does not know, is unsupported.
fn check_options(options: &Value) -> Faulty<Value> {
    if is_undefined(options) || options.is_null() {
        return Ok(Value::Null);
    }
    let Some(map) = options.as_object() else {
        return Err(Fault::Unsupported(
            "model manager options that are not an object".into(),
        ));
    };
    for (key, value) in map {
        let inert = key == "skipLocationNodes" || INERT_OPTIONS.contains(&key.as_str());
        if !inert && (truthy(value) || !UNMODELLED_OPTIONS.contains(&key.as_str())) {
            let reason =
                format!("ModelManager option `{key}` is not modelled by the Rust engine yet");
            return Err(match option_reader(key) {
                Some(member) => blocked(reason, member),
                None => Fault::Blocked(reason, Blocker::Owner(UNOWNED.into())),
            });
        }
    }
    Ok(match map.get("skipLocationNodes") {
        None => Value::Null,
        Some(v) if is_undefined(v) => Value::Null,
        Some(v) => v.clone(),
    })
}

impl Replayed {
    /// `new ModelManager(options)`.
    pub fn new(kind: Kind, options: &Value) -> Faulty<Self> {
        let skip_location_nodes = check_options(options)?;
        let mm = ModelManager::new()
            .map_err(|e| divergence_from(&to_oracle_error(&e), "ModelManager::new"))?;
        Ok(Self {
            kind,
            skip_location_nodes,
            files: Vec::new(),
            mm,
        })
    }

    /// A model manager another op returned (a `derived` recipe), with its
    /// user model files as they were loaded: without a file name (`fromAst`
    /// passes none), and known to validate when it was validated.
    pub fn from_derived(kind: Kind, derived: super::ops::DerivedModelManager) -> Self {
        let files = derived
            .mm
            .model_files()
            .filter(|mf| !EXCLUDE_NS.contains(&mf.namespace()))
            .map(|mf| Entry {
                ast: mf.ast().clone(),
                file_name: mf.file_name().map(str::to_string),
                nullish_name: undefined(),
                known_valid: derived.validated,
            })
            .collect();
        Self {
            kind,
            skip_location_nodes: Value::Null,
            files,
            mm: derived.mm,
        }
    }

    /// The Rust manager rebuilt from `files`, all of which loaded before.
    fn rebuild(&mut self) -> Faulty<()> {
        let mut mm = ModelManager::new()
            .map_err(|e| divergence_from(&to_oracle_error(&e), "ModelManager::new"))?;
        for entry in &self.files {
            mm.add_model(&entry.ast, entry.file_name.clone())
                .map_err(|e| {
                    Fault::Harness(format!(
                        "a model file that loaded before failed to reload: {}",
                        to_oracle_error(&e).message
                    ))
                })?;
        }
        self.mm = mm;
        Ok(())
    }

    fn file_arg(&self, ns: &str) -> Option<FileArg> {
        let mf = self.mm.model_file(ns)?;
        Some(FileArg {
            ast: mf.ast().clone(),
            file_name: mf.file_name().map(str::to_string),
            nullish_name: self.nullish_name(ns),
        })
    }

    fn nullish_name(&self, ns: &str) -> Value {
        self.files
            .iter()
            .find(|e| e.ast.get("namespace").and_then(Value::as_str) == Some(ns))
            .map_or_else(undefined, |e| e.nullish_name.clone())
    }

    /// Every namespace, in registration order (TS `getNamespaces()`,
    /// `Object.keys(this.modelFiles)`).
    pub fn namespaces(&self) -> Vec<String> {
        self.mm
            .model_files()
            .map(|mf| mf.namespace().to_string())
            .collect()
    }

    /// The outcome-only `ModelManager` summary (`makeOutputEncoder`):
    /// `{ctor, namespaces, ast: getAst(false, true)}`.
    pub fn summary(&self) -> Value {
        summary_of(self.kind, &self.mm)
    }

    /// TS `getAst(false, includeConcertoNamespaces)`.
    pub fn ast(&self, include_concerto_namespaces: bool) -> Value {
        ast_of(&self.mm, include_concerto_namespaces)
    }

    /// The outcome-only `ModelFile` summary: `{namespace, name, ast}`.
    pub fn model_file_summary(&self, ns: &str) -> Option<Value> {
        let mf = self.mm.model_file(ns)?;
        Some(json!({
            M: "ModelFile",
            "namespace": mf.namespace(),
            "name": mf.file_name().map_or_else(|| self.nullish_name(ns), |n| json!(n)),
            "ast": mf.ast(),
        }))
    }

    /// The outcome-only `Declaration` summary: `{ctor, fqn}`.
    pub fn declaration_summary(&self, id: DeclId) -> Option<Value> {
        let declaration = self.mm.declaration(id)?;
        let file = self.mm.file(self.mm.model_file_of(id)?)?;
        let ctor = match declaration {
            Declaration::Class(class) => class.declaration_kind(),
            Declaration::Enum(_) => "EnumDeclaration",
            Declaration::Scalar(_) => "ScalarDeclaration",
            Declaration::Map(_) => "MapDeclaration",
        };
        Some(json!({
            M: "Declaration",
            "ctor": ctor,
            "fqn": format!("{}.{}", file.namespace(), declaration.name()),
        }))
    }

    /// The outcome-only `MapKeyType`/`MapValueType` summary
    /// (`makeOutputEncoder`'s generic `Property` shape, confirmed against a
    /// recorded `MapDeclaration.getKey`/`getValue` fixture): `{ctor, type}`,
    /// `type` being `MapKeyType.getType`/`MapValueType.getType`'s result.
    pub fn map_part_summary(&self, id: DeclId, is_key: bool) -> Option<Value> {
        let Declaration::Map(map) = self.mm.declaration(id)? else {
            return None;
        };
        Some(json!({
            M: "Property",
            "ctor": if is_key { "MapKeyType" } else { "MapValueType" },
            "type": if is_key { map.key_type_name() } else { map.value_type_name() },
        }))
    }

    pub fn file_id(&self, ns: &str) -> Option<ModelFileId> {
        self.mm.model_file_id(ns)
    }

    fn cto_ast(&self, h: &Harness, cto: &str, file_name: Option<&str>) -> Faulty<Outcome> {
        let cache = h
            .cache
            .as_ref()
            .ok_or_else(|| Fault::Harness("no CTO -> AST cache (build-cto-cache.js)".into()))?;
        match cache
            .lookup(cto, file_name, &self.skip_location_nodes)
            .map_err(Fault::Harness)?
        {
            CacheEntry::Ast(ast) => Ok(Ok(ast)),
            CacheEntry::Error(error) => Ok(Err(
                OracleError::from_cached_error(&error).map_err(Fault::Harness)?
            )),
        }
    }

    /// `processFile(fileName, modelInput)`: the CTO parser for a
    /// `ModelManager`, the identity for the other two kinds.
    fn process_file(&self, h: &Harness, input: &Value, file_name: &Value) -> Faulty<Outcome> {
        match self.kind {
            Kind::ModelManager => {
                let Value::String(cto) = input else {
                    // TS parses `String(input)`; build-cto-cache.js collects
                    // string arguments only, so the cache has no entry for it.
                    return Err(Fault::Blocked(
                        "a ModelManager given a non-string model input, which TS parses as \
                         String(input): the P1-07a CTO cache collects string inputs only"
                            .into(),
                        Blocker::Owner("P1-07a".into()),
                    ));
                };
                self.cto_ast(h, cto, file_name.as_str())
            }
            Kind::BaseModelManager | Kind::AstModelManager => Ok(Ok(input.clone())),
        }
    }

    /// TS `addModelFile(modelFile, cto, fileName, disableValidation)` for a
    /// model file built from `ast` (see the module doc for validation).
    fn add_file(&mut self, file: FileArg, validate: bool) -> Faulty<Outcome> {
        let ns = file
            .ast
            .get("namespace")
            .and_then(Value::as_str)
            .map(str::to_string);
        if let Err(e) = self.mm.add_model(&file.ast, file.file_name.clone()) {
            return Ok(Err(to_oracle_error(&e)));
        }
        let before_valid = self.files.iter().all(|e| e.known_valid);
        self.files.push(Entry {
            ast: file.ast,
            file_name: file.file_name,
            nullish_name: file.nullish_name,
            known_valid: false,
        });
        if validate {
            if !before_valid {
                return Err(blocked(
                    "validating one added model file needs ModelFile.validate, not ported yet, \
                     and an earlier file was added without validation",
                    "ModelFile.validate",
                ));
            }
            if let Err(e) = self.mm.validate_models() {
                self.files.pop();
                self.rebuild()?;
                return Ok(Err(to_oracle_error(&e)));
            }
            self.mark_valid();
        }
        let ns = ns.unwrap_or_default();
        Ok(Ok(self.model_file_summary(&ns).unwrap_or_else(undefined)))
    }

    fn mark_valid(&mut self) {
        for entry in &mut self.files {
            entry.known_valid = true;
        }
    }

    /// Runs one state-changing call (a recipe step, or a fixture whose op is
    /// that call) on this model manager. `Ok(Err(_))` is the engine's error
    /// outcome; `Err(_)` means the call could not be replayed.
    pub fn apply(&mut self, h: &Harness, method: &str, args: &[Arg]) -> Faulty<Outcome> {
        let plain = |i: usize| -> Faulty<Value> {
            match args.get(i) {
                Some(Arg::Plain(v)) => Ok(v.clone()),
                None => Ok(undefined()),
                Some(_) => Err(Fault::Unsupported(format!(
                    "ModelManager.{method} with a handle where TS takes plain data (argument {i})"
                ))),
            }
        };
        let no_validation = |i: usize| plain(i).map(|v| truthy(&v));
        match method {
            "addCTOModel" | "addModel" => {
                if method == "addCTOModel" && self.kind != Kind::ModelManager {
                    return Err(Fault::Unsupported(format!(
                        "addCTOModel on a {}",
                        self.kind.ctor()
                    )));
                }
                // addCTOModel(cto, fileName?, disableValidation?) is
                // addModel(cto, cto, fileName, disableValidation).
                let (name_at, flag_at) = if method == "addCTOModel" {
                    (1, 2)
                } else {
                    (2, 3)
                };
                let input = plain(0)?;
                let file_name_value = plain(name_at)?;
                let (file_name, nullish_name) = nullish_or_string(&file_name_value)?;
                let validate = !no_validation(flag_at)?;
                let ast = match self.process_file(h, &input, &file_name_value)? {
                    Ok(ast) => ast,
                    Err(parse_error) => return Ok(Err(parse_error)),
                };
                // `new ModelFile(...)` then `addModelFile(...)`: `add_model`
                // builds the model file first, so its errors come first.
                self.add_file(
                    FileArg {
                        ast,
                        file_name,
                        nullish_name,
                    },
                    validate,
                )
            }
            "addModelFile" => {
                let Some(Arg::File(file)) = args.first() else {
                    return Err(Fault::Unsupported(
                        "addModelFile with an argument that is not a model file".into(),
                    ));
                };
                let validate = !no_validation(3)?;
                self.add_file(file.clone(), validate)
            }
            "addModelFiles" => {
                let validate = !no_validation(2)?;
                self.add_model_files(h, args, validate)
            }
            "validateModelFiles" => match self.mm.validate_models() {
                Ok(()) => {
                    self.mark_valid();
                    Ok(Ok(undefined()))
                }
                Err(e) => Ok(Err(to_oracle_error(&e))),
            },
            "clearModelFiles" => {
                self.files.clear();
                self.rebuild()?;
                Ok(Ok(undefined()))
            }
            "fromAst" => {
                let ast = plain(0)?;
                let options = plain(1)?;
                let Some(models) = ast.get("models").and_then(Value::as_array) else {
                    return Err(Fault::Unsupported(
                        "fromAst with no `models` array (a JS TypeError in TS)".into(),
                    ));
                };
                self.files.clear();
                self.rebuild()?;
                for model in models {
                    let ns = model.get("namespace").and_then(Value::as_str);
                    if ns.is_some_and(|ns| EXCLUDE_NS.contains(&ns)) {
                        continue;
                    }
                    let file = FileArg {
                        ast: model.clone(),
                        file_name: None,
                        nullish_name: undefined(),
                    };
                    // `new ModelFile(this, model)`, then `addModelFile(…,
                    // true)`; TS keeps whatever loaded before an error.
                    if let Err(e) = self.add_file(file, false)? {
                        return Ok(Err(e));
                    }
                }
                let disable = options.get("disableValidation").is_some_and(truthy);
                if !disable {
                    if let Err(e) = self.mm.validate_models() {
                        return Ok(Err(to_oracle_error(&e)));
                    }
                    self.mark_valid();
                }
                Ok(Ok(undefined()))
            }
            other => Err(blocked(
                format!("ModelManager.{other} has no Rust counterpart yet"),
                format!("ModelManager.{other}"),
            )),
        }
    }

    /// TS `addModelFiles(modelFiles, fileNames?, disableValidation?)`.
    fn add_model_files(&mut self, h: &Harness, args: &[Arg], validate: bool) -> Faulty<Outcome> {
        let items: Vec<Arg> = match args.first() {
            Some(Arg::Plain(Value::Array(items))) => {
                items.iter().cloned().map(Arg::Plain).collect()
            }
            Some(Arg::List(items)) => items.clone(),
            _ => {
                return Err(Fault::Unsupported(
                    "addModelFiles with an argument that is not a list".into(),
                ));
            }
        };
        let file_names = match args.get(1) {
            Some(Arg::Plain(Value::Array(names))) => Some(names.clone()),
            _ => None,
        };
        let snapshot = self.files.clone();
        let mut added = Vec::new();
        let restore = |r: &mut Self, e: OracleError| -> Faulty<Outcome> {
            r.files = snapshot.clone();
            r.rebuild()?;
            Ok(Err(e))
        };
        for (n, item) in items.iter().enumerate() {
            let file_name_value = match &file_names {
                Some(names) => names.get(n).cloned().unwrap_or_else(undefined),
                None => Value::Null,
            };
            let (file_name, nullish_name, ast) = match item {
                // A string is parsed; an already-built ModelFile keeps its
                // own name (TS ignores `fileNames[n]` for it).
                Arg::Plain(input) => {
                    if !input.is_string() && self.kind == Kind::ModelManager {
                        return Err(Fault::Unsupported(
                            "addModelFiles with a plain object where TS expects CTO text or a ModelFile"
                                .into(),
                        ));
                    }
                    let (file_name, nullish_name) = nullish_or_string(&file_name_value)?;
                    match self.process_file(h, input, &file_name_value)? {
                        Ok(ast) => (file_name, nullish_name, ast),
                        Err(e) => return restore(self, e),
                    }
                }
                Arg::File(file) => (
                    file.file_name.clone(),
                    file.nullish_name.clone(),
                    file.ast.clone(),
                ),
                _ => {
                    return Err(Fault::Unsupported(
                        "addModelFiles with an element that is not CTO text or a ModelFile".into(),
                    ));
                }
            };
            let ns = ast
                .get("namespace")
                .and_then(Value::as_str)
                .map(str::to_string);
            if let Err(e) = self.mm.add_model(&ast, file_name.clone()) {
                return restore(self, to_oracle_error(&e));
            }
            self.files.push(Entry {
                ast,
                file_name,
                nullish_name,
                known_valid: false,
            });
            added.push(ns.unwrap_or_default());
        }
        if validate {
            if let Err(e) = self.mm.validate_models() {
                return restore(self, to_oracle_error(&e));
            }
            self.mark_valid();
        }
        Ok(Ok(Value::Array(
            added
                .iter()
                .map(|ns| self.model_file_summary(ns).unwrap_or_else(undefined))
                .collect(),
        )))
    }
}

impl Clone for Arg {
    fn clone(&self) -> Self {
        match self {
            Self::Plain(v) => Self::Plain(v.clone()),
            Self::File(f) => Self::File(f.clone()),
            Self::Mm(i) => Self::Mm(*i),
            Self::SelfMm => Self::SelfMm,
            Self::Decl(m, d) => Self::Decl(*m, *d),
            Self::Prop(m, p) => Self::Prop(*m, *p),
            Self::MapPart(m, d, is_key) => Self::MapPart(*m, *d, *is_key),
            Self::DeclNew { fqn, processed } => Self::DeclNew {
                fqn: fqn.clone(),
                processed: processed.clone(),
            },
            Self::Validator(m, p, part) => Self::Validator(*m, *p, part.clone()),
            Self::Deco(m, parent, i) => Self::Deco(*m, parent.clone(), *i),
            Self::List(items) => Self::List(items.clone()),
            Self::Typed(m, inst) => Self::Typed(*m, inst.clone()),
        }
    }
}

/// Whether `ns` is one of TS `EXCLUDE_NS`, the system namespaces `fromAst`
/// and `getModelFiles()` leave out.
pub fn is_system_namespace(ns: &str) -> bool {
    EXCLUDE_NS.contains(&ns)
}

/// The outcome-only `ModelManager` summary (`makeOutputEncoder`) of any
/// Rust model manager, as constructed by `kind`'s TS class:
/// `{ctor, namespaces, ast: getAst(false, true)}`.
pub fn summary_of(kind: Kind, mm: &ModelManager) -> Value {
    let namespaces: Vec<&str> = mm.model_files().map(ModelFile::namespace).collect();
    json!({
        M: "ModelManager",
        "ctor": kind.ctor(),
        "namespaces": namespaces,
        "ast": ast_of(mm, true),
    })
}

/// TS `getAst(false, includeConcertoNamespaces)` of any Rust model manager.
pub fn ast_of(mm: &ModelManager, include_concerto_namespaces: bool) -> Value {
    let models: Vec<Value> = mm
        .model_files()
        .filter(|mf| include_concerto_namespaces || !EXCLUDE_NS.contains(&mf.namespace()))
        .map(|mf| mf.ast().clone())
        .collect();
    json!({ "$class": "concerto.metamodel@1.0.0.Models", "models": models })
}

/// The handle of a model file registered in `r`, as a [`Node`].
pub fn model_file_node(r: &Replayed, file: &FileArg) -> Faulty<Node> {
    let ns = file
        .ast
        .get("namespace")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match r.file_id(ns) {
        Some(id) if r.mm.file(id).map(ModelFile::ast) == Some(&file.ast) => Ok(Node::ModelFile(id)),
        _ => Err(Fault::Unsupported(
            "a model file that is not the one registered in the model manager has no Rust handle"
                .into(),
        )),
    }
}
