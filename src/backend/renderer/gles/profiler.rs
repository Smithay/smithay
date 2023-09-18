use super::ffi;

const MAX_QUERIES: usize = 1024;

pub struct GpuProfiler {
    context: tracy_client::GpuContext,
    pool: QueryPool,
}

struct QueryPool {
    context: tracy_client::GpuContext,
    pool: [GpuQuery; MAX_QUERIES],
    // Index of first free query in the pool.
    head: usize,
    // Index of first pending query in the pool.
    tail: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(transparent)]
struct GpuQuery(ffi::types::GLuint);

pub struct EnteredGpuTracepoint {
    active: bool,
}

impl Drop for EnteredGpuTracepoint {
    fn drop(&mut self) {
        assert!(!self.active, "GPU span must be properly exited");
    }
}

pub struct ScopedGpuTracepoint<'a, 'b> {
    span: Option<EnteredGpuTracepoint>,
    profiler: &'a mut GpuProfiler,
    gl: &'b ffi::Gles2,
}

impl<'a, 'b> Drop for ScopedGpuTracepoint<'a, 'b> {
    fn drop(&mut self) {
        let span = self.span.take().unwrap();
        self.profiler.exit(self.gl, span);
    }
}

impl QueryPool {
    fn new(context: tracy_client::GpuContext, gl: &ffi::Gles2) -> Self {
        let mut pool = [GpuQuery(0); MAX_QUERIES];
        unsafe {
            gl.GenQueriesEXT(pool.len() as ffi::types::GLsizei, pool.as_mut_ptr().cast());
        }

        Self {
            context,
            pool,
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
                gl.GetError();
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

    pub fn enter(
        &mut self,
        span_location: &'static tracy_client::SpanLocation,
        gl: &ffi::Gles2,
    ) -> EnteredGpuTracepoint {
        if !tracy_client::Client::is_connected() {
            return EnteredGpuTracepoint { active: false };
        }

        let query = self.pool.next(gl);
        self.context.begin_span(span_location, query.0 as u16);

        EnteredGpuTracepoint { active: true }
    }

    pub fn exit(&mut self, gl: &ffi::Gles2, mut entered: EnteredGpuTracepoint) {
        if !entered.active {
            return;
        }
        entered.active = false;

        let query = self.pool.next(gl);
        self.context.end_span(query.0 as u16);

        // Flush right away so GL knows about our exit query.
        unsafe { gl.Flush() };
    }

    pub fn scope<'a, 'b>(
        &'a mut self,
        span_location: &'static tracy_client::SpanLocation,
        gl: &'b ffi::Gles2,
    ) -> ScopedGpuTracepoint<'a, 'b> {
        let span = self.enter(span_location, gl);

        ScopedGpuTracepoint {
            span: Some(span),
            gl,
            profiler: self,
        }
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
