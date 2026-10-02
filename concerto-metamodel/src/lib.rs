//! Rust types for the Concerto metamodel.
//!
//! The types are generated at build time from the model ASTs, which are pinned
//! by tag and SHA-256 checksum (see `build.rs`). Abstract types are enums
//! tagged by `$class`, so deserialising an AST keeps each node's concrete type.

/// A node's `$class`, as a generated struct keeps it: a borrowed static
/// string when it names a type declared in one of the models this crate is
/// generated from (every `$class` of a well-formed AST), so reading one
/// allocates nothing ([`utils::deserialize_class`]); an owned copy for any
/// other string. Build one with `.into()` from a `&'static str` or a
/// `String`, or with [`utils::class_name`].
pub type ClassName = std::borrow::Cow<'static, str>;

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

mod name;
pub mod utils;

pub use name::{Name, with_source};
