use super::*;
use syn::parse_quote;

fn same(actual: TokenStream, expected: TokenStream) {
    assert_eq!(actual.to_string(), expected.to_string());
}

fn error(result: Result<TokenStream>) -> String {
    result
        .expect_err("the derive should be rejected")
        .to_string()
}

#[test]
fn named_reads_the_name_field_of_each_newtype_variant() {
    let input: DeriveInput = parse_quote! {
        enum Property { String(mm::StringProperty), Enum(mm::EnumProperty) }
    };
    same(
        named(&input).unwrap(),
        quote! {
            #[automatically_derived]
            impl ::concerto_core::introspect::Named for Property {
                fn name(&self) -> &str {
                    match self {
                        Self::String(inner) => &inner.name,
                        Self::Enum(inner) => &inner.name,
                    }
                }
            }
        },
    );
}

#[test]
fn named_binds_the_name_field_of_a_struct_variant() {
    let input: DeriveInput = parse_quote! {
        enum Map { Typed(mm::MapDeclaration), Untyped { name: String, key_kind: String } }
    };
    let expanded = named(&input).unwrap().to_string();
    assert!(expanded.contains(&quote!(Self::Typed(inner) => &inner.name).to_string()));
    assert!(expanded.contains(&quote!(Self::Untyped { name, .. } => name).to_string()));
}

#[test]
fn named_forwards_to_the_wrapped_value_when_delegating() {
    let input: DeriveInput = parse_quote! {
        #[concerto(delegate)]
        enum Declaration { Class(ClassDeclaration), Scalar(ScalarDeclaration) }
    };
    let expanded = named(&input).unwrap().to_string();
    assert!(expanded.contains(
        &quote!(Self::Class(inner) => ::concerto_core::introspect::Named::name(inner)).to_string()
    ));

    let one: DeriveInput = parse_quote! {
        enum Declaration { #[concerto(delegate)] Class(ClassDeclaration), Enum(mm::EnumDeclaration) }
    };
    let expanded = named(&one).unwrap().to_string();
    assert!(expanded.contains(
        &quote!(Self::Class(inner) => ::concerto_core::introspect::Named::name(inner)).to_string()
    ));
    assert!(expanded.contains(&quote!(Self::Enum(inner) => &inner.name).to_string()));
}

#[test]
fn named_on_structs_reads_the_wrapped_node_or_the_own_field() {
    let tuple: DeriveInput = parse_quote! { struct EnumDeclaration(mm::EnumDeclaration); };
    assert!(
        named(&tuple)
            .unwrap()
            .to_string()
            .contains(&quote!(&self.0.name).to_string())
    );
    let fields: DeriveInput = parse_quote! { struct Thing { name: String } };
    assert!(
        named(&fields)
            .unwrap()
            .to_string()
            .contains(&quote!(&self.name).to_string())
    );
}

#[test]
fn named_keeps_the_generics() {
    let input: DeriveInput = parse_quote! { struct View<'a, T: Clone>(&'a T) where T: Copy; };
    let expanded = named(&input).unwrap().to_string();
    assert!(expanded.contains(
            &quote!(impl<'a, T: Clone> ::concerto_core::introspect::Named for View<'a, T> where T: Copy)
                .to_string()
        ));
}

#[test]
fn named_rejects_what_it_cannot_read_a_name_from() {
    for input in [
        parse_quote! { enum E { Unit } },
        parse_quote! { enum E { Pair(A, B) } },
        parse_quote! { enum E { Other { label: String } } },
        parse_quote! { enum E {} },
        parse_quote! { struct S { label: String } },
        parse_quote! { union U { a: u32 } },
    ] {
        assert!(!error(named(&input)).is_empty());
    }
    assert!(error(named(&parse_quote! { enum E { Unit } })).starts_with("Named needs"));
}

#[test]
fn declaration_kind_uses_each_variant_kind() {
    let input: DeriveInput = parse_quote! {
        enum ClassKind {
            #[concerto(kind = "ConceptDeclaration")] Concept,
            #[concerto(kind = "AssetDeclaration")] Asset,
        }
    };
    same(
        declaration_kind(&input).unwrap(),
        quote! {
            #[automatically_derived]
            impl ::concerto_core::introspect::DeclarationKind for ClassKind {
                fn declaration_kind(&self) -> &'static str {
                    match self {
                        Self::Concept { .. } => "ConceptDeclaration",
                        Self::Asset { .. } => "AssetDeclaration",
                    }
                }
            }
        },
    );
}

#[test]
fn declaration_kind_of_the_whole_type() {
    for input in [
        parse_quote! { #[concerto(kind = "MapDeclaration")] enum Map { Typed(M), Untyped { name: String } } },
        parse_quote! { #[concerto(kind = "MapDeclaration")] struct Map(M); },
    ] {
        let expanded = declaration_kind(&input).unwrap().to_string();
        assert!(
            expanded.contains(
                &quote!(
                    fn declaration_kind(&self) -> &'static str {
                        "MapDeclaration"
                    }
                )
                .to_string()
            )
        );
    }
}

#[test]
fn declaration_kind_delegates_or_uses_a_variant_kind() {
    let input: DeriveInput = parse_quote! {
        #[concerto(delegate)]
        enum Declaration { Class(ClassDeclaration), #[concerto(kind = "EnumDeclaration")] Enum(E) }
    };
    let expanded = declaration_kind(&input).unwrap().to_string();
    assert!(expanded.contains(
            &quote!(Self::Class(inner) => ::concerto_core::introspect::DeclarationKind::declaration_kind(inner))
                .to_string()
        ));
    assert!(expanded.contains(&quote!(Self::Enum { .. } => "EnumDeclaration").to_string()));

    let wrapper: DeriveInput = parse_quote! { #[concerto(delegate)] struct W(Declaration); };
    assert!(
        declaration_kind(&wrapper).unwrap().to_string().contains(
            &quote!(::concerto_core::introspect::DeclarationKind::declaration_kind(&self.0))
                .to_string()
        )
    );
}

#[test]
fn declaration_kind_rejects_a_variant_with_no_kind() {
    for input in [
        parse_quote! { enum E { A } },
        parse_quote! { #[concerto(delegate)] enum E { A(X, Y) } },
        parse_quote! { struct S(X); },
    ] {
        assert!(error(declaration_kind(&input)).starts_with("DeclarationKind needs"));
    }
}

#[test]
fn an_unknown_or_malformed_attribute_is_rejected() {
    let unknown: DeriveInput = parse_quote! { #[concerto(rename = "x")] struct S(X); };
    assert!(error(named(&unknown)).contains("expected `delegate` or `kind"));
    let not_a_string: DeriveInput = parse_quote! { #[concerto(kind = 3)] struct S(X); };
    assert!(!error(declaration_kind(&not_a_string)).is_empty());
}
