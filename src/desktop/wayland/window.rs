#[cfg(feature = "xwayland")]
use crate::{desktop::space::SpaceElement, xwayland::X11Surface};
use crate::{
    desktop::{space::RenderZindex, utils::*, PopupManager},
    output::Output,
    utils::{user_data::UserDataMap, IsAlive, Logical, Point, Rectangle},
    wayland::{
        compositor::{with_states, SurfaceData},
        dmabuf::DmabufFeedback,
        seat::WaylandFocus,
        shell::xdg::{SurfaceCachedState, ToplevelSurface},
    },
};
use std::{
    borrow::Cow,
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

/// Represents the surface of a [`Window`]
#[derive(Debug)]
pub enum WindowSurface {
    /// An xdg toplevel surface
    Wayland(ToplevelSurface),
    /// An X11 surface
    #[cfg(feature = "xwayland")]
    X11(X11Surface),
}

#[derive(Debug)]
pub(crate) struct WindowInner {
    pub(crate) id: usize,
    surface: WindowSurface,
    bbox: Mutex<Rectangle<i32, Logical>>,
    pub(crate) z_index: AtomicU8,
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
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for Window {}

impl Hash for Window {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id.hash(state);
    }
}

impl IsAlive for Window {
    #[inline]
    fn alive(&self) -> bool {
        match &self.0.surface {
            WindowSurface::Wayland(s) => s.alive(),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(s) => s.alive(),
        }
    }
}

bitflags::bitflags! {
    /// Defines the surface types that can be
    /// queried with [`Window::surface_under`]
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    pub struct WindowSurfaceType: u32 {
        /// Include the toplevel surface
        const TOPLEVEL = 1;
        /// Include all subsurfaces
        ///
        /// This value only works in addition to either `TOPLEVEL` or `POPUP`.
        const SUBSURFACE = 2;
        /// Include all popup surfaces
        const POPUP = 4;
        /// Query all surfaces
        const ALL = Self::TOPLEVEL.bits() | Self::SUBSURFACE.bits() | Self::POPUP.bits();
    }
}

impl Window {
    /// Construct a new [`Window`] from a xdg toplevel surface
    ///
    /// This function is deprecated. Use [`Window::new_wayland_window`] instead.
    #[deprecated]
    pub fn new(toplevel: ToplevelSurface) -> Window {
        Self::new_wayland_window(toplevel)
    }

    /// Construct a new [`Window`] from a xdg toplevel surface
    pub fn new_wayland_window(toplevel: ToplevelSurface) -> Window {
        let id = next_window_id();

        Window(Arc::new(WindowInner {
            id,
            surface: WindowSurface::Wayland(toplevel),
            bbox: Mutex::new(Rectangle::from_loc_and_size((0, 0), (0, 0))),
            z_index: AtomicU8::new(RenderZindex::Shell as u8),
            user_data: UserDataMap::new(),
        }))
    }

    /// Construct a new [`Window`] from an X11 surface
    #[cfg(feature = "xwayland")]
    pub fn new_x11_window(surface: X11Surface) -> Window {
        let id = next_window_id();

        Window(Arc::new(WindowInner {
            id,
            surface: WindowSurface::X11(surface),
            bbox: Mutex::new(Rectangle::from_loc_and_size((0, 0), (0, 0))),
            z_index: AtomicU8::new(RenderZindex::Shell as u8),
            user_data: UserDataMap::new(),
        }))
    }

    /// Checks if the window is a wayland one.
    #[inline]
    pub fn is_wayland(&self) -> bool {
        matches!(self.0.surface, WindowSurface::Wayland(_))
    }

    /// Checks if the window is an X11 one.
    #[cfg(feature = "xwayland")]
    #[inline]
    pub fn is_x11(&self) -> bool {
        matches!(self.0.surface, WindowSurface::X11(_))
    }

    /// Returns the geometry of this window.
    pub fn geometry(&self) -> Rectangle<i32, Logical> {
        let bbox = self.bbox();

        if let Some(surface) = self.wl_surface() {
            // It's the set geometry clamped to the bounding box with the full bounding box as the fallback.
            with_states(&surface, |states| {
                states
                    .cached_state
                    .get::<SurfaceCachedState>()
                    .current()
                    .geometry
                    .and_then(|geo| geo.intersection(bbox))
            })
            .unwrap_or(bbox)
        } else {
            bbox
        }
    }

    /// Returns a bounding box over this window and its children.
    pub fn bbox(&self) -> Rectangle<i32, Logical> {
        match &self.0.surface {
            WindowSurface::Wayland(_) => *self.0.bbox.lock().unwrap(),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(s) => s.bbox(),
        }
    }

    /// Returns a bounding box over this window and children including popups.
    ///
    /// Note: You need to use a [`PopupManager`] to track popups, otherwise the bounding box
    /// will not include the popups.
    pub fn bbox_with_popups(&self) -> Rectangle<i32, Logical> {
        let mut bounding_box = self.bbox();
        if let Some(surface) = self.wl_surface() {
            for (popup, location) in PopupManager::popups_for_surface(&surface) {
                let surface = popup.wl_surface();
                let offset = self.geometry().loc + location - popup.geometry().loc;
                bounding_box = bounding_box.merge(bbox_from_surface_tree(surface, offset));
            }
        }

        bounding_box
    }

    /// Activate/Deactivate this window
    pub fn set_activated(&self, active: bool) -> bool {
        match &self.0.surface {
            WindowSurface::Wayland(s) => s.with_pending_state(|state| {
                if active {
                    state.states.set(xdg_toplevel::State::Activated)
                } else {
                    state.states.unset(xdg_toplevel::State::Activated)
                }
            }),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(s) => {
                let already_activated = s.is_activated();
                if s.set_activated(active).is_ok() {
                    already_activated ^ active
                } else {
                    false
                }
            }
        }
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
        if let Some(surface) = self.wl_surface() {
            send_frames_surface_tree(&surface, output, time, throttle, primary_scan_out_output);
            for (popup, _) in PopupManager::popups_for_surface(&surface) {
                let surface = popup.wl_surface();
                send_frames_surface_tree(surface, output, time, throttle, primary_scan_out_output);
            }
        }
    }

    /// Sends the dmabuf feedback to all the subsurfaces in this window that requested it
    ///
    /// See [`send_dmabuf_feedback_surface_tree`] for more information
    pub fn send_dmabuf_feedback<'a, P, F>(
        &self,
        output: &Output,
        primary_scan_out_output: P,
        select_dmabuf_feedback: F,
    ) where
        P: FnMut(&wl_surface::WlSurface, &SurfaceData) -> Option<Output> + Copy,
        F: Fn(&wl_surface::WlSurface, &SurfaceData) -> &'a DmabufFeedback + Copy,
    {
        if let Some(surface) = self.wl_surface() {
            send_dmabuf_feedback_surface_tree(
                &surface,
                output,
                primary_scan_out_output,
                select_dmabuf_feedback,
            );
            for (popup, _) in PopupManager::popups_for_surface(&surface) {
                let surface = popup.wl_surface();
                send_dmabuf_feedback_surface_tree(
                    surface,
                    output,
                    primary_scan_out_output,
                    select_dmabuf_feedback,
                );
            }
        }
    }

    /// Takes the [`PresentationFeedbackCallback`](crate::wayland::presentation::PresentationFeedbackCallback)s from all subsurfaces in this window
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
        if let Some(surface) = self.wl_surface() {
            take_presentation_feedback_surface_tree(
                &surface,
                output_feedback,
                primary_scan_out_output,
                presentation_feedback_flags,
            );
            for (popup, _) in PopupManager::popups_for_surface(&surface) {
                let surface = popup.wl_surface();
                take_presentation_feedback_surface_tree(
                    surface,
                    output_feedback,
                    primary_scan_out_output,
                    presentation_feedback_flags,
                );
            }
        }
    }

    /// Run a closure on all surfaces in this window (including it's popups, if [`PopupManager`] is used)
    pub fn with_surfaces<F>(&self, mut processor: F)
    where
        F: FnMut(&wl_surface::WlSurface, &SurfaceData),
    {
        if let Some(surface) = self.wl_surface() {
            with_surfaces_surface_tree(&surface, &mut processor);
            for (popup, _) in PopupManager::popups_for_surface(&surface) {
                let surface = popup.wl_surface();
                with_surfaces_surface_tree(surface, &mut processor);
            }
        }
    }

    /// Updates internal values
    ///
    /// Needs to be called whenever the toplevel surface or any unsynchronized subsurfaces of this window are updated
    /// to correctly update the bounding box of this window.
    pub fn on_commit(&self) {
        if let Some(surface) = self.wl_surface() {
            *self.0.bbox.lock().unwrap() = bbox_from_surface_tree(&surface, (0, 0));
        }
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
        if let Some(surface) = self.wl_surface() {
            if surface_type.contains(WindowSurfaceType::POPUP) {
                for (popup, location) in PopupManager::popups_for_surface(&surface) {
                    let offset = self.geometry().loc + location - popup.geometry().loc;
                    if let Some(result) =
                        under_from_surface_tree(popup.wl_surface(), point, offset, surface_type)
                    {
                        return Some(result);
                    }
                }
            }

            if surface_type.contains(WindowSurfaceType::TOPLEVEL) {
                return under_from_surface_tree(&surface, point, (0, 0), surface_type);
            }
        }

        None
    }

    /// Returns the underlying xdg toplevel surface
    pub fn toplevel(&self) -> Option<&ToplevelSurface> {
        match &self.0.surface {
            WindowSurface::Wayland(s) => Some(s),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(_) => None,
        }
    }

    /// Returns the underlying X11 surface
    #[cfg(feature = "xwayland")]
    pub fn x11_surface(&self) -> Option<&X11Surface> {
        match &self.0.surface {
            WindowSurface::Wayland(_) => None,
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(s) => Some(s),
        }
    }

    /// Returns the underlying surface
    pub fn underlying_surface(&self) -> &WindowSurface {
        &self.0.surface
    }

    /// Override the z_index of this Window
    pub fn override_z_index(&self, z_index: u8) {
        self.0.z_index.store(z_index, Ordering::SeqCst);
    }

    /// Returns a [`UserDataMap`] to allow associating arbitrary data with this window.
    #[inline]
    pub fn user_data(&self) -> &UserDataMap {
        &self.0.user_data
    }
}

impl WaylandFocus for Window {
    #[inline]
    fn wl_surface(&self) -> Option<Cow<'_, wl_surface::WlSurface>> {
        match &self.0.surface {
            WindowSurface::Wayland(s) => Some(Cow::Borrowed(s.wl_surface())),
            #[cfg(feature = "xwayland")]
            WindowSurface::X11(s) => s.wl_surface().map(Cow::Owned),
        }
    }
}
