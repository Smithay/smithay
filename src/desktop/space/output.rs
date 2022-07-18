use crate::{
    backend::renderer::{ImportAll, Renderer},
    desktop::space::{RenderElement, SpaceElement},
    utils::{Logical, Physical, Point, Rectangle},
    wayland::output::Output,
};
use indexmap::IndexMap;
use wayland_server::backend::ObjectId;

use std::{
    any::TypeId,
    cell::{RefCell, RefMut},
    collections::{HashMap, HashSet, VecDeque},
};

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct ToplevelId {
    t_id: TypeId,
    id: usize,
}

impl<'a, R, E> From<&SpaceElement<'a, R, E>> for ToplevelId
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
    E: RenderElement<R>,
{
    fn from(elem: &SpaceElement<'a, R, E>) -> ToplevelId {
        ToplevelId {
            t_id: elem.type_of(),
            id: elem.id(),
        }
    }
}

#[derive(Clone, Default)]
pub struct OutputState {
    pub location: Point<i32, Logical>,

    // damage and last_toplevel_state are in space coordinate space
    // old_damage represents the damage from the last n render iterations
    // used to track the damage for different buffer ages
    pub old_damage: VecDeque<Vec<Rectangle<i32, Physical>>>,
    // z_index and physical geometry of the toplevels from the last render iteration
    pub last_toplevel_state: IndexMap<ToplevelId, (usize, Rectangle<i32, Physical>)>,
    // output geometry from the last render iteration
    // used to react on output geometry changes, like damaging
    // the whole output
    pub last_output_geo: Option<Rectangle<i32, Physical>>,

    // surfaces for tracking enter and leave events
    pub surfaces: HashSet<ObjectId>,
}

pub type OutputUserdata = RefCell<HashMap<usize, OutputState>>;
pub fn output_state(space: usize, o: &Output) -> RefMut<'_, OutputState> {
    let userdata = o.user_data();
    userdata.insert_if_missing(OutputUserdata::default);
    RefMut::map(userdata.get::<OutputUserdata>().unwrap().borrow_mut(), |m| {
        m.entry(space).or_default()
    })
}
