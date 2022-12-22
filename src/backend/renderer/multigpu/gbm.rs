//! Implementation of the multi-gpu [`GraphicsApi`] using
//! EGL for device enumeration and OpenGL ES for rendering.

#[cfg(all(feature = "wayland_frontend", feature = "use_system_lib"))]
use wayland_server::protocol::wl_buffer;

use crate::backend::{
    allocator::{dmabuf::AsDmabuf, Allocator},
    drm::{CreateDrmNodeError, DrmNode},
    egl::{EGLContext, EGLDevice, EGLDisplay, Error as EGLError},
    renderer::{
        gles2::{Gles2Error, Gles2Renderer},
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
use std::{cell::RefCell, collections::HashMap, os::unix::prelude::AsRawFd, rc::Rc};

/// Errors raised by the [`EglGlesBackend`]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Gbm Error: {0:?}")]
    Gbm(#[source] std::io::Error),
    /// EGL api error
    #[error(transparent)]
    Egl(#[from] EGLError),
    /// OpenGL error
    #[error(transparent)]
    Gl(#[from] Gles2Error),
    /// Error creating a drm node
    #[error(transparent)]
    DrmNode(#[from] CreateDrmNodeError),
}

impl From<Error> for SwapBuffersError {
    fn from(err: Error) -> SwapBuffersError {
        match err {
            x @ Error::Gbm(_) | x @ Error::DrmNode(_) | x @ Error::Egl(_) => {
                SwapBuffersError::ContextLost(Box::new(x))
            }
            Error::Gl(x) => x.into(),
        }
    }
}

/// A [`GraphicsApi`] utilizing EGL for device enumeration and OpenGL ES for rendering.
///
/// If not necessary for other operations, it is recommended to not use a
/// [`Gles2Texture`](crate::backend::renderer::gles2::Gles2Texture), but a
/// [`Gles2Renderbuffer`](crate::backend::renderer::gles2::Gles2Renderbuffer)
/// as a `Target`, when creating [`MultiRenderer`](super::MultiRenderer)s
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
    pub fn add_node<T: AsRawFd + Send>(
        &mut self,
        node: DrmNode,
        gbm: Rc<RefCell<GbmDevice<T>>>,
    ) -> Result<(), EGLError> {
        self.devices
            .insert(dbg!(node), unsafe { EGLDisplay::new(&*gbm.borrow(), None)? });
        Ok(())
    }
    pub fn remove_node(&mut self, node: &DrmNode) {
        self.devices.remove(node);
    }
}

impl<R: From<Gles2Renderer> + Renderer<Error = Gles2Error>> GraphicsApi for GbmGlesBackend<R> {
    type Device = GbmGlesDevice<R>;
    type Error = Error;

    fn enumerate(&self, list: &mut Vec<Self::Device>, log: &slog::Logger) -> Result<(), Self::Error> {
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
                let context = EGLContext::new(&display, None).map_err(Error::Egl)?;
                let renderer = unsafe { Gles2Renderer::new(context, None).map_err(Error::Gl)? }.into();
                let node = match EGLDevice::device_for_display(&display)
                    .ok()
                    .and_then(|x| x.try_get_render_node().ok().flatten())
                {
                    Some(node) => dbg!(node),
                    None => *node,
                };

                Ok(GbmGlesDevice {
                    node,
                    _display: display.clone(),
                    renderer,
                })
            })
            .flat_map(|x: Result<GbmGlesDevice<R>, Error>| match x {
                Ok(x) => Some(x),
                Err(x) => {
                    slog::warn!(log, "Skipping GbmDevice: {}", x);
                    None
                }
            })
            .collect::<Vec<GbmGlesDevice<R>>>();
        list.extend(new_renderers);
        // but don't replace already initialized renderers

        Ok(())
    }
}

// TODO: Replace with specialization impl in multigpu/mod once possible
impl<T: GraphicsApi, R: From<Gles2Renderer> + Renderer<Error = Gles2Error>, Alloc>
    std::convert::From<Gles2Error> for MultiError<GbmGlesBackend<R>, T, Alloc>
where
    Alloc: Allocator,
    <Alloc as Allocator>::Buffer: AsDmabuf,
    T::Error: 'static,
    <<T::Device as ApiDevice>::Renderer as Renderer>::Error: 'static,
{
    fn from(err: Gles2Error) -> MultiError<GbmGlesBackend<R>, T, Alloc> {
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
impl<'a, 'b, 'c, R, Alloc> ImportEgl
    for MultiRenderer<'a, 'b, 'c, GbmGlesBackend<R>, GbmGlesBackend<R>, Alloc>
where
    Alloc: Allocator,
    <Alloc as Allocator>::Buffer: AsDmabuf,
    <Alloc as Allocator>::Error: 'static,
    <<Alloc as Allocator>::Buffer as AsDmabuf>::Error: 'static,
    R: From<Gles2Renderer>
        + BorrowMut<Gles2Renderer>
        + Renderer<Error = Gles2Error>
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
impl<'a, 'b, 'c, R, Alloc> MultiRenderer<'a, 'b, 'c, GbmGlesBackend<R>, GbmGlesBackend<R>, Alloc>
where
    Alloc: Allocator,
    <Alloc as Allocator>::Buffer: AsDmabuf,
    R: From<Gles2Renderer>
        + BorrowMut<Gles2Renderer>
        + Renderer<Error = Gles2Error>
        + ImportDma
        + ImportMem
        + ImportEgl
        + ExportMem
        + 'static,
{
    fn try_import_egl(
        renderer: &mut R,
        buffer: &wl_buffer::WlBuffer,
    ) -> Result<Dmabuf, MultigpuError<GbmGlesBackend<R>, GbmGlesBackend<R>, Alloc>> {
        if !renderer
            .borrow_mut()
            .extensions
            .iter()
            .any(|ext| ext == "GL_OES_EGL_image")
        {
            return Err(MultigpuError::Render(Gles2Error::GLExtensionNotSupported(&[
                "GL_OES_EGL_image",
            ])));
        }

        if renderer.egl_reader().is_none() {
            return Err(MultigpuError::Render(Gles2Error::EGLBufferAccessError(
                crate::backend::egl::BufferAccessError::NotManaged(crate::backend::egl::EGLError::BadDisplay),
            )));
        }

        renderer
            .borrow_mut()
            .make_current()
            .map_err(Gles2Error::from)
            .map_err(MultigpuError::Render)?;

        let egl = renderer
            .egl_reader()
            .as_ref()
            .unwrap()
            .egl_buffer_contents(buffer)
            .map_err(Gles2Error::EGLBufferAccessError)
            .map_err(MultigpuError::Render)?;
        renderer
            .borrow_mut()
            .egl_context()
            .display()
            .create_dmabuf_from_image(egl.image(0).unwrap(), egl.size, egl.y_inverted)
            .map_err(Gles2Error::BindBufferEGLError)
            .map_err(MultigpuError::Render)
    }
}
