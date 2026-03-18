use crate::{types::DecoratorAst, ConcertoError, FromAst};
use concerto_metamodel::concerto_metamodel_1_0_0::Decorator as CDecorator;

pub struct Decorator {
    inner: DecoratorAst,
}

// Builder methods
impl Decorator {
    pub fn new(ast_json: &str) -> Result<Self, ConcertoError> {
        let parsed: CDecorator = serde_json::from_str(ast_json)?;
        Ok(Decorator::from_ast(parsed))
    }
}

// Public methods
impl Decorator {
    pub fn get_name(&self) -> &str {
        &self.inner.0.name
    }
}

impl FromAst for Decorator {
    type ConcertoType = CDecorator;

    fn from_ast(concerto_type: Self::ConcertoType) -> Self {
        Decorator {
            inner: DecoratorAst(concerto_type),
        }
    }
}
