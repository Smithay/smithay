use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::{quote, ToTokens};
use syn::{parse_macro_input, Token};

mod bundle;
mod delegate_dispatch;
mod item_impl;

fn smithay() -> TokenStream2 {
    match proc_macro_crate::crate_name("smithay").expect("smithay should be present in `Cargo.toml`") {
        // Tehnically `Itself` should result in `quote!(crate)` but doc tests and examples are
        // missreported as `Itself` so that would breake those,
        // Smithay never uses those macros so this is fine, and if there is a need to use them
        // somewhere we can always just do `use crate as smithay`
        proc_macro_crate::FoundCrate::Itself => quote!(smithay),
        proc_macro_crate::FoundCrate::Name(name) => {
            let ident = syn::Ident::new(&name, proc_macro2::Span::call_site());
            ident.to_token_stream()
        }
    }
}

struct DelegateBundleInput {
    item_impl: item_impl::ItemImpl,
    bundle: bundle::Bundle,
}

impl syn::parse::Parse for DelegateBundleInput {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let item_impl: item_impl::ItemImpl = input.parse()?;
        let _: syn::Token![,] = input.parse()?;
        let bundle: bundle::Bundle = input.parse()?;
        let _: Option<Token![,]> = input.parse().ok();

        Ok(DelegateBundleInput { item_impl, bundle })
    }
}

#[proc_macro]
pub fn delegate_bundle(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DelegateBundleInput);

    let rs = delegate_dispatch::delegate_module(
        &input.item_impl.self_ty,
        &input.item_impl.generics,
        &input.bundle,
    );

    quote!(#rs).into()
}
