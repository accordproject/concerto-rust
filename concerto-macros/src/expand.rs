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
mod tests;
