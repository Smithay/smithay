#![cfg(not(feature = "egl"))] // TODO: headless EGL support in Anvil::start().

extern crate anvil;

use slog::info;
use smithay::backend::input::MouseButton;

mod common;
use common::*;

#[test]
fn move_test() {
    let mut anvil = Anvil::start();
    let log = anvil.log();

    anvil.spawn_client("weston-terminal");
    anvil.wait_for_surface_map();

    let (x, y) = anvil.window_location();
    let geometry = anvil.window_geometry();
    info!(log, "Window"; "location" => ?(x, y), "geometry" => ?geometry);

    // There should be draggable window frame area there.
    anvil.pointer_move(x + geometry.x + 10, y + geometry.y + 10);
    anvil.pointer_press(MouseButton::Left);
    anvil.wait_for_move();
    anvil.pointer_move(x + geometry.x, y + geometry.y);

    assert_eq!(anvil.window_location(), (x - 10, y - 10));
}
