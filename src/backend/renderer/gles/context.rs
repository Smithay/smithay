use std::{fmt, ops};

use super::{ffi, GlesTexture};
use crate::backend::{
    egl::{EGLContext, EGLSurface, MakeCurrentError},
    renderer::ContextId,
};

pub struct GlesContext {
    egl: EGLContext,
    // XXX only access through CurrentCGlesContext
    pub(super) gl: ffi::Gles2,
}

impl fmt::Debug for GlesContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GlesContext").field("egl", &self.egl).finish()
    }
}

impl GlesContext {
    pub unsafe fn new(egl: EGLContext) -> Self {
        let gl = ffi::Gles2::load_with(|s| crate::backend::egl::get_proc_address(s) as *const _);
        egl.user_data()
            .insert_if_missing_threadsafe(ContextId::<GlesTexture>::new);
        Self { egl, gl }
    }

    pub fn context_id(&self) -> ContextId<GlesTexture> {
        self.egl
            .user_data()
            .get::<ContextId<GlesTexture>>()
            .unwrap()
            .clone()
    }

    pub fn egl(&self) -> &EGLContext {
        &self.egl
    }

    pub unsafe fn make_current(&mut self) -> Result<CurrentGlesContext<'_>, MakeCurrentError> {
        // TODO test current context on thread; re-enterency
        self.egl.make_current()?;
        Ok(CurrentGlesContext(self))
    }

    pub unsafe fn make_current_with_surface(
        &mut self,
        surface: &EGLSurface,
    ) -> Result<CurrentGlesContext<'_>, MakeCurrentError> {
        self.egl.make_current_with_surface(surface)?;
        Ok(CurrentGlesContext(self))
    }

    pub unsafe fn make_current_with_draw_and_read_surface(
        &mut self,
        draw: &EGLSurface,
        read: &EGLSurface,
    ) -> Result<CurrentGlesContext<'_>, MakeCurrentError> {
        self.egl.make_current_with_draw_and_read_surface(draw, read)?;
        Ok(CurrentGlesContext(self))
    }
}

#[derive(Debug)]
pub struct CurrentGlesContext<'a>(&'a mut GlesContext);

impl CurrentGlesContext<'_> {
    pub fn egl(&self) -> &EGLContext {
        &self.0.egl
    }

    pub fn context_id(&self) -> ContextId<GlesTexture> {
        self.0.context_id()
    }
}

impl ops::Deref for CurrentGlesContext<'_> {
    type Target = ffi::Gles2;

    fn deref(&self) -> &ffi::Gles2 {
        &self.0.gl
    }
}
