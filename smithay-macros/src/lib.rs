use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{parse_macro_input, Token};

mod bundle;
mod delegate_dispatch;
mod item_impl;

fn smithay() -> TokenStream2 {
    // Could use proc_macro_crate here to detect proper name?
    // But I don't want to bring to many deps in
    quote!(smithay)
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
