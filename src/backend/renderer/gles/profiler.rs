use super::ffi;

const MAX_QUERIES: usize = 64 * 1024;

pub struct GpuProfiler {
    context: tracy_client::GpuContext,
    pool: Vec<GpuTracepoint>,
    index: usize,
    tracepoints: Vec<ExitedGpuTracepoint>,
}

impl GpuProfiler {
    pub fn new(gl: &ffi::Gles2) -> Self {
        let mut queries = [0; MAX_QUERIES];
        unsafe {
            gl.GenQueriesEXT(queries.len() as ffi::types::GLsizei, queries.as_mut_ptr());
        }
        let pool = queries
            .chunks_exact(2)
            .map(|pairs| {
                let (entered, exited) = unsafe { (*pairs.get_unchecked(0), *pairs.get_unchecked(1)) };
                GpuTracepoint { entered, exited }
            })
            .collect::<Vec<_>>();

        let client = tracy_client::Client::start();

        let mut bits = 0;
        unsafe { gl.GetQueryivEXT(ffi::TIMESTAMP_EXT, ffi::QUERY_COUNTER_BITS_EXT, &mut bits) };

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

        Self {
            context,
            pool,
            index: 0,
            tracepoints: Vec::with_capacity(MAX_QUERIES / 2),
        }
    }

    pub fn scope<'a, 'b>(
        &'a mut self,
        span_location: &'static tracy_client::SpanLocation,
        gl: &'b ffi::Gles2,
    ) -> ScopedGpuTracepoint<'a, 'b> {
        let tracepoint = unsafe { *self.pool.get_unchecked(self.index) };
        self.index = (self.index + 1) % self.pool.len();

        unsafe {
            gl.QueryCounterEXT(tracepoint.entered, ffi::TIMESTAMP_EXT);
        }

        let span = self.context.span(span_location).unwrap();

        ScopedGpuTracepoint {
            span: Some(EnteredGpuTracepoint {
                span: Some(GpuSpan { tracepoint, span }),
            }),
            gl,
            profiler: self,
        }
    }

    pub fn enter(
        &mut self,
        span_location: &'static tracy_client::SpanLocation,
        gl: &ffi::Gles2,
    ) -> EnteredGpuTracepoint {
        let tracepoint = unsafe { *self.pool.get_unchecked(self.index) };
        self.index = (self.index + 1) % self.pool.len();

        unsafe {
            gl.QueryCounterEXT(tracepoint.entered, ffi::TIMESTAMP_EXT);
        }

        let span = self.context.span(span_location).unwrap();

        EnteredGpuTracepoint {
            span: Some(GpuSpan { tracepoint, span }),
        }
    }

    pub fn exit(&mut self, gl: &ffi::Gles2, mut exited: EnteredGpuTracepoint) {
        let Some(mut span) = exited.span.take() else {
            return;
        };

        unsafe {
            gl.QueryCounterEXT(span.tracepoint.exited, ffi::TIMESTAMP_EXT);
        }
        span.span.end_zone();

        self.tracepoints.push(ExitedGpuTracepoint { span });
    }

    pub fn collect(&mut self, gl: &ffi::Gles2) {
        let mut i = 0;
        while i != self.tracepoints.len() {
            let mut available: ffi::types::GLuint = 0;
            unsafe {
                gl.GetQueryObjectuivEXT(
                    self.tracepoints[i].span.tracepoint.exited,
                    ffi::QUERY_RESULT_AVAILABLE,
                    &mut available,
                );
                if gl.GetError() != ffi::NO_ERROR {
                    self.tracepoints.remove(i);
                    continue;
                }
            }

            if available == 1 {
                let tracepoint = self.tracepoints.remove(i);
                let mut start_timestamp = 0;
                let mut end_timestamp = 0;
                unsafe {
                    gl.GetQueryObjecti64vEXT(
                        tracepoint.span.tracepoint.entered,
                        ffi::QUERY_RESULT,
                        &mut start_timestamp,
                    );
                    gl.GetQueryObjecti64vEXT(
                        tracepoint.span.tracepoint.exited,
                        ffi::QUERY_RESULT,
                        &mut end_timestamp,
                    );
                }
                tracepoint
                    .span
                    .span
                    .upload_timestamp(start_timestamp, end_timestamp);
            } else {
                i += 1;
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct GpuTracepoint {
    entered: ffi::types::GLuint,
    exited: ffi::types::GLuint,
}

struct GpuSpan {
    tracepoint: GpuTracepoint,
    span: tracy_client::GpuSpan,
}

pub struct EnteredGpuTracepoint {
    span: Option<GpuSpan>,
}

pub struct ScopedGpuTracepoint<'a, 'b> {
    span: Option<EnteredGpuTracepoint>,
    profiler: &'a mut GpuProfiler,
    gl: &'b ffi::Gles2,
}

impl<'a, 'b> Drop for ScopedGpuTracepoint<'a, 'b> {
    fn drop(&mut self) {
        if let Some(span) = self.span.take() {
            self.profiler.exit(self.gl, span);
        }
    }
}

struct ExitedGpuTracepoint {
    span: GpuSpan,
}

impl Drop for EnteredGpuTracepoint {
    fn drop(&mut self) {
        debug_assert!(self.span.is_none());
    }
}
