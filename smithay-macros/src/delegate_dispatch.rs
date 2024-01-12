use proc_macro2::TokenStream as TokenStream2;
use quote::quote;

use crate::{
    bundle::{Bundle, Global, Resource},
    smithay,
};

pub(crate) fn delegate_module(
    self_ty: &syn::Type,
    generics: &syn::Generics,
    module: &Bundle,
) -> TokenStream2 {
    let Bundle {
        dispatch_to,
        globals,
        resources,
        ..
    } = module;

    let globals = globals
        .iter()
        .map(|g| delegate_global_dispatch(self_ty, generics, dispatch_to, g));
    let resources = resources
        .iter()
        .map(|r| delegate_dispatch(self_ty, generics, dispatch_to, r));

    quote! {
        #(#globals)*
        #(#resources)*
    }
}

pub(crate) fn delegate_global_dispatch(
    self_ty: &syn::Type,
    generics: &syn::Generics,

    dispatch_to: &syn::Type,
    global: &Global,
) -> TokenStream2 {
    let smithay = smithay();
    let wayland_server = quote!(#smithay::reexports::wayland_server);

    let Global { interface, data } = &global;

    let (impl_generics, _type_generics, where_clause) = generics.split_for_impl();
    let trait_name = quote!(#wayland_server::GlobalDispatch<#interface, #data>);

    quote! {
        #[automatically_derived]
        impl #impl_generics #trait_name for #self_ty #where_clause {
            fn bind(
                state: &mut Self,
                dhandle: &#wayland_server::DisplayHandle,
                client: &#wayland_server::Client,
                resource: #wayland_server::New<#interface>,
                global_data: &#data,
                data_init: &mut #wayland_server::DataInit<'_, Self>,
            ) {
                <#dispatch_to as #wayland_server::GlobalDispatch<
                    #interface,
                    #data,
                    Self,
                >>::bind(state, dhandle, client, resource, global_data, data_init)
            }

            fn can_view(client: #wayland_server::Client, global_data: &#data) -> bool {
                <#dispatch_to as #wayland_server::GlobalDispatch<
                    #interface,
                    #data,
                    Self,
                >>::can_view(client, global_data)
            }
        }
    }
}

pub(crate) fn delegate_dispatch(
    self_ty: &syn::Type,
    generics: &syn::Generics,

    dispatch_to: &syn::Type,
    resource: &Resource,
) -> TokenStream2 {
    let smithay = smithay();
    let wayland_server = quote!(#smithay::reexports::wayland_server);

    let Resource { interface, data } = &resource;

    let (impl_generics, _type_generics, where_clause) = generics.split_for_impl();
    let trait_name = quote!(#wayland_server::Dispatch<#interface, #data>);

    quote! {
        #[automatically_derived]
        impl #impl_generics #trait_name for #self_ty #where_clause {
            fn request(
                state: &mut Self,
                client: &#wayland_server::Client,
                resource: &#interface,
                request: <#interface as #wayland_server::Resource>::Request,
                data: &#data,
                dhandle: &#wayland_server::DisplayHandle,
                data_init: &mut #wayland_server::DataInit<'_, Self>,
            ) {
                <#dispatch_to as #wayland_server::Dispatch<#interface, #data, Self>>::request(
                    state, client, resource, request, data, dhandle, data_init,
                )
            }

            fn destroyed(
                state: &mut Self,
                client: #wayland_server::backend::ClientId,
                resource: &#interface,
                data: &#data,
            ) {
                <#dispatch_to as #wayland_server::Dispatch<#interface, #data, Self>>::destroyed(
                    state, client, resource, data,
                )
            }
        }
    }
}
