use concerto_metamodel::concerto_metamodel_1_0_0::{Declaration, Model};

use crate::{types::Ast, ConcertoError};

#[derive(Debug)]
pub struct ModelFile {
    ast: Ast,
}

impl ModelFile {
    pub fn new(ast_json: &str) -> Result<Self, ConcertoError> {
        let parsed: Model = serde_json::from_str(ast_json)?;
        let ast = Ast(parsed);
        Ok(ModelFile { ast })
    }
}

impl ModelFile {
    pub fn get_namespace(&self) -> String {
        return self.ast.0.namespace.clone();
    }

    pub fn get_declarations(&self) -> Option<Vec<Declaration>> {
        return self.ast.0.declarations.clone();
    }

    pub fn validate(&self) -> Result<(), ConcertoError> {
        unimplemented!()
    }
}
