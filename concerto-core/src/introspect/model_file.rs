use concerto_metamodel::concerto_metamodel_1_0_0::{Declaration, Model};

use crate::{types::Ast, ConcertoError};

#[derive(Debug)]
pub struct ModelFile {
    inner: Ast,
}

impl ModelFile {
    pub fn new(ast_json: &str) -> Result<Self, ConcertoError> {
        let parsed: Model = serde_json::from_str(ast_json)?;
        let ast = Ast(parsed);
        ast.into()
    }
}

impl ModelFile {
    pub fn get_namespace(&self) -> &str {
        &self.inner.0.namespace
    }

    pub fn get_declarations(&self) -> Option<Vec<Declaration>> {
        self.inner.0.declarations.clone()
    }

    pub fn validate(&self) -> Result<(), ConcertoError> {
        unimplemented!()
    }
}

impl From<Ast> for Result<ModelFile, ConcertoError> {
    fn from(value: Ast) -> Self {
        Ok(ModelFile { inner: value })
    }
}
