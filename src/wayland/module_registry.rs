use proc_macro2::TokenStream as TokenStream2;
use std::collections::HashMap;

#[derive(Debug)]
pub struct ModuleRegistry {
    modules: HashMap<String, ModuleDescriptor>,
}

impl Default for ModuleRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            modules: HashMap::new(),
        };

        registry.register(super::compositor::descriptor());
        registry.register(super::output::descriptor());
        registry.register(super::shm::descriptor());
        registry
    }

    pub fn get(&self, name: &str) -> Option<&ModuleDescriptor> {
        self.modules.get(name)
    }

    fn register(&mut self, m: ModuleDescriptor) {
        self.modules.insert(m.name.to_string(), m);
    }
}

#[doc(hidden)]
#[macro_export]
macro_rules! smithay {
    () => {{
        match ::proc_macro_crate::crate_name("smithay").expect("smithay should be present in `Cargo.toml`") {
            ::proc_macro_crate::FoundCrate::Itself => ::quote::quote!(crate),
            ::proc_macro_crate::FoundCrate::Name(name) => {
                let ident = ::proc_macro2::Ident::new(&name, proc_macro2::Span::call_site());
                quote::quote!( #ident )
            }
        }
    }};
    ($tt:path) => {{
        let smithay = smithay!();
        ::quote::quote!(#smithay::$tt)
    }}
}
pub use smithay;

macro_rules! wayland_server {
    () => {{
        use crate::wayland::module_registry::smithay;
        smithay!(reexports::wayland_server)
    }};
    ($tt:path) => {{
        let wayland_server = wayland_server!();
        quote::quote!(#wayland_server::$tt)
    }};
}
pub(crate) use wayland_server;

macro_rules! wayland_core {
    () => {{
        use crate::wayland::module_registry::wayland_server;
        wayland_server!(protocol)
    }};
    ($tt:path) => {{
        let wayland_core = wayland_core!();
        quote::quote!(#wayland_core::$tt)
    }};
}
pub(crate) use wayland_core;

macro_rules! wayland_protocols {
    () => {{
        use crate::wayland::module_registry::smithay;
        smithay!(reexports::wayland_protocols)
    }};
    ($tt:path) => {{
        let wayland_protocols = crate::wayland::module_registry::wayland_protocols!();
        ::quote::quote!(#wayland_protocols::$tt)
    }};
}
pub(crate) use wayland_protocols;

#[derive(Debug)]
pub struct ModuleDescriptor {
    pub name: TokenStream2,
    pub dispatch_to: TokenStream2,
    pub globals: Vec<Global>,
    pub resources: Vec<Resource>,
}

#[derive(Debug)]
pub struct Global {
    pub interface: TokenStream2,
    pub data: TokenStream2,
}

#[derive(Debug)]
pub struct Resource {
    pub interface: TokenStream2,
    pub data: TokenStream2,
}
