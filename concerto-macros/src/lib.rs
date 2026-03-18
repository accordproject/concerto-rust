use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, Ident, parse_macro_input};

#[proc_macro_derive(FromAst, attributes(concerto_ast_type, wrapper))]
pub fn derive_from_ast(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    let concerto_ast_ident = get_concerto_ast_type(&input);
    let wrapper_ident = get_wrapper_type(&input);
    let name = &input.ident;

    let expanded = quote! {
        impl FromAst for #name {
            type ConcertoType = #concerto_ast_ident;

            fn from_ast(concerto_type: Self::ConcertoType) -> Self {
                Self {
                    inner: #wrapper_ident(concerto_type),
                }
            }
        }
    };
    TokenStream::from(expanded)
}

fn get_concerto_ast_type(input: &DeriveInput) -> Ident {
    for attr in &input.attrs {
        if attr.path().is_ident("concerto_ast_type") {
            let nested_ident: Ident = attr.parse_args().expect("Expected a Concerto Type");
            return nested_ident;
        }
    }

    // Fallback to `CConcertoType`, e.g. `CDecorator` as a convention
    let fallback_name = format!("C{}", input.ident);
    Ident::new(&fallback_name, input.ident.span())
}

fn get_wrapper_type(input: &DeriveInput) -> Ident {
    for attr in &input.attrs {
        if attr.path().is_ident("wrapper") {
            let nested_ident: Ident = attr.parse_args().expect("Expected a Wrapper Type");
            return nested_ident;
        }
    }

    // Fallback to `CConcertoType`, e.g. `CDecorator` as a convention
    let fallback_name = format!("{}Ast", input.ident);
    Ident::new(&fallback_name, input.ident.span())
}
