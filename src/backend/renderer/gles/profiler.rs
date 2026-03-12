//! GPU profiling helpers

use super::ffi;

/// GPU profiling span location.
///
/// Create using the [`crate::gpu_span_location!`] macro.
///
/// When `tracy_gpu_profiling` feature is enabled, this wraps a Tracy span location.
/// When disabled, this is a zero-sized no-op type.
#[derive(Clone, Copy)]
#[allow(missing_debug_implementations)] // Tracy SpanLocation doesn't impl Debug.
pub struct SpanLocation(#[cfg(feature = "tracy_gpu_profiling")] pub &'static tracy_client::SpanLocation);

/// Creates a GPU span location for profiling.
///
/// When `tracy_gpu_profiling` feature is enabled, this wraps `tracy_client::span_location!`.
/// When disabled, returns a no-op placeholder.
#[cfg(feature = "tracy_gpu_profiling")]
#[macro_export]
macro_rules! gpu_span_location {
    () => {
        $crate::backend::renderer::gles::profiler::SpanLocation(
            $crate::reexports::tracy_client::span_location!(),
        )
    };
    ($name:expr) => {
        $crate::backend::renderer::gles::profiler::SpanLocation(
            $crate::reexports::tracy_client::span_location!($name),
        )
    };
}

/// Creates a GPU span location for profiling.
///
/// When `tracy_gpu_profiling` feature is enabled, this wraps `tracy_client::span_location!`.
/// When disabled, returns a no-op placeholder.
#[cfg(not(feature = "tracy_gpu_profiling"))]
#[macro_export]
macro_rules! gpu_span_location {
    () => {
        $crate::backend::renderer::gles::profiler::SpanLocation()
    };
    ($name:expr) => {
        $crate::backend::renderer::gles::profiler::SpanLocation()
    };
}

/// An active GPU profiling span.
///
/// This type represents a GPU profiling span that has been entered but not yet exited.
/// It must be passed to `exit_gpu_span()` when the profiled code section is complete.
///
/// # Panics
///
/// Dropping this type without calling `exit_gpu_span()` will panic to ensure GPU profiling
/// spans are properly closed.
#[derive(Debug)]
pub struct GpuSpan {
    active: bool,
}

impl Drop for GpuSpan {
    fn drop(&mut self) {
        debug_assert!(!self.active, "GPU span must be properly exited");
    }
}

pub(crate) struct ScopedGpuSpan<'a, 'b> {
    span: Option<GpuSpan>,
    profiler: &'a GpuProfiler,
    gl: &'b ffi::Gles2,
}

impl Drop for ScopedGpuSpan<'_, '_> {
    fn drop(&mut self) {
        let span = self.span.take().unwrap();
        self.profiler.exit(self.gl, span);
    }
}

#[cfg(feature = "tracy_gpu_profiling")]
mod imp {
    use std::cell::Cell;
    use std::ffi::CStr;
    use std::os::raw::c_char;

    use tracing::warn;

    use super::{ffi, GpuSpan, SpanLocation};

    // Number of timestamp queries in the pool. Limited by Tracy's use of u16 for query IDs.
    const MAX_QUERIES: usize = u16::MAX as usize;

    pub struct GpuProfiler {
        // `None` means the required GL extension is not supported.
        pool: Option<QueryPool>,
    }

    struct QueryPool {
        context: tracy_client::GpuContext,
        pool: Vec<GpuQuery>,
        // Index of first free and first pending queries in the pool.
        head_tail: Cell<(usize, usize)>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    #[repr(transparent)]
    struct GpuQuery(ffi::types::GLuint);

    impl QueryPool {
        fn new(context: tracy_client::GpuContext, gl: &ffi::Gles2) -> Self {
            let mut pool = vec![GpuQuery(0); MAX_QUERIES];
            unsafe {
                gl.GenQueriesEXT(pool.len() as ffi::types::GLsizei, pool.as_mut_ptr().cast());
            }

            Self {
                context,
                pool,
                head_tail: Cell::new((0, 0)),
            }
        }

        fn next(&self, gl: &ffi::Gles2) -> GpuQuery {
            let (head, tail) = self.head_tail.get();
            let query = self.pool[head];

            let new_head = (head + 1) % MAX_QUERIES;
            assert_ne!(new_head, tail, "ran out of queries");
            self.head_tail.set((new_head, tail));

            unsafe { gl.QueryCounterEXT(query.0, ffi::TIMESTAMP_EXT) };
            query
        }

        fn collect(&mut self, gl: &ffi::Gles2) {
            let (head, tail) = self.head_tail.get_mut();
            if tail == head {
                return;
            }
            profiling::scope!("QueryPool::collect");

            while tail != head {
                let query = self.pool[*tail];

                let mut available = 0;
                unsafe {
                    while gl.GetError() != ffi::NO_ERROR {}
                    gl.GetQueryObjectuivEXT(query.0, ffi::QUERY_RESULT_AVAILABLE, &mut available);
                    if gl.GetError() != ffi::NO_ERROR {
                        // Don't really have a good way out of this.
                        available = 1;
                    }
                }
                if available == 0 {
                    return;
                }

                let mut timestamp = 0;
                unsafe { gl.GetQueryObjecti64vEXT(query.0, ffi::QUERY_RESULT, &mut timestamp) };

                self.context.upload_gpu_timestamp(query.0 as u16, timestamp);

                *tail = (*tail + 1) % MAX_QUERIES;
            }
        }

        /// Clean up the timestamp query pool by deleting them.
        ///
        /// If `gl` is `None` then all queries are leaked. Pass `None` only when failing to obtain
        /// the GL context during drop.
        fn cleanup(&mut self, gl: Option<&ffi::Gles2>) {
            if self.pool.is_empty() {
                return;
            }

            if let Some(gl) = gl {
                unsafe {
                    gl.DeleteQueriesEXT(self.pool.len() as ffi::types::GLsizei, self.pool.as_ptr().cast());
                }
            }

            self.pool.clear();
        }
    }

    impl Drop for QueryPool {
        fn drop(&mut self) {
            debug_assert!(self.pool.is_empty(), "QueryPool must be cleaned up before drop");
        }
    }

    impl GpuProfiler {
        pub fn new(gl: &ffi::Gles2, extensions: &[String]) -> Self {
            let has_timer_query = extensions.iter().any(|ext| ext == "GL_EXT_disjoint_timer_query");
            if !has_timer_query {
                warn!("GPU profiling enabled but GL_EXT_disjoint_timer_query is not supported");
                return Self { pool: None };
            }

            let gpu_name = unsafe { CStr::from_ptr(gl.GetString(ffi::RENDERER) as *const c_char).to_str() };
            let gpu_name = gpu_name.unwrap_or("GlesRenderer");

            let client = tracy_client::Client::start();

            let mut gpu_timestamp = 0;
            unsafe { gl.GetInteger64v(ffi::TIMESTAMP_EXT, &mut gpu_timestamp) };

            let context = client
                .new_gpu_context(
                    Some(gpu_name),
                    tracy_client::GpuContextType::OpenGL,
                    gpu_timestamp,
                    1.0,
                )
                .unwrap();

            let pool = QueryPool::new(context, gl);

            Self { pool: Some(pool) }
        }

        pub fn enter(&self, span_location: SpanLocation, gl: &ffi::Gles2) -> GpuSpan {
            let Some(pool) = &self.pool else {
                return GpuSpan { active: false };
            };

            if !tracy_client::Client::is_connected() {
                return GpuSpan { active: false };
            }

            let query = pool.next(gl);
            pool.context.begin_span(span_location.0, query.0 as u16);

            GpuSpan { active: true }
        }

        pub fn exit(&self, gl: &ffi::Gles2, mut entered: GpuSpan) {
            if !entered.active {
                return;
            }
            entered.active = false;

            let Some(pool) = &self.pool else {
                return;
            };

            let query = pool.next(gl);
            pool.context.end_span(query.0 as u16);
        }

        /// Collect completed timestamp queries and send them to the profiler.
        ///
        /// Must be called regularly to avoid filling up the query pool. A good place is right
        /// before a batch of rendering operations.
        pub fn collect(&mut self, gl: &ffi::Gles2) {
            if let Some(pool) = &mut self.pool {
                pool.collect(gl);
            }
        }

        /// Sync the GPU and CPU times by uploading the current GPU timestamp to the profiler.
        ///
        /// Necessary to avoid GPU timestamp drift. A good place to call this is right after
        /// flushing a batch of rendering operations.
        pub fn sync_gpu(&self, gl: &ffi::Gles2) {
            let Some(pool) = &self.pool else {
                return;
            };

            let mut gpu_timestamp = 0;
            unsafe { gl.GetInteger64v(ffi::TIMESTAMP_EXT, &mut gpu_timestamp) };
            pool.context.sync_gpu_time(gpu_timestamp);
        }

        /// Clean up the timestamp query pool by deleting them.
        ///
        /// If `gl` is `None` then all queries are leaked. Pass `None` only when failing to obtain
        /// the GL context during drop.
        pub fn cleanup(&mut self, gl: Option<&ffi::Gles2>) {
            if let Some(pool) = &mut self.pool {
                pool.cleanup(gl);
            }
        }
    }
}

#[cfg(not(feature = "tracy_gpu_profiling"))]
mod imp {
    use super::{ffi, GpuSpan, SpanLocation};

    pub struct GpuProfiler(());

    impl GpuProfiler {
        pub fn new(_gl: &ffi::Gles2, _extensions: &[String]) -> Self {
            Self(())
        }

        pub fn enter(&self, _span_location: SpanLocation, _gl: &ffi::Gles2) -> GpuSpan {
            GpuSpan { active: true }
        }

        pub fn exit(&self, _gl: &ffi::Gles2, mut entered: GpuSpan) {
            entered.active = false;
        }

        pub fn collect(&mut self, _gl: &ffi::Gles2) {}

        pub fn sync_gpu(&self, _gl: &ffi::Gles2) {}

        pub fn cleanup(&mut self, _gl: Option<&ffi::Gles2>) {}
    }
}

pub(crate) use imp::*;

impl GpuProfiler {
    pub fn scope<'a, 'b>(&'a self, span_location: SpanLocation, gl: &'b ffi::Gles2) -> ScopedGpuSpan<'a, 'b> {
        let span = self.enter(span_location, gl);

        ScopedGpuSpan {
            span: Some(span),
            gl,
            profiler: self,
        }
    }
}
