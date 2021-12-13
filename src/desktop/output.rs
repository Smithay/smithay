use crate::{
    utils::{Logical, Point, Rectangle},
    wayland::output::Output,
};
use indexmap::IndexMap;
use wayland_server::protocol::wl_surface::WlSurface;

use std::{
    cell::{RefCell, RefMut},
    collections::{HashMap, VecDeque},
};

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub(super) enum ToplevelId {
    Xdg(usize),
}

impl ToplevelId {
    pub fn is_xdg(&self) -> bool {
        match self {
            ToplevelId::Xdg(_) => true,
            _ => false,
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct OutputState {
    pub location: Point<i32, Logical>,
    pub render_scale: f64,

    // damage and last_state are in space coordinate space
    pub old_damage: VecDeque<Vec<Rectangle<i32, Logical>>>,
    pub last_state: IndexMap<ToplevelId, Rectangle<i32, Logical>>,

    // surfaces for tracking enter and leave events
    pub surfaces: Vec<WlSurface>,
}

pub(super) type OutputUserdata = RefCell<HashMap<usize, OutputState>>;
pub(super) fn output_state(space: usize, o: &Output) -> RefMut<'_, OutputState> {
    let userdata = o.user_data();
    userdata.insert_if_missing(OutputUserdata::default);
    RefMut::map(userdata.get::<OutputUserdata>().unwrap().borrow_mut(), |m| {
        m.entry(space).or_default()
    })
}
