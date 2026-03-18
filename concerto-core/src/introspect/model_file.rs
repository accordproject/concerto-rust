use concerto_macros::FromAst;
use concerto_metamodel::concerto_metamodel_1_0_0::{Declaration, Model};

use crate::{types::ModelAst, ConcertoError, FromAst};

#[derive(Debug, FromAst)]
#[concerto_ast_type(Model)]
#[wrapper(ModelAst)]
pub struct ModelFile {
    inner: ModelAst,
}

// Builder methods
impl ModelFile {
    pub fn new(ast_json: &str) -> Result<Self, ConcertoError> {
        ModelFile::from_json(ast_json)
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
