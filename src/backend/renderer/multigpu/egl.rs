//! Implementation of the multi-gpu [`GraphicsApi`] using
//! EGL for device enumeration and OpenGL ES for rendering.

use crate::backend::{
    drm::{CreateDrmNodeError, DrmNode, NodeType},
    egl::{EGLContext, EGLDevice, EGLDisplay, Error as EGLError},
    renderer::{
        gles2::{Gles2Error, Gles2Renderer},
        multigpu::{ApiDevice, GraphicsApi},
    },
    SwapBuffersError,
};
#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
use crate::{
    backend::{
        allocator::{dmabuf::Dmabuf, Buffer},
        egl::display::EGLBufferReader,
        renderer::{
            multigpu::{Error as MultigpuError, MultiRenderer, MultiTexture},
            ImportEgl, Offscreen, Renderer,
        },
    },
    reexports::wayland_server::protocol::wl_buffer,
    utils::{Buffer as BufferCoords, Rectangle},
};

/// Errors raised by the [`EglGlesBackend`]
#[derive(Debug, thiserror::Error)]
pub enum Error {
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
            x @ Error::DrmNode(_) | x @ Error::Egl(_) => SwapBuffersError::ContextLost(Box::new(x)),
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
pub struct EglGlesBackend;
impl GraphicsApi for EglGlesBackend {
    type Device = EglGlesDevice;
    type Error = Error;

    fn enumerate(&self, list: &mut Vec<Self::Device>, log: &slog::Logger) -> Result<(), Self::Error> {
        let devices = EGLDevice::enumerate()
            .map_err(Error::Egl)?
            .flat_map(|device| {
                let path = device.drm_device_path().ok()?;
                Some((
                    device,
                    DrmNode::from_path(path)
                        .ok()?
                        .node_with_type(NodeType::Render)?
                        .ok()?,
                ))
            })
            .collect::<Vec<_>>();
        // remove old stuff
        list.retain(|renderer| devices.iter().any(|(_, node)| &renderer.node == node));
        // add new stuff
        let new_renderers = devices
            .into_iter()
            .filter(|(_, node)| !list.iter().any(|renderer| &renderer.node == node))
            .map(|(device, node)| {
                slog::info!(log, "Trying to initialize {:?} from {}", device, node);
                let display = EGLDisplay::new(&device, None).map_err(Error::Egl)?;
                let context = EGLContext::new(&display, None).map_err(Error::Egl)?;
                let renderer = unsafe { Gles2Renderer::new(context, None).map_err(Error::Gl)? };

                Ok(EglGlesDevice {
                    node,
                    _device: device,
                    _display: display,
                    renderer,
                })
            })
            .flat_map(|x: Result<EglGlesDevice, Error>| match x {
                Ok(x) => Some(x),
                Err(x) => {
                    slog::warn!(log, "Skipping EGLDevice: {}", x);
                    None
                }
            })
            .collect::<Vec<EglGlesDevice>>();
        list.extend(new_renderers);
        // but don't replace already initialized renderers

        Ok(())
    }
}

/// [`ApiDevice`] of the [`EglGlesBackend`]
#[derive(Debug)]
pub struct EglGlesDevice {
    node: DrmNode,
    renderer: Gles2Renderer,
    _display: EGLDisplay,
    _device: EGLDevice,
}

impl ApiDevice for EglGlesDevice {
    type Renderer = Gles2Renderer;

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

#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
impl<'a, 'b, Target> ImportEgl for MultiRenderer<'a, 'b, EglGlesBackend, EglGlesBackend, Target>
where
    Gles2Renderer: Offscreen<Target>,
{
    fn bind_wl_display(&mut self, display: &wayland_server::Display) -> Result<(), EGLError> {
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

#[cfg(all(
    feature = "wayland_frontend",
    feature = "backend_egl",
    feature = "use_system_lib"
))]
impl<'a, 'b, Target> MultiRenderer<'a, 'b, EglGlesBackend, EglGlesBackend, Target> {
    fn try_import_egl(
        renderer: &mut Gles2Renderer,
        buffer: &wl_buffer::WlBuffer,
    ) -> Result<Dmabuf, MultigpuError<EglGlesBackend, EglGlesBackend>> {
        if !renderer.extensions.iter().any(|ext| ext == "GL_OES_EGL_image") {
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
            .egl_context()
            .display
            .create_dmabuf_from_image(egl.image(0).unwrap(), egl.size, egl.y_inverted)
            .map_err(Gles2Error::BindBufferEGLError)
            .map_err(MultigpuError::Render)
    }
}
