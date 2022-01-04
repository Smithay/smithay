use crate::{
    backend::renderer::{Frame, ImportAll, Renderer, Texture},
    desktop::space::SpaceElement,
    utils::{Logical, Point, Rectangle},
    wayland::output::Output,
};
use indexmap::IndexMap;
use wayland_server::protocol::wl_surface::WlSurface;

use std::{
    any::TypeId,
    cell::{RefCell, RefMut},
    collections::{HashMap, VecDeque},
};

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct ToplevelId {
    t_id: TypeId,
    id: usize,
}

impl<R, F, E, T> From<&dyn SpaceElement<R, F, E, T>> for ToplevelId
where
    R: Renderer<Error = E, TextureId = T, Frame = F> + ImportAll + 'static,
    F: Frame<Error = E, TextureId = T> + 'static,
    E: std::error::Error + 'static,
    T: Texture + 'static,
{
    fn from(elem: &dyn SpaceElement<R, F, E, T>) -> ToplevelId {
        ToplevelId {
            t_id: elem.type_of(),
            id: elem.id(),
        }
    }
}

#[derive(Clone, Default)]
pub struct OutputState {
    pub location: Point<i32, Logical>,
    pub render_scale: f64,

    // damage and last_state are in space coordinate space
    pub old_damage: VecDeque<Vec<Rectangle<i32, Logical>>>,
    pub last_state: IndexMap<ToplevelId, Rectangle<i32, Logical>>,

    // surfaces for tracking enter and leave events
    pub surfaces: Vec<WlSurface>,
}

pub type OutputUserdata = RefCell<HashMap<usize, OutputState>>;
pub fn output_state(space: usize, o: &Output) -> RefMut<'_, OutputState> {
    let userdata = o.user_data();
    userdata.insert_if_missing(OutputUserdata::default);
    RefMut::map(userdata.get::<OutputUserdata>().unwrap().borrow_mut(), |m| {
        m.entry(space).or_default()
    })
}
