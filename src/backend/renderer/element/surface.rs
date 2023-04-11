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
//! #         TextureFilter,
//! #     },
//! #     utils::{Buffer, Physical},
//! #     wayland::compositor::SurfaceData,
//! # };
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
//! #     fn finish(self) -> Result<(), Self::Error> { unimplemented!() }
//! # }
//! #
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
//!         element::surface::{render_elements_from_surface_tree, WaylandSurfaceRenderElement},
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
//!         render_elements_from_surface_tree(&mut renderer, &surface, location, 1.0);
//!
//!     // Render the element(s)
//!     damage_tracker
//!         .render_output(&mut renderer, 0, &*render_elements, [0.8, 0.8, 0.9, 1.0])
//!         .expect("failed to render output");
//! }
//! ```

use std::fmt;

use tracing::{instrument, warn};
use wayland_server::protocol::wl_surface;

use crate::{
    backend::renderer::{
        utils::{RendererSurfaceState, RendererSurfaceStateUserData},
        ImportAll, Renderer,
    },
    render_elements,
    utils::{Physical, Point, Rectangle, Scale},
    wayland::compositor::{self, SurfaceData, TraversalAction},
};

use super::{solid::SolidColorRenderElement, texture::TextureRenderElement, Id, UnderlyingStorage};

render_elements! {
    /// A single surface render element
    pub WaylandSurfaceRenderElement<R> where R: ImportAll;
    /// The texture representing the current surface buffer
    Texture=TextureRenderElement<<R as Renderer>::TextureId>,
    /// The single color representing the current surface buffer
    Solid=SolidColorRenderElement,
}

impl<R> fmt::Debug for WaylandSurfaceRenderElement<R>
where
    R: Renderer,
    <R as Renderer>::TextureId: fmt::Debug,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Texture(arg0) => f.debug_tuple("Texture").field(arg0).finish(),
            Self::Solid(arg0) => f.debug_tuple("Solid").field(arg0).finish(),
            Self::_GenericCatcher(arg0) => f.debug_tuple("_GenericCatcher").field(arg0).finish(),
        }
    }
}

impl<R> WaylandSurfaceRenderElement<R>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: Clone + 'static,
{
    /// Create a render element from a surface
    pub fn from_surface(
        renderer: &mut R,
        surface: &wl_surface::WlSurface,
        states: &SurfaceData,
        location: impl Into<Point<f64, Physical>>,
        scale: impl Into<Scale<f64>>,
    ) -> Result<Option<Self>, <R as Renderer>::Error> {
        if let Some(state) = states.data_map.get::<RendererSurfaceStateUserData>() {
            Self::from_surface_renderer_state(
                renderer,
                surface,
                states,
                &mut state.borrow_mut(),
                location,
                scale,
            )
        } else {
            Ok(None)
        }
    }

    /// Create a render element from a surface renderer state
    pub fn from_surface_renderer_state(
        renderer: &mut R,
        surface: &wl_surface::WlSurface,
        states: &SurfaceData,
        state: &mut RendererSurfaceState,
        location: impl Into<Point<f64, Physical>>,
        scale: impl Into<Scale<f64>>,
    ) -> Result<Option<Self>, <R as Renderer>::Error> {
        crate::backend::renderer::utils::import_renderer_surface(renderer, states, state)?;
        Ok(Self::from_renderer_surface_state_internal(
            renderer, surface, state, location, scale,
        ))
    }

    fn from_renderer_surface_state_internal(
        renderer: &mut R,
        surface: &wl_surface::WlSurface,
        state: &mut RendererSurfaceState,
        location: impl Into<Point<f64, Physical>>,
        scale: impl Into<Scale<f64>>,
    ) -> Option<Self> {
        let location = location.into();

        let view = state.view()?;
        let buffer = state.buffer()?;
        let buffer_type = crate::backend::renderer::buffer_type(buffer)?;
        let id = Id::from_wayland_resource(surface);

        if matches!(buffer_type, crate::backend::renderer::BufferType::SinglePixel) {
            let spb = crate::wayland::single_pixel_buffer::get_single_pixel_buffer(buffer).ok()?;
            let color = spb.rgba32f();
            let geometry = Rectangle::from_loc_and_size(
                location.to_i32_round(),
                view.dst.to_physical_precise_round(scale),
            );
            Some(SolidColorRenderElement::new(id, geometry, state.current_commit(), color).into())
        } else {
            let texture = state.texture::<R>(renderer.id())?;

            Some(
                TextureRenderElement::from_texture_with_damage(
                    id,
                    renderer.id(),
                    location,
                    texture.clone(),
                    state.buffer_scale(),
                    state.buffer_transform(),
                    None,
                    Some(view.src),
                    Some(view.dst),
                    state.opaque_regions().map(|r| r.to_vec()), // TODO: maybe make the opaque regions cow
                    state.damage(),
                    Some(UnderlyingStorage::Wayland(buffer.clone())),
                )
                .into(),
            )
        }
    }
}

/// Get a [`WaylandSurfaceRenderElement`] from a [`wl_surface::WlSurface`]
///
/// Note: If the buffer type is not managed by smithay,
/// the surface is not mapped or the import failed `None` is returned
pub fn wayland_surface_render_element<R, E>(
    renderer: &mut R,
    surface: &wl_surface::WlSurface,
    states: &SurfaceData,
    state: &mut RendererSurfaceState,
    location: impl Into<Point<f64, Physical>>,
    scale: impl Into<Scale<f64>>,
) -> Option<E>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: Clone + 'static,
    E: From<WaylandSurfaceRenderElement<R>>,
{
    match WaylandSurfaceRenderElement::from_surface_renderer_state(
        renderer, surface, states, state, location, scale,
    ) {
        Ok(element) => element.map(E::from),
        Err(err) => {
            warn!("Failed to import surface: {}", err);
            None
        }
    }
}

/// Retrieve the render elements for this surface tree by using
/// the provided element factory
pub fn custom_render_elements_from_surface_tree<R, E, F>(
    renderer: &mut R,
    surface: &wl_surface::WlSurface,
    location: impl Into<Point<i32, Physical>>,
    scale: impl Into<Scale<f64>>,
    factory: F,
) -> Vec<E>
where
    R: Renderer,
    F: Fn(
        &mut R,
        &wl_surface::WlSurface,
        &SurfaceData,
        &mut RendererSurfaceState,
        Point<f64, Physical>,
        Scale<f64>,
    ) -> Option<E>,
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
                    if let Some(element) =
                        factory(renderer, surface, states, &mut data.borrow_mut(), location, scale)
                    {
                        surfaces.push(element);
                    }
                }
            }
        },
        |_, _, _| true,
    );

    surfaces
}

/// Retrieve the [`WaylandSurfaceRenderElement`]s for a surface tree
#[instrument(level = "trace", skip(renderer, location, scale))]
pub fn render_elements_from_surface_tree<R, E>(
    renderer: &mut R,
    surface: &wl_surface::WlSurface,
    location: impl Into<Point<i32, Physical>>,
    scale: impl Into<Scale<f64>>,
) -> Vec<E>
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: Clone + 'static,
    E: From<WaylandSurfaceRenderElement<R>>,
{
    custom_render_elements_from_surface_tree(
        renderer,
        surface,
        location,
        scale,
        wayland_surface_render_element,
    )
}
