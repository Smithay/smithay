//! Element to render a wayland surface
//!
//! # Why use this implementation
//!
//! The [`WaylandSurfaceRenderElement`] provides an easy way to
//! integrate a [`WlSurface`](wayland_server::protocol::wl_surface::WlSurface) in the smithay rendering pipeline
//!
//! # How to use it
//!
//! [`WaylandSurfaceRenderElement::from_surface`] allows you to obtain a [`WaylandSurfaceRenderElement`] for a single [`WlSurface`](wayland_server::protocol::wl_surface::WlSurface).
//! To retrieve [`WaylandSurfaceRenderElement`]s for a whole surface tree you can use [`render_elements_from_surface_tree`].
//!
//! ```no_run
//! # #[cfg(all(
//! #     feature = "wayland_frontend",
//! #     feature = "backend_egl",
//! #     feature = "use_system_lib"
//! # ))]
//! # use smithay::backend::{
//! #     egl::{self, display::EGLBufferReader},
//! #     renderer::ImportEgl,
//! # };
//! # use smithay::{
//! #     backend::allocator::{Fourcc, dmabuf::Dmabuf},
//! #     backend::renderer::{
//! #         DebugFlags, Frame, ImportDma, ImportDmaWl, ImportMem, ImportMemWl, Renderer, Texture,
//! #         TextureFilter, sync::SyncPoint,
//! #     },
//! #     utils::{Buffer, Physical},
//! #     wayland::compositor::SurfaceData,
//! # };
//! #
//! # #[derive(Clone, Debug)]
//! # struct FakeTexture;
//! #
//! # impl Texture for FakeTexture {
//! #     fn width(&self) -> u32 {
//! #         unimplemented!()
//! #     }
//! #     fn height(&self) -> u32 {
//! #         unimplemented!()
//! #     }
//! #     fn format(&self) -> Option<Fourcc> {
//! #         unimplemented!()
//! #     }
//! # }
//! #
//! # struct FakeFrame;
//! #
//! # impl Frame for FakeFrame {
//! #     type Error = std::convert::Infallible;
//! #     type TextureId = FakeTexture;
//! #
//! #     fn id(&self) -> usize { unimplemented!() }
//! #     fn clear(&mut self, _: [f32; 4], _: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn draw_solid(
//! #         &mut self,
//! #         _dst: Rectangle<i32, Physical>,
//! #         _damage: &[Rectangle<i32, Physical>],
//! #         _color: [f32; 4],
//! #     ) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn render_texture_from_to(
//! #         &mut self,
//! #         _: &Self::TextureId,
//! #         _: Rectangle<f64, Buffer>,
//! #         _: Rectangle<i32, Physical>,
//! #         _: &[Rectangle<i32, Physical>],
//! #         _: Transform,
//! #         _: f32,
//! #     ) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn transformation(&self) -> Transform {
//! #         unimplemented!()
//! #     }
//! #     fn finish(self) -> Result<SyncPoint, Self::Error> { unimplemented!() }
//! #     fn wait(&mut self, sync: &SyncPoint) -> Result<(), Self::Error> { unimplemented!() }
//! # }
//! #
//! # #[derive(Debug)]
//! # struct FakeRenderer;
//! #
//! # impl Renderer for FakeRenderer {
//! #     type Error = std::convert::Infallible;
//! #     type TextureId = FakeTexture;
//! #     type Frame<'a> = FakeFrame;
//! #
//! #     fn id(&self) -> usize {
//! #         unimplemented!()
//! #     }
//! #     fn downscale_filter(&mut self, _: TextureFilter) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn upscale_filter(&mut self, _: TextureFilter) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn set_debug_flags(&mut self, _: DebugFlags) {
//! #         unimplemented!()
//! #     }
//! #     fn debug_flags(&self) -> DebugFlags {
//! #         unimplemented!()
//! #     }
//! #     fn render(&mut self, _: Size<i32, Physical>, _: Transform) -> Result<Self::Frame<'_>, Self::Error>
//! #     {
//! #         unimplemented!()
//! #     }
//! #     fn wait(&mut self, sync: &SyncPoint) -> Result<(), Self::Error> { unimplemented!() }
//! # }
//! #
//! # impl ImportMem for FakeRenderer {
//! #     fn import_memory(
//! #         &mut self,
//! #         _: &[u8],
//! #         _: Fourcc,
//! #         _: Size<i32, Buffer>,
//! #         _: bool,
//! #     ) -> Result<Self::TextureId, Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn update_memory(
//! #         &mut self,
//! #         _: &Self::TextureId,
//! #         _: &[u8],
//! #         _: Rectangle<i32, Buffer>,
//! #     ) -> Result<(), Self::Error> {
//! #         unimplemented!()
//! #     }
//! #     fn mem_formats(&self) -> Box<dyn Iterator<Item=Fourcc>> {
//! #         unimplemented!()
//! #     }
//! # }
//! #
//! # impl ImportMemWl for FakeRenderer {
//! #     fn import_shm_buffer(
//! #         &mut self,
//! #         _buffer: &wayland_server::protocol::wl_buffer::WlBuffer,
//! #         _surface: Option<&SurfaceData>,
//! #         _damage: &[Rectangle<i32, Buffer>],
//! #     ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
//! #         unimplemented!()
//! #     }
//! # }
//! # #[cfg(all(
//! #     feature = "wayland_frontend",
//! #     feature = "backend_egl",
//! #     feature = "use_system_lib"
//! # ))]
//! # impl ImportEgl for FakeRenderer {
//! #     fn bind_wl_display(
//! #         &mut self,
//! #         _display: &wayland_server::DisplayHandle,
//! #     ) -> Result<(), egl::Error> {
//! #         unimplemented!()
//! #     }
//! #
//! #     fn unbind_wl_display(&mut self) {
//! #         unimplemented!()
//! #     }
//! #
//! #     fn egl_reader(&self) -> Option<&EGLBufferReader> {
//! #         unimplemented!()
//! #     }
//! #
//! #     fn import_egl_buffer(
//! #         &mut self,
//! #         _buffer: &wayland_server::protocol::wl_buffer::WlBuffer,
//! #         _surface: Option<&SurfaceData>,
//! #         _damage: &[Rectangle<i32, Buffer>],
//! #     ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
//! #         unimplemented!()
//! #     }
//! # }
//! #
//! # impl ImportDma for FakeRenderer {
//! #     fn import_dmabuf(
//! #         &mut self,
//! #         _dmabuf: &Dmabuf,
//! #         _damage: Option<&[Rectangle<i32, Buffer>]>,
//! #     ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
//! #         unimplemented!()
//! #     }
//! # }
//! #
//! # impl ImportDmaWl for FakeRenderer {}
//! use smithay::{
//!     backend::renderer::{
//!         damage::OutputDamageTracker,
//!         element::{
//!             Kind,
//!             surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
//!         },
//!     },
//!     utils::{Point, Rectangle, Size, Transform},
//! };
//! # use wayland_server::{backend::ObjectId, protocol::wl_surface::WlSurface, Resource};
//! # let display = wayland_server::Display::<()>::new().unwrap();
//! # let dh = display.handle();
//! # let surface = WlSurface::from_id(&dh, ObjectId::null()).unwrap();
//!
//! // Initialize a static damage tracked renderer
//! let mut damage_tracker = OutputDamageTracker::new((800, 600), 1.0, Transform::Normal);
//! # let mut renderer = FakeRenderer;
//!
//! loop {
//!     // Create the render elements from the surface
//!     let location = Point::from((100, 100));
//!     let render_elements: Vec<WaylandSurfaceRenderElement<FakeRenderer>> =
//!         render_elements_from_surface_tree(&mut renderer, &surface, location, 1.0, 1.0, Kind::Unspecified);
//!
//!     // Render the element(s)
//!     damage_tracker
//!         .render_output(&mut renderer, 0, &*render_elements, [0.8, 0.8, 0.9, 1.0])
//!         .expect("failed to render output");
//! }
//! ```

use std::{fmt, marker::PhantomData};

use tracing::{instrument, warn};
use wayland_server::protocol::wl_surface;

use crate::{
    backend::renderer::{
        utils::{DamageSet, RendererSurfaceStateUserData},
        Frame, ImportAll, Renderer, Texture,
    },
    utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::compositor::{self, SurfaceData, TraversalAction},
};

use super::{CommitCounter, Element, Id, Kind, RenderElement, UnderlyingStorage};

/// Retrieve the [`WaylandSurfaceRenderElement`]s for a surface tree
#[instrument(level = "trace", skip(renderer, location, scale))]
#[profiling::function]
pub fn render_elements_from_surface_tree<R, E>(
    renderer: &mut R,
    surface: &wl_surface::WlSurface,
    location: impl Into<Point<i32, Physical>>,
    scale: impl Into<Scale<f64>>,
    alpha: f32,
    kind: Kind,
) -> Vec<E>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
    E: From<WaylandSurfaceRenderElement<R>>,
{
    let location = location.into().to_f64();
    let scale = scale.into();
    let mut surfaces: Vec<E> = Vec::new();

    compositor::with_surface_tree_downward(
        surface,
        location,
        |_, states, location| {
            let mut location = *location;
            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            if let Some(data) = data {
                let data = &*data.borrow();

                if let Some(view) = data.view() {
                    location += view.offset.to_f64().to_physical(scale);
                    TraversalAction::DoChildren(location)
                } else {
                    TraversalAction::SkipChildren
                }
            } else {
                TraversalAction::SkipChildren
            }
        },
        |surface, states, location| {
            let mut location = *location;
            let data = states.data_map.get::<RendererSurfaceStateUserData>();

            if let Some(data) = data {
                let has_view = if let Some(view) = data.borrow().view() {
                    location += view.offset.to_f64().to_physical(scale);
                    true
                } else {
                    false
                };

                if has_view {
                    match WaylandSurfaceRenderElement::from_surface(
                        renderer, surface, states, location, alpha, kind,
                    ) {
                        Ok(surface) => surfaces.push(surface.into()),
                        Err(err) => {
                            warn!("Failed to import surface: {}", err);
                        }
                    };
                }
            }
        },
        |_, _, _| true,
    );

    surfaces
}

/// A single surface render element
pub struct WaylandSurfaceRenderElement<R> {
    id: Id,
    location: Point<f64, Physical>,
    alpha: f32,
    surface: wl_surface::WlSurface,
    renderer_type: PhantomData<R>,
    kind: Kind,
}

impl<R> fmt::Debug for WaylandSurfaceRenderElement<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WaylandSurfaceRenderElement")
            .field("id", &self.id)
            .field("location", &self.location)
            .field("surface", &self.surface)
            .finish()
    }
}

impl<R: Renderer + ImportAll> WaylandSurfaceRenderElement<R> {
    /// Create a render element from a surface
    #[profiling::function]
    pub fn from_surface(
        renderer: &mut R,
        surface: &wl_surface::WlSurface,
        states: &SurfaceData,
        location: Point<f64, Physical>,
        alpha: f32,
        kind: Kind,
    ) -> Result<Self, <R as Renderer>::Error>
    where
        <R as Renderer>::TextureId: 'static,
    {
        let id = Id::from_wayland_resource(surface);
        crate::backend::renderer::utils::import_surface(renderer, states)?;

        Ok(Self {
            id,
            location,
            alpha,
            surface: surface.clone(),
            renderer_type: PhantomData,
            kind,
        })
    }

    fn size(&self, scale: impl Into<Scale<f64>>) -> Size<i32, Physical> {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.and_then(|d| d.borrow().view()).map(|surface_view| {
                ((surface_view.dst.to_f64().to_physical(scale).to_point() + self.location).to_i32_round()
                    - self.location.to_i32_round())
                .to_size()
            })
        })
        .unwrap_or_default()
    }
}

impl<R: Renderer + ImportAll> Element for WaylandSurfaceRenderElement<R> {
    fn id(&self) -> &Id {
        &self.id
    }

    fn current_commit(&self) -> CommitCounter {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.map(|d| d.borrow().current_commit())
        })
        .unwrap_or_default()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        Rectangle::from_loc_and_size(self.location.to_i32_round(), self.size(scale))
    }

    fn src(&self) -> Rectangle<f64, Buffer> {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            if let Some(data) = data {
                let data = data.borrow();

                if let Some(view) = data.view() {
                    Some(view.src.to_buffer(
                        data.buffer_scale as f64,
                        data.buffer_transform,
                        &data.buffer_size().unwrap().to_f64(),
                    ))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .unwrap_or_default()
    }

    fn transform(&self) -> Transform {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.map(|d| d.borrow().buffer_transform)
        })
        .unwrap_or_default()
    }

    fn damage_since(&self, scale: Scale<f64>, commit: Option<CommitCounter>) -> DamageSet<i32, Physical> {
        let dst_size = self.size(scale);

        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.and_then(|d| {
                let data = d.borrow();
                if let Some(surface_view) = data.view() {
                    let damage = data
                        .damage_since(commit)
                        .iter()
                        .filter_map(|rect| {
                            rect.to_f64()
                                // first bring the damage into logical space
                                // Note: We use f64 for this as the damage could
                                // be not dividable by the buffer scale without
                                // a rest
                                .to_logical(
                                    data.buffer_scale as f64,
                                    data.buffer_transform,
                                    &data.buffer_dimensions.unwrap().to_f64(),
                                )
                                // then crop by the surface view (viewporter for example could define a src rect)
                                .intersection(surface_view.src)
                                // move and scale the cropped rect (viewporter could define a dst size)
                                .map(|rect| surface_view.rect_to_global(rect).to_i32_up::<i32>())
                                // now bring the damage to physical space
                                .map(|rect| {
                                    // We calculate the scale between to rounded
                                    // surface size and the scaled surface size
                                    // and use it to scale the damage to the rounded
                                    // surface size by multiplying the output scale
                                    // with the result.
                                    let surface_scale =
                                        dst_size.to_f64() / surface_view.dst.to_f64().to_physical(scale);
                                    rect.to_physical_precise_up(surface_scale * scale)
                                })
                        })
                        .collect::<DamageSet<_, _>>();

                    Some(damage)
                } else {
                    None
                }
            })
            .unwrap_or_default()
        })
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        if self.alpha < 1.0 {
            return Vec::new();
        }

        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.map(|d| {
                let data = d.borrow();
                data.opaque_regions()
                    .map(|r| {
                        r.iter()
                            .map(|r| {
                                let loc = r.loc.to_physical_precise_round(scale);
                                let size = ((r.size.to_f64().to_physical(scale).to_point() + self.location)
                                    .to_i32_round()
                                    - self.location.to_i32_round())
                                .to_size();
                                Rectangle::from_loc_and_size(loc, size)
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default()
        })
    }

    fn alpha(&self) -> f32 {
        self.alpha
    }

    fn kind(&self) -> Kind {
        self.kind
    }
}

impl<R> RenderElement<R> for WaylandSurfaceRenderElement<R>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: Texture + 'static,
{
    fn underlying_storage(&self, _renderer: &mut R) -> Option<UnderlyingStorage> {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.and_then(|d| d.borrow().buffer().cloned())
                .map(UnderlyingStorage::Wayland)
        })
    }

    #[instrument(level = "trace", skip(frame))]
    #[profiling::function]
    fn draw<'a>(
        &self,
        frame: &mut <R as Renderer>::Frame<'a>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
    ) -> Result<(), R::Error> {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            if let Some(data) = data {
                let data = data.borrow();

                if let Some(texture) = data.texture::<R>(frame.id()) {
                    frame.render_texture_from_to(
                        texture,
                        src,
                        dst,
                        damage,
                        data.buffer_transform,
                        self.alpha,
                    )?;
                } else {
                    warn!("trying to render texture from different renderer");
                }
            }

            Ok(())
        })
    }
}
