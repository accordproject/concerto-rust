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
//! The generated [`mm::Decorator`] is not faithful enough to read: its
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
//! [`ModelFile`] have room for the same `Vec<Decorator>` as an ordinary field.

use serde_json::Value;

use crate::ecma::number_to_string;
use crate::error::{ConcertoError, ContractError, ErrorKind, Result};
use crate::introspect::declaration::ClassDeclaration;
use crate::introspect::property::Property;
use crate::introspect::{Named, Typed, qualified_class};
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
    name: String,
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
        let name = ast
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let arguments = ast
            .get("arguments")
            .and_then(Value::as_array)
            .map(|items| items.iter().filter_map(decode_argument).collect())
            .unwrap_or_default();
        Decorator {
            name,
            arguments,
            location: ast.get("location").cloned(),
        }
    }

    /// The name of this decorator.
    ///
    /// TS: `Decorator.getName`.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The arguments given to this decorator, in order.
    ///
    /// TS: `Decorator.getArguments`.
    pub fn arguments(&self) -> &[DecoratorArgument] {
        &self.arguments
    }

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

    /// The body of the `try` block in TS `Decorator.validate`.
    fn try_validate(
        &self,
        manager: &ModelManager,
        namespace: &str,
        context: Option<&str>,
        options: &DecoratorValidationOptions,
    ) -> std::result::Result<(), ConcertoError> {
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
        // (`ModelManager::get_all_properties`); only the property is read here.
        let all_properties: Vec<Property> = manager
            .get_all_properties(&fqn)?
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
    ) -> std::result::Result<String, ConcertoError> {
        if is_primitive_type(&self.name) {
            return Ok(self.name.clone());
        }
        manager
            .resolve_type_name(namespace, &self.name, self.location.clone())
            .map_err(|_| {
                let err: ConcertoError = ContractError::new(
                    ErrorKind::IllegalModel,
                    "modelfile-resolvetype-undecltype",
                    vec![
                        ("type", self.name.clone()),
                        ("context", context.unwrap_or("undefined").to_string()),
                    ],
                )
                .into();
                // DV-016: TS `mf.resolveType(...)` throws `new IllegalModelException(
                // message, this, fileLocation)` — `this` is the model file `mf` is
                // called on, so the "File '<name>': " suffix is already part of this
                // error's own message by the time `Decorator.validate`'s `catch`
                // re-wraps it. Reproducing that requires this error to carry its own
                // model file *now*, not only once the caller backstops it later.
                self.attach_file(manager, namespace, err)
            })
    }

    /// `manager.model_file(namespace)`, attached to `err` the way
    /// [`crate::validation::attach_model_file`] backstops every other check in
    /// this module — reused here (rather than deferring to that backstop) so
    /// an error this module builds already carries its file *before*
    /// [`Self::rethrow`] re-reports it, matching TS's `this`/`this.getParent().
    /// getModelFile()`, which is resolved synchronously at each throw site
    /// (DV-016).
    fn attach_file(
        &self,
        manager: &ModelManager,
        namespace: &str,
        err: ConcertoError,
    ) -> ConcertoError {
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
    ) -> std::result::Result<(), ConcertoError> {
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
    ) -> std::result::Result<(), ConcertoError> {
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
            .resolve_type_name(namespace, &type_reference.name, None)
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
                    .resolve_type_name(namespace, declared_type, None)
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
    ) -> std::result::Result<(), ConcertoError> {
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
    /// this.ast.location)` — the file is attached right here, at construction
    /// (DV-016), not only once the caller backstops it later: that is what
    /// makes [`Self::rethrow`]'s own double-wrap carry the file suffix
    /// twice, exactly as TS's does.
    fn handle(
        &self,
        manager: &ModelManager,
        namespace: &str,
        level: Option<&str>,
        message: String,
    ) -> std::result::Result<(), ConcertoError> {
        if level == Some("error") {
            let err = illegal_model(message, self.location.clone());
            return Err(self.attach_file(manager, namespace, err));
        }
        Ok(())
    }

    /// TS: the outer `catch (err) { this.handleError(validationOptions.missingDecorator, err); }`.
    ///
    /// Every error caught here was itself thrown as an `IllegalModelException`
    /// — either by [`Self::resolve_own_name`], or by [`Self::handle`] a
    /// moment ago — so re-reporting it constructs a *new* `IllegalModelException`
    /// whose message is the caught one, coerced to a string the way a JS
    /// template literal coerces an `Error`: `"<name>: <message>"`. This is a
    /// real double-wrap in the TS reference (`new IllegalModelException(err, ...)`
    /// with `err` an `Error`, not a string), not a simplification. That caught
    /// error's own message already carries its own "File '…': " suffix
    /// (`Self::handle`/`Self::resolve_own_name` attach it eagerly, same as
    /// TS), and this rethrow attaches the *same* file again to its own new
    /// exception — so a caller with `missingDecorator: "error"` sees the
    /// suffix twice over, faithfully (DV-016, DIVERGENCES.md).
    fn rethrow(
        &self,
        manager: &ModelManager,
        namespace: &str,
        level: Option<&str>,
        problem: ConcertoError,
    ) -> Result<()> {
        if level == Some("error") {
            let (class, message) = js_class_and_message(&problem);
            let err = illegal_model(format!("{class}: {message}"), self.location.clone());
            return Err(self.attach_file(manager, namespace, err));
        }
        Ok(())
    }
}

/// `new IllegalModelException(message, mf, location)`, built the way every
/// pre-port throw site in this crate is (`ContractError::pre_port`): this is
/// not a catalogue template, since the message was already assembled inline.
fn illegal_model(message: String, location: Option<Value>) -> ConcertoError {
    ContractError::pre_port(ErrorKind::IllegalModel, message, location).into()
}

/// The TS class name and message an already-thrown error would report, the
/// same two fields `ops.rs`'s oracle harness reads off a [`ConcertoError`]
/// (`to_oracle_error`), needed here to reproduce [`Decorator::rethrow`]'s
/// string coercion of a caught exception.
fn js_class_and_message(err: &ConcertoError) -> (&'static str, String) {
    match err {
        ConcertoError::Contract(ce) => (ce.kind.ts_class(), ce.final_message()),
        ConcertoError::TypeNotFound { type_name } => (
            ErrorKind::TypeNotFound.ts_class(),
            format!("Type \"{type_name}\" not found."),
        ),
        ConcertoError::IllegalModel { message, .. } => {
            (ErrorKind::IllegalModel.ts_class(), message.clone())
        }
    }
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

/// The decorators found on a raw AST node's `decorators` array, or empty if
/// it has none.
///
/// TS: `Decorated.process` (src/introspect/decorated.ts), the part that is
/// not about picking a `DecoratorFactory`'s decorator over the default (that
/// stays TS, module doc).
pub(crate) fn parse_decorators(ast: &Value) -> Vec<Decorator> {
    ast.get("decorators")
        .and_then(Value::as_array)
        .map(|items| items.iter().map(Decorator::from_ast).collect())
        .unwrap_or_default()
}

/// Wraps a generated metamodel node together with its processed decorators
/// (module doc). `Deref` gives access to every other field of `T` unchanged,
/// so a caller reading (say) `p.is_array` on a wrapped property still just
/// works.
#[derive(Debug, Clone)]
pub struct WithDecorators<T> {
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
/// [`Decorator::handle`]).
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
    fn get_decorators(&self) -> &[Decorator];

    /// The decorator attached to the element with the given name, or `None`
    /// if it has none by that name.
    ///
    /// TS: `Decorated.getDecorator`.
    fn get_decorator(&self, name: &str) -> Option<&Decorator> {
        self.get_decorators().iter().find(|d| d.name() == name)
    }
}

impl Decorated for ClassDeclaration {
    fn get_decorators(&self) -> &[Decorator] {
        ClassDeclaration::decorators(self)
    }
}

impl Decorated for crate::introspect::declaration::Declaration {
    fn get_decorators(&self) -> &[Decorator] {
        use crate::introspect::declaration::Declaration;
        match self {
            Declaration::Class(class) => class.get_decorators(),
            Declaration::Enum(enm) => enm.get_decorators(),
            Declaration::Scalar(scalar) => scalar.get_decorators(),
            Declaration::Map(map) => map.get_decorators(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decorator(json: Value) -> Decorator {
        Decorator::from_ast(&json)
    }

    fn ast(name: &str, arguments: Value) -> Value {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Decorator",
            "name": name,
            "arguments": arguments
        })
    }

    fn string_arg(value: &str) -> Value {
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.DecoratorString", "value": value })
    }

    fn number_arg(value: f64) -> Value {
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.DecoratorNumber", "value": value })
    }

    fn boolean_arg(value: bool) -> Value {
        serde_json::json!({ "$class": "concerto.metamodel@1.0.0.DecoratorBoolean", "value": value })
    }

    fn type_ref_arg(name: &str, array: bool) -> Value {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.DecoratorTypeReference",
            "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": name },
            "isArray": array
        })
    }

    // TS: Decorator #constructor "should store values".
    #[test]
    fn stores_name_and_string_arguments() {
        let d = decorator(ast(
            "Test",
            serde_json::json!([string_arg("one"), string_arg("two"), string_arg("three")]),
        ));
        assert_eq!(d.name(), "Test");
        assert_eq!(
            d.arguments(),
            &[
                DecoratorArgument::String("one".into()),
                DecoratorArgument::String("two".into()),
                DecoratorArgument::String("three".into()),
            ]
        );
    }

    #[test]
    fn no_arguments_field_gives_an_empty_list() {
        let d = Decorator::from_ast(&serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.Decorator",
            "name": "noargs"
        }));
        assert!(d.arguments().is_empty());
    }

    // TS: Decorators #grammar covers every literal kind and a type reference,
    // array and non-array, plus a boolean literal where a type reference is
    // written in the CTO source (`@returns(true)`).
    #[test]
    fn reads_every_argument_kind() {
        let d = decorator(ast(
            "all",
            serde_json::json!([
                string_arg("foo"),
                number_arg(1.0),
                number_arg(-1.0),
                number_arg(10.2),
                number_arg(-10.2),
                boolean_arg(false),
                boolean_arg(true),
            ]),
        ));
        assert_eq!(
            d.arguments(),
            &[
                DecoratorArgument::String("foo".into()),
                DecoratorArgument::Number(1.0),
                DecoratorArgument::Number(-1.0),
                DecoratorArgument::Number(10.2),
                DecoratorArgument::Number(-10.2),
                DecoratorArgument::Boolean(false),
                DecoratorArgument::Boolean(true),
            ]
        );

        let non_array = decorator(ast(
            "returns",
            serde_json::json!([type_ref_arg("MyConcept", false)]),
        ));
        assert_eq!(
            non_array.arguments(),
            &[DecoratorArgument::TypeReference(TypeReferenceArgument {
                name: "MyConcept".into(),
                array: Some(false)
            })]
        );

        let array = decorator(ast(
            "returns",
            serde_json::json!([type_ref_arg("MyConcept", true)]),
        ));
        assert_eq!(
            array.arguments(),
            &[DecoratorArgument::TypeReference(TypeReferenceArgument {
                name: "MyConcept".into(),
                array: Some(true)
            })]
        );

        let boolean_where_identifier_expected =
            decorator(ast("returns", serde_json::json!([boolean_arg(true)])));
        assert_eq!(
            boolean_where_identifier_expected.arguments(),
            &[DecoratorArgument::Boolean(true)]
        );
    }

    #[test]
    fn a_short_class_is_accepted_for_the_type_reference() {
        let d = decorator(ast(
            "returns",
            serde_json::json!([{
                "$class": "DecoratorTypeReference",
                "type": { "$class": "concerto.metamodel@1.0.0.TypeIdentifier", "name": "String" },
                "isArray": false
            }]),
        ));
        assert_eq!(
            d.arguments(),
            &[DecoratorArgument::TypeReference(TypeReferenceArgument {
                name: "String".into(),
                array: Some(false)
            })]
        );
    }

    fn manager_with(cto_declarations: Value) -> ModelManager {
        let mut manager = ModelManager::new().expect("system models load");
        manager
            .add_model(
                &serde_json::json!({
                    "$class": "concerto.metamodel@1.0.0.Model",
                    "namespace": "org.acme@1.0.0",
                    "declarations": cto_declarations
                }),
                None,
            )
            .expect("model loads");
        manager
    }

    fn decorated_concept(name: &str, decorators: Value) -> Value {
        serde_json::json!({
            "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
            "name": name,
            "isAbstract": false,
            "decorators": decorators,
            "properties": []
        })
    }

    /// Disabled by default (`DEFAULT_DECORATOR_VALIDATION`): even a decorator
    /// whose name resolves nowhere at all is accepted without a manager
    /// opting in, matching every "should validate" test in `decorators.js`
    /// that never sets `decoratorValidation`.
    #[test]
    fn validate_is_a_no_op_when_disabled() {
        let manager = manager_with(serde_json::json!([decorated_concept(
            "Car",
            serde_json::json!([{ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "category", "arguments": [] }])
        )]));
        let decl = manager.get_declaration("org.acme@1.0.0.Car").unwrap();
        let decorator = decl.get_decorator("category").unwrap();
        assert!(
            decorator
                .validate(&manager, "org.acme@1.0.0", Some("org.acme@1.0.0.Car"))
                .is_ok()
        );
    }

    /// TS `decorators.js` "#validate should fail to validate type refs that
    /// are not defined locally": `missingDecorator: 'error'`, a decorator
    /// whose own name ("category") is undeclared anywhere, double-wrapped
    /// into a message that still contains the inner `IllegalModelException:
    /// Undeclared type` text (module doc on `Decorator::rethrow`).
    #[test]
    fn missing_decorator_error_reports_the_undeclared_type_wrapped_once() {
        let mut manager = manager_with(serde_json::json!([decorated_concept(
            "Car",
            serde_json::json!([{ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "category", "arguments": [] }])
        )]));
        manager.set_decorator_validation(DecoratorValidationOptions {
            missing_decorator: Some("error".into()),
            invalid_decorator: None,
        });
        let decl = manager.get_declaration("org.acme@1.0.0.Car").unwrap();
        let decorator = decl.get_decorator("category").unwrap();
        let err = decorator
            .validate(&manager, "org.acme@1.0.0", Some("org.acme@1.0.0.Car"))
            .unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("IllegalModelException: Undeclared type"),
            "{message}"
        );
    }

    /// The same failure with `missingDecorator` left off: the error is
    /// logged, not thrown (module doc on `Decorator::handle`).
    #[test]
    fn missing_decorator_off_is_silent() {
        let mut with_invalid_only = manager_with(serde_json::json!([decorated_concept(
            "Car",
            serde_json::json!([{ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "category", "arguments": [] }])
        )]));
        with_invalid_only.set_decorator_validation(DecoratorValidationOptions {
            missing_decorator: None,
            invalid_decorator: Some("error".into()),
        });
        let decl = with_invalid_only
            .get_declaration("org.acme@1.0.0.Car")
            .unwrap();
        let decorator = decl.get_decorator("category").unwrap();
        assert!(
            decorator
                .validate(
                    &with_invalid_only,
                    "org.acme@1.0.0",
                    Some("org.acme@1.0.0.Car")
                )
                .is_ok()
        );
    }

    /// A decorator whose name resolves to a real declaration: too few
    /// arguments is reported through `invalidDecorator`. `missingDecorator`
    /// must *also* be `'error'` for the throw to reach the caller: TS wraps
    /// the whole check in one `try`/`catch`, so an `invalidDecorator` throw
    /// is itself caught and re-reported through `missingDecorator` (module
    /// doc on `Decorator::rethrow`) — with `missingDecorator` off, the same
    /// failure is only logged (`missing_decorator_off_is_silent` covers
    /// exactly that half of this behaviour, for the resolve-own-name case).
    #[test]
    fn too_few_arguments_is_reported_through_invalid_decorator() {
        let mut manager = manager_with(serde_json::json!([
            decorated_concept("Marker", serde_json::json!([])),
            {
                "$class": "concerto.metamodel@1.0.0.ConceptDeclaration",
                "name": "Category",
                "isAbstract": false,
                "properties": [
                    { "$class": "concerto.metamodel@1.0.0.StringProperty", "name": "value", "isArray": false, "isOptional": false }
                ]
            },
            decorated_concept(
                "Car",
                serde_json::json!([{ "$class": "concerto.metamodel@1.0.0.Decorator", "name": "Category", "arguments": [] }])
            ),
        ]));
        manager.set_decorator_validation(DecoratorValidationOptions {
            missing_decorator: Some("error".into()),
            invalid_decorator: Some("error".into()),
        });
        let decl = manager.get_declaration("org.acme@1.0.0.Car").unwrap();
        let decorator = decl.get_decorator("Category").unwrap();
        let err = decorator
            .validate(&manager, "org.acme@1.0.0", Some("org.acme@1.0.0.Car"))
            .unwrap_err();
        assert!(err.to_string().contains("too few arguments"), "{err}");
    }
}
