use concerto_metamodel::concerto_metamodel_1_0_0::{Declaration, Model};

use crate::{types::ModelAst, ConcertoError, FromAst};

#[derive(Debug)]
pub struct ModelFile {
    inner: ModelAst,
}

// Builder methods
impl ModelFile {
    pub fn new(ast_json: &str) -> Result<Self, ConcertoError> {
        let parsed: Model = serde_json::from_str(ast_json)?;
        Ok(ModelFile::from_ast(parsed))
    }
}

// Public methods
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

impl FromAst for ModelFile {
    type ConcertoType = Model;

    fn from_ast(concerto_type: Self::ConcertoType) -> Self {
        ModelFile {
            inner: ModelAst(concerto_type),
        }
    }
}
