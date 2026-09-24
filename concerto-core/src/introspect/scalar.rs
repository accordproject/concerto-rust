//! Scalar declarations: a port of `scalardeclaration.ts` from the TypeScript
//! reference.
//!
//! A scalar is a named alias for a primitive, sometimes with a validator
//! attached. TS computes everything a scalar knows in `process()`, which the
//! declaration's constructor runs; [`ScalarDeclaration::process`] is the port
//! of that method (after `super.process()`, which belongs to `Declaration`),
//! and its result, [`ProcessedScalar`], is what the getters read. The WASM
//! binding returns the same result to the TS view as its snapshot.

use std::collections::HashSet;

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;
use serde_json::Value;

use crate::ecma;
use crate::error::{ContractError, ErrorKind};
use crate::introspect::validators::NumberValidator;
use crate::introspect::{
    DeclarationKind, FullyQualified, HasValidators, Named, Typed, check_length, check_pattern,
};
use crate::model_manager::{ResolutionContext, ValidatedElement};
use crate::model_util::is_primitive_type;

/// The metamodel namespace (`MetaModelNamespace`).
const METAMODEL_NAMESPACE: &str = "concerto.metamodel@1.0.0";

/// The validator `ScalarDeclaration.process` attaches.
#[derive(Debug, Clone, PartialEq)]
pub enum ScalarValidator {
    /// `new NumberValidator(this, this.ast.validator)`, for Integer, Long and
    /// Double scalars.
    Number(NumberValidator),
    /// `new StringValidator(this, this.ast.validator, this.ast.lengthValidator)`,
    /// for String scalars. `StringValidator` is ported in P2-02; until then the
    /// port records the arguments TS passes (`None` is `undefined`) and the
    /// caller builds the validator: the loader runs its own checks, and the
    /// WASM view builds the TS `StringValidator`.
    String {
        /// `this.ast.validator`.
        validator: Option<Value>,
        /// `this.ast.lengthValidator`.
        length_validator: Option<Value>,
    },
}

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

/// The scalar as the element its own `NumberValidator` is attached to: TS
/// passes `this`, so the validator reads `this.ast.defaultValue` and
/// `this.getFullyQualifiedName()`.
struct ScalarElement<'a, E> {
    ast: &'a Value,
    fully_qualified_name: &'a dyn Fn() -> Result<String, E>,
}

impl<E: From<ContractError>> FullyQualified for ScalarElement<'_, E> {
    type Error = E;

    fn fully_qualified_name(&self) -> Result<String, E> {
        (self.fully_qualified_name)()
    }
}

impl<E: From<ContractError>> ValidatedElement for ScalarElement<'_, E> {
    fn default_value(&self) -> Result<Option<Value>, E> {
        Ok(self.ast.get("defaultValue").cloned())
    }
}

/// A scalar declaration loaded into a model file.
#[derive(Debug, Clone)]
pub struct ScalarDeclaration {
    node: mm::ScalarDeclaration,
    processed: ProcessedScalar,
}

impl ScalarDeclaration {
    /// Computes the scalar's type, validator and default value from its AST,
    /// in the TS order: the primitive-name check, the type, the validator
    /// (whose constructor may fail), then the default value.
    ///
    /// `model_file_name` is `modelFile.getName()` of the file TS passes to the
    /// exception; `fully_qualified_name` is `this.getFullyQualifiedName()`,
    /// called only if the validator reports an error.
    ///
    /// TS: ScalarDeclaration.process (src/introspect/scalardeclaration.ts)
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

        let class = ast.get("$class").and_then(Value::as_str);
        let scalar_type = ["Boolean", "Integer", "Long", "Double", "String", "DateTime"]
            .into_iter()
            .find(|primitive| {
                class == Some(format!("{METAMODEL_NAMESPACE}.{primitive}Scalar").as_str())
            });

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
                Some(ScalarValidator::String {
                    validator: ast.get("validator").cloned(),
                    length_validator: ast.get("lengthValidator").cloned(),
                })
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

    /// Wraps a loaded node with what [`ScalarDeclaration::process`] computed
    /// from the same AST.
    pub(crate) fn new(node: mm::ScalarDeclaration, processed: ProcessedScalar) -> Self {
        Self { node, processed }
    }

    /// Validates a scalar declaration's AST the way TS
    /// `new ScalarDeclaration(modelFile, ast)` does, and returns its fully
    /// qualified name (`Declaration`'s constructor: `super(ast); this.modelFile
    /// = modelFile; this.process();`, where `super(ast)` only stores the AST
    /// and reads `ast.name`).
    ///
    /// This does not build a [`ScalarDeclaration`]: unlike the loader
    /// (`introspect::declaration`), which only ever sees an AST the metamodel
    /// crate's generated `mm::ScalarDeclaration` accepts, this runs over
    /// whatever AST the caller has, including one with no `$class` a real
    /// scalar carries (PORTING.md 1.2, OD-3: "a member that TS runs over any
    /// JS object reads the AST as `serde_json::Value`", exactly like
    /// [`ScalarDeclaration::process`]) — the oracle harness replays
    /// `ScalarDeclaration.new` fixtures recorded from a `ModelFile` built
    /// directly, before `ModelManager.addModelFiles` runs, and several unit
    /// tests build the AST by hand (PORTING.md 6.2: "a recipe the pre-port
    /// loader cannot replay is the unit's problem").
    pub fn validate_new(
        namespace: &str,
        file_name: Option<&str>,
        ast: &Value,
    ) -> crate::error::Result<String> {
        let name = ast.get("name").and_then(Value::as_str).unwrap_or_default();
        let fqn = crate::model_util::get_fully_qualified_name(namespace, name);
        let processed =
            Self::process::<crate::error::ConcertoError>(ast, file_name, &|| Ok(fqn.clone()))?;
        // `HasValidators::check_validators`, over the raw AST `process` already
        // read rather than a loaded node's typed one (only a `String` scalar
        // has anything left to check here; the number check already ran
        // inside `process`, as the constructor it ports).
        if let Some(ScalarValidator::String {
            validator,
            length_validator,
        }) = &processed.validator
        {
            let bad = |e: serde_json::Error| crate::error::ConcertoError::IllegalModel {
                message: format!("invalid string validator: {e}"),
                file_name: None,
                location: None,
            };
            if let Some(v) = validator {
                check_pattern(name, &serde_json::from_value(v.clone()).map_err(bad)?)?;
            }
            if let Some(v) = length_validator {
                check_length(name, &serde_json::from_value(v.clone()).map_err(bad)?)?;
            }
        }
        Ok(fqn)
    }

    /// The generated metamodel node.
    pub fn ast(&self) -> &mm::ScalarDeclaration {
        &self.node
    }

    /// The primitive type this scalar aliases, or `None` (JS `null`) when its
    /// `$class` is not one of the six fully-qualified scalar classes.
    ///
    /// TS: ScalarDeclaration.getType (src/introspect/scalardeclaration.ts)
    pub fn scalar_type(&self) -> Option<&'static str> {
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
    pub fn to_string(fully_qualified_name: &str) -> String {
        format!("ScalarDeclaration {{id={fully_qualified_name}}}")
    }

    /// The scalar's own semantic check, run after `Declaration.validate`: no
    /// two declarations of its model file may share a fully qualified name.
    ///
    /// TS: ScalarDeclaration.validate (src/introspect/scalardeclaration.ts)
    pub fn validate<C: ResolutionContext>(ctx: &C, declaration: &C::Node) -> Result<(), C::Error> {
        let model_file = ctx.get_model_file(declaration)?;
        let declarations = ctx.get_all_declarations(&model_file)?;
        let names = declarations
            .iter()
            .map(|d| ctx.get_fully_qualified_name(d))
            .collect::<Result<Vec<_>, _>>()?;
        // The first name equal to an earlier one is `duplicateElements[0]`.
        // The set is only probed, never iterated.
        let mut seen = HashSet::new();
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
    /// The same as [`ScalarDeclaration::scalar_type`].
    fn type_name(&self) -> Option<&str> {
        self.scalar_type()
    }
}

impl HasValidators for ScalarDeclaration {
    /// `StringValidator` is not ported yet (P2-02): the checks its constructor
    /// makes on a String scalar are still the loader's own. A Number
    /// validator is checked by [`ScalarDeclaration::process`], which builds it.
    fn check_validators(&self) -> crate::error::Result<()> {
        if let (Some(ScalarValidator::String { .. }), mm::ScalarDeclaration::StringScalar(s)) =
            (self.validator(), &self.node)
        {
            if let Some(validator) = &s.validator {
                check_pattern(&s.name, validator)?;
            }
            if let Some(validator) = &s.length_validator {
                check_length(&s.name, validator)?;
            }
        }
        Ok(())
    }
}

/// Ported TS tests (PORTING.md 6.1). `scalardeclaration.js` is this unit's
/// own test file; `#accept`, `#getName`, `#getNamespace` and every constant
/// marker (`#isIdentified`, `#isAsset`, …) belong to `Declaration` (ledger:
/// TS, `not ported (Declaration)`) and are not ported here. `scalars.js`
/// (the other TS file this task's issue names) tests only
/// `Field.getScalarField`/`isTypeScalar` (ledger: `Field`, P2-04):
/// `not ported (Field)`, nothing in that file is this unit's own member.
#[cfg(test)]
#[allow(clippy::result_large_err)] // ContractError is fine as a by-value test Err; production code always boxes it as ConcertoError::Contract.
mod tests {
    use super::*;
    use serde_json::json;

    const NS: &str = METAMODEL_NAMESPACE;

    // ScalarDeclaration > Primitive type name conflict
    // TS: "should throw an error when scalar name is a primitive type"
    #[test]
    fn throws_when_scalar_name_is_a_primitive_type() {
        for primitive in ["String", "Integer", "Boolean", "DateTime", "Double", "Long"] {
            let ast = json!({
                "name": primitive,
                "$class": format!("{NS}.StringScalar"),
            });
            let err = ScalarDeclaration::process::<ContractError>(&ast, None, &|| unreachable!())
                .expect_err("a scalar named like a primitive must be rejected");
            assert_eq!(err.kind, ErrorKind::IllegalModel);
            assert_eq!(
                err.message(),
                format!("Invalid scalar name '{primitive}'. Name conflicts with primitive type.")
            );
        }
    }

    // TS: "should not throw when scalar name is valid"
    #[test]
    fn does_not_throw_for_a_valid_scalar_name() {
        let ast = json!({
            "name": "ValidScalar",
            "$class": format!("{NS}.StringScalar"),
        });
        assert!(
            ScalarDeclaration::process::<ContractError>(&ast, None, &|| unreachable!()).is_ok()
        );
    }

    // ScalarDeclaration#getName (Declaration's own member, not ported here)
    // also asserts toString(), which this unit does port.
    // TS: "#getName should return the scalar name"
    #[test]
    fn to_string_matches_the_ts_format() {
        assert_eq!(
            ScalarDeclaration::to_string("com.hyperledger.testing@1.0.0.suchName"),
            "ScalarDeclaration {id=com.hyperledger.testing@1.0.0.suchName}"
        );
    }

    // TS: "#getValidator should return the validator"
    // (test/data/parser/scalardeclaration.ssn.cto:
    //  `scalar SSN extends String default="000-00-0000" regex=/\d{3}-\d{2}-\d{4}/`)
    #[test]
    fn get_validator_returns_the_string_validator_for_a_regex_scalar() {
        let ast = json!({
            "$class": format!("{NS}.StringScalar"),
            "name": "SSN",
            "defaultValue": "000-00-0000",
            "validator": {
                "$class": format!("{NS}.StringRegexValidator"),
                "pattern": "\\d{3}-\\d{2}-\\d{4}",
                "flags": "",
            },
        });
        let processed = ScalarDeclaration::process::<ContractError>(&ast, None, &|| unreachable!())
            .expect("a valid regex validator must not error");
        let Some(ScalarValidator::String {
            validator: Some(v), ..
        }) = processed.validator
        else {
            panic!("expected a String validator carrying the AST's `validator`");
        };
        assert_eq!(
            v.get("pattern").and_then(Value::as_str),
            Some("\\d{3}-\\d{2}-\\d{4}")
        );
    }

    // TS: "#getDefaultValue should return the default value"
    #[test]
    fn default_value_is_read_from_the_ast() {
        let ast = json!({
            "$class": format!("{NS}.StringScalar"),
            "name": "SSN",
            "defaultValue": "000-00-0000",
        });
        let processed =
            ScalarDeclaration::process::<ContractError>(&ast, None, &|| unreachable!()).unwrap();
        assert_eq!(processed.default_value, Some(json!("000-00-0000")));
    }

    // TS: "#getDefaultValue should return the default value for falsy cases"
    // (test/data/parser/scalardeclaration.ssn.cto:
    //  `scalar BoolWithDefault extends Boolean default=false`)
    #[test]
    fn default_value_keeps_a_falsy_but_present_value() {
        let ast = json!({
            "$class": format!("{NS}.BooleanScalar"),
            "name": "BoolWithDefault",
            "defaultValue": false,
        });
        let processed =
            ScalarDeclaration::process::<ContractError>(&ast, None, &|| unreachable!()).unwrap();
        assert_eq!(processed.default_value, Some(json!(false)));
    }

    // TS: "#getDefaultValue should return null"
    // (test/data/parser/scalardeclaration.permutations.cto:
    //  `scalar StringScalar extends String`, no default)
    #[test]
    fn default_value_is_none_when_absent() {
        let ast = json!({
            "$class": format!("{NS}.StringScalar"),
            "name": "StringScalar",
        });
        let processed =
            ScalarDeclaration::process::<ContractError>(&ast, None, &|| unreachable!()).unwrap();
        assert_eq!(processed.default_value, None);
    }

    // `ScalarDeclaration.validate` has no direct TS `it()`: TS only ever
    // reaches it through `ModelFile`/`ModelManager` loading, so its own-op
    // oracle fixtures are cross-op ones under `ModelManager.addCTOModel`,
    // deferred to P2-08 (PORTING.md 6.2). This exercises it directly through
    // a minimal `ResolutionContext` double instead, as AGENTS.md asks for a
    // unit test of every ported method.
    struct FakeCtx {
        names: Vec<&'static str>,
    }

    impl ResolutionContext for FakeCtx {
        type Node = u32;
        type Error = ContractError;

        fn get_type(
            &self,
            _model_file: &u32,
            _type_name: Option<&str>,
        ) -> Result<Option<u32>, ContractError> {
            unreachable!()
        }
        fn get_all_super_type_declarations(
            &self,
            _declaration: &u32,
        ) -> Result<Vec<u32>, ContractError> {
            unreachable!()
        }
        fn get_fully_qualified_name(&self, declaration: &u32) -> Result<String, ContractError> {
            Ok(self.names[*declaration as usize].to_string())
        }
        fn get_fully_qualified_type_name(&self, _property: &u32) -> Result<String, ContractError> {
            unreachable!()
        }
        fn get_parent(&self, _property: &u32) -> Result<u32, ContractError> {
            unreachable!()
        }
        fn get_model_file(&self, _declaration: &u32) -> Result<u32, ContractError> {
            Ok(0)
        }
        fn get_type_name(&self, _property: &u32) -> Result<Option<String>, ContractError> {
            unreachable!()
        }
        fn is_enum(&self, _declaration: &u32) -> Result<bool, ContractError> {
            unreachable!()
        }
        fn is_map_declaration(&self, _declaration: &u32) -> Result<Option<bool>, ContractError> {
            unreachable!()
        }
        fn is_scalar_declaration(&self, _declaration: &u32) -> Result<Option<bool>, ContractError> {
            unreachable!()
        }
        fn get_ast_class(&self, _declaration: &u32) -> Result<Option<String>, ContractError> {
            unreachable!()
        }
        fn get_all_declarations(&self, _model_file: &u32) -> Result<Vec<u32>, ContractError> {
            Ok((0..self.names.len() as u32).collect())
        }
    }

    #[test]
    fn validate_rejects_a_duplicate_fully_qualified_name() {
        let ctx = FakeCtx {
            names: vec![
                "org.acme@1.0.0.A",
                "org.acme@1.0.0.B",
                "org.acme@1.0.0.A",
            ],
        };
        let err = ScalarDeclaration::validate(&ctx, &2)
            .expect_err("a duplicate fully qualified name must be rejected");
        assert_eq!(err.code, "scalardeclaration-validate-duplicateclassname");
        assert_eq!(err.message(), "Duplicate class name org.acme@1.0.0.A");
    }

    #[test]
    fn validate_accepts_unique_fully_qualified_names() {
        let ctx = FakeCtx {
            names: vec!["org.acme@1.0.0.A", "org.acme@1.0.0.B"],
        };
        assert!(ScalarDeclaration::validate(&ctx, &0).is_ok());
    }
}
