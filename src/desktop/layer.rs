use crate::{
    backend::renderer::{utils::draw_surface_tree, ImportAll, Renderer},
    desktop::{utils::*, PopupManager, Space},
    utils::{user_data::UserDataMap, Logical, Point, Rectangle},
    wayland::{
        compositor::{with_states, with_surface_tree_downward, TraversalAction},
        output::{Inner as OutputInner, Output, OutputGlobalData},
        shell::wlr_layer::{
            Anchor, ExclusiveZone, KeyboardInteractivity, Layer as WlrLayer, LayerSurface as WlrLayerSurface,
            LayerSurfaceCachedState,
        },
    },
};
use indexmap::IndexSet;
use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle};

use std::{
    cell::{RefCell, RefMut},
    hash::{Hash, Hasher},
    rc::Rc,
    sync::{Arc, Mutex, Weak},
};

use super::WindowSurfaceType;

crate::utils::ids::id_gen!(next_layer_id, LAYER_ID, LAYER_IDS);

/// Map of [`LayerSurface`]s on an [`Output`]
#[derive(Debug)]
pub struct LayerMap {
    layers: IndexSet<LayerSurface>,
    output: Weak<(Mutex<OutputInner>, UserDataMap)>,
    zone: Rectangle<i32, Logical>,
    // surfaces for tracking enter and leave events
    surfaces: Vec<WlSurface>,
    logger: ::slog::Logger,
}

/// Retrieve a [`LayerMap`] for a given [`Output`].
///
/// If none existed before a new empty [`LayerMap`] is attached
/// to the output and returned on subsequent calls.
///
/// Note: This function internally uses a [`RefCell`] per
/// [`Output`] as exposed by its return type. Therefor
/// trying to hold on to multiple references of a [`LayerMap`]
/// of the same output using this function *will* result in a panic.
pub fn layer_map_for_output(o: &Output) -> RefMut<'_, LayerMap> {
    let userdata = o.user_data();
    let weak_output = Arc::downgrade(&o.data.inner);
    userdata.insert_if_missing(|| {
        RefCell::new(LayerMap {
            layers: IndexSet::new(),
            output: weak_output,
            zone: Rectangle::from_loc_and_size(
                (0, 0),
                o.current_mode()
                    .map(|mode| mode.size.to_logical(o.current_scale()))
                    .unwrap_or_else(|| (0, 0).into()),
            ),
            surfaces: Vec::new(),
            logger: (*o.data.inner.0.lock().unwrap())
                .log
                .new(slog::o!("smithay_module" => "layer_map")),
        })
    });
    userdata.get::<RefCell<LayerMap>>().unwrap().borrow_mut()
}

#[derive(Debug, thiserror::Error)]
pub enum LayerError {
    #[error("Layer is already mapped to a different map")]
    AlreadyMapped,
}

impl LayerMap {
    /// Map a [`LayerSurface`] to this [`LayerMap`].
    pub fn map_layer(&mut self, dh: &mut DisplayHandle<'_>, layer: &LayerSurface) -> Result<(), LayerError> {
        if !self.layers.contains(layer) {
            if layer
                .0
                .userdata
                .get::<LayerUserdata>()
                .map(|s| s.borrow().is_some())
                .unwrap_or(false)
            {
                return Err(LayerError::AlreadyMapped);
            }

            self.layers.insert(layer.clone());
            self.arrange(dh);
        }
        Ok(())
    }

    /// Remove a [`LayerSurface`] from this [`LayerMap`].
    pub fn unmap_layer(&mut self, dh: &mut DisplayHandle<'_>, layer: &LayerSurface) {
        if self.layers.shift_remove(layer) {
            let _ = layer.user_data().get::<LayerUserdata>().take();
            self.arrange(dh);
        }
        if let (Some(output), surface) = (self.output(), layer.wl_surface()) {
            with_surface_tree_downward(
                surface,
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |wl_surface, _, _| {
                    output_leave(dh, &output, &mut self.surfaces, wl_surface, &self.logger);
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
                        output_leave(dh, &output, &mut self.surfaces, wl_surface, &self.logger);
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
        let mut bbox = layer.bbox_with_popups();
        let state = layer_state(layer);
        bbox.loc += state.location;
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
            let bbox = self.layer_geometry(l).unwrap();
            bbox.to_f64().contains(point)
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
    pub fn layer_for_surface(&self, surface: &WlSurface) -> Option<&LayerSurface> {
        self.layers.iter().find(|w| w.wl_surface() == surface)
    }

    /// Force re-arranging the layer surfaces, e.g. when the output size changes.
    ///
    /// Note: Mapping or unmapping a layer surface will automatically cause a re-arrangement.
    pub fn arrange(&mut self, dh: &mut DisplayHandle<'_>) {
        if let Some(output) = self.output() {
            let output_rect = Rectangle::from_loc_and_size(
                (0, 0),
                output
                    .current_mode()
                    .map(|mode| mode.size.to_logical(output.current_scale()))
                    .unwrap_or_else(|| (0, 0).into()),
            );
            let mut zone = output_rect;
            slog::trace!(self.logger, "Arranging layers into {:?}", output_rect.size);

            for layer in self.layers.iter() {
                let surface = layer.wl_surface();

                let logger_ref = &self.logger;
                let surfaces_ref = &mut self.surfaces;
                with_surface_tree_downward(
                    surface,
                    (),
                    |_, _, _| TraversalAction::DoChildren(()),
                    |wl_surface, _, _| {
                        output_enter(dh, &output, surfaces_ref, wl_surface, logger_ref);
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
                            output_enter(dh, &output, surfaces_ref, wl_surface, logger_ref);
                        },
                        |_, _, _| true,
                    )
                }

                let data = with_states(surface, |states| {
                    *states.cached_state.current::<LayerSurfaceCachedState>()
                });

                let source = match data.exclusive_zone {
                    ExclusiveZone::Neutral | ExclusiveZone::Exclusive(_) => &zone,
                    ExclusiveZone::DontCare => &output_rect,
                };

                let mut size = data.size;
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
                    source.loc.x + (source.size.w - size.w) - data.margin.right
                } else {
                    source.loc.x + ((source.size.w / 2) - (size.w / 2))
                };

                let y = if data.anchor.contains(Anchor::TOP) {
                    source.loc.y + data.margin.top
                } else if data.anchor.contains(Anchor::BOTTOM) {
                    source.loc.y + (source.size.h - size.h) - data.margin.bottom
                } else {
                    source.loc.y + ((source.size.h / 2) - (size.h / 2))
                };

                let location: Point<i32, Logical> = (x, y).into();

                if let ExclusiveZone::Exclusive(amount) = data.exclusive_zone {
                    match data.anchor {
                        x if x.contains(Anchor::LEFT) && !x.contains(Anchor::RIGHT) => {
                            zone.loc.x += amount as i32 + data.margin.left + data.margin.right;
                            zone.size.w -= amount as i32 + data.margin.left + data.margin.right;
                        }
                        x if x.contains(Anchor::TOP) && !x.contains(Anchor::BOTTOM) => {
                            zone.loc.y += amount as i32 + data.margin.top + data.margin.bottom;
                            zone.size.h -= amount as i32 + data.margin.top + data.margin.bottom;
                        }
                        x if x.contains(Anchor::RIGHT) && !x.contains(Anchor::LEFT) => {
                            zone.size.w -= amount as i32 + data.margin.left + data.margin.right;
                        }
                        x if x.contains(Anchor::BOTTOM) && !x.contains(Anchor::TOP) => {
                            zone.size.h -= amount as i32 + data.margin.top + data.margin.bottom;
                        }
                        _ => {}
                    }
                }

                slog::trace!(
                    self.logger,
                    "Setting layer to pos {:?} and size {:?}",
                    location,
                    size
                );
                let size_changed = layer.0.surface.with_pending_state(|state| {
                    state.size.replace(size).map(|old| old != size).unwrap_or(true)
                });
                if size_changed {
                    layer.0.surface.send_configure(dh);
                }

                layer_state(layer).location = location;
            }

            slog::trace!(self.logger, "Remaining zone {:?}", zone);
            self.zone = zone;
        }
    }

    fn output(&self) -> Option<Output> {
        self.output.upgrade().map(|inner| Output {
            data: OutputGlobalData { inner },
        })
    }

    /// Cleanup some internally used resources.
    ///
    /// This function needs to be called periodically (though not necessarily frequently)
    /// to be able cleanup internally used resources.
    pub fn cleanup(&mut self) {
        self.layers.retain(|layer| layer.alive());
        // TODO(desktop-0.30)
        // self.surfaces.retain(|s| s.as_ref().is_alive());
    }

    /// Returns layers count
    #[allow(clippy::len_without_is_empty)] //we don't need is_empty on that struct for now, mark as allow
    pub fn len(&self) -> usize {
        self.layers.len()
    }
}

#[derive(Debug, Default)]
pub struct LayerState {
    pub location: Point<i32, Logical>,
}

type LayerUserdata = RefCell<Option<LayerState>>;
pub fn layer_state(layer: &LayerSurface) -> RefMut<'_, LayerState> {
    let userdata = layer.user_data();
    userdata.insert_if_missing(LayerUserdata::default);
    RefMut::map(userdata.get::<LayerUserdata>().unwrap().borrow_mut(), |opt| {
        if opt.is_none() {
            *opt = Some(LayerState::default());
        }
        opt.as_mut().unwrap()
    })
}

/// A [`LayerSurface`] represents a single layer surface as given by the wlr-layer-shell protocol.
#[derive(Debug, Clone)]
pub struct LayerSurface(pub(crate) Rc<LayerSurfaceInner>);

impl PartialEq for LayerSurface {
    fn eq(&self, other: &Self) -> bool {
        self.0.id == other.0.id
    }
}

impl Eq for LayerSurface {}

impl Hash for LayerSurface {
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
    fn drop(&mut self) {
        LAYER_IDS.lock().unwrap().remove(&self.id);
    }
}

impl LayerSurface {
    /// Create a new [`LayerSurface`] from a given [`WlrLayerSurface`] and its namespace.
    pub fn new(surface: WlrLayerSurface, namespace: String) -> LayerSurface {
        LayerSurface(Rc::new(LayerSurfaceInner {
            id: next_layer_id(),
            surface,
            namespace,
            userdata: UserDataMap::new(),
        }))
    }

    /// Checks if the surface is still alive
    pub fn alive(&self) -> bool {
        todo!();
        // self.0.surface.alive()
    }

    /// Returns the underlying [`WlrLayerSurface`]
    pub fn layer_surface(&self) -> &WlrLayerSurface {
        &self.0.surface
    }

    /// Returns the underlying [`WlSurface`]
    pub fn wl_surface(&self) -> &WlSurface {
        self.0.surface.wl_surface()
    }

    /// Returns the cached protocol state
    pub fn cached_state(&self) -> LayerSurfaceCachedState {
        with_states(self.0.surface.wl_surface(), |states| {
            *states.cached_state.current::<LayerSurfaceCachedState>()
        })
    }

    /// Returns true, if the surface has indicated, that it is able to process keyboard events.
    pub fn can_receive_keyboard_focus(&self) -> bool {
        with_states(self.0.surface.wl_surface(), |states| {
            match states
                .cached_state
                .current::<LayerSurfaceCachedState>()
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
            states.cached_state.current::<LayerSurfaceCachedState>().layer
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
            bounding_box = bounding_box.merge(bbox_from_surface_tree(popup.wl_surface(), location));
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
        for (popup, location) in PopupManager::popups_for_surface(surface) {
            let surface = popup.wl_surface();
            if let Some(result) = under_from_surface_tree(surface, point, location, surface_type) {
                return Some(result);
            }
        }

        under_from_surface_tree(surface, point, (0, 0), surface_type)
    }

    /// Returns the damage of all the surfaces of this layer surface.
    ///
    /// If `for_values` is `Some(_)` it will only return the damage on the
    /// first call for a given [`Space`] and [`Output`], if the buffer hasn't changed.
    /// Subsequent calls will return an empty vector until the buffer is updated again.
    pub(super) fn accumulated_damage(
        &self,
        for_values: Option<(&Space, &Output)>,
    ) -> Vec<Rectangle<i32, Logical>> {
        let mut damage = Vec::new();
        let surface = self.wl_surface();
        damage.extend(
            damage_from_surface_tree(surface, (0, 0), for_values)
                .into_iter()
                .flat_map(|rect| rect.intersection(self.bbox())),
        );
        for (popup, location) in PopupManager::popups_for_surface(surface) {
            let surface = popup.wl_surface();
            let bbox = bbox_from_surface_tree(surface, location);
            let popup_damage = damage_from_surface_tree(surface, location, for_values);
            damage.extend(popup_damage.into_iter().flat_map(|rect| rect.intersection(bbox)));
        }
        damage
    }

    /// Sends the frame callback to all the subsurfaces in this
    /// window that requested it
    pub fn send_frame(&self, dh: &mut DisplayHandle<'_>, time: u32) {
        let wl_surface = self.0.surface.wl_surface();

        send_frames_surface_tree(dh, wl_surface, time);
        for (popup, _) in PopupManager::popups_for_surface(wl_surface) {
            send_frames_surface_tree(dh, popup.wl_surface(), time);
        }
    }

    /// Returns a [`UserDataMap`] to allow associating arbitrary data with this surface.
    pub fn user_data(&self) -> &UserDataMap {
        &self.0.userdata
    }
}

/// Renders a given [`LayerSurface`] using a provided renderer and frame.
///
/// - `scale` needs to be equivalent to the fractional scale the rendered result should have.
/// - `location` is the position the layer surface should be drawn at.
/// - `damage` is the set of regions of the layer surface that should be drawn.
///
/// Note: This function will render nothing, if you are not using
/// [`crate::backend::renderer::utils::on_commit_buffer_handler`]
/// to let smithay handle buffer management.
pub fn draw_layer_surface<R, P>(
    dh: &mut DisplayHandle<'_>,
    renderer: &mut R,
    frame: &mut <R as Renderer>::Frame,
    layer: &LayerSurface,
    scale: f64,
    location: P,
    damage: &[Rectangle<i32, Logical>],
    log: &slog::Logger,
) -> Result<(), <R as Renderer>::Error>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
    P: Into<Point<i32, Logical>>,
{
    let location = location.into();
    let surface = layer.wl_surface();
    draw_surface_tree(dh, renderer, frame, surface, scale, location, damage, log)?;
    for (popup, p_location) in PopupManager::popups_for_surface(surface) {
        let surface = popup.wl_surface();
        let damage = damage
            .iter()
            .cloned()
            .map(|mut geo| {
                geo.loc -= p_location;
                geo
            })
            .collect::<Vec<_>>();
        draw_surface_tree(
            dh,
            renderer,
            frame,
            surface,
            scale,
            location + p_location,
            &damage,
            log,
        )?;
    }
    Ok(())
}
