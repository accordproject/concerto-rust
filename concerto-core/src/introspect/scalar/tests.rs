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
    assert!(ScalarDeclaration::process::<ContractError>(&ast, None, &|| unreachable!()).is_ok());
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
    let Some(ScalarValidator::String(v)) = processed.validator else {
        panic!("expected a String validator built from the AST's `validator`");
    };
    assert_eq!(v.regex().as_deref(), Some("/\\d{3}-\\d{2}-\\d{4}/"));
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
// oracle fixtures are cross-op ones under `ModelManager.addCTOModel`.
// This exercises it directly through
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
        names: vec!["org.acme@1.0.0.A", "org.acme@1.0.0.B", "org.acme@1.0.0.A"],
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
