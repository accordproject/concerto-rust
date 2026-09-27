//! The expansions behind the derives, written over `proc_macro2` so that the
//! unit tests below can run them outside the compiler.

use proc_macro2::TokenStream;
use quote::quote;
use syn::{Attribute, Data, DeriveInput, Error, Fields, LitStr, Result, Variant};

/// What `#[concerto(...)]` says about a type or a variant.
#[derive(Default)]
struct Options {
    /// `delegate`: forward the method to the wrapped value.
    delegate: bool,
    /// `kind = "…"`: the declaration kind.
    kind: Option<LitStr>,
}

fn options(attrs: &[Attribute]) -> Result<Options> {
    let mut options = Options::default();
    for attr in attrs.iter().filter(|attr| attr.path().is_ident("concerto")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("delegate") {
                options.delegate = true;
                Ok(())
            } else if meta.path.is_ident("kind") {
                options.kind = Some(meta.value()?.parse()?);
                Ok(())
            } else {
                Err(meta.error("expected `delegate` or `kind = \"...\"`"))
            }
        })?;
    }
    Ok(options)
}

/// `true` for a variant or struct with exactly one unnamed field.
fn is_newtype(fields: &Fields) -> bool {
    matches!(fields, Fields::Unnamed(f) if f.unnamed.len() == 1)
}

/// `true` for a variant or struct with a named field called `field`.
fn has_field(fields: &Fields, field: &str) -> bool {
    matches!(fields, Fields::Named(f) if f.named.iter().any(|f| f.ident.as_ref().is_some_and(|i| i == field)))
}

/// Wraps a method in `impl <trait> for <type>`, keeping the type's generics.
fn implement(input: &DeriveInput, trait_path: TokenStream, method: TokenStream) -> TokenStream {
    let ident = &input.ident;
    let (impl_generics, type_generics, where_clause) = input.generics.split_for_impl();
    quote! {
        #[automatically_derived]
        impl #impl_generics #trait_path for #ident #type_generics #where_clause {
            #method
        }
    }
}

/// The variants of an enum, or an error for an enum with none.
fn variants(input: &DeriveInput) -> Result<Option<Vec<&Variant>>> {
    match &input.data {
        Data::Enum(data) if data.variants.is_empty() => Err(Error::new_spanned(
            &input.ident,
            "cannot derive for an enum with no variants",
        )),
        Data::Enum(data) => Ok(Some(data.variants.iter().collect())),
        Data::Struct(_) => Ok(None),
        Data::Union(_) => Err(Error::new_spanned(
            &input.ident,
            "cannot derive for a union",
        )),
    }
}

/// The expansion of `#[derive(Named)]`.
pub fn named(input: &DeriveInput) -> Result<TokenStream> {
    let container = options(&input.attrs)?;
    let trait_path = quote!(::concerto_core::introspect::Named);
    let unsupported = "Named needs a single unnamed field, or a named field `name`";

    let body = match variants(input)? {
        Some(variants) => {
            let arms = variants
                .into_iter()
                .map(|variant| {
                    let ident = &variant.ident;
                    let delegate = container.delegate || options(&variant.attrs)?.delegate;
                    if is_newtype(&variant.fields) {
                        Ok(if delegate {
                            quote!(Self::#ident(inner) => #trait_path::name(inner))
                        } else {
                            quote!(Self::#ident(inner) => &inner.name)
                        })
                    } else if !delegate && has_field(&variant.fields, "name") {
                        Ok(quote!(Self::#ident { name, .. } => name))
                    } else {
                        Err(Error::new_spanned(variant, unsupported))
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            quote!(match self { #(#arms,)* })
        }
        None => {
            let Data::Struct(data) = &input.data else {
                unreachable!("variants() returns None only for a struct")
            };
            if is_newtype(&data.fields) {
                if container.delegate {
                    quote!(#trait_path::name(&self.0))
                } else {
                    quote!(&self.0.name)
                }
            } else if !container.delegate && has_field(&data.fields, "name") {
                quote!(&self.name)
            } else {
                return Err(Error::new_spanned(&input.ident, unsupported));
            }
        }
    };

    Ok(implement(
        input,
        trait_path,
        quote! {
            fn name(&self) -> &str {
                #body
            }
        },
    ))
}

/// The expansion of `#[derive(DeclarationKind)]`.
pub fn declaration_kind(input: &DeriveInput) -> Result<TokenStream> {
    let container = options(&input.attrs)?;
    let trait_path = quote!(::concerto_core::introspect::DeclarationKind);
    let unsupported = "DeclarationKind needs `#[concerto(kind = \"...\")]`, or \
                       `#[concerto(delegate)]` on a single unnamed field";

    let body = match (&container.kind, variants(input)?) {
        (Some(kind), _) => quote!(#kind),
        (None, Some(variants)) => {
            let arms = variants
                .into_iter()
                .map(|variant| {
                    let ident = &variant.ident;
                    let options = options(&variant.attrs)?;
                    if let Some(kind) = &options.kind {
                        Ok(quote!(Self::#ident { .. } => #kind))
                    } else if (container.delegate || options.delegate)
                        && is_newtype(&variant.fields)
                    {
                        Ok(quote!(Self::#ident(inner) => #trait_path::declaration_kind(inner)))
                    } else {
                        Err(Error::new_spanned(variant, unsupported))
                    }
                })
                .collect::<Result<Vec<_>>>()?;
            quote!(match self { #(#arms,)* })
        }
        (None, None) => match &input.data {
            Data::Struct(data) if container.delegate && is_newtype(&data.fields) => {
                quote!(#trait_path::declaration_kind(&self.0))
            }
            _ => return Err(Error::new_spanned(&input.ident, unsupported)),
        },
    };

    Ok(implement(
        input,
        trait_path,
        quote! {
            fn declaration_kind(&self) -> &'static str {
                #body
            }
        },
    ))
}

#[cfg(test)]
mod tests {
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
        assert!(
            expanded.contains(
                &quote!(Self::Class(inner) => ::concerto_core::introspect::Named::name(inner))
                    .to_string()
            )
        );

        let one: DeriveInput = parse_quote! {
            enum Declaration { #[concerto(delegate)] Class(ClassDeclaration), Enum(mm::EnumDeclaration) }
        };
        let expanded = named(&one).unwrap().to_string();
        assert!(
            expanded.contains(
                &quote!(Self::Class(inner) => ::concerto_core::introspect::Named::name(inner))
                    .to_string()
            )
        );
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
}
