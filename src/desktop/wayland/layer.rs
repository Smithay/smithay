use crate::{
    desktop::{utils::*, PopupManager},
    output::{Output, WeakOutput},
    utils::{user_data::UserDataMap, IsAlive, Logical, Point, Rectangle},
    wayland::{
        compositor::{with_states, with_surface_tree_downward, SurfaceData, TraversalAction},
        dmabuf::DmabufFeedback,
        seat::WaylandFocus,
        shell::wlr_layer::{
            Anchor, ExclusiveZone, KeyboardInteractivity, Layer as WlrLayer, LayerSurface as WlrLayerSurface,
            LayerSurfaceCachedState, LayerSurfaceData,
        },
    },
};
use indexmap::IndexSet;
use tracing::{debug_span, trace};
use wayland_protocols::wp::presentation_time::server::wp_presentation_feedback;
use wayland_server::protocol::wl_surface::{self, WlSurface};

use std::{
    borrow::Cow,
    hash::{Hash, Hasher},
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use crate::desktop::WindowSurfaceType;

crate::utils::ids::id_gen!(layer_id);

/// Map of [`LayerSurface`]s on an [`Output`]
#[derive(Debug)]
pub struct LayerMap {
    layers: IndexSet<LayerSurface>,
    output: WeakOutput,
    zone: Rectangle<i32, Logical>,
}

/// Retrieve a [`LayerMap`] for a given [`Output`].
///
/// If none existed before a new empty [`LayerMap`] is attached
/// to the output and returned on subsequent calls.
///
/// Note: This function internally uses a [`Mutex`] per
/// [`Output`] as exposed by its return type. Therefor
/// trying to hold on to multiple references of a [`LayerMap`]
/// of the same output using this function *will* result in a deadlock.
pub fn layer_map_for_output(o: &Output) -> MutexGuard<'_, LayerMap> {
    let userdata = o.user_data();
    userdata.insert_if_missing_threadsafe(|| {
        Mutex::new(LayerMap {
            layers: IndexSet::new(),
            output: o.downgrade(),
            zone: Rectangle::from_loc_and_size(
                (0, 0),
                o.current_mode()
                    .map(|mode| {
                        let logical_size = mode
                            .size
                            .to_f64()
                            .to_logical(o.current_scale().fractional_scale())
                            .to_i32_round();
                        o.current_transform().transform_size(logical_size)
                    })
                    .unwrap_or_else(|| (0, 0).into()),
            ),
        })
    });
    userdata.get::<Mutex<LayerMap>>().unwrap().lock().unwrap()
}

#[derive(Debug, thiserror::Error)]
pub enum LayerError {
    #[error("Layer is already mapped to a different map")]
    AlreadyMapped,
}

impl LayerMap {
    /// Map a [`LayerSurface`] to this [`LayerMap`].
    pub fn map_layer(&mut self, layer: &LayerSurface) -> Result<(), LayerError> {
        if !self.layers.contains(layer) {
            if layer
                .0
                .userdata
                .get::<LayerUserdata>()
                .map(|s| s.lock().unwrap().location.is_some())
                .unwrap_or(false)
            {
                return Err(LayerError::AlreadyMapped);
            }

            self.layers.insert(layer.clone());
            self.arrange();
        }
        Ok(())
    }

    /// Remove a [`LayerSurface`] from this [`LayerMap`].
    pub fn unmap_layer(&mut self, layer: &LayerSurface) {
        if self.layers.shift_remove(layer) {
            let _ = layer
                .user_data()
                .get::<LayerUserdata>()
                .unwrap()
                .lock()
                .unwrap()
                .location
                .take();
            self.arrange();
        }
        if let (Some(output), surface) = (self.output(), layer.wl_surface()) {
            with_surface_tree_downward(
                surface,
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |wl_surface, _, _| {
                    output.leave(wl_surface);
                },
                |_, _, _| true,
            );
            for (popup, _) in PopupManager::popups_for_surface(surface) {
                let surface = popup.wl_surface();
                with_surface_tree_downward(
                    surface,
                    (),
                    |_, _, _| TraversalAction::DoChildren(()),
                    |wl_surface, _, _| {
                        output.leave(wl_surface);
                    },
                    |_, _, _| true,
                )
            }
        }
    }

    /// Return the area of this output, that is not exclusive to any [`LayerSurface`]s.
    pub fn non_exclusive_zone(&self) -> Rectangle<i32, Logical> {
        self.zone
    }

    /// Returns the geometry of a given mapped [`LayerSurface`].
    ///
    /// If the surface was not previously mapped onto this layer map,
    /// this function return `None`.
    pub fn layer_geometry(&self, layer: &LayerSurface) -> Option<Rectangle<i32, Logical>> {
        if !self.layers.contains(layer) {
            return None;
        }
        let mut bbox = layer.bbox();
        let state = layer_state(layer);
        bbox.loc += state.location.unwrap_or_default();
        Some(bbox)
    }

    /// Returns a [`LayerSurface`] under a given point and on a given layer, if any.
    pub fn layer_under<P: Into<Point<f64, Logical>>>(
        &self,
        layer: WlrLayer,
        point: P,
    ) -> Option<&LayerSurface> {
        let point = point.into();
        self.layers_on(layer).rev().find(|l| {
            let bbox_with_popups = {
                let mut bbox = l.bbox_with_popups();
                let state = layer_state(l);
                bbox.loc += state.location.unwrap_or_default();
                bbox
            };
            bbox_with_popups.to_f64().contains(point)
        })
    }

    /// Iterator over all [`LayerSurface`]s currently mapped.
    pub fn layers(&self) -> impl DoubleEndedIterator<Item = &LayerSurface> {
        self.layers.iter()
    }

    /// Iterator over all [`LayerSurface`]s currently mapped on a given layer.
    pub fn layers_on(&self, layer: WlrLayer) -> impl DoubleEndedIterator<Item = &LayerSurface> {
        self.layers.iter().filter(move |l| l.layer() == layer)
    }

    /// Returns the [`LayerSurface`] matching a given [`WlSurface`], if any.
    ///
    /// `surface_type` can be used to limit the types of surfaces queried for equality.
    pub fn layer_for_surface(
        &self,
        surface: &WlSurface,
        surface_type: WindowSurfaceType,
    ) -> Option<&LayerSurface> {
        use std::sync::atomic::{AtomicBool, Ordering};

        self.layers.iter().find(|layer| {
            if surface_type.contains(WindowSurfaceType::POPUP) {
                for (popup, _) in PopupManager::popups_for_surface(layer.wl_surface()) {
                    let toplevel = popup.wl_surface();
                    let found = AtomicBool::new(false);
                    with_surface_tree_downward(
                        toplevel,
                        surface,
                        |_, _, search| {
                            if surface_type.contains(WindowSurfaceType::SUBSURFACE) {
                                TraversalAction::DoChildren(search)
                            } else {
                                TraversalAction::SkipChildren
                            }
                        },
                        |s, _, search| {
                            found.fetch_or(s == *search, Ordering::SeqCst);
                        },
                        |_, _, _| !found.load(Ordering::SeqCst),
                    );
                    if found.load(Ordering::SeqCst) {
                        return true;
                    }
                }
            }

            if surface_type.contains(WindowSurfaceType::TOPLEVEL) {
                let toplevel = layer.wl_surface();
                let found = AtomicBool::new(false);
                with_surface_tree_downward(
                    toplevel,
                    surface,
                    |_, _, search| {
                        if surface_type.contains(WindowSurfaceType::SUBSURFACE) {
                            TraversalAction::DoChildren(search)
                        } else {
                            TraversalAction::SkipChildren
                        }
                    },
                    |s, _, search| {
                        found.fetch_or(s == *search, Ordering::SeqCst);
                    },
                    |_, _, _| !found.load(Ordering::SeqCst),
                );
                return found.load(Ordering::SeqCst);
            }

            false
        })
    }

    /// Force re-arranging the layer surfaces, e.g. when the output size changes.
    ///
    /// Note: Mapping or unmapping a layer surface will automatically cause a re-arrangement.
    ///
    /// Return whenever any position or size changes of existing surfaces were necessary.
    pub fn arrange(&mut self) -> bool {
        let mut changed = false;
        if let Some(output) = self.output() {
            let span = debug_span!("layer_map", output = output.name());
            let _guard = span.enter();

            let output_rect = Rectangle::from_loc_and_size(
                (0, 0),
                output
                    .current_mode()
                    .map(|mode| {
                        let logical_size = mode
                            .size
                            .to_f64()
                            .to_logical(output.current_scale().fractional_scale())
                            .to_i32_round();
                        output.current_transform().transform_size(logical_size)
                    })
                    .unwrap_or_else(|| (0, 0).into()),
            );
            let mut zone = output_rect;
            trace!("Arranging layers into {:?}", output_rect.size);

            for layer in self.layers.iter() {
                let surface = layer.wl_surface();

                with_surface_tree_downward(
                    surface,
                    (),
                    |_, _, _| TraversalAction::DoChildren(()),
                    |wl_surface, _, _| {
                        output.enter(wl_surface);
                    },
                    |_, _, _| true,
                );
                for (popup, _) in PopupManager::popups_for_surface(surface) {
                    let surface = popup.wl_surface();
                    with_surface_tree_downward(
                        surface,
                        (),
                        |_, _, _| TraversalAction::DoChildren(()),
                        |wl_surface, _, _| {
                            output.enter(wl_surface);
                        },
                        |_, _, _| true,
                    )
                }

                let data = with_states(surface, |states| {
                    *states.cached_state.get::<LayerSurfaceCachedState>().current()
                });

                let mut source = match data.exclusive_zone {
                    ExclusiveZone::Exclusive(_) | ExclusiveZone::Neutral => zone,
                    ExclusiveZone::DontCare => output_rect,
                };

                // adjust the copy rect to account for the margins
                if data.anchor.contains(Anchor::LEFT) {
                    source.size.w -= data.margin.left
                }
                if data.anchor.contains(Anchor::RIGHT) {
                    source.size.w -= data.margin.right
                }
                if data.anchor.contains(Anchor::TOP) {
                    source.size.h -= data.margin.top
                }
                if data.anchor.contains(Anchor::BOTTOM) {
                    source.size.h -= data.margin.bottom
                }

                let mut size = data.size;
                size.w = size.w.min(source.size.w);
                size.h = size.h.min(source.size.h);
                if size.w == 0 {
                    size.w = source.size.w / 2;
                }
                if size.h == 0 {
                    size.h = source.size.h / 2;
                }
                if data.anchor.anchored_horizontally() {
                    size.w = source.size.w;
                }
                if data.anchor.anchored_vertically() {
                    size.h = source.size.h;
                }

                let x = if data.anchor.contains(Anchor::LEFT) {
                    source.loc.x + data.margin.left
                } else if data.anchor.contains(Anchor::RIGHT) {
                    source.loc.x + (source.size.w - size.w)
                } else {
                    source.loc.x + ((source.size.w / 2) - (size.w / 2))
                };

                let y = if data.anchor.contains(Anchor::TOP) {
                    source.loc.y + data.margin.top
                } else if data.anchor.contains(Anchor::BOTTOM) {
                    source.loc.y + (source.size.h - size.h)
                } else {
                    source.loc.y + ((source.size.h / 2) - (size.h / 2))
                };

                let location: Point<i32, Logical> = (x, y).into();

                if let ExclusiveZone::Exclusive(amount) = data.exclusive_zone {
                    match data.anchor {
                        x if x.contains(Anchor::TOP) && x.contains(Anchor::BOTTOM) => {
                            zone.size.w -= amount as i32;
                            if x.contains(Anchor::LEFT) {
                                zone.loc.x += amount as i32 + data.margin.left;
                                zone.size.w -= data.margin.left;
                            }
                            if x.contains(Anchor::RIGHT) {
                                zone.size.w -= data.margin.right
                            }
                        }
                        x if x.contains(Anchor::LEFT) && x.contains(Anchor::RIGHT) => {
                            zone.size.h -= amount as i32;
                            if x.contains(Anchor::TOP) {
                                zone.loc.y += amount as i32 + data.margin.top;
                                zone.size.h -= data.margin.top
                            }
                            if x.contains(Anchor::BOTTOM) {
                                zone.size.h -= data.margin.bottom
                            }
                        }
                        x if x == Anchor::all() => {
                            zone.size.w = 0;
                            zone.size.h = 0;
                        }
                        x if x.contains(Anchor::LEFT) && !x.contains(Anchor::RIGHT) => {
                            zone.loc.x += amount as i32 + data.margin.left;
                            zone.size.w -= amount as i32 + data.margin.left;
                        }
                        x if x.contains(Anchor::TOP) && !x.contains(Anchor::BOTTOM) => {
                            zone.loc.y += amount as i32 + data.margin.top;
                            zone.size.h -= amount as i32 + data.margin.top;
                        }
                        x if x.contains(Anchor::RIGHT) && !x.contains(Anchor::LEFT) => {
                            zone.size.w -= amount as i32 + data.margin.right;
                        }
                        x if x.contains(Anchor::BOTTOM) && !x.contains(Anchor::TOP) => {
                            zone.size.h -= amount as i32 + data.margin.bottom;
                        }
                        _ => {}
                    }
                }

                trace!("Setting layer to pos {:?} and size {:?}", location, size);
                let size_changed = layer.0.surface.with_pending_state(|state| {
                    state.size.replace(size).map(|old| old != size).unwrap_or(true)
                });
                changed = changed || size_changed;
                let initial_configure_sent = with_states(surface, |states| {
                    states
                        .data_map
                        .get::<LayerSurfaceData>()
                        .map(|data| data.lock().unwrap().initial_configure_sent)
                })
                .unwrap_or_default();

                // arrange should never automatically send an configure
                // event if the surface has not been configured already.
                // The spec mandates that the initial configure has to be
                // send in response of the initial commit of the surface.
                // That also guarantees that the client is able set a size
                // before committing the surface. By not respecting that
                // we would send a wrong size to the client and also violate
                // the spec by sending a configure event before a prior commit.
                if size_changed && initial_configure_sent {
                    layer.0.surface.send_pending_configure();
                }

                {
                    let mut layer_state = layer_state(layer);
                    if layer_state.location != Some(location) {
                        layer_state.location = Some(location);
                        changed = true;
                    }
                }
            }

            trace!("Remaining zone {:?}", zone);
            self.zone = zone;
        }

        changed
    }

    fn output(&self) -> Option<Output> {
        self.output.upgrade()
    }

    /// Cleanup some internally used resources.
    ///
    /// This function needs to be called periodically (though not necessarily frequently)
    /// to be able cleanup internally used resources.
    pub fn cleanup(&mut self) {
        if self.layers.iter().any(|l| !l.alive()) {
            self.layers.retain(|layer| layer.alive());
            self.arrange();
        }
    }

    /// Returns layers count
    #[allow(clippy::len_without_is_empty)] //we don't need is_empty on that struct for now, mark as allow
    pub fn len(&self) -> usize {
        self.layers.len()
    }
}

#[derive(Debug, Default, Clone, Copy)]
pub struct LayerState {
    pub location: Option<Point<i32, Logical>>,
}

type LayerUserdata = Mutex<LayerState>;
pub fn layer_state(layer: &LayerSurface) -> MutexGuard<'_, LayerState> {
    let userdata = layer.user_data();
    userdata.insert_if_missing_threadsafe(LayerUserdata::default);
    userdata.get::<LayerUserdata>().unwrap().lock().unwrap()
}

/// A [`LayerSurface`] represents a single layer surface as given by the wlr-layer-shell protocol.
#[derive(Debug, Clone)]
pub struct LayerSurface(pub(crate) Arc<LayerSurfaceInner>);

impl PartialEq for LayerSurface {
    #[inline]
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for LayerSurface {}

impl Hash for LayerSurface {
    #[inline]
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.0.id.hash(state);
    }
}

#[derive(Debug)]
pub(crate) struct LayerSurfaceInner {
    pub(crate) id: usize,
    surface: WlrLayerSurface,
    namespace: String,
    userdata: UserDataMap,
}

impl Drop for LayerSurfaceInner {
    #[inline]
    fn drop(&mut self) {
        layer_id::remove(self.id);
    }
}

impl IsAlive for LayerSurface {
    #[inline]
    fn alive(&self) -> bool {
        self.0.surface.alive()
    }
}

impl LayerSurface {
    /// Create a new [`LayerSurface`] from a given [`WlrLayerSurface`] and its namespace.
    pub fn new(surface: WlrLayerSurface, namespace: String) -> LayerSurface {
        LayerSurface(Arc::new(LayerSurfaceInner {
            id: layer_id::next(),
            surface,
            namespace,
            userdata: UserDataMap::new(),
        }))
    }

    /// Returns the underlying [`WlrLayerSurface`]
    pub fn layer_surface(&self) -> &WlrLayerSurface {
        &self.0.surface
    }

    /// Returns the underlying [`WlSurface`]
    #[inline]
    #[allow(clippy::same_name_method)]
    pub fn wl_surface(&self) -> &WlSurface {
        self.0.surface.wl_surface()
    }

    /// Returns the cached protocol state
    pub fn cached_state(&self) -> LayerSurfaceCachedState {
        with_states(self.0.surface.wl_surface(), |states| {
            *states.cached_state.get::<LayerSurfaceCachedState>().current()
        })
    }

    /// Returns true, if the surface has indicated, that it is able to process keyboard events.
    pub fn can_receive_keyboard_focus(&self) -> bool {
        with_states(self.0.surface.wl_surface(), |states| {
            match states
                .cached_state
                .get::<LayerSurfaceCachedState>()
                .current()
                .keyboard_interactivity
            {
                KeyboardInteractivity::Exclusive | KeyboardInteractivity::OnDemand => true,
                KeyboardInteractivity::None => false,
            }
        })
    }

    /// Returns the layer this surface resides on, if any yet.
    pub fn layer(&self) -> WlrLayer {
        with_states(self.0.surface.wl_surface(), |states| {
            states
                .cached_state
                .get::<LayerSurfaceCachedState>()
                .current()
                .layer
        })
    }

    /// Returns the namespace of this surface
    pub fn namespace(&self) -> &str {
        &self.0.namespace
    }

    /// Returns the bounding box over this layer surface and its subsurfaces.
    pub fn bbox(&self) -> Rectangle<i32, Logical> {
        bbox_from_surface_tree(self.0.surface.wl_surface(), (0, 0))
    }

    /// Returns the bounding box over this layer surface, it subsurfaces as well as any popups.
    ///
    /// Note: You need to use a [`PopupManager`] to track popups, otherwise the bounding box
    /// will not include the popups.
    pub fn bbox_with_popups(&self) -> Rectangle<i32, Logical> {
        let mut bounding_box = self.bbox();
        let surface = self.0.surface.wl_surface();
        for (popup, location) in PopupManager::popups_for_surface(surface) {
            let offset = location - popup.geometry().loc;
            bounding_box = bounding_box.merge(bbox_from_surface_tree(popup.wl_surface(), offset));
        }

        bounding_box
    }

    /// Finds the topmost surface under this point if any and returns it together with the location of this
    /// surface.
    ///
    /// - `point` needs to be relative to (0,0) of the layer surface.
    pub fn surface_under<P: Into<Point<f64, Logical>>>(
        &self,
        point: P,
        surface_type: WindowSurfaceType,
    ) -> Option<(WlSurface, Point<i32, Logical>)> {
        let point = point.into();
        let surface = self.wl_surface();
        if surface_type.contains(WindowSurfaceType::POPUP) {
            for (popup, location) in PopupManager::popups_for_surface(surface) {
                let surface = popup.wl_surface();
                let offset = location - popup.geometry().loc;
                if let Some(result) = under_from_surface_tree(surface, point, offset, surface_type) {
                    return Some(result);
                }
            }
        }

        if surface_type.contains(WindowSurfaceType::TOPLEVEL) {
            return under_from_surface_tree(surface, point, (0, 0), surface_type);
        }

        None
    }

    /// Sends the frame callback to all the subsurfaces in this layer that requested it
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
        F: FnMut(&WlSurface, &SurfaceData) -> Option<Output> + Copy,
    {
        let time = time.into();
        let surface = self.0.surface.wl_surface();

        send_frames_surface_tree(surface, output, time, throttle, primary_scan_out_output);
        for (popup, _) in PopupManager::popups_for_surface(surface) {
            let surface = popup.wl_surface();
            send_frames_surface_tree(surface, output, time, throttle, primary_scan_out_output);
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
        let surface = self.0.surface.wl_surface();
        send_dmabuf_feedback_surface_tree(surface, output, primary_scan_out_output, select_dmabuf_feedback);
        for (popup, _) in PopupManager::popups_for_surface(surface) {
            let surface = popup.wl_surface();
            send_dmabuf_feedback_surface_tree(
                surface,
                output,
                primary_scan_out_output,
                select_dmabuf_feedback,
            );
        }
    }

    /// Takes the [`PresentationFeedbackCallback`](crate::wayland::presentation::PresentationFeedbackCallback)s from all subsurfaces in this layer
    ///
    /// see [`take_presentation_feedback_surface_tree`] for more information
    pub fn take_presentation_feedback<F1, F2>(
        &self,
        output_feedback: &mut OutputPresentationFeedback,
        primary_scan_out_output: F1,
        presentation_feedback_flags: F2,
    ) where
        F1: FnMut(&WlSurface, &SurfaceData) -> Option<Output> + Copy,
        F2: FnMut(&WlSurface, &SurfaceData) -> wp_presentation_feedback::Kind + Copy,
    {
        let surface = self.0.surface.wl_surface();
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

    /// Run a closure on all surfaces in this layer (including it's popups, if [`PopupManager`] is used)
    pub fn with_surfaces<F>(&self, mut processor: F)
    where
        F: FnMut(&WlSurface, &SurfaceData),
    {
        let surface = self.0.surface.wl_surface();

        with_surfaces_surface_tree(surface, &mut processor);
        for (popup, _) in PopupManager::popups_for_surface(surface) {
            let surface = popup.wl_surface();
            with_surfaces_surface_tree(surface, &mut processor);
        }
    }

    /// Returns a [`UserDataMap`] to allow associating arbitrary data with this surface.
    pub fn user_data(&self) -> &UserDataMap {
        &self.0.userdata
    }
}

impl WaylandFocus for LayerSurface {
    #[inline]
    fn wl_surface(&self) -> Option<Cow<'_, wl_surface::WlSurface>> {
        Some(Cow::Borrowed(self.wl_surface()))
    }
}
