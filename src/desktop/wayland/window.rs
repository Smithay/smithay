use crate::{
    backend::input::KeyState,
    desktop::{space::RenderZindex, utils::*, PopupManager},
    input::{
        keyboard::{KeyboardTarget, KeysymHandle, ModifiersState},
        pointer::{AxisFrame, ButtonEvent, MotionEvent, PointerTarget},
        Seat, SeatHandler,
    },
    output::Output,
    utils::{user_data::UserDataMap, IsAlive, Logical, Point, Rectangle, Serial},
    wayland::{
        compositor::{with_states, SurfaceData},
        seat::WaylandFocus,
        shell::xdg::{SurfaceCachedState, ToplevelSurface},
    },
};
use std::{
    hash::{Hash, Hasher},
    sync::{
        atomic::{AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use wayland_protocols::{
    wp::presentation_time::server::wp_presentation_feedback, xdg::shell::server::xdg_toplevel,
};
use wayland_server::protocol::wl_surface;

crate::utils::ids::id_gen!(next_window_id, WINDOW_ID, WINDOW_IDS);

#[derive(Debug)]
pub(crate) struct WindowInner {
    pub(crate) id: usize,
    toplevel: ToplevelSurface,
    bbox: Mutex<Rectangle<i32, Logical>>,
    pub(crate) z_index: AtomicU8,
    focused_surface: Mutex<Option<wl_surface::WlSurface>>,
    user_data: UserDataMap,
}

impl Drop for WindowInner {
    fn drop(&mut self) {
        WINDOW_IDS.lock().unwrap().remove(&self.id);
    }
}

/// Represents a single application window
#[derive(Debug, Clone)]
pub struct Window(pub(crate) Arc<WindowInner>);

impl PartialEq for Window {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for Window {}

impl Hash for Window {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id.hash(state);
    }
}

impl IsAlive for Window {
    fn alive(&self) -> bool {
        self.0.toplevel.alive()
    }
}

bitflags::bitflags! {
    /// Defines the surface types that can be
    /// queried with [`Window::surface_under`]
    pub struct WindowSurfaceType: u32 {
        /// Include the toplevel surface
        const TOPLEVEL = 1;
        /// Include all subsurfaces
        const SUBSURFACE = 2;
        /// Include all popup surfaces
        const POPUP = 4;
        /// Query all surfaces
        const ALL = Self::TOPLEVEL.bits | Self::SUBSURFACE.bits | Self::POPUP.bits;
    }
}

impl Window {
    /// Construct a new [`Window`] from a xdg toplevel surface
    pub fn new(toplevel: ToplevelSurface) -> Window {
        let id = next_window_id();

        Window(Arc::new(WindowInner {
            id,
            toplevel,
            bbox: Mutex::new(Rectangle::from_loc_and_size((0, 0), (0, 0))),
            z_index: AtomicU8::new(RenderZindex::Shell as u8),
            focused_surface: Mutex::new(None),
            user_data: UserDataMap::new(),
        }))
    }

    /// Returns the geometry of this window.
    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        // It's the set geometry with the full bounding box as the fallback.
        with_states(self.0.toplevel.wl_surface(), |states| {
            states.cached_state.current::<SurfaceCachedState>().geometry
        })
        .unwrap_or_else(|| self.bbox())
    }

    /// Returns a bounding box over this window and its children.
    pub fn bbox(&self) -> Rectangle<i32, Logical> {
        *self.0.bbox.lock().unwrap()
    }

    /// Returns a bounding box over this window and children including popups.
    ///
    /// Note: You need to use a [`PopupManager`] to track popups, otherwise the bounding box
    /// will not include the popups.
    pub fn bbox_with_popups(&self) -> Rectangle<i32, Logical> {
        let mut bounding_box = self.bbox();
        let surface = self.0.toplevel.wl_surface();
        for (popup, location) in PopupManager::popups_for_surface(surface) {
            let surface = popup.wl_surface();
            let offset = self.geometry().loc + location - popup.geometry().loc;
            bounding_box = bounding_box.merge(bbox_from_surface_tree(surface, offset));
        }

        bounding_box
    }

    /// Activate/Deactivate this window
    pub fn set_activated(&self, active: bool) -> bool {
        self.0.toplevel.with_pending_state(|state| {
            if active {
                state.states.set(xdg_toplevel::State::Activated)
            } else {
                state.states.unset(xdg_toplevel::State::Activated)
            }
        })
    }

    /// Sends the frame callback to all the subsurfaces in this window that requested it
    ///
    /// See [`send_frames_surface_tree`] for more information
    pub fn send_frame<T, F>(
        &self,
        output: &Output,
        time: T,
        throttle: Option<Duration>,
        primary_scan_out_output: F,
    ) where
        T: Into<Duration>,
        F: FnMut(&wl_surface::WlSurface, &SurfaceData) -> Option<Output> + Copy,
    {
        let time = time.into();
        let surface = self.0.toplevel.wl_surface();
        send_frames_surface_tree(surface, output, time, throttle, primary_scan_out_output);
        for (popup, _) in PopupManager::popups_for_surface(surface) {
            let surface = popup.wl_surface();
            send_frames_surface_tree(surface, output, time, throttle, primary_scan_out_output);
        }
    }

    /// Takes the [`PresentationFeedbackCallback`]s from all subsurfaces in this window
    ///
    /// see [`take_presentation_feedback_surface_tree`] for more information
    pub fn take_presentation_feedback<F1, F2>(
        &self,
        output_feedback: &mut OutputPresentationFeedback,
        primary_scan_out_output: F1,
        presentation_feedback_flags: F2,
    ) where
        F1: FnMut(&wl_surface::WlSurface, &SurfaceData) -> Option<Output> + Copy,
        F2: FnMut(&wl_surface::WlSurface, &SurfaceData) -> wp_presentation_feedback::Kind + Copy,
    {
        let surface = self.0.toplevel.wl_surface();
        take_presentation_feedback_surface_tree(
            surface,
            output_feedback,
            primary_scan_out_output,
            presentation_feedback_flags,
        );
        for (popup, _) in PopupManager::popups_for_surface(surface) {
            let surface = popup.wl_surface();
            take_presentation_feedback_surface_tree(
                surface,
                output_feedback,
                primary_scan_out_output,
                presentation_feedback_flags,
            );
        }
    }

    /// Run a closure on all surfaces in this window (including it's popups, if [`PopupManager`] is used)
    pub fn with_surfaces<F>(&self, processor: F)
    where
        F: FnMut(&wl_surface::WlSurface, &SurfaceData) + Copy,
    {
        let surface = self.0.toplevel.wl_surface();
        with_surfaces_surface_tree(surface, processor);
        for (popup, _) in PopupManager::popups_for_surface(surface) {
            let surface = popup.wl_surface();
            with_surfaces_surface_tree(surface, processor);
        }
    }

    /// Updates internal values
    ///
    /// Needs to be called whenever the toplevel surface or any unsynchronized subsurfaces of this window are updated
    /// to correctly update the bounding box of this window.
    pub fn on_commit(&self) {
        let surface = self.0.toplevel.wl_surface();
        *self.0.bbox.lock().unwrap() = bbox_from_surface_tree(surface, (0, 0));
    }

    /// Finds the topmost surface under this point matching the input regions of the surface and returns
    /// it together with the location of this surface.
    ///
    /// In case no surface input region matches the point [`None`] is returned.
    ///
    /// - `point` should be relative to (0,0) of the window.
    pub fn surface_under<P: Into<Point<f64, Logical>>>(
        &self,
        point: P,
        surface_type: WindowSurfaceType,
    ) -> Option<(wl_surface::WlSurface, Point<i32, Logical>)> {
        let point = point.into();
        let surface = self.0.toplevel.wl_surface();
        if surface_type.contains(WindowSurfaceType::POPUP) {
            for (popup, location) in PopupManager::popups_for_surface(surface) {
                let offset = self.geometry().loc + location - popup.geometry().loc;
                if let Some(result) = under_from_surface_tree(popup.wl_surface(), point, offset, surface_type)
                {
                    return Some(result);
                }
            }
        }

        under_from_surface_tree(surface, point, (0, 0), surface_type)
    }

    /// Returns the underlying xdg toplevel surface
    pub fn toplevel(&self) -> &ToplevelSurface {
        &self.0.toplevel
    }

    /// Override the z_index of this Window
    pub fn override_z_index(&self, z_index: u8) {
        self.0.z_index.store(z_index, Ordering::SeqCst);
    }

    /// Returns a [`UserDataMap`] to allow associating arbitrary data with this window.
    pub fn user_data(&self) -> &UserDataMap {
        &self.0.user_data
    }
}

impl<D: SeatHandler + 'static> PointerTarget<D> for Window {
    fn enter(&self, seat: &Seat<D>, data: &mut D, event: &MotionEvent) {
        if let Some((surface, loc)) = self.surface_under(event.location, WindowSurfaceType::ALL) {
            let mut new_event = event.clone();
            new_event.location -= loc.to_f64();
            if let Some(old_surface) = self.0.focused_surface.lock().unwrap().replace(surface.clone()) {
                if old_surface != surface {
                    PointerTarget::<D>::leave(&old_surface, seat, data, event.serial, event.time);
                    PointerTarget::<D>::enter(&surface, seat, data, &new_event);
                } else {
                    PointerTarget::<D>::motion(&surface, seat, data, &new_event)
                }
            } else {
                PointerTarget::<D>::enter(&surface, seat, data, &new_event)
            }
        }
    }
    fn motion(&self, seat: &Seat<D>, data: &mut D, event: &MotionEvent) {
        PointerTarget::<D>::enter(self, seat, data, event)
    }
    fn button(&self, seat: &Seat<D>, data: &mut D, event: &ButtonEvent) {
        if let Some(surface) = self.0.focused_surface.lock().unwrap().as_ref() {
            PointerTarget::<D>::button(surface, seat, data, event)
        }
    }
    fn axis(&self, seat: &Seat<D>, data: &mut D, frame: AxisFrame) {
        if let Some(surface) = self.0.focused_surface.lock().unwrap().as_ref() {
            PointerTarget::<D>::axis(surface, seat, data, frame)
        }
    }
    fn leave(&self, seat: &Seat<D>, data: &mut D, serial: Serial, time: u32) {
        if let Some(surface) = self.0.focused_surface.lock().unwrap().take() {
            PointerTarget::<D>::leave(&surface, seat, data, serial, time)
        }
    }
}

impl<D: SeatHandler + 'static> KeyboardTarget<D> for Window {
    fn enter(&self, seat: &Seat<D>, data: &mut D, keys: Vec<KeysymHandle<'_>>, serial: Serial) {
        KeyboardTarget::<D>::enter(self.0.toplevel.wl_surface(), seat, data, keys, serial)
    }
    fn leave(&self, seat: &Seat<D>, data: &mut D, serial: Serial) {
        KeyboardTarget::<D>::leave(self.0.toplevel.wl_surface(), seat, data, serial)
    }
    fn key(
        &self,
        seat: &Seat<D>,
        data: &mut D,
        key: KeysymHandle<'_>,
        state: KeyState,
        serial: Serial,
        time: u32,
    ) {
        KeyboardTarget::<D>::key(self.0.toplevel.wl_surface(), seat, data, key, state, serial, time)
    }
    fn modifiers(&self, seat: &Seat<D>, data: &mut D, modifiers: ModifiersState, serial: Serial) {
        KeyboardTarget::<D>::modifiers(self.0.toplevel.wl_surface(), seat, data, modifiers, serial)
    }
}

impl WaylandFocus for Window {
    fn wl_surface(&self) -> Option<wl_surface::WlSurface> {
        Some(self.0.toplevel.wl_surface().clone())
    }
}
