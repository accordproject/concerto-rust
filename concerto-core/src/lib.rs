// Main library file for concerto_core

// Define modules
pub mod error;
pub mod introspect;
pub mod metamodel_validation;
pub mod model_file;
pub mod model_manager;
pub mod traits;
pub mod util;
pub mod validation;

// Re-export the main components
pub use model_file::ModelFile;
pub use model_manager::ModelManager;
pub use traits::*;

// Metamodel types exports
pub use concerto_metamodel::concerto_metamodel_1_0_0::{
    Declaration, Decorator, Import, Model, Property, TypeIdentifier,
};

pub use error::ConcertoError;
pub use validation::Validate;

/// Version of the Concerto core library
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
