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
//! | `addCTOModel`, `addModel` (CTO text or AST), `addModelFile` | `add_model`, then, unless validation is disabled, `validate_model_file` on the new file alone (below) |
//! | `addModelFiles` | `add_model` per file, then `validate_models` unless validation is disabled; any error restores the files that were there before, as TS does |
//! | `validateModelFiles` | `validate_models` |
//! | `clearModelFiles` | a fresh `ModelManager::new()` (TS: `modelFiles = {}`, then the decorator and root models again) |
//! | `fromAst` | `clearModelFiles`, `add_model` per non-system model, then `validate_models` unless disabled |
//! | `updateModelFile` | [`ModelManager::update_model_file`] (P2-08b) |
//! | `deleteModelFile` | [`ModelManager::delete_model_file`] (P2-08b) |
//! | `updateExternalModels` (an op only) | [`ModelManager::update_external_models`] over the recorded download (P2-08b, [`Replayed::update_external_models`]) |
//! | `addDecoratorFactory` | `unsupported`: the Rust engine has no counterpart yet |
//!
//! **Validation on add.** TS `addModelFile` validates *only the new file*
//! (`modelFile.validate()`) *before* registering it, so a file added earlier
//! with validation disabled is never re-checked (P2-08: a later
//! `validateModelFiles` is what rejects it), and a self-import cannot yet
//! resolve through the manager (P2-08d, accordproject/concerto-rust#151).
//! The harness replays this as "validate the new file with
//! [`ModelManager::validate_detached_model_file`] against the manager as it
//! stands (before registering), then register" — matching TS's own order,
//! including its duplicate-namespace check firing first regardless of
//! validation (`add_model_with_definitions`'s own check, unconditional).
//! Validation failing leaves the manager untouched, since nothing is
//! registered yet; removal has no Rust counterpart, so a later step that
//! must undo a registered file still rebuilds the manager from the
//! surviving files (all of which loaded before).
//!
//! **Options.** `skipLocationNodes` (it selects the cache entry),
//! `dangerouslyAllowReservedSystemTypeNamesInUserModels` (P2-08:
//! `ModelManager::set_dangerously_allow_reserved_system_type_names_in_user_models`,
//! read by `Declaration.validate`) and `decoratorValidation` (P2-08b:
//! `ModelManager::set_decorator_validation`, read by `Decorator.validate`;
//! P2-09b gave the harness its own recognised key for it, having previously
//! rejected it outright) are replayed. Any other option with a truthy value
//! changes TS behaviour the Rust engine does not model yet
//! (`metamodelValidation`, `addMetamodel`, `regExp`), so such a recipe is
//! `unsupported`.
//!
//! # Model files, declarations, properties
//!
//! `mfref` is the Rust model file registered under `ns`; `mfnew` is built
//! with `ModelFile::from_json` (TS `new ModelFile(mm, ast, definitions,
//! fileName)`), a failure being "input construction failed" (a failure).
//! `declref` and `propref` become [`Node`] handles by position, checked by
//! name as `codec.js` checks them; a `declref` into an `mfnew` (other than
//! a map, P2-06b's `Arg::DeclDetached`) is a handle into a copy of its
//! manager with that file registered in place of its namespace's, unvalidated
//! (P2-08b, `Session::detached_owner`). `decoref` (P2-07) becomes a
//! [`DecoParent`] plus its position, resolved against its parent's processed
//! decorators at dispatch time (`ops.rs`); its `parent` must itself be a
//! `declref`, `propref` or `mfref`. A map key/value `propref` (one with a
//! `part`) becomes an [`Arg::MapPart`] (P2-06). A `declnew` (a declaration built
//! directly via `new Cls(modelFile, ast)`, never added to `modelFile`) is
//! rebuilt with `ScalarDeclaration::build_standalone` when `cls` is
//! `ScalarDeclaration` (P2-05); any other `cls` is `unsupported`, for its own
//! owner. `validatorref`, `factory`, `serializer` and `introspector` have no
//! Rust counterpart yet: `unsupported`. `predicate` decodes into
//! [`Arg::Predicate`] for its one corpus `kind`, `"fqn-in"` (P2-08b,
//! `BaseModelManager.filter`); any other `kind` is `unsupported`.
//! `decoratorfactory` has no Rust counterpart yet: `unsupported`. `typed`
//! (P3-01 review, task
//! `accordproject-concerto-rust#56` follow-up) is decoded directly into
//! [`DecodedInstance`] by [`Session::typed`], from the node's own `fields`
//! object rather than a `Factory`/`JSONPopulator` replay — see
//! [`Arg::Typed`]'s doc.

use std::collections::HashMap;

use concerto_core::instance::validate::{
    DAYJS_TAG, RELATIONSHIP_TAG, js_map, js_special_number, js_undefined,
};
use concerto_core::introspect::scalar::ProcessedScalar;
use concerto_core::introspect::{
    Declaration, DeclarationKind, DecoratorValidationOptions, ModelFile, Named, ScalarDeclaration,
};
use concerto_core::model_manager::{
    DeclId, ModelFileId, ModelFileSource, ModelManager, Node, PropId,
};
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
/// `definitions` (P2-08b) is the CTO source text, when this entry's own
/// load kept one (`ModelManager::add_model_with_definitions`'s doc); a
/// rebuild threads it back through so it survives a rollback.
#[derive(Debug, Clone)]
struct Entry {
    ast: Value,
    file_name: Option<String>,
    nullish_name: Value,
    definitions: Option<String>,
}

/// A replayed model manager.
pub struct Replayed {
    pub kind: Kind,
    /// The constructor's options, as recorded (`BaseModelManager` hands
    /// them to its `Serializer`, which `Resource.toJSON` uses).
    pub options: Value,
    skip_location_nodes: Value,
    /// TS `options.dangerouslyAllowReservedSystemTypeNamesInUserModels`
    /// (JS truthiness), set on every manager this recipe builds or rebuilds.
    allow_reserved_system_type_names: bool,
    /// TS `options.decoratorValidation` (P2-09b), set on every manager this
    /// recipe builds or rebuilds.
    decorator_validation: DecoratorValidationOptions,
    files: Vec<Entry>,
    pub mm: ModelManager,
}

/// A model file argument, rebuilt from `mfref` or `mfnew`.
#[derive(Debug, Clone)]
pub struct FileArg {
    pub(crate) ast: Value,
    pub(crate) file_name: Option<String>,
    pub(crate) nullish_name: Value,
    /// TS `ModelFile.getDefinitions`: the CTO source text the recipe's own
    /// `definitions` field carries (`mfnew` only — a manager-registered
    /// `mfref` never has one, since `ModelManager.add_model` never threads
    /// one through, P2-08).
    pub(crate) definitions: Option<String>,
    /// The owning model manager's index in the session's pool (P2-08:
    /// `ModelFile.getModelManager`), when this file was decoded at the top
    /// level rather than mid model-manager-step replay (where the owner may
    /// still be under construction, with no pool index of its own yet).
    pub(crate) mm_index: Option<usize>,
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
    /// A `MapDeclaration` reached from a `ModelFile` built directly
    /// (`mfnew`) and never registered: `declref`'s `mf` is `mfnew` rather
    /// than `mfref`, so there is no arena `DeclId` for it (P2-06b, closing
    /// "ModelFile.new" for `MapDeclaration`/`MapKeyType`/`MapValueType`;
    /// every other declaration kind on an `mfnew` receiver gets an
    /// [`Arg::Decl`] into a copy of its manager instead, P2-08b,
    /// `Session::detached_owner`). Carries the built `ModelFile`,
    /// its position in [`concerto_core::introspect::ModelFile::declarations`],
    /// and the owning model manager's pool index (for the cross-file
    /// resolution `ModelManager::validate_detached_declaration` needs).
    DeclDetached {
        mm_index: Option<usize>,
        file: ModelFile,
        index: usize,
    },
    /// [`Arg::DeclDetached`]'s key (`is_key = true`) or value part, the same
    /// relationship [`Arg::MapPart`] has to [`Arg::Decl`].
    MapPartDetached {
        mm_index: Option<usize>,
        file: ModelFile,
        index: usize,
        is_key: bool,
    },
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
    /// A `BaseModelManager.filter` predicate (`"predicate"`, README "Value
    /// encoding"), decoded from the oracle's own `{kind: "fqn-in", names}`
    /// encoding into the fully-qualified names it keeps (P2-08b): every
    /// `predicate` fixture in the canonical corpus uses this one `kind`, so
    /// no other shape is decoded — see [`Session::decode`]'s `"predicate"`
    /// arm.
    Predicate(Vec<String>),
    /// A `Resource`, `ValidatedResource` or `Relationship` (`"typed"`,
    /// README "Value encoding"), decoded into [`DecodedInstance`]: the
    /// model manager it belongs to (pool index) plus the instance itself.
    /// P3-01 review (task `accordproject-concerto-rust#56` follow-up):
    /// `Resource.validate` and the `Identifiable`/`Typed`/`Relationship`
    /// accessors are dispatched from this.
    Typed(usize, DecodedInstance),
}

/// What a `declref` resolved to: a handle into an already-registered model
/// manager, or (P2-06b) a `MapDeclaration` read directly out of a `ModelFile`
/// that was built but never registered (`mfnew`).
#[allow(clippy::large_enum_variant)]
enum DeclTarget {
    Registered(usize, DeclId),
    Detached {
        mm_index: Option<usize>,
        file: ModelFile,
        index: usize,
    },
}

impl DeclTarget {
    fn into_arg(self) -> Arg {
        match self {
            Self::Registered(mm, id) => Arg::Decl(mm, id),
            Self::Detached {
                mm_index,
                file,
                index,
            } => Arg::DeclDetached {
                mm_index,
                file,
                index,
            },
        }
    }
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
            // TS `Introspector` (src/introspect/introspector.ts) is a thin
            // wrapper that stores its `ModelManager` and delegates every
            // member to it (P2-08): its handle is just that manager's own
            // pool index, the same `Arg::Mm` a `ModelManager` receiver is.
            "introspector" => {
                let mm_node = v
                    .get("mm")
                    .ok_or_else(|| Fault::Harness("introspector without mm".into()))?;
                self.mm_index(mm_node).map(Arg::Mm)
            }
            "mfref" | "mfnew" => self.file(v, self_mm).map(Arg::File),
            "declref" => self.declref(v).map(DeclTarget::into_arg),
            "propref" if v.get("part").and_then(Value::as_str).is_some() => self.map_part(v),
            "declnew" => self.declnew(v, self_mm),
            "predicate" => match v.get("kind").and_then(Value::as_str) {
                Some("fqn-in") => {
                    let names = v
                        .get("names")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect();
                    Ok(Arg::Predicate(names))
                }
                other => Err(Fault::Unsupported(format!(
                    "a predicate of kind {other:?}, which BaseModelManager.filter's oracle wiring does not decode"
                ))),
            },
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
            let mut file = r.file_arg(ns).ok_or_else(|| {
                Fault::Divergence(format!(
                    "state divergence: model file {ns} not registered after replay"
                ))
            })?;
            file.mm_index = owner;
            return Ok(file);
        }
        let ast = v.get("ast").cloned().unwrap_or(Value::Null);
        let (file_name, nullish_name) =
            nullish_or_string(v.get("fileName").unwrap_or(&undefined()))?;
        let definitions = match v.get("definitions") {
            None => None,
            Some(d) if d.is_null() || is_undefined(d) => None,
            Some(Value::String(s)) => Some(s.clone()),
            Some(_) => {
                return Err(Fault::Unsupported(
                    "a model file's definitions argument that is not a string".into(),
                ));
            }
        };
        // TS `new ModelFile(mm, ast, definitions, fileName)` runs while the
        // input is decoded, so a failure here is "input construction failed".
        ModelFile::from_json_with_definitions(&ast, definitions.clone(), file_name.clone())
            .map_err(|e| divergence_from(&to_oracle_error(&e), "new ModelFile"))?;
        Ok(FileArg {
            ast,
            file_name,
            nullish_name,
            definitions,
            mm_index: owner,
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
            && let Ok(DeclTarget::Registered(owner, id)) = self.declref(decl)
            && owner == mm_index
        {
            inst.class_declaration = Some(id);
        }
        Ok(Arg::Typed(mm_index, inst))
    }

    fn declref(&mut self, v: &Value) -> Faulty<DeclTarget> {
        let mf = v
            .get("mf")
            .ok_or_else(|| Fault::Harness("declref without mf".into()))?;
        let index = v.get("index").and_then(Value::as_u64).unwrap_or(u64::MAX);
        let name = v.get("name").and_then(Value::as_str).unwrap_or_default();
        let index = usize::try_from(index).unwrap_or(usize::MAX);

        let registered_as = if mf.get(M).and_then(Value::as_str) == Some("mfref") {
            None
        } else {
            // A `MapDeclaration` on an unregistered file (`mfnew`) is read
            // straight out of its `ModelFile` (P2-06b); every other kind
            // gets a handle in a copy of its manager (P2-08b,
            // `detached_owner`).
            let is_map = mf
                .get("ast")
                .and_then(|ast| ast.get("declarations"))
                .and_then(Value::as_array)
                .and_then(|decls| decls.get(index))
                .and_then(|d| d.get("$class"))
                .and_then(Value::as_str)
                .is_some_and(|c| c.ends_with(".MapDeclaration"));
            if is_map {
                return self.detached_map(mf, index, name);
            }
            Some(self.detached_owner(mf)?)
        };
        let (owner, ns) = match registered_as {
            Some(owner_and_ns) => owner_and_ns,
            None => {
                let owner = self.mm_index(
                    mf.get("mm")
                        .ok_or_else(|| Fault::Harness("mfref without mm".into()))?,
                )?;
                let ns = mf.get("ns").and_then(Value::as_str).unwrap_or_default();
                (owner, ns.to_string())
            }
        };
        let ns = ns.as_str();
        let mm = &self.pool[owner].mm;
        let not_found =
            || Fault::Divergence(format!("state divergence: declaration {name} not found"));
        let file = mm.model_file_id(ns).ok_or_else(|| {
            Fault::Divergence(format!(
                "state divergence: model file {ns} not registered after replay"
            ))
        })?;
        let id = mm.declaration_ids(file).nth(index).ok_or_else(not_found)?;
        match mm.declaration(id) {
            Some(d) if d.name() == name => Ok(DeclTarget::Registered(owner, id)),
            _ => Err(not_found()),
        }
    }

    /// P2-06b's `MapDeclaration` of an `mfnew` model file, read directly out
    /// of the built `ModelFile` ([`DeclTarget::Detached`]).
    fn detached_map(&mut self, mf: &Value, index: usize, name: &str) -> Faulty<DeclTarget> {
        let file_arg = self.file(mf, None)?;
        let built = ModelFile::from_json_with_definitions(
            &file_arg.ast,
            file_arg.definitions.clone(),
            file_arg.file_name.clone(),
        )
        .map_err(|e| divergence_from(&to_oracle_error(&e), "new ModelFile"))?;
        if built.declarations().get(index).map(Named::name) != Some(name) {
            return Err(Fault::Divergence(
                "state divergence: declaration not found in an unregistered ModelFile".into(),
            ));
        }
        Ok(DeclTarget::Detached {
            mm_index: file_arg.mm_index,
            file: built,
            index,
        })
    }

    /// The Rust owner of a non-map declaration of an `mfnew` model file:
    /// TS's `new ModelFile(mm, ast, …)`, never registered with `mm`. The
    /// arena hands out handles only for a registered file, so the file is
    /// registered, unvalidated, into a copy of `mm` pushed onto the pool
    /// ([`Replayed::with_detached_file`]); `mm` itself stays as TS leaves
    /// it. TS resolves such a file's own names through the file and its
    /// imports through `mm`, which the copy answers the same way. An
    /// `mfnew` inside a step, or of a system namespace, stays unsupported.
    fn detached_owner(&mut self, mf: &Value) -> Faulty<(usize, String)> {
        if mf.get("mm").and_then(|m| m.get(M)).and_then(Value::as_str) == Some("self") {
            return Err(blocked(
                "a declaration of an unregistered model file (mfnew) inside a model manager step",
                "ModelFile.new",
            ));
        }
        let file = self.file(mf, None)?;
        let owner = file
            .mm_index
            .ok_or_else(|| Fault::Harness("an mfnew without its model manager".into()))?;
        let ns = file
            .ast
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let Some(copy) = self.pool[owner].with_detached_file(&file)? else {
            return Err(blocked(
                "a declaration of an unregistered model file (mfnew) of a system namespace",
                "ModelFile.new",
            ));
        };
        self.pool.push(copy);
        Ok((self.pool.len() - 1, ns))
    }

    /// A `MapKeyType`/`MapValueType` target: `{decl: <declref>, part: "key" |
    /// "value"}` (`migration/oracle/lib/codec.js`). The referenced
    /// declaration is checked to be a `MapDeclaration` here, once, rather
    /// than by every op that takes an [`Arg::MapPart`]/[`Arg::MapPartDetached`].
    fn map_part(&mut self, v: &Value) -> Faulty<Arg> {
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
        match self.declref(decl)? {
            DeclTarget::Registered(owner, id) => match self.pool[owner].mm.declaration(id) {
                Some(Declaration::Map(_)) => Ok(Arg::MapPart(owner, id, is_key)),
                _ => Err(Fault::Divergence(
                    "state divergence: the declaration did not load as a map".into(),
                )),
            },
            DeclTarget::Detached {
                mm_index,
                file,
                index,
            } => match file.declarations().get(index) {
                Some(Declaration::Map(_)) => Ok(Arg::MapPartDetached {
                    mm_index,
                    file,
                    index,
                    is_key,
                }),
                _ => Err(Fault::Divergence(
                    "state divergence: the declaration did not load as a map".into(),
                )),
            },
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
        let (owner, decl) = match self.declref(decl)? {
            DeclTarget::Registered(owner, id) => (owner, id),
            DeclTarget::Detached { .. } => {
                return Err(blocked(
                    "a property of a declaration in a model file that is not registered \
                     (mfnew) has no Rust handle",
                    "ModelFile.new",
                ));
            }
        };
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
                let (owner, id) = match self.declref(v)? {
                    DeclTarget::Registered(owner, id) => (owner, id),
                    DeclTarget::Detached { .. } => {
                        return Err(blocked(
                            "a decorator of a declaration in a model file that is not \
                             registered (mfnew) has no Rust handle",
                            "ModelFile.new",
                        ));
                    }
                };
                Ok((owner, DecoParent::Decl(id)))
            }
            // A map's key or value (`{decl, part}`, P2-06): the engine's
            // `MapDeclaration` does not read its key's or value's decorators
            // (its doc comment), which `MapKeyType.process`/
            // `MapValueType.process` do in TS.
            Some("propref") if v.get("part").is_some() => {
                let member = match v.get("part").and_then(Value::as_str) {
                    Some("value") => "MapValueType.process",
                    _ => "MapKeyType.process",
                };
                Err(blocked(
                    "the decorators of a map's key or value, which the Rust MapDeclaration does \
                     not read",
                    member,
                ))
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
        // "entries": [[key, value], ...]}` -> `MAP_TAG`'s value (task
        // P3-01b), which keeps each key's JS type and tells a `Map` from a
        // plain object.
        "map" => {
            let entries = map
                .get("entries")
                .and_then(Value::as_array)
                .ok_or_else(|| Fault::Harness("map value without entries".into()))?;
            let mut pairs = Vec::with_capacity(entries.len());
            for entry in entries {
                let pair = entry
                    .as_array()
                    .ok_or_else(|| Fault::Harness("a map entry that is not [key, value]".into()))?;
                let key = typed_field_value(pair.first().unwrap_or(&Value::Null))?;
                let value = typed_field_value(pair.get(1).unwrap_or(&Value::Null))?;
                pairs.push((key, value));
            }
            Ok(js_map(pairs))
        }
        // `{"@@oracle":"number","value":"NaN"|"Infinity"|"-Infinity"|"-0"}`:
        // a non-finite number as `NUMBER_TAG`'s value (task P3-01b), `-0`
        // as the JSON number it compares equal to.
        "number" => Ok(match map.get("value").and_then(Value::as_str) {
            Some("-0") => json!(-0.0),
            Some(text) => js_special_number(text),
            None => Value::Null,
        }),
        other => Err(Fault::Unsupported(format!(
            "a typed field value of kind {other} is not decoded"
        ))),
    }
}

/// Model manager options that concerto-core 5.0.0 reads on the paths this
/// harness replays and the Rust engine does not model yet: metamodel
/// validation (`basemodelmanager.ts` `addModelFile`), adding the metamodel
/// (constructor) and a custom `RegExp` (`stringvalidator.ts`).
/// `decoratorValidation` (`Decorated.validate`) used to be here too; the
/// engine has modelled it since P2-08b (`ModelManager::set_decorator_validation`),
/// so P2-09b moved it to its own recognised key below, alongside
/// `ALLOW_RESERVED_SYSTEM_TYPE_NAMES`.
const UNMODELLED_OPTIONS: [&str; 3] = ["metamodelValidation", "addMetamodel", "regExp"];

/// TS `ModelManagerOptions.dangerouslyAllowReservedSystemTypeNamesInUserModels`,
/// read back as `Boolean(modelFile.getModelManager()?.options?.<this>)` by
/// `Declaration.validate` (declaration.ts) and modelled by the Rust engine
/// (P2-08).
const ALLOW_RESERVED_SYSTEM_TYPE_NAMES: &str =
    "dangerouslyAllowReservedSystemTypeNamesInUserModels";

/// TS `ModelManagerOptions.decoratorValidation`
/// (`{missingDecorator?, invalidDecorator?}`), read by `Decorator.validate`
/// (introspect/decorator.ts) and modelled by the Rust engine since P2-08b
/// (`ModelManager::set_decorator_validation`/`DecoratorValidationOptions`).
/// P2-09b: the harness used to reject this option outright (it was in
/// `UNMODELLED_OPTIONS`) even though the engine already replays it.
const DECORATOR_VALIDATION: &str = "decoratorValidation";

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
        // `StringValidator`'s constructor builds the custom RegExp.
        "regExp" => "StringValidator.new",
        _ => return None,
    })
}

/// TS `ModelManagerOptions.decoratorValidation`: an object with optional
/// `missingDecorator`/`invalidDecorator` strings. Only the exact string
/// `"error"` ever throws (`Decorator::validate`'s doc comment); any other
/// value, including a non-string one, only logs — and this port has no
/// logger (`Decorator::handle`), so it is silently unobservable either way.
/// Decoding a non-string field as absent is therefore exact for every
/// fixture this harness can see: whether the check runs at all
/// (`DecoratorValidationOptions::is_enabled`) never changes what is
/// observable, only a throw (string `"error"`) does.
fn decode_decorator_validation(value: &Value) -> Faulty<DecoratorValidationOptions> {
    let Some(map) = value.as_object() else {
        return Err(Fault::Unsupported(
            "decoratorValidation option that is not an object".into(),
        ));
    };
    let field = |key: &str| match map.get(key) {
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    };
    Ok(DecoratorValidationOptions {
        missing_decorator: field("missingDecorator"),
        invalid_decorator: field("invalidDecorator"),
    })
}

/// A recipe's options as the harness replays them: `skipLocationNodes`
/// (which only selects the cache entry, i.e. the AST's shape),
/// `dangerouslyAllowReservedSystemTypeNamesInUserModels` and
/// `decoratorValidation`.
struct Options {
    skip_location_nodes: Value,
    allow_reserved_system_type_names: bool,
    decorator_validation: DecoratorValidationOptions,
}

/// Checks a recipe's options. An unmodelled option with a truthy value, or
/// an option this harness does not know, is unsupported.
fn check_options(options: &Value) -> Faulty<Options> {
    if is_undefined(options) || options.is_null() {
        return Ok(Options {
            skip_location_nodes: Value::Null,
            allow_reserved_system_type_names: false,
            decorator_validation: DecoratorValidationOptions::default(),
        });
    }
    let Some(map) = options.as_object() else {
        return Err(Fault::Unsupported(
            "model manager options that are not an object".into(),
        ));
    };
    for (key, value) in map {
        let inert = key == "skipLocationNodes"
            || key == ALLOW_RESERVED_SYSTEM_TYPE_NAMES
            || key == DECORATOR_VALIDATION
            || INERT_OPTIONS.contains(&key.as_str());
        if !inert && (truthy(value) || !UNMODELLED_OPTIONS.contains(&key.as_str())) {
            let reason =
                format!("ModelManager option `{key}` is not modelled by the Rust engine yet");
            return Err(match option_reader(key) {
                Some(member) => blocked(reason, member),
                None => Fault::Blocked(reason, Blocker::Owner(UNOWNED.into())),
            });
        }
    }
    Ok(Options {
        skip_location_nodes: match map.get("skipLocationNodes") {
            None => Value::Null,
            Some(v) if is_undefined(v) => Value::Null,
            Some(v) => v.clone(),
        },
        allow_reserved_system_type_names: map
            .get(ALLOW_RESERVED_SYSTEM_TYPE_NAMES)
            .is_some_and(truthy),
        decorator_validation: match map.get(DECORATOR_VALIDATION) {
            None => DecoratorValidationOptions::default(),
            Some(v) if is_undefined(v) || v.is_null() => DecoratorValidationOptions::default(),
            Some(v) => decode_decorator_validation(v)?,
        },
    })
}

impl Replayed {
    /// `new ModelManager(options)`.
    pub fn new(kind: Kind, options: &Value) -> Faulty<Self> {
        let Options {
            skip_location_nodes,
            allow_reserved_system_type_names,
            decorator_validation,
        } = check_options(options)?;
        let mm = Self::fresh_manager(
            allow_reserved_system_type_names,
            decorator_validation.clone(),
        )?;
        Ok(Self {
            kind,
            options: options.clone(),
            skip_location_nodes,
            allow_reserved_system_type_names,
            decorator_validation,
            files: Vec::new(),
            mm,
        })
    }

    /// A model manager another op returned (a `derived` recipe), with its
    /// user model files as they were loaded: without a file name (`fromAst`
    /// passes none). Its options are the derived manager's own (merge with
    /// P2-08: `dangerouslyAllowReservedSystemTypeNamesInUserModels` is
    /// carried over, so a rebuild keeps it; a validating add checks only the
    /// new file, so whether the derived manager was validated no longer
    /// matters here). `decoratorValidation` is read back the same way
    /// (P2-09b): the engine's own derivation (e.g. `dcs/mod.rs`) already
    /// carries it onto the returned manager, so `derived.mm` is the source
    /// of truth, not a re-decode of `options`.
    pub fn from_derived(kind: Kind, derived: super::ops::DerivedModelManager) -> Self {
        let files = derived
            .mm
            .model_files()
            .filter(|mf| !EXCLUDE_NS.contains(&mf.namespace()))
            .map(|mf| Entry {
                ast: mf.ast().clone(),
                file_name: mf.file_name().map(str::to_string),
                nullish_name: undefined(),
                definitions: mf.definitions().map(str::to_string),
            })
            .collect();
        Self {
            kind,
            // TS builds a derived manager with `new ModelManager()`
            // (`extract*`) or with options that name no `Serializer` option
            // (`decorateModels`, `validate`), so its serializer is the one an
            // absent `options` gives (as `new` records it: `undefined`).
            options: undefined(),
            skip_location_nodes: Value::Null,
            allow_reserved_system_type_names: derived
                .mm
                .dangerously_allow_reserved_system_type_names_in_user_models(),
            decorator_validation: derived.mm.decorator_validation().clone(),
            files,
            mm: derived.mm,
        }
    }

    /// `new ModelManager(options)` for this recipe's modelled options.
    fn fresh_manager(
        allow_reserved_system_type_names: bool,
        decorator_validation: DecoratorValidationOptions,
    ) -> Faulty<ModelManager> {
        let mut mm = ModelManager::new()
            .map_err(|e| divergence_from(&to_oracle_error(&e), "ModelManager::new"))?;
        mm.set_decorator_validation(decorator_validation);
        mm.set_dangerously_allow_reserved_system_type_names_in_user_models(
            allow_reserved_system_type_names,
        );
        Ok(mm)
    }

    /// The Rust manager rebuilt from `files`, all of which loaded before.
    fn rebuild(&mut self) -> Faulty<()> {
        let mut mm = Self::fresh_manager(
            self.allow_reserved_system_type_names,
            self.decorator_validation.clone(),
        )?;
        for entry in &self.files {
            mm.add_model_with_definitions(
                &entry.ast,
                entry.definitions.clone(),
                entry.file_name.clone(),
            )
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

    /// A copy of this manager with `file` (an `mfnew`,
    /// `Session::detached_owner`) registered without validation: in place of
    /// the file this manager has under the same namespace, if any (TS
    /// resolves the detached file's own names through the file itself, never
    /// through the registered one), and last otherwise. `None` for a system
    /// namespace, which the copy cannot re-register.
    fn with_detached_file(&self, file: &FileArg) -> Faulty<Option<Self>> {
        let ns = file.ast.get("namespace").and_then(Value::as_str);
        if ns.is_none_or(is_system_namespace) {
            return Ok(None);
        }
        let mut copy = Self {
            kind: self.kind,
            options: self.options.clone(),
            skip_location_nodes: self.skip_location_nodes.clone(),
            allow_reserved_system_type_names: self.allow_reserved_system_type_names,
            decorator_validation: self.decorator_validation.clone(),
            files: self.files.clone(),
            mm: Self::fresh_manager(
                self.allow_reserved_system_type_names,
                self.decorator_validation.clone(),
            )?,
        };
        let entry = Entry {
            ast: file.ast.clone(),
            file_name: file.file_name.clone(),
            nullish_name: file.nullish_name.clone(),
            definitions: file.definitions.clone(),
        };
        match copy
            .files
            .iter_mut()
            .find(|e| e.ast.get("namespace").and_then(Value::as_str) == ns)
        {
            Some(existing) => *existing = entry,
            None => copy.files.push(entry),
        }
        copy.rebuild()?;
        Ok(Some(copy))
    }

    /// `mm_index` is left `None`: this method doesn't know its own index in
    /// the session's pool, so [`Session::file`] fills it in on the result.
    fn file_arg(&self, ns: &str) -> Option<FileArg> {
        let mf = self.mm.model_file(ns)?;
        Some(FileArg {
            ast: mf.ast().clone(),
            file_name: mf.file_name().map(str::to_string),
            nullish_name: self.nullish_name(ns),
            definitions: mf.definitions().map(str::to_string),
            mm_index: None,
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
                let cto = match input {
                    Value::String(s) => s.clone(),
                    // TS: `ctoProcessFile`'s `typeof data === 'string' ?
                    // data : String(data)`. A JSON object decoded from a
                    // fixture is always a plain object with no custom
                    // `toString`/`Symbol.toPrimitive`, so `String()` on one
                    // is always the fixed literal `"[object Object]"` — the
                    // only non-string shape `addModel`'s `modelInput` takes
                    // in the corpus today (`build-cto-cache.js`'s own
                    // `jsToString`, P2-09b). Any other JSON kind (a number,
                    // boolean, array, ...) is left unsupported rather than
                    // guessed at: nothing in the corpus exercises `String()`
                    // on one here, and their JS coercions are not this
                    // simple (an array, for one, stringifies each element
                    // and joins with commas). A missing argument arrives as
                    // the `{"@@oracle":"undefined"}` marker, which is
                    // `String(undefined)`, `"undefined"`, as in `jsToString`.
                    v if is_undefined(v) => "undefined".to_string(),
                    Value::Object(_) => "[object Object]".to_string(),
                    _ => {
                        return Err(Fault::Unsupported(
                            "a ModelManager given a non-string, non-object model input, whose \
                             String(input) coercion this harness does not (yet) replicate"
                                .into(),
                        ));
                    }
                };
                self.cto_ast(h, &cto, file_name.as_str())
            }
            Kind::BaseModelManager | Kind::AstModelManager => Ok(Ok(input.clone())),
        }
    }

    /// TS `addModelFile(modelFile, cto, fileName, disableValidation)` for a
    /// model file built from `ast` (see the module doc for validation).
    ///
    /// **Validation on add, corrected (P2-08d, accordproject/concerto-rust#151).**
    /// TS validates the new file *before* registering it
    /// (`if (!this.modelFiles[ns]) { if (!disableValidation) { modelFile.validate(); }
    /// this.modelFiles[ns] = modelFile; } else { this._throwAlreadyExists(...); }`),
    /// so a duplicate namespace is rejected first, exactly as it is here
    /// (`add_model_with_definitions`'s own check, below, unconditionally),
    /// and — only for a genuinely new namespace — `modelFile.validate()`
    /// runs while `this.modelFiles` still lacks this file's own namespace:
    /// an import naming it (a self-import) cannot resolve. Replayed the
    /// same way, via [`ModelManager::validate_detached_model_file`], which
    /// checks `getImports()` against `self.mm` as it stands here (not yet
    /// holding this namespace) while still resolving the file's own local
    /// types, the same as `self.mm.validate_model_file` would once
    /// registered (that function's doc comment). A namespace already
    /// registered skips straight to `add_model_with_definitions`, which
    /// raises TS's `_throwAlreadyExists` for it, matching TS's order.
    fn add_file(&mut self, file: FileArg, validate: bool) -> Faulty<Outcome> {
        let ns = file
            .ast
            .get("namespace")
            .and_then(Value::as_str)
            .map(str::to_string);
        if validate {
            let already_registered = ns
                .as_deref()
                .is_some_and(|ns| self.mm.model_file(ns).is_some());
            if !already_registered {
                let mf = match ModelFile::from_json_with_definitions(
                    &file.ast,
                    file.definitions.clone(),
                    file.file_name.clone(),
                ) {
                    Ok(mf) => mf,
                    Err(e) => return Ok(Err(to_oracle_error(&e))),
                };
                if let Err(e) = self.mm.validate_detached_model_file(&mf) {
                    return Ok(Err(to_oracle_error(&e)));
                }
            }
        }
        if let Err(e) = self.mm.add_model_with_definitions(
            &file.ast,
            file.definitions.clone(),
            file.file_name.clone(),
        ) {
            return Ok(Err(to_oracle_error(&e)));
        }
        self.files.push(Entry {
            ast: file.ast,
            file_name: file.file_name,
            nullish_name: file.nullish_name,
            definitions: file.definitions,
        });
        let ns = ns.unwrap_or_default();
        Ok(Ok(self.model_file_summary(&ns).unwrap_or_else(undefined)))
    }

    /// TS `BaseModelManager.validateModelFile(modelFile, fileName)`'s string
    /// overload (P2-08b): `processFile` then `new ModelFile(this, ast,
    /// modelFile, fileName).validate()`, replayed against `self` without
    /// registering the file — [`ModelManager::validate_detached_model_file`]
    /// is exactly this (module doc on `ops.rs`'s `ModelFile.validate` wiring,
    /// which shares it). CTO parsing goes through the P1-07a cache
    /// (`process_file`), never a Rust parser (module doc, top of file).
    pub fn validate_model_file_text(
        &self,
        h: &Harness,
        cto: &str,
        file_name: &Value,
    ) -> Faulty<Outcome> {
        let ast = match self.process_file(h, &Value::String(cto.to_string()), file_name)? {
            Ok(ast) => ast,
            Err(parse_error) => return Ok(Err(parse_error)),
        };
        let file_name_str = match file_name {
            Value::String(s) => Some(s.clone()),
            _ => None,
        };
        let mf = ModelFile::from_json_with_definitions(&ast, Some(cto.to_string()), file_name_str)
            .map_err(|e| {
                Fault::Divergence(format!(
                    "state divergence: a model file the CTO cache accepted failed to build: {}",
                    to_oracle_error(&e).message
                ))
            })?;
        Ok(match self.mm.validate_detached_model_file(&mf) {
            Ok(()) => Ok(undefined()),
            Err(e) => Err(to_oracle_error(&e)),
        })
    }

    /// TS `updateModelFile(modelFile, fileName, disableValidation)`'s
    /// registration (P2-08b), for an already-parsed `file`: rebuilds it as a
    /// [`ModelFile`] the same way [`Session::decode`]'s `mfnew`/`mfref`
    /// already do, then delegates to
    /// [`ModelManager::update_model_file`]. On success, `self.files` is kept
    /// in sync with the replacement (so a later rollback in the same
    /// fixture, `rebuild`, still has it); on error, neither `self.mm` nor
    /// `self.files` changes, matching TS's own catch leaving `this`
    /// unchanged.
    fn update_file(&mut self, file: FileArg, validate: bool) -> Faulty<Outcome> {
        let mf = match ModelFile::from_json_with_definitions(
            &file.ast,
            file.definitions.clone(),
            file.file_name.clone(),
        ) {
            Ok(mf) => mf,
            Err(e) => return Ok(Err(to_oracle_error(&e))),
        };
        let ns = mf.namespace().to_string();
        match self.mm.update_model_file(mf, validate) {
            Ok(updated) => {
                self.mm = updated;
                if let Some(entry) = self
                    .files
                    .iter_mut()
                    .find(|e| e.ast.get("namespace").and_then(Value::as_str) == Some(ns.as_str()))
                {
                    *entry = Entry {
                        ast: file.ast,
                        file_name: file.file_name,
                        nullish_name: file.nullish_name,
                        definitions: file.definitions,
                    };
                }
                Ok(Ok(self.model_file_summary(&ns).unwrap_or_else(undefined)))
            }
            Err(e) => Ok(Err(to_oracle_error(&e))),
        }
    }

    /// TS `deleteModelFile(namespace)` (P2-08b), delegating to
    /// [`ModelManager::delete_model_file`] and keeping `self.files` in sync
    /// with the survivors on success.
    fn delete_file(&mut self, namespace: &str) -> Faulty<Outcome> {
        match self.mm.delete_model_file(namespace) {
            Ok(updated) => {
                self.mm = updated;
                self.files
                    .retain(|e| e.ast.get("namespace").and_then(Value::as_str) != Some(namespace));
                Ok(Ok(undefined()))
            }
            Err(e) => Ok(Err(to_oracle_error(&e))),
        }
    }

    /// TS `BaseModelManager.updateExternalModels(options, fileDownloader)`
    /// (P2-08b). The ledger makes it HYBRID: the download stays in JS, the
    /// rest is [`ModelManager::update_external_models`]. The fixtures record
    /// the download's network responses (`inputs.net`, URL -> `{status,
    /// body}`), not the ASTs it produced, so the JS half is replayed here as
    /// the default downloader runs it over those responses —
    /// `FileDownloader.downloadExternalDependencies` with a
    /// `DefaultFileLoader` (concerto-util) — only as far as the corpus
    /// exercises it; anything past that is `unsupported`:
    ///
    /// - the jobs are every `MetaModelUtil.getExternalImports` URI of every
    ///   `getModelFiles()` model, in order (more than one is `unsupported`:
    ///   `PromisePool` returns results in completion order);
    /// - `github://x` is fetched as `https://raw.githubusercontent.com/x`,
    ///   `http(s)://` as itself, and any other scheme fails
    ///   `CompositeFileLoader.load`; a non-2xx response fails
    ///   `HTTPFileLoader.load`; either error is rethrown by
    ///   `handleJobError` as `Failed to load model file. Job: <url> Details:
    ///   Error: <message>`;
    /// - a 2xx body is `processFile('@' + host + path with / as ., body)`,
    ///   through the CTO cache, whose builder collects every 2xx `net` body
    ///   (P2-09b): a body with no entry is a harness error (a stale cache). A
    ///   downloaded model with external imports of its own (the recursive
    ///   walk) is `unsupported`.
    ///
    /// A download failure happens before anything is registered, so the
    /// manager is left as it was, as TS's `catch` leaves it.
    pub fn update_external_models(
        &mut self,
        h: &Harness,
        args: &[Arg],
        net: Option<&Value>,
    ) -> Faulty<Outcome> {
        if self.kind != Kind::ModelManager {
            return Err(Fault::Unsupported(format!(
                "updateExternalModels on a {}",
                self.kind.ctor()
            )));
        }
        if args.len() > 1 {
            return Err(Fault::Unsupported(
                "updateExternalModels with a custom fileDownloader".into(),
            ));
        }
        let jobs: Vec<String> = self
            .mm
            .model_files()
            .filter(|mf| !EXCLUDE_NS.contains(&mf.namespace()))
            .flat_map(|mf| external_import_uris(mf.ast()))
            .collect();
        if jobs.len() > 1 {
            return Err(Fault::Unsupported(
                "updateExternalModels with more than one download job (JS PromisePool \
                 returns them in completion order)"
                    .into(),
            ));
        }
        let mut sources = Vec::new();
        for url in jobs {
            let failed = |message: String| {
                Ok(Err(OracleError {
                    class: "Error".into(),
                    message: format!(
                        "Failed to load model file. Job: {url} Details: Error: {message}"
                    ),
                    location: None,
                    component: None,
                }))
            };
            let fetched = if let Some(rest) = url.strip_prefix("github://") {
                format!("https://raw.githubusercontent.com/{rest}")
            } else if url.starts_with("http://") || url.starts_with("https://") {
                url.clone()
            } else {
                return failed(format!(
                    "Failed to find a model file loader that can handle: {url}"
                ));
            };
            let Some(response) = net.and_then(|n| n.get(&fetched)) else {
                return Err(Fault::Unsupported(format!(
                    "updateExternalModels fetching {fetched}, which has no recorded response"
                )));
            };
            let status = response.get("status").and_then(Value::as_u64);
            let body = response.get("body").and_then(Value::as_str);
            let (Some(status), Some(body)) = (status, body) else {
                return Err(Fault::Harness(format!(
                    "a recorded response for {fetched} without a status and a body"
                )));
            };
            if !(200..300).contains(&status) {
                return failed(format!("HTTP request failed with status: {status}"));
            }
            let name = downloaded_file_name(&fetched);
            let cache = h
                .cache
                .as_ref()
                .ok_or_else(|| Fault::Harness("no CTO -> AST cache (build-cto-cache.js)".into()))?;
            let ast = match cache.lookup(body, Some(&name), &self.skip_location_nodes) {
                Ok(CacheEntry::Ast(ast)) => ast,
                Ok(CacheEntry::Error(_)) => {
                    return Err(Fault::Unsupported(
                        "updateExternalModels downloading a model that fails to parse".into(),
                    ));
                }
                Err(e) => {
                    return Err(Fault::Harness(format!(
                        "updateExternalModels: the downloaded CTO (a recorded `net` response \
                         body) has no CTO cache entry: {e}"
                    )));
                }
            };
            if external_import_uris(&ast).next().is_some() {
                return Err(Fault::Unsupported(
                    "updateExternalModels downloading a model with external imports of its own"
                        .into(),
                ));
            }
            sources.push(ModelFileSource {
                ast,
                definitions: Some(body.to_string()),
                file_name: Some(name),
            });
        }
        let registered = match self.mm.update_external_models(sources) {
            Ok(registered) => registered,
            Err(e) => return Ok(Err(to_oracle_error(&e))),
        };
        let mut summaries = Vec::new();
        for mf in registered {
            let entry = Entry {
                ast: mf.ast().clone(),
                file_name: mf.file_name().map(str::to_string),
                nullish_name: undefined(),
                definitions: mf.definitions().map(str::to_string),
            };
            let ns = mf.namespace();
            match self
                .files
                .iter_mut()
                .find(|e| e.ast.get("namespace").and_then(Value::as_str) == Some(ns))
            {
                Some(existing) => *existing = entry,
                None => self.files.push(entry),
            }
            summaries.push(json!({
                M: "ModelFile",
                "namespace": ns,
                "name": mf.file_name().map_or_else(undefined, |n| json!(n)),
                "ast": mf.ast(),
            }));
        }
        Ok(Ok(Value::Array(summaries)))
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
                // TS `ModelFile.getDefinitions` (P2-08b, `get_models`'s only
                // consumer so far): `ctoProcessFile` always returns
                // `definitions: content` — the input coerced to a string
                // when it was not already one (`String(data)`) — and
                // `addModel(modelInput, cto, ...)`'s own `finalCto = cto ||
                // definitions` prefers an explicit `cto` argument over that.
                // For a non-string `Kind::ModelManager` input `process_file`
                // coerces as `String()` does, and that text never parses as
                // CTO, so the call throws before `definitions` is read; the other two
                // kinds pass their `input` straight through as the AST, with
                // no CTO text to keep, so they get `None`.
                let explicit_cto = (method == "addModel")
                    .then(|| args.get(1))
                    .flatten()
                    .and_then(|arg| match arg {
                        Arg::Plain(Value::String(s)) => Some(s.clone()),
                        _ => None,
                    });
                let definitions = explicit_cto.or_else(|| match (self.kind, &input) {
                    (Kind::ModelManager, Value::String(s)) => Some(s.clone()),
                    _ => None,
                });
                // `new ModelFile(...)` then `addModelFile(...)`: `add_model`
                // builds the model file first, so its errors come first.
                self.add_file(
                    FileArg {
                        ast,
                        file_name,
                        nullish_name,
                        definitions,
                        mm_index: None,
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
                Ok(()) => Ok(Ok(undefined())),
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
                        definitions: None,
                        mm_index: None,
                    };
                    // `new ModelFile(this, model)`, then `addModelFile(…,
                    // true)`; TS keeps whatever loaded before an error.
                    if let Err(e) = self.add_file(file, false)? {
                        return Ok(Err(e));
                    }
                }
                let disable = options.get("disableValidation").is_some_and(truthy);
                if !disable && let Err(e) = self.mm.validate_models() {
                    return Ok(Err(to_oracle_error(&e)));
                }
                Ok(Ok(undefined()))
            }
            // TS `updateModelFile(modelFile, fileName?, disableValidation?)`
            // (basemodelmanager.ts, P2-08b): `fileName` is read only for the
            // string overload — TS's object branch ignores its own
            // `fileName` parameter entirely — so `disableValidation` stays
            // at argument position 2 either way.
            "updateModelFile" => match args.first() {
                Some(Arg::Plain(Value::String(cto))) => {
                    let file_name_value = plain(1)?;
                    let validate = !no_validation(2)?;
                    let ast = match self.process_file(
                        h,
                        &Value::String(cto.clone()),
                        &file_name_value,
                    )? {
                        Ok(ast) => ast,
                        Err(parse_error) => return Ok(Err(parse_error)),
                    };
                    let (file_name, nullish_name) = nullish_or_string(&file_name_value)?;
                    self.update_file(
                        FileArg {
                            ast,
                            file_name,
                            nullish_name,
                            definitions: Some(cto.clone()),
                            mm_index: None,
                        },
                        validate,
                    )
                }
                Some(Arg::File(file)) => {
                    let validate = !no_validation(2)?;
                    self.update_file(file.clone(), validate)
                }
                _ => Err(Fault::Unsupported(
                    "updateModelFile with a modelFile argument that is not a string or model file"
                        .into(),
                )),
            },
            "deleteModelFile" => {
                let Some(Arg::Plain(Value::String(namespace))) = args.first() else {
                    return Err(Fault::Unsupported(
                        "deleteModelFile with a namespace argument that is not a string".into(),
                    ));
                };
                self.delete_file(namespace)
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
            let (file_name, nullish_name, ast, definitions) = match item {
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
                    // `ctoProcessFile`'s `definitions: content` (P2-08b,
                    // `add_model_with_definitions`'s doc). A non-string
                    // `Kind::ModelManager` input never parses (see
                    // `process_file`), so only a string needs its text kept.
                    let definitions = match (self.kind, input) {
                        (Kind::ModelManager, Value::String(s)) => Some(s.clone()),
                        _ => None,
                    };
                    match self.process_file(h, input, &file_name_value)? {
                        Ok(ast) => (file_name, nullish_name, ast, definitions),
                        Err(e) => return restore(self, e),
                    }
                }
                Arg::File(file) => (
                    file.file_name.clone(),
                    file.nullish_name.clone(),
                    file.ast.clone(),
                    file.definitions.clone(),
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
            if let Err(e) =
                self.mm
                    .add_model_with_definitions(&ast, definitions.clone(), file_name.clone())
            {
                return restore(self, to_oracle_error(&e));
            }
            self.files.push(Entry {
                ast,
                file_name,
                nullish_name,
                definitions,
            });
            added.push(ns.unwrap_or_default());
        }
        if validate && let Err(e) = self.mm.validate_models() {
            return restore(self, to_oracle_error(&e));
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
            Self::DeclDetached {
                mm_index,
                file,
                index,
            } => Self::DeclDetached {
                mm_index: *mm_index,
                file: file.clone(),
                index: *index,
            },
            Self::MapPartDetached {
                mm_index,
                file,
                index,
                is_key,
            } => Self::MapPartDetached {
                mm_index: *mm_index,
                file: file.clone(),
                index: *index,
                is_key: *is_key,
            },
            Self::DeclNew { fqn, processed } => Self::DeclNew {
                fqn: fqn.clone(),
                processed: processed.clone(),
            },
            Self::Validator(m, p, part) => Self::Validator(*m, *p, part.clone()),
            Self::Deco(m, parent, i) => Self::Deco(*m, parent.clone(), *i),
            Self::List(items) => Self::List(items.clone()),
            Self::Typed(m, inst) => Self::Typed(*m, inst.clone()),
            Self::Predicate(names) => Self::Predicate(names.clone()),
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

/// TS `Object.values(MetaModelUtil.getExternalImports(ast))`
/// (concerto-metamodel): the `uri` of every import that has one, keyed by
/// the import's first fully-qualified name (a later import with the same key
/// replaces the earlier one's URI, in the earlier one's place).
fn external_import_uris(ast: &Value) -> impl Iterator<Item = String> + use<> {
    let mut keyed: Vec<(String, String)> = Vec::new();
    for imp in ast
        .get("imports")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(uri) = imp
            .get("uri")
            .and_then(Value::as_str)
            .filter(|u| !u.is_empty())
        else {
            continue;
        };
        let ns = imp
            .get("namespace")
            .and_then(Value::as_str)
            .unwrap_or("undefined");
        let class = imp
            .get("$class")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let first = match class.rsplit('.').next() {
            Some("ImportAll") => "*".to_string(),
            Some("ImportType") => imp
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("undefined")
                .to_string(),
            _ => imp
                .get("types")
                .and_then(Value::as_array)
                .and_then(|t| t.first())
                .and_then(Value::as_str)
                .unwrap_or("undefined")
                .to_string(),
        };
        let key = format!("{ns}.{first}");
        match keyed.iter_mut().find(|(k, _)| *k == key) {
            Some(existing) => existing.1 = uri.to_string(),
            None => keyed.push((key, uri.to_string())),
        }
    }
    keyed.into_iter().map(|(_, uri)| uri)
}

/// `HTTPFileLoader.load`'s name for a downloaded file: `'@' + (url.host +
/// url.pathname).replace(/\//g, '.')`.
fn downloaded_file_name(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = rest.split(['?', '#']).next().unwrap_or_default();
    let (host, path) = match rest.split_once('/') {
        Some((host, path)) => (host, format!("/{path}")),
        None => (rest, "/".to_string()),
    };
    format!(
        "@{}",
        format!("{}{path}", host.to_ascii_lowercase()).replace('/', ".")
    )
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
