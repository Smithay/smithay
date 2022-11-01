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
//! #     backend::allocator::dmabuf::Dmabuf,
//! #     backend::renderer::{
//! #         Frame, ImportDma, ImportDmaWl, ImportMem, ImportMemWl, Renderer, Texture,
//! #         TextureFilter,
//! #     },
//! #     utils::{Buffer, Physical},
//! #     wayland::compositor::SurfaceData,
//! # };
//! # use slog::Drain;
//! #
//! # #[derive(Clone)]
//! # struct FakeTexture;
//! #
//! # impl Texture for FakeTexture {
//! #     fn width(&self) -> u32 {
//! #         unimplemented!()
//! #     }
//! #     fn height(&self) -> u32 {
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
//! #     fn clear(&mut self, _: [f32; 4], _: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
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
//! # }
//! #
//! # struct FakeRenderer;
//! #
//! # impl Renderer for FakeRenderer {
//! #     type Error = std::convert::Infallible;
//! #     type TextureId = FakeTexture;
//! #     type Frame = FakeFrame;
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
//! #     fn render<F, R>(&mut self, _: Size<i32, Physical>, _: Transform, _: F) -> Result<R, Self::Error>
//! #     where
//! #         F: FnOnce(&mut Self, &mut Self::Frame) -> R,
//! #     {
//! #         unimplemented!()
//! #     }
//! # }
//! #
//! # impl ImportMem for FakeRenderer {
//! #     fn import_memory(
//! #         &mut self,
//! #         _: &[u8],
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
//!         damage::DamageTrackedRenderer,
//!         element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
//!     },
//!     utils::{Point, Rectangle, Size, Transform},
//! };
//! # use wayland_server::{backend::ObjectId, protocol::wl_surface::WlSurface, Resource};
//! # let log = slog::Logger::root(slog::Discard.fuse(), slog::o!());
//! # let display = wayland_server::Display::<()>::new().unwrap();
//! # let dh = display.handle();
//! # let surface = WlSurface::from_id(&dh, ObjectId::null()).unwrap();
//!
//! // Initialize a static damage tracked renderer
//! let mut damage_tracked_renderer = DamageTrackedRenderer::new((800, 600), 1.0, Transform::Normal);
//! # let mut renderer = FakeRenderer;
//!
//! loop {
//!     // Create the render elements from the surface
//!     let location = Point::from((100, 100));
//!     let render_elements: Vec<WaylandSurfaceRenderElement> =
//!         render_elements_from_surface_tree(&surface, location, 1.0);
//!
//!     // Render the element(s)
//!     damage_tracked_renderer
//!         .render_output(&mut renderer, 0, &*render_elements, [0.8, 0.8, 0.9, 1.0], log.clone())
//!         .expect("failed to render output");
//! }
//! ```

use wayland_server::protocol::wl_surface;

use crate::{
    backend::renderer::{utils::RendererSurfaceStateUserData, Frame, ImportAll, Renderer, Texture},
    utils::{Buffer, Physical, Point, Rectangle, Scale, Size, Transform},
    wayland::compositor::{self, TraversalAction},
};

use super::{CommitCounter, Element, Id, RenderElement, UnderlyingStorage};

/// Retrieve the [`WaylandSurfaceRenderElement`]s for a surface tree
pub fn render_elements_from_surface_tree<E>(
    surface: &wl_surface::WlSurface,
    location: impl Into<Point<i32, Physical>>,
    scale: impl Into<Scale<f64>>,
) -> Vec<E>
where
    E: From<WaylandSurfaceRenderElement>,
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
                let data = &*data.borrow();

                if let Some(view) = data.view() {
                    location += view.offset.to_f64().to_physical(scale);

                    let surface = WaylandSurfaceRenderElement::from_surface(surface, location);
                    surfaces.push(surface.into());
                }
            }
        },
        |_, _, _| true,
    );

    surfaces
}

/// A single surface render element
#[derive(Debug)]
pub struct WaylandSurfaceRenderElement {
    id: Id,
    location: Point<f64, Physical>,
    surface: wl_surface::WlSurface,
}

impl WaylandSurfaceRenderElement {
    /// Create a render element from a surface
    pub fn from_surface(surface: &wl_surface::WlSurface, location: Point<f64, Physical>) -> Self {
        let id = Id::from_wayland_resource(surface);

        Self {
            id,
            location,
            surface: surface.clone(),
        }
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

impl Element for WaylandSurfaceRenderElement {
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

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> Vec<Rectangle<i32, Physical>> {
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
                        .collect::<Vec<_>>();

                    Some(damage)
                } else {
                    None
                }
            })
            .unwrap_or_default()
        })
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.map(|d| {
                let data = d.borrow();
                data.opaque_regions()
                    .map(|r| {
                        r.iter()
                            .map(|r| r.to_physical_precise_up(scale))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default()
        })
    }
}

impl<R> RenderElement<R> for WaylandSurfaceRenderElement
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: Texture + 'static,
{
    fn underlying_storage(&self, _renderer: &R) -> Option<UnderlyingStorage<'_, R>> {
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            data.and_then(|d| d.borrow().wl_buffer().cloned())
                .map(|b| UnderlyingStorage::Wayland(b))
        })
    }

    fn draw(
        &self,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        damage: &[Rectangle<i32, Physical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error> {
        crate::backend::renderer::utils::import_surface_tree(renderer, &self.surface, log)?;

        let dst_size = self.size(scale);
        compositor::with_states(&self.surface, |states| {
            let data = states.data_map.get::<RendererSurfaceStateUserData>();
            if let Some(data) = data {
                let data = data.borrow();

                if let Some(texture) = data.texture(renderer) {
                    if let Some(surface_view) = data.view() {
                        let src = surface_view.src.to_buffer(
                            data.buffer_scale as f64,
                            data.buffer_transform,
                            &data.buffer_size().unwrap().to_f64(),
                        );

                        let dst = Rectangle::from_loc_and_size(location, dst_size);

                        frame.render_texture_from_to(
                            texture,
                            src,
                            dst,
                            damage,
                            data.buffer_transform,
                            1.0f32,
                        )?;
                    }
                }
            }

            Ok(())
        })
    }
}
