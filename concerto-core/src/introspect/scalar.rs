//! Scalar declarations: a port of `scalardeclaration.ts` from the TypeScript
//! reference.
//!
//! A scalar is a named alias for a primitive, sometimes with a validator
//! attached. TS computes everything a scalar knows in `process()`, which the
//! declaration's constructor runs; `ScalarDeclaration::process` is the port
//! of that method (after `super.process()`, which belongs to `Declaration`),
//! and its result, `ProcessedScalar`, is what the getters read. The WASM
//! binding returns the same result to the TS view as its snapshot.

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use serde_json::Value;

#[cfg(feature = "js-compat")]
use crate::ecma;
use crate::error::{ContractError, ErrorKind};
use crate::introspect::decorator::{Decorated, Decorator};
use crate::introspect::kept::Kept;
#[cfg(feature = "js-compat")]
use crate::introspect::validators;
use crate::introspect::validators::{NumberValidator, StringValidator};
use crate::introspect::{DeclarationKind, FullyQualified, HasValidators, Named, Typed};
#[cfg(feature = "js-compat")]
use crate::model_manager::ResolutionContext;
use crate::model_manager::ValidatedElement;
use crate::model_util::is_primitive_type;

/// The metamodel namespace (`MetaModelNamespace`).
#[cfg(feature = "js-compat")]
const METAMODEL_NAMESPACE: &str = "concerto.metamodel@1.0.0";

/// The validator `ScalarDeclaration.process` attaches.
#[derive(Debug, Clone, PartialEq)]
pub enum ScalarValidator {
    /// `new NumberValidator(this, this.ast.validator)`, for Integer, Long and
    /// Double scalars.
    Number(NumberValidator),
    /// `new StringValidator(this, this.ast.validator, this.ast.lengthValidator)`,
    /// for String scalars: the validator as `ScalarDeclaration::process` builds
    /// (and so checks) it.
    String(StringValidator),
}

js_compat_pub! {
    /// What `ScalarDeclaration.process` computes.
    #[derive(Debug, Clone, PartialEq)]
    pub struct ProcessedScalar {
        /// `type`: the primitive the scalar aliases, from its exact `$class`, or
        /// `None` (JS `null`).
        pub scalar_type: Option<&'static str>,
        /// `validator`, or `None` (JS `null`).
        pub validator: Option<ScalarValidator>,
        /// `defaultValue`, or `None` (JS `null`) when the AST has none or a
        /// nullish one.
        pub default_value: Option<Value>,
    }
}

/// The scalar as the element its own `NumberValidator` is attached to: TS
/// passes `this`, so the validator reads `this.ast.defaultValue` and
/// `this.getFullyQualifiedName()`.
#[cfg(feature = "js-compat")]
struct ScalarElement<'a, E> {
    ast: &'a Value,
    fully_qualified_name: &'a dyn Fn() -> Result<String, E>,
}

#[cfg(feature = "js-compat")]
impl<E: From<ContractError>> FullyQualified for ScalarElement<'_, E> {
    type Error = E;

    fn fully_qualified_name(&self) -> Result<String, E> {
        (self.fully_qualified_name)()
    }
}

#[cfg(feature = "js-compat")]
impl<E: From<ContractError>> ValidatedElement for ScalarElement<'_, E> {
    fn default_value(&self) -> Result<Option<Value>, E> {
        Ok(self.ast.get("defaultValue").cloned())
    }

    fn name(&self) -> Result<String, E> {
        // `this.getName()`: a scalar's short name is its AST `name`.
        Ok(self
            .ast
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }
}

/// [`ScalarElement`] on the load path ([`ScalarDeclaration::process_loaded`]):
/// the scalar's node as the typed read keeps it, its name and its
/// fully-qualified name.
struct LoadedScalarElement<'a> {
    node: &'a Kept,
    name: &'a str,
    fully_qualified_name: &'a str,
}

impl FullyQualified for LoadedScalarElement<'_> {
    type Error = crate::error::Error;

    fn fully_qualified_name(&self) -> crate::error::Result<String> {
        Ok(self.fully_qualified_name.to_string())
    }
}

impl ValidatedElement for LoadedScalarElement<'_> {
    fn default_value(&self) -> crate::error::Result<Option<Value>> {
        Ok(self.node.get("defaultValue").map(Kept::to_value))
    }

    fn name(&self) -> crate::error::Result<String> {
        Ok(self.name.to_string())
    }
}

/// A scalar declaration loaded into a model file.
#[derive(Debug, Clone)]
pub struct ScalarDeclaration {
    node: mm::ScalarDeclaration,
    processed: ProcessedScalar,
    decorators: Vec<Decorator>,
}

impl ScalarDeclaration {
    /// The scalar's type, validator and default value from its AST, in TS
    /// order: primitive-name check, type, validator, default value.
    /// `fully_qualified_name` is called only if the validator reports an error.
    ///
    /// TS: ScalarDeclaration.process (src/introspect/scalardeclaration.ts)
    #[cfg(feature = "js-compat")]
    pub fn process<E: From<ContractError>>(
        ast: &Value,
        model_file_name: Option<&str>,
        fully_qualified_name: &dyn Fn() -> Result<String, E>,
    ) -> Result<ProcessedScalar, E> {
        // `ModelUtil.isPrimitiveType(this.getName())`: a name that is not a
        // string is never a primitive.
        if let Some(scalar_name) = ast.get("name").and_then(Value::as_str)
            && is_primitive_type(scalar_name)
        {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "scalardeclaration-process-primitivename",
                vec![("scalarName", scalar_name.to_string())],
            );
            err.model_file = Some(model_file_name.map(str::to_string));
            err.location = ast.get("location").cloned();
            return Err(err.into());
        }

        // The `$class` taken apart once, rather than formatting each
        // candidate's `$class` to compare it with.
        let primitive_of_class = ast.get("$class").and_then(Value::as_str).and_then(|class| {
            class
                .strip_prefix(METAMODEL_NAMESPACE)?
                .strip_prefix('.')?
                .strip_suffix("Scalar")
        });
        let scalar_type = ["Boolean", "Integer", "Long", "Double", "String", "DateTime"]
            .into_iter()
            .find(|primitive| primitive_of_class == Some(*primitive));

        let truthy = |key: &str| ast.get(key).is_some_and(ecma::is_truthy);
        let validator = match scalar_type {
            Some("Integer" | "Double" | "Long") if truthy("validator") => {
                let element = ScalarElement {
                    ast,
                    fully_qualified_name,
                };
                let validator_ast = ast.get("validator").unwrap_or(&Value::Null);
                Some(ScalarValidator::Number(NumberValidator::new(
                    &element,
                    validator_ast,
                )?))
            }
            Some("String") if truthy("validator") || truthy("lengthValidator") => {
                // TS: `this.validator = new StringValidator(this, this.ast.validator,
                // this.ast.lengthValidator)`, built eagerly like the
                // `NumberValidator` arm above, so this scalar's own
                // `defaultValue` is checked here, as TS checks it in the
                // constructor.
                let element = ScalarElement {
                    ast,
                    fully_qualified_name,
                };
                // On the model-file load path, `declaration::load_scalar` has
                // already read this node strictly (BR-09), so
                // `validator`/`lengthValidator` are well-formed here and a
                // wrongly-typed one never reaches this point.
                // `validators::regex_validator_from_ast`/`length_validator_from_ast`
                // read the raw AST untyped, as TS's `StringValidator` constructor
                // does, only because `process` also runs on ASTs that bypass the
                // loader: `build_standalone` (a `new ScalarDeclaration(modelFile,
                // ast)` never added to its file) and concerto-wasm's standalone
                // bindings.
                let validator = validators::regex_validator_from_ast(ast.get("validator"));
                let length_validator =
                    validators::length_validator_from_ast(ast.get("lengthValidator"));
                Some(ScalarValidator::String(StringValidator::new(
                    &element,
                    validator.as_ref(),
                    length_validator.as_ref(),
                    ast.get("lengthValidator"),
                )?))
            }
            _ => None,
        };

        // `!Util.isNull(this.ast.defaultValue)`.
        let default_value = match ast.get("defaultValue") {
            None | Some(Value::Null) => None,
            Some(value) => Some(value.clone()),
        };

        Ok(ProcessedScalar {
            scalar_type,
            validator,
            default_value,
        })
    }

    /// [`ScalarDeclaration::process`] on the model-file load path, over the
    /// typed `node`, with `kept` only for what TS reads off the raw AST
    /// (`location`, `defaultValue`, raw `lengthValidator` bounds). Checks,
    /// order and errors are `process`'s; the validators are built from the
    /// typed nodes.
    pub(crate) fn process_loaded(
        node: &mm::ScalarDeclaration,
        kept: &Kept,
        model_file_name: Option<&str>,
        fully_qualified_name: &str,
    ) -> crate::error::Result<ProcessedScalar> {
        let name = node_name(node);
        if is_primitive_type(name) {
            let mut err = ContractError::new(
                ErrorKind::IllegalModel,
                "scalardeclaration-process-primitivename",
                vec![("scalarName", name.to_string())],
            );
            err.model_file = Some(model_file_name.map(str::to_string));
            err.location = kept.get("location").map(Kept::to_value);
            return Err(err.into());
        }
        let element = LoadedScalarElement {
            node: kept,
            name,
            fully_qualified_name,
        };
        let validator = match node {
            mm::ScalarDeclaration::IntegerScalar(mm::IntegerScalar {
                validator: Some(v), ..
            }) => Some((v.lower, v.upper)),
            mm::ScalarDeclaration::LongScalar(mm::LongScalar {
                validator: Some(v), ..
            }) => Some((v.lower, v.upper)),
            mm::ScalarDeclaration::DoubleScalar(mm::DoubleScalar {
                validator: Some(v), ..
            }) => Some((v.lower, v.upper)),
            _ => None,
        }
        .map(|(lower, upper)| {
            NumberValidator::from_bounds(&element, lower, upper).map(ScalarValidator::Number)
        })
        .transpose()?;
        let validator = match node {
            mm::ScalarDeclaration::StringScalar(string)
                if string.validator.is_some() || string.length_validator.is_some() =>
            {
                // TS: `this.validator = new StringValidator(this,
                // this.ast.validator, this.ast.lengthValidator)`.
                let raw_length = kept.get("lengthValidator").map(Kept::to_value);
                Some(ScalarValidator::String(StringValidator::new(
                    &element,
                    string.validator.as_ref(),
                    string.length_validator.as_ref(),
                    raw_length.as_ref(),
                )?))
            }
            _ => validator,
        };
        // `!Util.isNull(this.ast.defaultValue)`.
        let default_value = kept
            .get("defaultValue")
            .filter(|value| !value.is_null())
            .map(Kept::to_value);
        Ok(ProcessedScalar {
            scalar_type: Some(scalar_type_of(node)),
            validator,
            default_value,
        })
    }

    /// Wraps a loaded node with what [`ScalarDeclaration::process`] computed
    /// from the same AST, and its processed decorators (module doc on
    /// [`crate::introspect::decorator::WithDecorators`]; a scalar keeps them
    /// as a plain field rather than that wrapper, since it already wraps its
    /// own `node`).
    pub(crate) fn new(
        node: mm::ScalarDeclaration,
        processed: ProcessedScalar,
        decorators: Vec<Decorator>,
    ) -> Self {
        Self {
            node,
            processed,
            decorators,
        }
    }

    /// The decorators attached to this declaration.
    ///
    /// TS: `Decorated.getDecorators` (src/introspect/decorated.ts).
    pub fn decorators(&self) -> &[Decorator] {
        &self.decorators
    }

    /// Validates a scalar declaration's AST as TS `new
    /// ScalarDeclaration(modelFile, ast)` does, and returns its fully qualified
    /// name. Unlike the loader, it reads any AST as `serde_json::Value`
    /// (PORTING.md 1.2), for the oracle's `ScalarDeclaration.new` fixtures and
    /// hand-built test ASTs.
    #[cfg(feature = "js-compat")]
    pub fn validate_new(
        namespace: &str,
        file_name: Option<&str>,
        ast: &Value,
    ) -> crate::error::Result<String> {
        Self::build_standalone(namespace, file_name, ast).map(|(fqn, _)| fqn)
    }

    /// [`ScalarDeclaration::validate_new`], also returning what
    /// [`ScalarDeclaration::process`] computed, for `getType`, `getValidator` or
    /// `getDefaultValue` on a scalar never added to its model file (the
    /// oracle's `declnew` fixtures).
    #[cfg(feature = "js-compat")]
    pub fn build_standalone(
        namespace: &str,
        file_name: Option<&str>,
        ast: &Value,
    ) -> crate::error::Result<(String, ProcessedScalar)> {
        let fqn = crate::model_util::qualify(
            namespace,
            ast.get("name").and_then(Value::as_str).unwrap_or_default(),
        );
        // `process` builds (and so validates) the scalar's `StringValidator`
        // and `NumberValidator` eagerly, so there is nothing left to check.
        let processed = Self::process::<crate::error::Error>(ast, file_name, &|| Ok(fqn.clone()))?;
        Ok((fqn, processed))
    }

    /// The generated metamodel node.
    pub fn ast(&self) -> &mm::ScalarDeclaration {
        &self.node
    }

    /// The primitive type this scalar aliases, read from the loaded node:
    /// `Boolean`, `Integer`, `Long`, `Double`, `String` or `DateTime`.
    ///
    /// [`Typed::type_name`] is the TS `ScalarDeclaration.getType`, which is
    /// `None` (JS `null`) when the AST's `$class` is not one of the six
    /// fully-qualified scalar classes.
    pub fn scalar_type(&self) -> &'static str {
        scalar_type_of(&self.node)
    }

    /// The primitive type, or `None` (JS `null`) when the AST's `$class` is
    /// not one of the six fully-qualified scalar classes.
    ///
    /// TS: ScalarDeclaration.getType (src/introspect/scalardeclaration.ts)
    pub(crate) fn processed_type(&self) -> Option<&'static str> {
        self.processed.scalar_type
    }

    /// The validator, or `None` (JS `null`).
    ///
    /// TS: ScalarDeclaration.getValidator (src/introspect/scalardeclaration.ts)
    pub fn validator(&self) -> Option<&ScalarValidator> {
        self.processed.validator.as_ref()
    }

    /// The default value, or `None` (JS `null`).
    ///
    /// TS: ScalarDeclaration.getDefaultValue (src/introspect/scalardeclaration.ts)
    pub fn default_value(&self) -> Option<&Value> {
        self.processed.default_value.as_ref()
    }

    /// `ScalarDeclaration {id=<fully qualified name>}`.
    ///
    /// TS: ScalarDeclaration.toString (src/introspect/scalardeclaration.ts)
    #[cfg(feature = "js-compat")]
    pub fn to_string(fully_qualified_name: &str) -> String {
        format!("ScalarDeclaration {{id={fully_qualified_name}}}")
    }

    /// The scalar's own semantic check, run after `Declaration.validate`: no
    /// two declarations of its model file may share a fully qualified name.
    ///
    /// TS: ScalarDeclaration.validate (src/introspect/scalardeclaration.ts)
    #[cfg(feature = "js-compat")]
    pub fn validate<C: ResolutionContext>(ctx: &C, declaration: &C::Node) -> Result<(), C::Error> {
        let model_file = ctx.get_model_file(declaration)?;
        let declarations = ctx.get_all_declarations(&model_file)?;
        let names = declarations
            .iter()
            .map(|d| ctx.get_fully_qualified_name(d))
            .collect::<Result<Vec<_>, _>>()?;
        // The first name equal to an earlier one is `duplicateElements[0]`.
        // The set is only probed, never iterated.
        let mut seen = crate::hash::SeededHashSet::default();
        if let Some(duplicate) = names.iter().find(|name| !seen.insert(name.as_str())) {
            return Err(ContractError::new(
                ErrorKind::IllegalModel,
                "scalardeclaration-validate-duplicateclassname",
                vec![("name", duplicate.clone())],
            )
            .into());
        }
        Ok(())
    }
}

/// The short name of a generated scalar node.
/// The primitive type a scalar node aliases, by its variant.
fn scalar_type_of(node: &mm::ScalarDeclaration) -> &'static str {
    match node {
        mm::ScalarDeclaration::BooleanScalar(_) => "Boolean",
        mm::ScalarDeclaration::IntegerScalar(_) => "Integer",
        mm::ScalarDeclaration::LongScalar(_) => "Long",
        mm::ScalarDeclaration::DoubleScalar(_) => "Double",
        mm::ScalarDeclaration::StringScalar(_) => "String",
        mm::ScalarDeclaration::DateTimeScalar(_) => "DateTime",
    }
}

pub(crate) fn node_name(node: &mm::ScalarDeclaration) -> &str {
    match node {
        mm::ScalarDeclaration::BooleanScalar(s) => &s.name,
        mm::ScalarDeclaration::IntegerScalar(s) => &s.name,
        mm::ScalarDeclaration::LongScalar(s) => &s.name,
        mm::ScalarDeclaration::DoubleScalar(s) => &s.name,
        mm::ScalarDeclaration::StringScalar(s) => &s.name,
        mm::ScalarDeclaration::DateTimeScalar(s) => &s.name,
    }
}

impl Named for ScalarDeclaration {
    /// The scalar's short name.
    fn name(&self) -> &str {
        node_name(&self.node)
    }
}

impl ScalarDeclaration {
    /// The scalar's short name (without namespace).
    ///
    /// TS: Declaration.getName (src/introspect/declaration.ts)
    pub fn name(&self) -> &str {
        Named::name(self)
    }

    /// The metamodel `$class` short name of the loaded node, such as
    /// `StringScalar`.
    pub fn declaration_kind(&self) -> &'static str {
        DeclarationKind::declaration_kind(self)
    }
}

impl Decorated for ScalarDeclaration {
    fn decorators(&self) -> &[Decorator] {
        self.decorators()
    }
}

impl DeclarationKind for ScalarDeclaration {
    /// The metamodel `$class` short name of the loaded node, e.g.
    /// `StringScalar`.
    fn declaration_kind(&self) -> &'static str {
        match &self.node {
            mm::ScalarDeclaration::BooleanScalar(_) => "BooleanScalar",
            mm::ScalarDeclaration::IntegerScalar(_) => "IntegerScalar",
            mm::ScalarDeclaration::LongScalar(_) => "LongScalar",
            mm::ScalarDeclaration::DoubleScalar(_) => "DoubleScalar",
            mm::ScalarDeclaration::StringScalar(_) => "StringScalar",
            mm::ScalarDeclaration::DateTimeScalar(_) => "DateTimeScalar",
        }
    }
}

impl Typed for ScalarDeclaration {
    /// The primitive type, or `None` (JS `null`) when the AST's `$class` is
    /// not one of the six fully-qualified scalar classes.
    ///
    /// TS: ScalarDeclaration.getType (src/introspect/scalardeclaration.ts)
    fn type_name(&self) -> Option<&str> {
        self.processed_type()
    }
}

impl HasValidators for ScalarDeclaration {
    /// A no-op: both a Number and a String scalar's validator are built (and
    /// so checked) eagerly by `ScalarDeclaration::process`, as TS's
    /// constructor does. Kept because the loader (`introspect::declaration`)
    /// calls it and it is public API.
    fn check_validators(&self) -> crate::error::Result<()> {
        Ok(())
    }
}

/// Ported TS tests (PORTING.md 6.1). `scalardeclaration.js` is this unit's
/// own test file; `#accept`, `#getName`, `#getNamespace` and every constant
/// marker (`#isIdentified`, `#isAsset`, …) belong to `Declaration` and are
/// tested there. `scalars.js` tests only `Field.getScalarField` and
/// `isTypeScalar`, which belong to `Field`.
#[cfg(test)]
#[allow(clippy::result_large_err)]
// ContractError is fine as a by-value test Err; production code always boxes it in an `Error`.
mod tests;
