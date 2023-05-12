//! Implementation of the multi-gpu [`GraphicsApi`] using
//! user provided GBM devices and OpenGL ES for rendering.

use tracing::warn;
#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
use wayland_server::protocol::wl_buffer;

use crate::backend::{
    drm::{CreateDrmNodeError, DrmNode},
    egl::{EGLContext, EGLDisplay, Error as EGLError},
    renderer::{
        gles::{GlesError, GlesRenderer},
        multigpu::{ApiDevice, Error as MultiError, GraphicsApi},
        Renderer,
    },
    SwapBuffersError,
};
#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
use crate::{
    backend::{
        allocator::{dmabuf::Dmabuf, Buffer as BufferTrait},
        egl::display::EGLBufferReader,
        renderer::{
            multigpu::{Error as MultigpuError, MultiRenderer, MultiTexture},
            Bind, ExportMem, ImportDma, ImportEgl, ImportMem,
        },
    },
    utils::{Buffer as BufferCoords, Rectangle},
};
use gbm::Device as GbmDevice;
#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
use std::borrow::BorrowMut;
use std::{collections::HashMap, os::unix::prelude::AsFd};

/// Errors raised by the [`GbmGlesBackend`]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// EGL api error
    #[error(transparent)]
    Egl(#[from] EGLError),
    /// OpenGL error
    #[error(transparent)]
    Gl(#[from] GlesError),
    /// Error creating a drm node
    #[error(transparent)]
    DrmNode(#[from] CreateDrmNodeError),
}

impl From<Error> for SwapBuffersError {
    fn from(err: Error) -> SwapBuffersError {
        match err {
            x @ Error::DrmNode(_) | x @ Error::Egl(_) => SwapBuffersError::ContextLost(Box::new(x)),
            Error::Gl(x) => x.into(),
        }
    }
}

/// A [`GraphicsApi`] utilizing user-provided GBM Devices and OpenGL ES for rendering.
#[derive(Debug)]
pub struct GbmGlesBackend<R> {
    devices: HashMap<DrmNode, EGLDisplay>,
    _renderer: std::marker::PhantomData<R>,
}

impl<R> Default for GbmGlesBackend<R> {
    fn default() -> Self {
        GbmGlesBackend {
            devices: HashMap::new(),
            _renderer: std::marker::PhantomData,
        }
    }
}

impl<R> GbmGlesBackend<R> {
    /// Add a new GBM device for a given node to the api
    pub fn add_node<T: AsFd + Send + 'static>(
        &mut self,
        node: DrmNode,
        gbm: GbmDevice<T>,
    ) -> Result<(), EGLError> {
        self.devices.insert(node, EGLDisplay::new(gbm)?);
        Ok(())
    }

    /// Remove a given node from the api
    pub fn remove_node(&mut self, node: &DrmNode) {
        self.devices.remove(node);
    }
}

impl<R: From<GlesRenderer> + Renderer<Error = GlesError>> GraphicsApi for GbmGlesBackend<R> {
    type Device = GbmGlesDevice<R>;
    type Error = Error;

    fn enumerate(&self, list: &mut Vec<Self::Device>) -> Result<(), Self::Error> {
        // remove old stuff
        list.retain(|renderer| {
            self.devices
                .keys()
                .any(|node| renderer.node.dev_id() == node.dev_id())
        });

        // add new stuff
        let new_renderers = self
            .devices
            .iter()
            .filter(|(node, _)| {
                !list
                    .iter()
                    .any(|renderer| renderer.node.dev_id() == node.dev_id())
            })
            .map(|(node, display)| {
                let context = EGLContext::new(display).map_err(Error::Egl)?;
                let renderer = unsafe { GlesRenderer::new(context).map_err(Error::Gl)? }.into();

                Ok(GbmGlesDevice {
                    node: *node,
                    _display: display.clone(),
                    renderer,
                })
            })
            .flat_map(|x: Result<GbmGlesDevice<R>, Error>| match x {
                Ok(x) => Some(x),
                Err(x) => {
                    warn!("Skipping GbmDevice: {}", x);
                    None
                }
            })
            .collect::<Vec<GbmGlesDevice<R>>>();
        list.extend(new_renderers);

        // but don't replace already initialized renderers

        Ok(())
    }

    fn identifier() -> &'static str {
        "gbm_gles"
    }
}

// TODO: Replace with specialization impl in multigpu/mod once possible
impl<T: GraphicsApi, R: From<GlesRenderer> + Renderer<Error = GlesError>> std::convert::From<GlesError>
    for MultiError<GbmGlesBackend<R>, T>
where
    T::Error: 'static,
    <<T::Device as ApiDevice>::Renderer as Renderer>::Error: 'static,
{
    fn from(err: GlesError) -> MultiError<GbmGlesBackend<R>, T> {
        MultiError::Render(err)
    }
}

/// [`ApiDevice`] of the [`EglGlesBackend`]
#[derive(Debug)]
pub struct GbmGlesDevice<R> {
    node: DrmNode,
    renderer: R,
    _display: EGLDisplay,
}

impl<R: Renderer> ApiDevice for GbmGlesDevice<R> {
    type Renderer = R;

    fn renderer(&self) -> &Self::Renderer {
        &self.renderer
    }
    fn renderer_mut(&mut self) -> &mut Self::Renderer {
        &mut self.renderer
    }
    fn node(&self) -> &DrmNode {
        &self.node
    }
}

#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
impl<'a, 'b, 'c, R> ImportEgl for MultiRenderer<'a, 'b, 'c, GbmGlesBackend<R>, GbmGlesBackend<R>>
where
    R: From<GlesRenderer>
        + BorrowMut<GlesRenderer>
        + Renderer<Error = GlesError>
        + Bind<Dmabuf>
        + ImportDma
        + ImportMem
        + ImportEgl
        + ExportMem
        + 'static,
{
    fn bind_wl_display(&mut self, display: &wayland_server::DisplayHandle) -> Result<(), EGLError> {
        self.render.renderer_mut().bind_wl_display(display)
    }
    fn unbind_wl_display(&mut self) {
        self.render.renderer_mut().unbind_wl_display()
    }
    fn egl_reader(&self) -> Option<&EGLBufferReader> {
        self.render.renderer().egl_reader()
    }

    fn import_egl_buffer(
        &mut self,
        buffer: &wl_buffer::WlBuffer,
        surface: Option<&crate::wayland::compositor::SurfaceData>,
        damage: &[Rectangle<i32, BufferCoords>],
    ) -> Result<<Self as Renderer>::TextureId, <Self as Renderer>::Error> {
        if let Some(ref mut renderer) = self.target.as_mut() {
            if let Ok(dmabuf) = Self::try_import_egl(renderer.device.renderer_mut(), buffer) {
                let node = *renderer.device.node();
                let texture = MultiTexture::from_surface(surface, dmabuf.size());
                let texture_ref = texture.0.clone();
                let res = self.import_dmabuf_internal(Some(node), &dmabuf, texture, Some(damage));
                if res.is_ok() {
                    if let Some(surface) = surface {
                        surface.data_map.insert_if_missing(|| texture_ref);
                    }
                }
                return res;
            }
        }
        for renderer in self.other_renderers.iter_mut() {
            if let Ok(dmabuf) = Self::try_import_egl(renderer.renderer_mut(), buffer) {
                let node = *renderer.node();
                let texture = MultiTexture::from_surface(surface, dmabuf.size());
                let texture_ref = texture.0.clone();
                let res = self.import_dmabuf_internal(Some(node), &dmabuf, texture, Some(damage));
                if res.is_ok() {
                    if let Some(surface) = surface {
                        surface.data_map.insert_if_missing(|| texture_ref);
                    }
                }
                return res;
            }
        }
        Err(MultigpuError::DeviceMissing)
    }
}

#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
impl<'a, 'b, 'c, R> MultiRenderer<'a, 'b, 'c, GbmGlesBackend<R>, GbmGlesBackend<R>>
where
    R: From<GlesRenderer>
        + BorrowMut<GlesRenderer>
        + Renderer<Error = GlesError>
        + ImportDma
        + ImportMem
        + ImportEgl
        + ExportMem
        + 'static,
{
    fn try_import_egl(
        renderer: &mut R,
        buffer: &wl_buffer::WlBuffer,
    ) -> Result<Dmabuf, MultigpuError<GbmGlesBackend<R>, GbmGlesBackend<R>>> {
        if !renderer
            .borrow_mut()
            .extensions
            .iter()
            .any(|ext| ext == "GL_OES_EGL_image")
        {
            return Err(MultigpuError::Render(GlesError::GLExtensionNotSupported(&[
                "GL_OES_EGL_image",
            ])));
        }

        if renderer.egl_reader().is_none() {
            return Err(MultigpuError::Render(GlesError::EGLBufferAccessError(
                crate::backend::egl::BufferAccessError::NotManaged(crate::backend::egl::EGLError::BadDisplay),
            )));
        }

        renderer
            .borrow_mut()
            .make_current()
            .map_err(GlesError::from)
            .map_err(MultigpuError::Render)?;

        let egl = renderer
            .egl_reader()
            .as_ref()
            .unwrap()
            .egl_buffer_contents(buffer)
            .map_err(GlesError::EGLBufferAccessError)
            .map_err(MultigpuError::Render)?;
        renderer
            .borrow_mut()
            .egl_context()
            .display()
            .create_dmabuf_from_image(egl.image(0).unwrap(), egl.size, egl.y_inverted)
            .map_err(GlesError::BindBufferEGLError)
            .map_err(MultigpuError::Render)
    }
}
