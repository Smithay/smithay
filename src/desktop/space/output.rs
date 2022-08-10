use crate::{
    utils::{Logical, Point},
    wayland::output::Output,
};
use wayland_server::backend::ObjectId;

use std::{
    cell::{RefCell, RefMut},
    collections::{HashMap, HashSet},
};

#[derive(Clone, Default)]
pub struct OutputState {
    pub location: Point<i32, Logical>,
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
