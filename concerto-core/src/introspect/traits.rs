use concerto_metamodel::concerto_metamodel_1_0_0::{Decorator, Range, TypeIdentifier};

use super::properties::PropertyDecl;

// ---------------------------------------------------------------------------
// Decorated
// ---------------------------------------------------------------------------

/// Mirrors the JS `Decorated` base class — provides access to decorators.
pub trait Decorated {
    fn decorators(&self) -> &[Decorator];
    fn decorator(&self, name: &str) -> Option<&Decorator> {
        self.decorators().iter().find(|d| d.name == name)
    }
}

// ---------------------------------------------------------------------------
// Named
// ---------------------------------------------------------------------------

/// Something that has a name and an optional source location.
pub trait Named: Decorated {
    fn name(&self) -> &str;
    fn location(&self) -> Option<&Range>;
}

// ---------------------------------------------------------------------------
// HasProperties
// ---------------------------------------------------------------------------

/// A declaration that owns properties and may extend a super-type.
pub trait HasProperties: Named {
    fn own_properties(&self) -> &[PropertyDecl];
    fn super_type(&self) -> Option<&TypeIdentifier>;
    fn is_abstract(&self) -> bool;
}

// ---------------------------------------------------------------------------
// Identifiable
// ---------------------------------------------------------------------------

/// A declaration that may carry an identity field.
pub trait Identifiable: HasProperties {
    fn is_identified(&self) -> bool;
    fn is_system_identified(&self) -> bool;
    fn is_explicitly_identified(&self) -> bool;
    fn identifier_field_name(&self) -> Option<&str>;
}

// ---------------------------------------------------------------------------
// Validate
// ---------------------------------------------------------------------------

/// Types that can be validated against a [`ValidationContext`].
pub trait Validate {
    fn validate(&self, ctx: &super::validation::ValidationContext<'_>) -> crate::error::Result<()>;
}
