use concerto_metamodel::concerto_metamodel_1_0_0::Range;

/// Errors produced by concerto-core.
#[derive(Debug, thiserror::Error)]
pub enum ConcertoError {
    /// The model contains an illegal construct.
    #[error("{message}")]
    IllegalModel {
        message: String,
        file_name: Option<String>,
        location: Option<Range>,
    },

    /// A referenced type could not be found.
    #[error("Type not found: {type_name}")]
    TypeNotFound { type_name: String },

    /// A value failed validation against the model.
    #[error("Validation error on {component}: {message}")]
    Validation { message: String, component: String },
}

pub type Result<T> = std::result::Result<T, ConcertoError>;
