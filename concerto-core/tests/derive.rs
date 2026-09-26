//! The derives, used from outside the crate through `concerto_core::derive`.

use concerto_core::derive;
use concerto_core::{DeclarationKind, Named};

/// A node that carries its own name, like the generated metamodel structs.
struct Node {
    name: String,
}

#[derive(derive::Named)]
struct Wrapper(Node);

#[derive(derive::Named)]
struct Plain {
    name: String,
}

#[derive(derive::Named, derive::DeclarationKind)]
#[concerto(kind = "MapDeclaration")]
enum Shapes {
    Wrapped(Node),
    Inline { name: String, _size: usize },
}

#[derive(derive::DeclarationKind)]
enum Kinds {
    #[concerto(kind = "ConceptDeclaration")]
    Concept,
    #[concerto(kind = "EventDeclaration")]
    Event,
}

#[derive(derive::Named, derive::DeclarationKind)]
#[concerto(delegate)]
enum Outer {
    Shape(Shapes),
    Wrapper(Wrapper),
}

// `Outer::Wrapper` delegates its kind to `Wrapper`.
impl DeclarationKind for Wrapper {
    fn declaration_kind(&self) -> &'static str {
        "EnumDeclaration"
    }
}

fn node(name: &str) -> Node {
    Node { name: name.into() }
}

#[test]
fn named_reads_the_name_of_the_wrapped_node_or_the_own_field() {
    assert_eq!(Wrapper(node("A")).name(), "A");
    assert_eq!(Plain { name: "B".into() }.name(), "B");
    assert_eq!(Shapes::Wrapped(node("C")).name(), "C");
    let inline = Shapes::Inline {
        name: "D".into(),
        _size: 0,
    };
    assert_eq!(inline.name(), "D");
}

#[test]
fn declaration_kind_is_per_variant_or_for_the_whole_type() {
    assert_eq!(Kinds::Concept.declaration_kind(), "ConceptDeclaration");
    assert_eq!(Kinds::Event.declaration_kind(), "EventDeclaration");
    assert_eq!(
        Shapes::Wrapped(node("M")).declaration_kind(),
        "MapDeclaration"
    );
}

#[test]
fn delegating_forwards_to_the_wrapped_value() {
    let shape = Outer::Shape(Shapes::Wrapped(node("S")));
    assert_eq!(shape.name(), "S");
    assert_eq!(shape.declaration_kind(), "MapDeclaration");

    let wrapper = Outer::Wrapper(Wrapper(node("W")));
    assert_eq!(wrapper.name(), "W");
    assert_eq!(wrapper.declaration_kind(), "EnumDeclaration");
}
