//! Decorators, and the elements that carry them.
//!
//! TS: `Decorator`, `Decorated` (src/introspect/decorator.ts,
//! src/introspect/decorated.ts). `DecoratorFactory`
//! (src/introspect/decoratorfactory.ts) is not ported: its one member,
//! `newDecorator`, is an abstract `throw new Error('not implemented')` stub
//! that only a user subclass overrides (PORTING.md 1.1 rules 4 and 5), so the
//! ledger keeps it TS. `Decorated.process` picking a user factory's decorator
//! over the default `Decorator` is the same TS-side concern, and stays there.
//!
//! # Reading a decorator's arguments (OD-3)
//!
//! The generated `mm::Decorator` is not faithful enough to read: its
//! `arguments` are typed `Vec<DecoratorLiteral>`, and `DecoratorLiteral` is
//! codegen's abstract base for the union (`DecoratorString`,
//! `DecoratorNumber`, `DecoratorBoolean`, `DecoratorTypeReference`) with none
//! of the subtypes' own fields — deserializing through it silently drops
//! every argument's actual value. PORTING.md 1.2 covers exactly this case:
//! such members read the AST as [`serde_json::Value`] instead. [`Decorator`]
//! is built from the raw node ([`Decorator::from_ast`]), and every element
//! that can carry decorators keeps its processed [`Decorator`]s alongside its
//! generated node rather than relying on the generated `decorators` field.
//! [`WithDecorators`] is the small wrapper that does this for a
//! newtype-over-`mm::*` element ([`super::declaration::EnumDeclaration`],
//! every [`super::property::Property`] variant); [`ClassDeclaration`] and
//! `ModelFile` have room for the same `Vec<Decorator>` as an ordinary field.

use concerto_metamodel::Name;
use serde_json::Value;

use crate::ecma::number_to_string;
use crate::error::{ContractError, Error, ErrorKind, Result};
use crate::introspect::declaration::ClassDeclaration;
use crate::introspect::kept::Kept;
use crate::introspect::property::Property;
use crate::introspect::qualified_class;
use crate::model_manager::ModelManager;
use crate::model_util::is_primitive_type;

/// A decorator argument that references a type, produced from a
/// `DecoratorTypeReference` node in the metamodel AST.
///
/// TS: `DecoratorTypeReferenceArgument` (src/introspect/decorator.ts).
#[derive(Debug, Clone, PartialEq)]
pub struct TypeReferenceArgument {
    /// The referenced type's short name.
    pub name: String,
    /// Whether the reference is to an array of the type: `None` when the
    /// AST node's `isArray` is itself missing (TS: `array: thing.isArray`,
    /// with no default — the pushed argument's `array` is `undefined`, not
    /// `false`, and the oracle records that distinction; P4-05 found this
    /// while wiring `Decorator.process` to a WASM binding).
    pub array: Option<bool>,
}

/// One value a decorator can be given.
///
/// TS: `DecoratorArgument` (src/introspect/decorator.ts).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum DecoratorArgument {
    /// A `DecoratorString` literal.
    String(String),
    /// A `DecoratorNumber` literal.
    Number(f64),
    /// A `DecoratorBoolean` literal.
    Boolean(bool),
    /// A `DecoratorTypeReference`.
    TypeReference(TypeReferenceArgument),
}

/// A decorator (annotation) on a class, property or model file.
///
/// TS: `Decorator` (src/introspect/decorator.ts).
#[derive(Debug, Clone, PartialEq)]
pub struct Decorator {
    /// P5-93: a [`Name`], which shares the text the decorator is read
    /// from, as the generated node's does.
    name: Name,
    /// Whether the AST node has a `name` at all. TS's `this.name =
    /// ast.name` leaves `name` `undefined` for a node without one, which
    /// includes every element a malformed, non-array `decorators` value
    /// yields ([`parse_decorators`]), and `Decorated.validate` then tells
    /// such a name apart from every string one (its `Set`) and prints it
    /// as `undefined` (`Duplicate decorator undefined`).
    /// [`Decorator::name`] still reads `""` for it, as it always has.
    /// Only the duplicate-decorator check ([`Decorator::js_name`]) tells the
    /// two apart (accordproject/concerto-rust#218).
    name_present: bool,
    arguments: Vec<DecoratorArgument>,
    /// `this.ast.location`, copied verbatim (PORTING.md 2.1).
    location: Option<Value>,
}

impl Decorator {
    /// Builds a decorator from its raw `Decorator` AST node.
    ///
    /// TS: `Decorator.process` (`this.name = ast.name`, then one argument at
    /// a time). A `DecoratorTypeReference` argument becomes
    /// [`DecoratorArgument::TypeReference`]; anything else contributes its
    /// `value` as the literal it already is (`DecoratorString`,
    /// `DecoratorNumber` or `DecoratorBoolean`, by construction of the CTO
    /// grammar and the AST codec).
    pub fn from_ast(ast: &Value) -> Self {
        let name_present = ast.get("name").is_some();
        let name = ast
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into();
        let arguments = ast
            .get("arguments")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(decode_argument).collect())
            .unwrap_or_default();
        Decorator {
            name,
            name_present,
            arguments,
            location: ast.get("location").cloned(),
        }
    }

    /// [`Decorator::from_ast`], for a node the typed read kept as a
    /// [`Kept`] (P5-76): the same decorator as from its `Value`.
    pub(crate) fn from_kept(ast: &Kept) -> Self {
        let name = ast.get("name");
        let arguments = match ast.get("arguments") {
            Some(Kept::Array(items)) => items.iter().filter_map(decode_kept_argument).collect(),
            _ => Vec::new(),
        };
        Decorator {
            name: match name {
                Some(Kept::Other(Value::String(name))) => Name::from(name),
                _ => Name::default(),
            },
            name_present: name.is_some(),
            arguments,
            location: ast.get("location").map(Kept::to_value),
        }
    }

    /// A decorator node of a string `name` and the given arguments, with
    /// no `location`, as [`Decorator::from_kept`] builds it from such a
    /// node (P5-93: for a decorator the typed read reads field by field,
    /// `kept::DecoratorsSeed`).
    pub(crate) fn from_read(name: Name, arguments: Vec<DecoratorArgument>) -> Self {
        Decorator {
            name,
            name_present: true,
            arguments,
            location: None,
        }
    }

    /// The name of this decorator.
    ///
    /// TS: `Decorator.getName`.
    pub fn name(&self) -> &str {
        &self.name
    }

    js_compat_pub! {
        /// The name as TS's `Decorator.getName()` holds it for
        /// `Decorated.validate`'s duplicate check: `None` for a node with no
        /// `name` at all (JS `undefined`), which is a different `Set` entry from
        /// every string name, `""` included. `pub`, not `pub(crate)`: the WASM
        /// binding's own `decoratorProcess` (`concerto-wasm/src/lib.rs`) needs
        /// this to give the JS-side `Decorator.name` field the same `undefined`
        /// TS's own unconditional `this.name = ast.name` leaves it with,
        /// rather than the empty-string default [`Decorator::name`] gives every
        /// other reader (accordproject/concerto-rust#219: a model-file-level
        /// `Decorator` built this way, with no name, previously surfaced as
        /// `this.name === ""`, so two of them collided as "Duplicate decorator "
        /// instead of TS's own "Duplicate decorator undefined").
        pub fn js_name(&self) -> Option<&str> {
            self.name_present.then_some(self.name.as_str())
        }
    }

    /// The arguments given to this decorator, in order.
    ///
    /// TS: `Decorator.getArguments`.
    pub fn arguments(&self) -> &[DecoratorArgument] {
        &self.arguments
    }

    js_compat_pub! {
        /// Semantic validation of the decorator: that its name and any type
        /// reference argument resolve, and that its arguments match the count
        /// and types of the properties of the type it names, if that type is
        /// itself a declaration.
        ///
        /// Runs only when `manager`'s [`DecoratorValidationOptions`] enable it: TS
        /// guards the whole body on `validationOptions.missingDecorator ||
        /// validationOptions.invalidDecorator` and does nothing at all otherwise
        /// (`DEFAULT_DECORATOR_VALIDATION` leaves both `undefined`).
        ///
        /// `context` is the fully qualified name of the decorated element, used
        /// only to describe *where* an unresolved name was found; pass `None` for
        /// a model file's own decorators, which have no such name in TS either.
        ///
        /// **Log vs throw**, faithfully: every problem found is reported through
        /// [`DecoratorValidationOptions::invalid_decorator`], except the
        /// decorator's own name failing to resolve, which is reported through
        /// [`DecoratorValidationOptions::missing_decorator`] instead — and *any*
        /// problem thrown while checking arguments is also caught and re-reported
        /// through `missing_decorator` (TS wraps the whole check in one
        /// `try`/`catch`). Reporting only throws when the option is the exact
        /// string `"error"`; anything else (including `"warn"`) only logs, and
        /// this Rust port has no logger yet (Logger.dispatch is not ported;
        /// nothing observes it), so an option other than `"error"` here is
        /// silently accepted.
        ///
        /// TS: `Decorator.validate` (src/introspect/decorator.ts).
        pub fn validate(
            &self,
            manager: &ModelManager,
            namespace: &str,
            context: Option<&str>,
        ) -> Result<()> {
            let options = manager.decorator_validation();
            if !options.is_enabled() {
                return Ok(());
            }
            match self.try_validate(manager, namespace, context, options) {
                Ok(()) => Ok(()),
                Err(problem) => self.rethrow(
                    manager,
                    namespace,
                    options.missing_decorator.as_deref(),
                    problem,
                ),
            }
        }
    }

    /// The body of the `try` block in TS `Decorator.validate`.
    fn try_validate(
        &self,
        manager: &ModelManager,
        namespace: &str,
        context: Option<&str>,
        options: &DecoratorValidationOptions,
    ) -> std::result::Result<(), Error> {
        // TS: `mf.resolveType(decoratedName, this.getName(), this.ast.location)`.
        let fqn = self.resolve_own_name(manager, namespace, context)?;
        // TS: `mf.getType(this.getName())`.
        let Ok(decorator_decl) = manager.get_declaration(&fqn) else {
            return Ok(());
        };
        let Some(_) = decorator_decl.as_class() else {
            // TS calls `decoratorDecl.getProperties()` unconditionally; only a
            // class-like declaration has one. No fixture or test names a
            // decorator whose own type is an enum, scalar or map.
            return Ok(());
        };
        // Each property comes paired with its declaring type's name
        // (`ModelManager::properties`); only the property is read here.
        let all_properties: Vec<&Property> = manager
            .properties(&fqn)?
            .into_iter()
            .map(|(_, p)| p)
            .collect();
        let (required, optional): (Vec<&Property>, Vec<&Property>) =
            all_properties.iter().partition(|p| !p.is_optional());
        let ordered: Vec<&Property> = required
            .iter()
            .copied()
            .chain(optional.iter().copied())
            .collect();

        let args = self.arguments();
        if args.len() < required.len() {
            let names = required
                .iter()
                .map(|p| p.name())
                .collect::<Vec<_>>()
                .join(",");
            let message = format!(
                "Decorator {} has too few arguments. Required properties are: [{names}]",
                self.name
            );
            self.report_invalid(manager, namespace, options, message)?;
        }
        for (n, arg) in args.iter().enumerate() {
            if n >= ordered.len() {
                let names = ordered
                    .iter()
                    .map(|p| p.name())
                    .collect::<Vec<_>>()
                    .join(",");
                let message = format!(
                    "Decorator {} has too many arguments. Properties are: [{names}]",
                    self.name
                );
                self.report_invalid(manager, namespace, options, message)?;
                continue;
            }
            self.check_argument(manager, namespace, ordered[n], arg, options)?;
        }
        Ok(())
    }

    /// `mf.resolveType(context, this.getName(), location)`: `this.getName()`
    /// must be a primitive, a locally declared type, or an imported one.
    ///
    /// This does not yet call through to `BaseModelManager.resolveType` for
    /// an imported name the way `ModelFile.resolveType` does (that deeper
    /// check is `ModelFile`'s own port, P2-08): every other failure of
    /// [`ModelManager::resolve_type_name`] is reported here as TS reports an
    /// undeclared type, which is the only failure any test or fixture in this
    /// scope exercises.
    ///
    /// TS: `ModelFile.resolveType` (src/introspect/modelfile.ts).
    fn resolve_own_name(
        &self,
        manager: &ModelManager,
        namespace: &str,
        context: Option<&str>,
    ) -> std::result::Result<String, Error> {
        if is_primitive_type(&self.name) {
            return Ok(self.name.to_string());
        }
        manager
            .resolve_type_name_at(namespace, &self.name, self.location.clone())
            .map_err(|_| {
                let err: Error = ContractError::new(
                    ErrorKind::IllegalModel,
                    "modelfile-resolvetype-undecltype",
                    vec![
                        ("type", self.name.to_string()),
                        ("context", context.unwrap_or("undefined").to_string()),
                    ],
                )
                .into();
                // TS `mf.resolveType(...)` throws `new IllegalModelException(
                // message, this, fileLocation)`: `this` is the model file `mf` is
                // called on, so this error carries its file from the start, and
                // [`Self::rethrow`] passes it on unchanged (BC-14).
                self.attach_file(manager, namespace, err)
            })
    }

    /// `manager.model_file(namespace)`, attached to `err` the way
    /// [`crate::validation::attach_model_file`] backstops every other check in
    /// this module — reused here (rather than deferring to that backstop) so
    /// an error this module builds already carries its file *before*
    /// [`Self::rethrow`] re-reports it, matching TS's `this`/`this.getParent().
    /// getModelFile()`, which is resolved synchronously at each throw site.
    /// It never attaches a second file (`attach_model_file` only fills an
    /// empty one).
    fn attach_file(&self, manager: &ModelManager, namespace: &str, err: Error) -> Error {
        match manager.model_file(namespace) {
            Some(model_file) => crate::validation::attach_model_file(err, model_file),
            None => err,
        }
    }

    /// One argument against the property it lines up with, by position.
    ///
    /// TS: the `for (args)` loop body in `Decorator.validate`.
    fn check_argument(
        &self,
        manager: &ModelManager,
        namespace: &str,
        property: &Property,
        arg: &DecoratorArgument,
        options: &DecoratorValidationOptions,
    ) -> std::result::Result<(), Error> {
        match property.type_name() {
            Some("Integer") | Some("Double") | Some("Long") => {
                if !matches!(arg, DecoratorArgument::Number(_)) {
                    self.report_invalid(
                        manager,
                        namespace,
                        options,
                        format!(
                            "Decorator {} has invalid decorator argument. Expected number. Found {}, with value {}",
                            self.name, js_typeof(arg), json_stringify(arg)
                        ),
                    )?;
                }
            }
            Some("String") => {
                if !matches!(arg, DecoratorArgument::String(_)) {
                    self.report_invalid(
                        manager,
                        namespace,
                        options,
                        format!(
                            "Decorator {} has invalid decorator argument. Expected string. Found {}, with value {}",
                            self.name, js_typeof(arg), json_stringify(arg)
                        ),
                    )?;
                }
            }
            Some("Boolean") => {
                if !matches!(arg, DecoratorArgument::Boolean(_)) {
                    self.report_invalid(
                        manager,
                        namespace,
                        options,
                        format!(
                            "Decorator {} has invalid decorator argument. Expected boolean. Found {}, with value {}",
                            self.name, js_typeof(arg), json_stringify(arg)
                        ),
                    )?;
                }
            }
            _ => self.check_type_reference_argument(manager, namespace, property, arg, options)?,
        }
        Ok(())
    }

    /// The `default:` arm: the argument must be a type reference, resolvable,
    /// and assignable to the property's declared type.
    fn check_type_reference_argument(
        &self,
        manager: &ModelManager,
        namespace: &str,
        property: &Property,
        arg: &DecoratorArgument,
        options: &DecoratorValidationOptions,
    ) -> std::result::Result<(), Error> {
        let Some(type_reference) = (match arg {
            DecoratorArgument::TypeReference(t) => Some(t),
            _ => None,
        }) else {
            return self.report_invalid(
                manager,
                namespace,
                options,
                format!(
                    "Decorator {} has invalid decorator argument. Expected object. Found {}, with value {}",
                    self.name, js_typeof(arg), json_stringify(arg)
                ),
            );
        };

        // TS: `mf.getType(typeReference.name)` — non-throwing; `None` is its
        // nullish result, whether the name resolves to nothing or resolves to
        // something this model manager has not loaded.
        let resolved = manager
            .resolve_type_name_at(namespace, &type_reference.name, None)
            .ok()
            .and_then(|fqn| manager.get_declaration(&fqn).ok().map(|_| fqn));

        match resolved {
            None => self.report_invalid(
                manager,
                namespace,
                options,
                format!(
                    "Decorator {} references a type {} which has not been defined/imported.",
                    self.name, type_reference.name
                ),
            ),
            Some(type_fqn) => {
                let Some(declared_type) = property.type_name() else {
                    return Ok(());
                };
                let property_fqn = manager
                    .resolve_type_name_at(namespace, declared_type, None)
                    .unwrap_or_else(|_| declared_type.to_string());
                if !manager.is_assignable_to(&type_fqn, &property_fqn)? {
                    self.report_invalid(
                        manager,
                        namespace,
                        options,
                        format!(
                            "Decorator {} references a type {} which cannot be assigned to the declared type {property_fqn}",
                            self.name, type_reference.name
                        ),
                    )?;
                }
                Ok(())
            }
        }
    }

    /// TS: `this.handleError(validationOptions.invalidDecorator, err)`.
    fn report_invalid(
        &self,
        manager: &ModelManager,
        namespace: &str,
        options: &DecoratorValidationOptions,
        message: String,
    ) -> std::result::Result<(), Error> {
        self.handle(
            manager,
            namespace,
            options.invalid_decorator.as_deref(),
            message,
        )
    }

    /// `handleError(level, err)`: logs (not yet ported; nothing observes it),
    /// then throws only when `level` is the exact string `"error"`.
    ///
    /// TS: `new IllegalModelException(err, this.getParent().getModelFile(),
    /// this.ast.location)` — the file is attached right here, at construction,
    /// not only once the caller backstops it later.
    fn handle(
        &self,
        manager: &ModelManager,
        namespace: &str,
        level: Option<&str>,
        message: String,
    ) -> std::result::Result<(), Error> {
        if level == Some("error") {
            let err = illegal_model(message, self.location.clone());
            return Err(self.attach_file(manager, namespace, err));
        }
        Ok(())
    }

    /// TS: the outer `catch (err) { this.handleError(validationOptions.missingDecorator, err); }`:
    /// whatever the `try` block threw is reported again at the
    /// `missingDecorator` level, so it is thrown only when that level is
    /// `"error"`.
    ///
    /// BC-14 (R1): an `IllegalModelException` caught here (from
    /// [`Self::resolve_own_name`] or [`Self::handle`], already carrying its
    /// file) is thrown as it is. Any other error becomes an
    /// `IllegalModelException` with the caught error's own message and one
    /// file suffix. TS 5.0.0 built `new IllegalModelException(err, ...)` from
    /// the caught `Error` itself, so its message embedded
    /// `"IllegalModelException: "` and the caught error's own `File '<name>': `
    /// suffix, and then added the suffix a second time (DV-016).
    fn rethrow(
        &self,
        manager: &ModelManager,
        namespace: &str,
        level: Option<&str>,
        problem: Error,
    ) -> Result<()> {
        if level != Some("error") {
            return Ok(());
        }
        if problem.contract().kind == ErrorKind::IllegalModel {
            return Err(self.attach_file(manager, namespace, problem));
        }
        let message = js_message(&problem);
        let err = illegal_model(message, self.location.clone());
        Err(self.attach_file(manager, namespace, err))
    }
}

/// `new IllegalModelException(message, mf, location)`, built the way every
/// pre-port throw site in this crate is (`ContractError::pre_port`): this is
/// not a catalogue template, since the message was already assembled inline.
fn illegal_model(message: String, location: Option<Value>) -> Error {
    ContractError::pre_port(ErrorKind::IllegalModel, message, location).into()
}

/// The message an already-thrown error would report (what `ops.rs`'s
/// oracle harness reads off a [`Error`], `to_oracle_error`), for
/// [`Decorator::rethrow`] to carry into its `IllegalModelException`.
fn js_message(err: &Error) -> String {
    if let Some(type_name) = err.unported_type_not_found() {
        return format!("Type \"{type_name}\" not found.");
    }
    if let Some(message) = err.unported_illegal_model() {
        return message.to_string();
    }
    err.contract().final_message()
}

/// JS `typeof` of a decoded argument, as `Decorator.validate` reports it.
fn js_typeof(arg: &DecoratorArgument) -> &'static str {
    match arg {
        DecoratorArgument::Number(_) => "number",
        DecoratorArgument::String(_) => "string",
        DecoratorArgument::Boolean(_) => "boolean",
        DecoratorArgument::TypeReference(_) => "object",
    }
}

/// `JSON.stringify(arg)`, for the small set of shapes an argument can be.
fn json_stringify(arg: &DecoratorArgument) -> String {
    match arg {
        DecoratorArgument::String(s) => format!("{s:?}"),
        DecoratorArgument::Number(n) => number_to_string(*n),
        DecoratorArgument::Boolean(b) => b.to_string(),
        // `JSON.stringify` omits a property whose value is `undefined`.
        DecoratorArgument::TypeReference(t) => match t.array {
            Some(array) => format!(
                r#"{{"type":"Identifier","name":{:?},"array":{}}}"#,
                t.name, array
            ),
            None => format!(r#"{{"type":"Identifier","name":{:?}}}"#, t.name),
        },
    }
}

/// One `arguments[n]` node: a `DecoratorTypeReference`, or a literal whose
/// `value` is taken as it is. `None` for a node with no usable `value` (never
/// produced by a well-formed AST; skipped rather than failing the whole
/// decorator, since TS's `if (thing)` guard already tolerates a hole in a
/// sparse `arguments` array the same way).
fn decode_argument(node: &Value) -> Option<DecoratorArgument> {
    if node.is_null() {
        return None;
    }
    let class = node.get("$class").and_then(Value::as_str).unwrap_or("");
    if class == qualified_class("DecoratorTypeReference") || class == "DecoratorTypeReference" {
        let type_name = node.get("type")?.get("name")?.as_str()?.to_string();
        let array = node.get("isArray").and_then(Value::as_bool);
        return Some(DecoratorArgument::TypeReference(TypeReferenceArgument {
            name: type_name,
            array,
        }));
    }
    match node.get("value")? {
        Value::String(s) => Some(DecoratorArgument::String(s.clone())),
        Value::Number(n) => Some(DecoratorArgument::Number(n.as_f64()?)),
        Value::Bool(b) => Some(DecoratorArgument::Boolean(*b)),
        _ => None,
    }
}

/// [`decode_argument`], for a node the typed read kept as a [`Kept`]
/// (P5-76): the same argument as from its `Value`.
fn decode_kept_argument(node: &Kept) -> Option<DecoratorArgument> {
    fn string(value: Option<&Kept>) -> Option<&str> {
        match value {
            Some(Kept::Other(Value::String(s))) => Some(s.as_str()),
            _ => None,
        }
    }

    if matches!(node, Kept::Other(Value::Null)) {
        return None;
    }
    let class = string(node.get("$class")).unwrap_or("");
    if class == qualified_class("DecoratorTypeReference") || class == "DecoratorTypeReference" {
        let type_name = string(node.get("type")?.get("name"))?.to_string();
        let array = match node.get("isArray") {
            Some(Kept::Other(Value::Bool(b))) => Some(*b),
            _ => None,
        };
        return Some(DecoratorArgument::TypeReference(TypeReferenceArgument {
            name: type_name,
            array,
        }));
    }
    match node.get("value")? {
        Kept::Other(Value::String(s)) => Some(DecoratorArgument::String(s.clone())),
        Kept::Other(Value::Number(n)) => Some(DecoratorArgument::Number(n.as_f64()?)),
        Kept::Other(Value::Bool(b)) => Some(DecoratorArgument::Boolean(*b)),
        _ => None,
    }
}

/// The decorators found on an AST node's `decorators` value, or empty if it
/// has none: a test helper since every loader reads the node as a [`Kept`]
/// ([`parse_decorator_list`], A-10).
///
/// TS: `Decorated.process` (src/introspect/decorated.ts), the part that is
/// not about picking a `DecoratorFactory`'s decorator over the default (that
/// stays TS, module doc). The loader reads every `decorators` value strictly
/// first (the generated structs' `Option<Vec<Decorator>>`), so this only
/// ever sees an array of decorator nodes, or `null` (P5-61: before BC-19 it
/// also reproduced TS's iteration of a string by UTF-16 code unit, #218).
#[cfg(test)]
pub(crate) fn parse_decorators(ast: &Value) -> Vec<Decorator> {
    decorators_of(ast.get("decorators"))
}

/// [`parse_decorators`] given the node's `decorators` value itself (`None`
/// when the node has no such key).
#[cfg(test)]
pub(crate) fn decorators_of(decorators: Option<&Value>) -> Vec<Decorator> {
    match decorators {
        Some(Value::Array(items)) => items.iter().map(Decorator::from_ast).collect(),
        _ => Vec::new(),
    }
}

/// [`parse_decorators`] given the node's `decorators` value itself (`None`
/// when the node has no such key), for a loader that has read that value
/// on its own (the typed AST path, P5-06c), as a [`Kept`] (P5-76): the same
/// decorators as from its `Value`.
pub(crate) fn parse_decorator_list(decorators: Option<&Kept>) -> Vec<Decorator> {
    match decorators {
        Some(Kept::Array(items)) => items.iter().map(Decorator::from_kept).collect(),
        _ => Vec::new(),
    }
}

js_compat_pub! {
    /// The DV-018 error for a decorator node that is `value` (`null`, or
    /// `undefined` through the WASM boundary), with no location and no model
    /// file yet. Raised by the `decoratorProcess` binding, for a decorator
    /// view built outside a model load (a model load reads every decorator
    /// node strictly, so a `null` one there is a `modelfile-load-unreadable`
    /// error, P5-61).
    pub fn not_an_object(value: &str) -> ContractError {
        ContractError::new(
            ErrorKind::IllegalModel,
            "decorator-process-notobject",
            vec![("value", value.to_string())],
        )
    }
}

/// Wraps a generated metamodel node together with its processed decorators
/// (module doc). `Deref` gives access to every other field of `T` unchanged,
/// so a caller reading (say) `p.is_array` on a wrapped property still just
/// works.
#[derive(Debug, Clone)]
pub struct WithDecorators<T> {
    /// Held inline (P5-93; boxed from P5-76): a generated node is a few
    /// hundred bytes, but a box was one allocation per property, and the
    /// typed read now collects a declaration's properties into a `Vec` of
    /// exactly their number (`typed_ast`), so they are no longer moved
    /// element by element as it grows.
    node: T,
    decorators: Vec<Decorator>,
}

impl<T> WithDecorators<T> {
    pub(crate) fn new(node: T, decorators: Vec<Decorator>) -> Self {
        Self { node, decorators }
    }

    /// The decorators processed for this node.
    pub fn decorators(&self) -> &[Decorator] {
        &self.decorators
    }
}

impl<T> std::ops::Deref for WithDecorators<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.node
    }
}

/// How to validate decorators (TS `ModelManagerOptions.decoratorValidation`).
/// Both fields are `None` (JS `undefined`) by default, which is also how TS
/// leaves the whole check disabled (`DEFAULT_DECORATOR_VALIDATION`).
///
/// Only the exact string `"error"` is ever tested against here (matching
/// every test and fixture in this scope); any other non-empty string,
/// including `"warn"`, enables the check but never throws (module doc on
/// `Decorator::handle`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DecoratorValidationOptions {
    /// The log level for a decorator whose own name does not resolve.
    pub missing_decorator: Option<String>,
    /// The log level for an invalid argument count, type or type reference.
    pub invalid_decorator: Option<String>,
}

impl DecoratorValidationOptions {
    /// JS `validationOptions.missingDecorator || validationOptions.invalidDecorator`:
    /// truthy for any non-empty string, whichever field it is.
    pub fn is_enabled(&self) -> bool {
        let truthy = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.is_empty());
        truthy(&self.missing_decorator) || truthy(&self.invalid_decorator)
    }
}

/// An element that can carry decorators: a declaration, a property, or a
/// model file.
///
/// TS: `Decorated` (src/introspect/decorated.ts).
pub trait Decorated {
    /// The decorators attached to the element, in the order they are given.
    ///
    /// TS: `Decorated.getDecorators`.
    fn decorators(&self) -> &[Decorator];

    /// The decorator attached to the element with the given name, or `None`
    /// if it has none by that name.
    ///
    /// TS: `Decorated.getDecorator`.
    fn decorator(&self, name: &str) -> Option<&Decorator> {
        self.decorators().iter().find(|d| d.name() == name)
    }

    /// Deprecated name of [`Decorated::decorators`].
    #[deprecated(since = "0.1.0", note = "use `decorators`")]
    fn get_decorators(&self) -> &[Decorator] {
        self.decorators()
    }

    /// Deprecated name of [`Decorated::decorator`].
    #[deprecated(since = "0.1.0", note = "use `decorator`")]
    fn get_decorator(&self, name: &str) -> Option<&Decorator> {
        self.decorator(name)
    }
}

impl Decorated for ClassDeclaration {
    fn decorators(&self) -> &[Decorator] {
        ClassDeclaration::decorators(self)
    }
}

impl Decorated for crate::introspect::declaration::Declaration {
    fn decorators(&self) -> &[Decorator] {
        use crate::introspect::declaration::Declaration;
        match self {
            Declaration::Class(class) => Decorated::decorators(class),
            Declaration::Enum(enm) => Decorated::decorators(enm),
            Declaration::Scalar(scalar) => Decorated::decorators(scalar),
            Declaration::Map(map) => Decorated::decorators(map),
        }
    }
}

#[cfg(test)]
mod tests;
