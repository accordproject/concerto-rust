//! Serde helpers for the `$class` fields of the generated types, plus
//! concerto-codegen's own helpers for their `DateTime` fields.

use std::borrow::Cow;
use std::fmt;

use serde::Deserializer;
use serde::de::{self, Visitor};

use crate::ClassName;

pub use crate::codegen_utils::*;

include!("generated/classes.rs");

/// The static copy of `class` when it is the `$class` of a type declared in
/// one of the models this crate is generated from.
pub fn intern_class(class: &str) -> Option<&'static str> {
    CLASSES
        .binary_search_by(|probe| {
            probe
                .len()
                .cmp(&class.len())
                .then_with(|| probe.cmp(&class))
        })
        .ok()
        .and_then(|index| CLASSES.get(index).copied())
}

/// `class` as a [`ClassName`]: interned ([`intern_class`]) when it can be,
/// an owned copy otherwise.
pub fn class_name(class: &str) -> ClassName {
    intern_class(class).map_or_else(|| Cow::Owned(class.to_string()), Cow::Borrowed)
}

/// [`class_name`] for an owned `class`: interned when it can be, otherwise
/// `class` itself, with no copy.
pub fn class_name_owned(class: String) -> ClassName {
    intern_class(&class).map_or(Cow::Owned(class), Cow::Borrowed)
}

/// Whether a `$class` is empty (a `Range` or `Position` read without one is
/// written back without one).
pub fn is_empty_class(class: &ClassName) -> bool {
    class.is_empty()
}

/// Deserializes a `$class` string as an interned [`ClassName`]: no
/// allocation for a declared type's name. It accepts and rejects exactly
/// what deserializing a `String` does.
pub fn deserialize_class<'de, D>(deserializer: D) -> Result<ClassName, D::Error>
where
    D: Deserializer<'de>,
{
    struct ClassVisitor;

    impl Visitor<'_> for ClassVisitor {
        type Value = ClassName;

        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a string")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> Result<ClassName, E> {
            Ok(class_name(v))
        }

        fn visit_string<E: de::Error>(self, v: String) -> Result<ClassName, E> {
            Ok(intern_class(&v).map_or(Cow::Owned(v), Cow::Borrowed))
        }
    }

    deserializer.deserialize_string(ClassVisitor)
}
