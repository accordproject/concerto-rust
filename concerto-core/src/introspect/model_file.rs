use concerto_metamodel::concerto_metamodel_1_0_0::{Declaration, Model};

use crate::{types::Ast, ConcertoError};

#[derive(Debug)]
pub struct ModelFile {
    ast: Ast,
    pub namespace: String,
    pub version: String,
}

impl ModelFile {
    pub fn new(ast_json: &str) -> Result<Self, ConcertoError> {
        let parsed: Model = serde_json::from_str(ast_json)?;
        let ast = Ast(parsed);
        ast.into()
    }
}

impl ModelFile {
    pub fn get_namespace(&self) -> String {
        self.namespace.clone()
    }

    pub fn get_declarations(&self) -> Option<Vec<Declaration>> {
        self.ast.0.declarations.clone()
    }

    pub fn validate(&self) -> Result<(), ConcertoError> {
        unimplemented!()
    }
}

impl From<Ast> for Result<ModelFile, ConcertoError> {
    fn from(value: Ast) -> Self {
        let (namespace, version) = parse_namespace_and_version(&value)?;
        Ok(ModelFile {
            ast: value,
            namespace: namespace,
            version: version,
        })
    }
}

fn parse_namespace_and_version(ast: &Ast) -> Result<(String, String), ConcertoError> {
    let x = ast.0.namespace.as_str();
    let parts: Vec<&str> = x.split('@').collect();
    // For now, assume all NS will have namespace@version
    if parts.len() != 2 {
        Err(ConcertoError::InvalidNS(x.to_string()))
    } else {
        Ok((parts[0].to_string(), parts[1].to_string()))
    }
}

mod test {
    use crate::types::Ast;
    use concerto_metamodel::concerto_metamodel_1_0_0::Model;

    fn make_ast() -> Ast {
        let basic_ast = include_str!("../../examples/assets/concerto_models/basic.json");
        let parsed: Model = serde_json::from_str(basic_ast).expect("Cannot parse basic.json");
        Ast(parsed)
    }

    #[test]
    fn test_parse_ns() {
        let ast = make_ast();
        if let Ok((ns, version)) = super::parse_namespace_and_version(&ast) {
            assert_eq!(ns, "com.example.foo".to_string(), "Can parse namespace");
            assert_eq!(version, "1.0.0".to_string(), "Can parse version");
        } else {
            assert!(false);
        }
    }
}
