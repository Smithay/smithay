mod generated {
    include!("./generated/pnp_ids.rs");
}

pub fn pnp_id_to_name(vendor: &[char; 3]) -> Option<&'static str> {
    generated::pnp_id_to_name(vendor)
}
