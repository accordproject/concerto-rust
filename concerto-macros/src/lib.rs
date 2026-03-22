use proc_macro::TokenStream;
use quote::quote;
use syn::{Data, DeriveInput, Fields, parse_macro_input};

/// Derive macro for the `Decorated` trait.
///
/// Expects the struct to have a field named `decorators` of type
/// `Option<Vec<Decorator>>`, or a newtype wrapping a struct that does.
///
/// For newtypes (single unnamed field), it delegates to `.0.decorators`.
/// For named structs, it reads the `decorators` field directly.
#[proc_macro_derive(Decorated)]
pub fn derive_decorated(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let body = match &input.data {
        Data::Struct(data) => match &data.fields {
            // Newtype: struct Foo(Inner)
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                quote! {
                    fn decorators(&self) -> &[concerto_metamodel::concerto_metamodel_1_0_0::Decorator] {
                        self.0.decorators.as_deref().unwrap_or(&[])
                    }
                }
            }
            // Named struct with `decorators` field
            Fields::Named(_) => {
                quote! {
                    fn decorators(&self) -> &[concerto_metamodel::concerto_metamodel_1_0_0::Decorator] {
                        self.decorators.as_deref().unwrap_or(&[])
                    }
                }
            }
            _ => {
                return syn::Error::new_spanned(
                    &input,
                    "Decorated can only be derived for named structs or newtypes",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(&input, "Decorated can only be derived for structs")
                .to_compile_error()
                .into();
        }
    };

    let expanded = quote! {
        impl #impl_generics crate::introspect::traits::Decorated for #name #ty_generics #where_clause {
            #body
        }
    };

    expanded.into()
}

/// Derive macro for the `Named` trait.
///
/// Expects the struct to have `name: String` and `location: Option<Range>` fields,
/// or be a newtype wrapping such a struct.
#[proc_macro_derive(Named)]
pub fn derive_named(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let body = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                quote! {
                    fn name(&self) -> &str {
                        &self.0.name
                    }
                    fn location(&self) -> Option<&concerto_metamodel::concerto_metamodel_1_0_0::Range> {
                        self.0.location.as_ref()
                    }
                }
            }
            Fields::Named(_) => {
                quote! {
                    fn name(&self) -> &str {
                        &self.name
                    }
                    fn location(&self) -> Option<&concerto_metamodel::concerto_metamodel_1_0_0::Range> {
                        self.location.as_ref()
                    }
                }
            }
            _ => {
                return syn::Error::new_spanned(
                    &input,
                    "Named can only be derived for named structs or newtypes",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(&input, "Named can only be derived for structs")
                .to_compile_error()
                .into();
        }
    };

    let expanded = quote! {
        impl #impl_generics crate::introspect::traits::Named for #name #ty_generics #where_clause {
            #body
        }
    };

    expanded.into()
}

/// Derive macro for the `HasProperties` trait.
///
/// Expects the struct to have `properties: Vec<PropertyDecl>`,
/// `super_type: Option<TypeIdentifier>`, and `is_abstract: bool` fields,
/// or be a newtype wrapping such a struct.
#[proc_macro_derive(HasProperties)]
pub fn derive_has_properties(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let body = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                quote! {
                    fn own_properties(&self) -> &[crate::introspect::properties::PropertyDecl] {
                        &self.0.properties
                    }
                    fn super_type(&self) -> Option<&concerto_metamodel::concerto_metamodel_1_0_0::TypeIdentifier> {
                        self.0.super_type.as_ref()
                    }
                    fn is_abstract(&self) -> bool {
                        self.0.is_abstract
                    }
                }
            }
            Fields::Named(_) => {
                quote! {
                    fn own_properties(&self) -> &[crate::introspect::properties::PropertyDecl] {
                        &self.properties
                    }
                    fn super_type(&self) -> Option<&concerto_metamodel::concerto_metamodel_1_0_0::TypeIdentifier> {
                        self.super_type.as_ref()
                    }
                    fn is_abstract(&self) -> bool {
                        self.is_abstract
                    }
                }
            }
            _ => {
                return syn::Error::new_spanned(
                    &input,
                    "HasProperties can only be derived for named structs or newtypes",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(
                &input,
                "HasProperties can only be derived for structs",
            )
            .to_compile_error()
            .into();
        }
    };

    let expanded = quote! {
        impl #impl_generics crate::introspect::traits::HasProperties for #name #ty_generics #where_clause {
            #body
        }
    };

    expanded.into()
}

/// Derive macro for the `Identifiable` trait.
///
/// Expects the struct to have an `identified: Option<Identified>` field,
/// or be a newtype wrapping a struct that does.
///
/// The `$class` discriminator on the `Identified` value determines whether
/// the identification is system-provided or explicit:
///   - `concerto.metamodel@1.0.0.Identified` → system identified
///   - `concerto.metamodel@1.0.0.IdentifiedBy` → explicitly identified
#[proc_macro_derive(Identifiable)]
pub fn derive_identifiable(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    let generics = &input.generics;
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let identified_expr = match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Unnamed(fields) if fields.unnamed.len() == 1 => {
                quote! { &self.0.identified }
            }
            Fields::Named(_) => {
                quote! { &self.identified }
            }
            _ => {
                return syn::Error::new_spanned(
                    &input,
                    "Identifiable can only be derived for named structs or newtypes",
                )
                .to_compile_error()
                .into();
            }
        },
        _ => {
            return syn::Error::new_spanned(&input, "Identifiable can only be derived for structs")
                .to_compile_error()
                .into();
        }
    };

    let expanded = quote! {
        impl #impl_generics crate::introspect::traits::Identifiable for #name #ty_generics #where_clause {
            fn is_identified(&self) -> bool {
                (#identified_expr).is_some()
            }
            fn is_system_identified(&self) -> bool {
                match #identified_expr {
                    Some(id) => !id._class.contains("IdentifiedBy"),
                    None => false,
                }
            }
            fn is_explicitly_identified(&self) -> bool {
                match #identified_expr {
                    Some(id) => id._class.contains("IdentifiedBy"),
                    None => false,
                }
            }
            fn identifier_field_name(&self) -> Option<&str> {
                match #identified_expr {
                    Some(id) if !id._class.contains("IdentifiedBy") => Some("$identifier"),
                    _ => None,
                }
            }
        }
    };

    expanded.into()
}
