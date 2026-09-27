//! The behaviour the introspection types share.
//!
//! The TypeScript classes inherit these members from a common base class.
//! Rust keeps each family as a sum type (PORTING.md 1.1), and a member shared
//! across more than one type becomes one of these traits, so that it is
//! written once per type and callers can be generic over it. The
//! [`Decorated`](crate::introspect::decorator::Decorated) trait lives with the
//! decorators, in [`introspect::decorator`](crate::introspect::decorator).
//!
//! Where a trait is implemented the same way over every variant of a sum
//! type, the implementation is derived with the macros in
//! [`concerto_core::derive`](crate::derive).

use crate::error::{ContractError, Result};
use crate::model_manager::ModelManager;

/// An element with a short name: a declaration or a property.
///
/// TS: Declaration.getName (src/introspect/declaration.ts), Property.getName
/// (src/introspect/property.ts)
pub trait Named {
    /// The short name, without the namespace.
    fn name(&self) -> &str;
}

/// An element with a fully qualified name: the namespace of its model file,
/// then its own name.
///
/// In TS the name is read through the element's collaborators (its model
/// file, or its parent), so computing it may fail with whatever those calls
/// raise.
///
/// TS: Declaration.getFullyQualifiedName (src/introspect/declaration.ts),
/// Property.getFullyQualifiedName (src/introspect/property.ts)
pub trait FullyQualified {
    /// What computing the name can raise.
    type Error: From<ContractError>;

    /// The fully qualified name.
    fn fully_qualified_name(&self) -> std::result::Result<String, Self::Error>;
}

/// An element whose AST can carry validators: a property, or a scalar
/// declaration.
pub trait HasValidators {
    /// Checks the validators the element declares, as they are when the
    /// element is loaded: a regular expression must compile, and the bounds of
    /// a range, a string length or a collection size must make sense.
    fn check_validators(&self) -> Result<()>;
}

/// An element with a type.
///
/// TS: Property.getType (src/introspect/property.ts), Declaration.getType
/// (src/introspect/declaration.ts), ScalarDeclaration.getType
/// (src/introspect/scalardeclaration.ts)
pub trait Typed {
    /// The name of the element's type, or `None` (JS `null`) where it has
    /// none.
    fn type_name(&self) -> Option<&str>;
}

/// An element with semantic checks that need the other loaded models in view.
///
/// These are the checks [`ModelManager::validate_models`] runs once every
/// model is loaded.
pub trait Validate {
    /// Validates the element, which is declared in the model file for
    /// `namespace`, against the models loaded in `manager`. Returns the first
    /// problem found.
    fn validate(&self, manager: &ModelManager, namespace: &str) -> Result<()>;
}

/// An element that knows which metamodel declaration it is.
///
/// Rust needs this for its own logic; the TS `isEnum()`-style markers stay in
/// the TS views (PORTING.md 1.1, rule 3).
pub trait DeclarationKind {
    /// The metamodel `$class` short name of the declaration, such as
    /// `ConceptDeclaration` or `StringScalar`.
    fn declaration_kind(&self) -> &'static str;
}
