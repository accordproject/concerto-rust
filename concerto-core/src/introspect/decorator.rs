//! Decorated elements.
//!
//! P1-03 adds only the [`Decorated`] trait. The `Decorator` struct and the
//! rest of `decorated.ts` and `decorator.ts` are ported in P2-07; until then a
//! decorator is its generated metamodel node.

use concerto_metamodel::concerto_metamodel_1_0_0 as mm;

/// An element that can carry decorators: a declaration or a property.
///
/// TS: Decorated.getDecorators (src/introspect/decorated.ts)
pub trait Decorated {
    /// The decorators attached to the element, in the order they are given.
    fn decorators(&self) -> &[mm::Decorator];
}
