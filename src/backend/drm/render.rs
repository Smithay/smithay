pub struct DrmRenderSurface<
    D: AsRawFd + 'static,
    A: Allocator<S>,
    S: Buffer,
    D: Buffer + TryFrom<S>,
    E: Error,
    T, F,
    R: Renderer<Error=Error, Texture=T, Frame=F, Buffer=D>,
> {
    drm: DrmSurface<A>,
    allocator: A,
    renderer: R,
    swapchain: Swapchain,
}

impl<D, A, S, D, E, T, R> DrmRenderSurface<D, A, S, D, E, T, R>
where
    D: AsRawFd + 'static,
    A: Allocator<S>,
    S: Buffer,
    D: Buffer + TryFrom<S>,
    E: Error,
    T, F,
    R: Renderer<Error=Error, Texture=T, Frame=F, Buffer=D>,
{
    
}