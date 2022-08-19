use wayland_server::{backend::ObjectId, Resource};

crate::utils::ids::id_gen!(next_external_id, EXTERNAL_ID, EXTERNAL_IDS);

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
/// A unique id
pub struct Id(InnerId);

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
enum InnerId {
    WaylandResource(ObjectId),
    External(usize),
}

impl Id {
    /// Create an id from a [`Resource`]
    ///
    /// Note that every call for the same resource will
    /// return the same id.
    pub fn from_wayland_resource<R: Resource>(resource: &R) -> Self {
        Id(InnerId::WaylandResource(resource.id()))
    }

    /// Create a new unique id
    pub fn new() -> Self {
        Id(InnerId::External(next_external_id()))
    }
}
