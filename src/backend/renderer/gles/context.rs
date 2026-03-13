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
        Ok(CurrentGlesContext {
            context: self,
            draw_read_surfaces: None,
        })
    }

    pub unsafe fn make_current_with_surface<'a>(
        &'a mut self,
        surface: &'a EGLSurface,
    ) -> Result<CurrentGlesContext<'a>, MakeCurrentError> {
        self.make_current_with_draw_and_read_surface(surface, surface)
    }

    pub unsafe fn make_current_with_draw_and_read_surface<'a>(
        &'a mut self,
        draw: &'a EGLSurface,
        read: &'a EGLSurface,
    ) -> Result<CurrentGlesContext<'a>, MakeCurrentError> {
        self.egl.make_current_with_draw_and_read_surface(draw, read)?;
        Ok(CurrentGlesContext {
            context: self,
            draw_read_surfaces: Some((draw, read)),
        })
    }
}

#[derive(Debug)]
pub struct CurrentGlesContext<'a> {
    context: &'a mut GlesContext,
    draw_read_surfaces: Option<(&'a EGLSurface, &'a EGLSurface)>,
}

impl CurrentGlesContext<'_> {
    pub fn egl(&self) -> &EGLContext {
        &self.context.egl
    }

    pub fn context_id(&self) -> ContextId<GlesTexture> {
        self.context.context_id()
    }

    pub fn call_without_current<T, F: FnOnce(&mut GlesContext) -> T>(
        &mut self,
        f: F,
    ) -> Result<T, MakeCurrentError> {
        // TODO make context not current?
        let res = f(self.context);
        unsafe {
            if let Some((draw, read)) = self.draw_read_surfaces {
                self.egl().make_current_with_draw_and_read_surface(draw, read)?;
            } else {
                self.egl().make_current()?;
            }
        }
        Ok(res)
    }
}

// TODO make no context curreent on Drop? At least on debug_assertions?

impl ops::Deref for CurrentGlesContext<'_> {
    type Target = ffi::Gles2;

    fn deref(&self) -> &ffi::Gles2 {
        &self.context.gl
    }
}
