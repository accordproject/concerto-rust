//! Rust types for the Concerto metamodel.
//!
//! The types are generated at build time from the model ASTs, which are pinned
//! by tag and SHA-256 checksum (see `build.rs`). Abstract types are enums
//! tagged by `$class`, so deserialising an AST keeps each node's concrete type.

/// Types for the `concerto@1.0.0` namespace.
pub mod concerto_1_0_0 {
    use serde::{Deserialize, Serialize};
    include!(concat!(env!("OUT_DIR"), "/concerto_1_0_0.rs"));
}

/// Types for the `concerto.decorator@1.0.0` namespace.
pub mod concerto_decorator_1_0_0 {
    use serde::{Deserialize, Serialize};
    include!(concat!(env!("OUT_DIR"), "/concerto_decorator_1_0_0.rs"));
}

/// Types for the `concerto.metamodel@1.0.0` namespace.
pub mod concerto_metamodel_1_0_0 {
    use serde::{Deserialize, Serialize};
    include!(concat!(env!("OUT_DIR"), "/concerto_metamodel_1_0_0.rs"));
}

/// Types for the `org.accordproject.decoratorcommands@0.4.0` namespace.
pub mod org_accordproject_decoratorcommands_0_4_0 {
    use serde::{Deserialize, Serialize};
    include!(concat!(
        env!("OUT_DIR"),
        "/org_accordproject_decoratorcommands_0_4_0.rs"
    ));
}

pub mod utils;
