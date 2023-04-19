use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Token};

use smithay::wayland::module_registry::ModuleRegistry;

mod delegate_dispatch;

struct DelegateDeriveInput {
    ident: syn::Ident,
    generics: syn::Generics,
    modules: Vec<syn::Ident>,
}

impl syn::parse::Parse for DelegateDeriveInput {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let input: syn::DeriveInput = input.parse()?;

        let mut modules = Vec::new();
        for attr in input.attrs {
            let args = attr.parse_args_with(|input: syn::parse::ParseStream| {
                input.parse_terminated(|input| input.parse::<syn::Ident>(), Token![,])
            })?;

            for module in args {
                modules.push(module);
            }
        }

        Ok(Self {
            ident: input.ident,
            generics: input.generics,
            modules,
        })
    }
}

#[proc_macro_derive(DelegateModule, attributes(delegate))]
pub fn delegate_global_dispatch(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DelegateDeriveInput);

    let registry = ModuleRegistry::new();

    let mut impls = Vec::new();
    for item in input.modules {
        if let Some(module) = registry.get(item.to_string().as_str()) {
            impls.push(delegate_dispatch::delegate_module(
                &input.ident,
                &input.generics,
                module,
            ));
        } else {
            return syn::Error::new(item.span(), "Unknown smithay module")
                .to_compile_error()
                .into();
        }
    }

    quote!(#(#impls)*).into()
}
