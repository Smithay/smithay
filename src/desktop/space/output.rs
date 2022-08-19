use crate::{
    output::Output,
    utils::{Logical, Point},
};

use std::{
    cell::{RefCell, RefMut},
    collections::HashMap,
};

#[derive(Clone, Default)]
pub struct OutputState {
    pub location: Point<i32, Logical>,
}

pub type OutputUserdata = RefCell<HashMap<usize, OutputState>>;
pub fn output_state(space: usize, o: &Output) -> RefMut<'_, OutputState> {
    let userdata = o.user_data();
    userdata.insert_if_missing(OutputUserdata::default);
    RefMut::map(userdata.get::<OutputUserdata>().unwrap().borrow_mut(), |m| {
        m.entry(space).or_default()
    })
}
