//! GPU profiling helpers

use super::ffi;

/// GPU profiling span location.
///
/// Create using the [`crate::gpu_span_location!`] macro.
///
/// When `tracy_gpu_profiling` feature is enabled, this wraps a Tracy span location.
/// When disabled, this is a zero-sized no-op type.
#[derive(Clone, Copy)]
#[allow(missing_debug_implementations)] // Tracy SpanLocation doens't impl Debug.
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
        assert!(!self.active, "GPU span must be properly exited");
    }
}

pub(crate) struct ScopedGpuSpan<'a, 'b> {
    span: Option<GpuSpan>,
    profiler: &'a mut GpuProfiler,
    gl: &'b ffi::Gles2,
}

impl<'a, 'b> Drop for ScopedGpuSpan<'a, 'b> {
    fn drop(&mut self) {
        let span = self.span.take().unwrap();
        self.profiler.exit(self.gl, span);
    }
}

#[cfg(feature = "tracy_gpu_profiling")]
mod imp {
    use super::{ffi, GpuSpan, SpanLocation};

    // Number of timestamp queries in the pool. Limited by Tracy's use of u16 for query IDs.
    const MAX_QUERIES: usize = u16::MAX as usize;

    pub struct GpuProfiler {
        context: tracy_client::GpuContext,
        pool: QueryPool,
    }

    struct QueryPool {
        context: tracy_client::GpuContext,
        pool: Box<[GpuQuery]>,
        // Index of first free query in the pool.
        head: usize,
        // Index of first pending query in the pool.
        tail: usize,
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
                pool: pool.into_boxed_slice(),
                head: 0,
                tail: 0,
            }
        }

        fn next(&mut self, gl: &ffi::Gles2) -> GpuQuery {
            let query = self.pool[self.head];

            let new_head = (self.head + 1) % MAX_QUERIES;
            assert_ne!(new_head, self.tail, "ran out of queries");
            self.head = new_head;

            unsafe { gl.QueryCounterEXT(query.0, ffi::TIMESTAMP_EXT) };
            query
        }

        fn collect(&mut self, gl: &ffi::Gles2) {
            while self.tail != self.head {
                let query = self.pool[self.tail];

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

                self.tail = (self.tail + 1) % MAX_QUERIES;
            }
        }
    }

    impl Drop for QueryPool {
        fn drop(&mut self) {
            // TODO no gl here, but we want to delete the queries
        }
    }

    impl GpuProfiler {
        pub fn new(gl: &ffi::Gles2) -> Self {
            let client = tracy_client::Client::start();

            let mut gpu_timestamp = 0;
            unsafe { gl.GetInteger64v(ffi::TIMESTAMP_EXT, &mut gpu_timestamp) };

            let context = client
                .new_gpu_context(
                    Some("GlesRenderer"),
                    tracy_client::GpuContextType::OpenGL,
                    gpu_timestamp,
                    1.0,
                )
                .unwrap();

            let pool = QueryPool::new(context.clone(), gl);

            Self { context, pool }
        }

        pub fn enter(&mut self, span_location: SpanLocation, gl: &ffi::Gles2) -> GpuSpan {
            if !tracy_client::Client::is_connected() {
                return GpuSpan { active: false };
            }

            let query = self.pool.next(gl);
            self.context.begin_span(span_location.0, query.0 as u16);

            GpuSpan { active: true }
        }

        pub fn exit(&mut self, gl: &ffi::Gles2, mut entered: GpuSpan) {
            if !entered.active {
                return;
            }
            entered.active = false;

            let query = self.pool.next(gl);
            self.context.end_span(query.0 as u16);
        }

        pub fn collect(&mut self, gl: &ffi::Gles2) {
            self.pool.collect(gl);
        }

        pub fn sync_gpu(&self, gl: &ffi::Gles2) {
            let mut gpu_timestamp = 0;
            unsafe { gl.GetInteger64v(ffi::TIMESTAMP_EXT, &mut gpu_timestamp) };
            self.context.sync_gpu_time(gpu_timestamp);
        }
    }
}

#[cfg(not(feature = "tracy_gpu_profiling"))]
mod imp {
    use super::{ffi, GpuSpan, SpanLocation};

    pub struct GpuProfiler(());

    impl GpuProfiler {
        pub fn new(_gl: &ffi::Gles2) -> Self {
            Self(())
        }

        pub fn enter(&mut self, _span_location: SpanLocation, _gl: &ffi::Gles2) -> GpuSpan {
            GpuSpan { active: true }
        }

        pub fn exit(&mut self, _gl: &ffi::Gles2, mut entered: GpuSpan) {
            entered.active = false;
        }

        pub fn collect(&mut self, _gl: &ffi::Gles2) {}

        pub fn sync_gpu(&self, _gl: &ffi::Gles2) {}
    }
}

pub(crate) use imp::*;

impl GpuProfiler {
    pub fn scope<'a, 'b>(
        &'a mut self,
        span_location: SpanLocation,
        gl: &'b ffi::Gles2,
    ) -> ScopedGpuSpan<'a, 'b> {
        let span = self.enter(span_location, gl);

        ScopedGpuSpan {
            span: Some(span),
            gl,
            profiler: self,
        }
    }
}
