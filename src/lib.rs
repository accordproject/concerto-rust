// Main library file for concerto_core

// Define modules
pub mod model_file;
pub mod model_manager;
pub mod error;
pub mod validation;
pub mod introspect;
pub mod util;
pub mod metamodel;
pub mod metamodel_validation;
pub mod traits;

// Re-export the main components
pub use model_file::ModelFile;
pub use model_manager::ModelManager;
pub use traits::*;

// Metamodel types exports
pub use metamodel::concerto_metamodel_1_0_0::{
    Model, Declaration, Import,
    Property, Decorator, TypeIdentifier
};

pub use validation::Validate;
pub use error::ConcertoError;

/// Version of the Concerto core library
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
