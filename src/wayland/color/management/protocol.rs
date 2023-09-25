// Re-export only the actual code, and then only use this re-export
// The `generated` module below is just some boilerplate to properly isolate stuff
// and avoid exposing internal details.
//
// You can use all the types from my_protocol as if they went from `wayland_client::protocol`.
pub use generated::{
    wp_color_management_output_v1, wp_color_management_surface_v1, wp_color_manager_v1,
    wp_image_description_creator_icc_v1, wp_image_description_creator_params_v1, wp_image_description_v1,
};

#[allow(non_upper_case_globals, non_snake_case, non_camel_case_types)]
mod generated {
    use wayland_server::{self, protocol::*};

    pub mod __interfaces {
        use wayland_backend;
        use wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/color-management-v1.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("protocols/color-management-v1.xml");
}
