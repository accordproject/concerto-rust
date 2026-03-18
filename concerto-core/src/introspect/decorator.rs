use crate::{types::DecoratorAst, ConcertoError, FromAst};
use concerto_macros::FromAst;
use concerto_metamodel::concerto_metamodel_1_0_0::Decorator as CDecorator;

#[derive(FromAst)]
pub struct Decorator {
    inner: DecoratorAst,
}

// Builder methods
impl Decorator {
    pub fn new(ast_json: &str) -> Result<Self, ConcertoError> {
        Decorator::from_json(ast_json)
    }
}

// Public methods
impl Decorator {
    pub fn get_name(&self) -> &str {
        &self.inner.0.name
    }
}
