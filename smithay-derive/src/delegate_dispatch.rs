use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use smithay::wayland::module_registry::{Global, ModuleDescriptor, Resource};

pub(crate) fn delegate_module(
    ident: &syn::Ident,
    generics: &syn::Generics,
    module: &ModuleDescriptor,
) -> TokenStream2 {
    let ModuleDescriptor {
        dispatch_to,
        globals,
        resources,
        ..
    } = module;

    let globals = globals.iter().map(|Global { interface, data }| {
        delegate_global_dispatch(ident, generics, dispatch_to, interface, data)
    });
    let resources = resources
        .iter()
        .map(|Resource { interface, data }| delegate_dispatch(ident, generics, dispatch_to, interface, data));

    quote! {
        #(#globals)*
        #(#resources)*
    }
}

pub(crate) fn delegate_global_dispatch(
    ident: &syn::Ident,
    generics: &syn::Generics,

    dispatch_to: &TokenStream2,
    interface: &TokenStream2,
    data: &TokenStream2,
) -> TokenStream2 {
    use smithay::wayland::module_registry::smithay;
    let wayland_server = smithay!(reexports::wayland_server);

    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    let trait_name = quote!(#wayland_server::GlobalDispatch<#interface, #data>);

    quote! {
        #[automatically_derived]
        impl #impl_generics #trait_name for #ident #type_generics #where_clause {
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
    ident: &syn::Ident,
    generics: &syn::Generics,

    dispatch_to: &TokenStream2,
    interface: &TokenStream2,
    data: &TokenStream2,
) -> TokenStream2 {
    use smithay::wayland::module_registry::smithay;
    let wayland_server = smithay!(reexports::wayland_server);

    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    let trait_name = quote!(#wayland_server::Dispatch<#interface, #data>);

    quote! {
        #[automatically_derived]
        impl #impl_generics #trait_name for #ident #type_generics #where_clause {
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
                resource: #wayland_server::backend::ObjectId,
                data: &#data,
            ) {
                <#dispatch_to as #wayland_server::Dispatch<#interface, #data, Self>>::destroyed(
                    state, client, resource, data,
                )
            }
        }
    }
}
