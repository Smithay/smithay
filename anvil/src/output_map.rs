use std::{cell::RefCell, rc::Rc};

use smithay::{
    reexports::{
        wayland_protocols::xdg_shell::server::xdg_toplevel,
        wayland_server::{
            protocol::{wl_output, wl_surface::WlSurface},
            Display, Global,
        },
    },
    utils::Rectangle,
    wayland::{
        compositor::{with_surface_tree_downward, SubsurfaceCachedState, TraversalAction},
        output::{self, Mode, PhysicalProperties},
    },
};

use crate::shell::SurfaceData;

struct Output {
    name: String,
    output: output::Output,
    global: Option<Global<wl_output::WlOutput>>,
    geometry: Rectangle,
    surfaces: Vec<WlSurface>,
    current_mode: Mode,
}

impl Output {
    fn new<N>(
        name: N,
        location: (i32, i32),
        display: &mut Display,
        physical: PhysicalProperties,
        mode: Mode,
        logger: slog::Logger,
    ) -> Self
    where
        N: AsRef<str>,
    {
        let (output, global) = output::Output::new(display, name.as_ref().into(), physical, logger);

        output.change_current_state(Some(mode), None, None);
        output.set_preferred(mode);

        Self {
            name: name.as_ref().to_owned(),
            global: Some(global),
            output,
            geometry: Rectangle {
                x: location.0,
                y: location.1,
                width: mode.width,
                height: mode.height,
            },
            surfaces: Vec::new(),
            current_mode: mode,
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.global.take().unwrap().destroy();
    }
}

#[derive(Debug)]
pub struct OutputNotFound;

impl std::fmt::Display for OutputNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("The output could not be found")
    }
}

impl std::error::Error for OutputNotFound {}

pub struct OutputMap {
    display: Rc<RefCell<Display>>,
    outputs: Vec<Output>,
    window_map: Rc<RefCell<crate::window_map::WindowMap>>,
    logger: slog::Logger,
}

impl OutputMap {
    pub fn new(
        display: Rc<RefCell<Display>>,
        window_map: Rc<RefCell<crate::window_map::WindowMap>>,
        logger: ::slog::Logger,
    ) -> Self {
        Self {
            display,
            outputs: Vec::new(),
            window_map,
            logger,
        }
    }

    pub fn arrange(&mut self) {
        // First recalculate the outputs location
        let mut output_x = 0;
        for output in self.outputs.iter_mut() {
            output.geometry.x = output_x;
            output.geometry.y = 0;
            output_x += output.geometry.width;
        }

        // Check if any windows are now out of outputs range
        // and move them to the primary output
        let primary_output_location = self
            .with_primary(|_, geometry| geometry)
            .ok()
            .map(|o| (o.x, o.y))
            .unwrap_or_default();
        let mut window_map = self.window_map.borrow_mut();
        // TODO: This is a bit unfortunate, we save the windows in a temp vector
        // cause we can not call window_map.set_location within the closure.
        let mut windows_to_move = Vec::new();
        window_map.with_windows_from_bottom_to_top(|kind, _, bbox| {
            let within_outputs = self.outputs.iter().any(|o| o.geometry.overlaps(bbox));

            if !within_outputs {
                windows_to_move.push((kind.to_owned(), primary_output_location));
            }
        });
        for (window, location) in windows_to_move.drain(..) {
            window_map.set_location(&window, location);
        }

        // Update the size and location for maximized and fullscreen windows
        window_map.with_windows_from_bottom_to_top(|kind, location, _| {
            if let crate::window_map::Kind::Xdg(xdg) = kind {
                if let Some(state) = xdg.current_state() {
                    if state.states.contains(xdg_toplevel::State::Maximized)
                        || state.states.contains(xdg_toplevel::State::Fullscreen)
                    {
                        let output_geometry = if let Some(output) = state.fullscreen_output.as_ref() {
                            self.find(output, |_, geometry| geometry).ok()
                        } else {
                            self.find_by_position(location, |_, geometry| geometry).ok()
                        };

                        if let Some(geometry) = output_geometry {
                            if location != (geometry.x, geometry.y) {
                                windows_to_move.push((kind.to_owned(), (geometry.x, geometry.y)));
                            }

                            let res = xdg.with_pending_state(|pending_state| {
                                pending_state.size = Some((geometry.width, geometry.height));
                            });

                            if res.is_ok() {
                                xdg.send_configure();
                            }
                        }
                    }
                }
            }
        });
        for (window, location) in windows_to_move.drain(..) {
            window_map.set_location(&window, location);
        }
    }

    pub fn add<N>(&mut self, name: N, physical: PhysicalProperties, mode: Mode)
    where
        N: AsRef<str>,
    {
        // Append the output to the end of the existing
        // outputs by placing it after the current overall
        // width
        let location = (self.width() as i32, 0);

        let output = Output::new(
            name,
            location,
            &mut *self.display.borrow_mut(),
            physical,
            mode,
            self.logger.clone(),
        );

        self.outputs.push(output);

        // We call arrange here albeit the output is only appended and
        // this would not affect windows, but arrange could re-organize
        // outputs from a configuration.
        self.arrange();
    }

    pub fn remove<N: AsRef<str>>(&mut self, name: N) {
        let removed_outputs = self.outputs.iter_mut().filter(|o| o.name == name.as_ref());

        for output in removed_outputs {
            for surface in output.surfaces.drain(..) {
                output.output.leave(&surface);
            }
        }
        self.outputs.retain(|o| o.name != name.as_ref());

        // Re-arrange outputs cause one or more outputs have
        // been removed
        self.arrange();
    }

    pub fn width(&self) -> u32 {
        // This is a simplification, we only arrange the outputs on the y axis side-by-side
        // so that the total width is simply the sum of all output widths.
        self.outputs
            .iter()
            .fold(0u32, |acc, output| acc + output.geometry.width as u32)
    }

    pub fn height(&self) -> u32 {
        // This is a simplification, we only arrange the outputs on the y axis side-by-side
        // so that the max height is simply the max of all output heights.
        self.outputs
            .iter()
            .map(|output| output.geometry.height as u32)
            .max()
            .unwrap_or_default()
    }

    pub fn size(&self) -> (u32, u32) {
        (self.width(), self.height())
    }

    pub fn is_empty(&self) -> bool {
        self.outputs.is_empty()
    }

    pub fn with_primary<F, T>(&self, f: F) -> Result<T, OutputNotFound>
    where
        F: FnOnce(&output::Output, Rectangle) -> T,
    {
        let output = self.outputs.get(0).ok_or(OutputNotFound)?;

        Ok(f(&output.output, output.geometry))
    }

    pub fn find<F, T>(&self, output: &wl_output::WlOutput, f: F) -> Result<T, OutputNotFound>
    where
        F: FnOnce(&output::Output, Rectangle) -> T,
    {
        let output = self
            .outputs
            .iter()
            .find(|o| o.output.owns(output))
            .ok_or(OutputNotFound)?;

        Ok(f(&output.output, output.geometry))
    }

    pub fn find_by_name<N, F, T>(&self, name: N, f: F) -> Result<T, OutputNotFound>
    where
        N: AsRef<str>,
        F: FnOnce(&output::Output, Rectangle) -> T,
    {
        let output = self
            .outputs
            .iter()
            .find(|o| o.name == name.as_ref())
            .ok_or(OutputNotFound)?;

        Ok(f(&output.output, output.geometry))
    }

    pub fn find_by_position<F, T>(&self, position: (i32, i32), f: F) -> Result<T, OutputNotFound>
    where
        F: FnOnce(&output::Output, Rectangle) -> T,
    {
        let output = self
            .outputs
            .iter()
            .find(|o| o.geometry.contains(position))
            .ok_or(OutputNotFound)?;

        Ok(f(&output.output, output.geometry))
    }

    pub fn find_by_index<F, T>(&self, index: usize, f: F) -> Result<T, OutputNotFound>
    where
        F: FnOnce(&output::Output, Rectangle) -> T,
    {
        let output = self.outputs.get(index).ok_or(OutputNotFound)?;

        Ok(f(&output.output, output.geometry))
    }

    pub fn update_mode<N: AsRef<str>>(&mut self, name: N, mode: Mode) {
        let output = self.outputs.iter_mut().find(|o| o.name == name.as_ref());

        // NOTE: This will just simply shift all outputs after
        // the output who's mode has changed left or right depending
        // on if the mode width increased or decreased.
        // We could also re-configure toplevels here.
        // If a surface is now visible on an additional output because
        // the output width decreased the refresh method will take
        // care and will send enter for the output.
        if let Some(output) = output {
            output.geometry.width = mode.width;
            output.geometry.height = mode.height;

            output.output.delete_mode(output.current_mode);
            output.output.change_current_state(Some(mode), None, None);
            output.output.set_preferred(mode);
            output.current_mode = mode;

            // Re-arrange outputs cause the size of one output changed
            self.arrange();
        }
    }

    pub fn refresh(&mut self) {
        // Clean-up dead surfaces
        self.outputs
            .iter_mut()
            .for_each(|o| o.surfaces.retain(|s| s.as_ref().is_alive()));

        let window_map = self.window_map.clone();

        window_map
            .borrow()
            .with_windows_from_bottom_to_top(|kind, location, bbox| {
                for output in self.outputs.iter_mut() {
                    // Check if the bounding box of the toplevel intersects with
                    // the output, if not no surface in the tree can intersect with
                    // the output.
                    if !output.geometry.overlaps(bbox) {
                        if let Some(surface) = kind.get_surface() {
                            with_surface_tree_downward(
                                surface,
                                (),
                                |_, _, _| TraversalAction::DoChildren(()),
                                |wl_surface, _, _| {
                                    if output.surfaces.contains(wl_surface) {
                                        output.output.leave(wl_surface);
                                        output.surfaces.retain(|s| s != wl_surface);
                                    }
                                },
                                |_, _, _| true,
                            )
                        }
                        continue;
                    }

                    if let Some(surface) = kind.get_surface() {
                        with_surface_tree_downward(
                            surface,
                            location,
                            |_, states, &(mut x, mut y)| {
                                let data = states.data_map.get::<RefCell<SurfaceData>>();

                                if data.is_some() {
                                    if states.role == Some("subsurface") {
                                        let current = states.cached_state.current::<SubsurfaceCachedState>();
                                        x += current.location.0;
                                        y += current.location.1;
                                    }

                                    TraversalAction::DoChildren((x, y))
                                } else {
                                    // If the parent surface is unmapped, then the child surfaces are hidden as
                                    // well, no need to consider them here.
                                    TraversalAction::SkipChildren
                                }
                            },
                            |wl_surface, states, &(x, y)| {
                                let data = states.data_map.get::<RefCell<SurfaceData>>();

                                if let Some((width, height)) = data.and_then(|d| d.borrow().size()) {
                                    let surface_rectangle = Rectangle { x, y, width, height };

                                    if output.geometry.overlaps(&surface_rectangle) {
                                        // We found a matching output, check if we already sent enter
                                        if !output.surfaces.contains(wl_surface) {
                                            output.output.enter(wl_surface);
                                            output.surfaces.push(wl_surface.clone());
                                        }
                                    } else {
                                        // Surface does not match output, if we sent enter earlier
                                        // we should now send leave
                                        if output.surfaces.contains(wl_surface) {
                                            output.output.leave(wl_surface);
                                            output.surfaces.retain(|s| s != wl_surface);
                                        }
                                    }
                                } else {
                                    // Maybe the the surface got unmapped, send leave on output
                                    if output.surfaces.contains(wl_surface) {
                                        output.output.leave(wl_surface);
                                        output.surfaces.retain(|s| s != wl_surface);
                                    }
                                }
                            },
                            |_, _, _| true,
                        )
                    }
                }
            });
    }
}
