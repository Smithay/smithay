use drm::Device as BasicDevice;
use drm::buffer::{Id, Buffer};
use drm::control::ResourceInfo;
use drm::control::Device as ControlDevice;
use drm::control::{Mode, crtc, framebuffer, connector};
use drm::result::Error as DrmError;

use gbm::Device as GbmDevice;
use gbm::Format as GbmFormat;
use gbm::{AsRaw, BufferObjectFlags};

use nix;
use nix::c_void;

use wayland_server::{EventLoop, EventLoopHandle};
use wayland_server::sources::{FdEventSourceHandler, FdEventSource, FdInterest, READ};

use std::cell::Cell;
use std::io::Error as IoError;
use std::fs::File;
use std::os::unix::io::{AsRawFd, RawFd};

use ::backend::graphics::GraphicsBackend;
use ::backend::graphics::egl::{CreationError, EGLContext, NativeDisplay, NativeSurface, PixelFormat, PixelFormatRequirements, GlAttributes, SwapBuffersError, EGLGraphicsBackend};

#[derive(Debug)]
pub struct DrmDevice(File);

impl AsRawFd for DrmDevice {
    fn as_raw_fd(&self) -> RawFd { self.0.as_raw_fd() }
}
impl BasicDevice for DrmDevice {}
impl ControlDevice for DrmDevice {}

impl DrmDevice {
    pub unsafe fn new_from_fd(fd: RawFd) -> Self {
        use std::os::unix::io::FromRawFd;
        DrmDevice(File::from_raw_fd(fd))
    }

    pub fn new_from_file(file: File) -> Self {
        DrmDevice(file)
    }
}

pub trait DrmHandler {
    fn ready(&mut self, evlh: &mut EventLoopHandle);
    fn error(&mut self, evlh: &mut EventLoopHandle, error: IoError);
}

struct DrmFdHandler<H: DrmHandler + 'static>(H);

impl<H: DrmHandler + 'static> FdEventSourceHandler for DrmFdHandler<H> {
    fn ready(&mut self, evlh: &mut EventLoopHandle, fd: RawFd, mask: FdInterest) {
        use std::any::Any;
        use std::time::Duration;

        struct DrmDeviceRef(RawFd);
        impl AsRawFd for DrmDeviceRef {
            fn as_raw_fd(&self) -> RawFd { self.0 }
        }
        impl BasicDevice for DrmDeviceRef {}
        impl ControlDevice for DrmDeviceRef {}

        struct PageFlipHandler<'a, 'b, H: DrmHandler + 'a>(&'a mut H, &'b mut EventLoopHandle);

        impl<'a, 'b, H: DrmHandler + 'a> crtc::PageFlipHandler<DrmDeviceRef> for PageFlipHandler<'a, 'b, H> {
            fn handle_event(&mut self, _device: &DrmDeviceRef, _frame: u32, _duration: Duration, _userdata: Box<Any>) {
                self.0.ready(self.1);
            }
        }

        crtc::handle_event(&DrmDeviceRef(fd), 2, None::<&mut ()>, Some(&mut PageFlipHandler(&mut self.0, evlh)), None::<&mut ()>).unwrap();
    }

    fn error(&mut self, evlh: &mut EventLoopHandle, _fd: RawFd, error: IoError) {
        self.0.error(evlh, error)
    }
}

/// This is madness! NO, THIS IS rentaaaaaal!
mod rental {
    use drm::control::framebuffer;
    use gbm::{Device, SurfaceBufferHandle, BufferObject};
        use ::backend::graphics::egl::{EGLContext, EGLSurface};

    use std::cell::Cell;

    pub struct Prefix {
        pub gbm: Device,
        pub egl: EGLContext,
    }

    pub struct Suffix<'a> {
        pub gbm: GbmTypes<'a>,
        pub egl_surface: EGLSurface<'a>,
    }

    pub struct GbmTypes<'a> {
        pub cursor: BufferObject<'a, ()>,
        pub surface: GbmSurface<'a>,
    }

    pub struct GbmBuffers<'a> {
        pub front_buffer: Cell<SurfaceBufferHandle<'a, framebuffer::Info>>,
        pub next_buffer: Cell<Option<SurfaceBufferHandle<'a, framebuffer::Info>>>,
    }

    rental! {
        mod rental {
            use drm::control::framebuffer;
            use gbm::{Device, Surface};
            use std::boxed::Box;

            #[rental]
            pub struct NativeGraphics {
                prefix: Box<super::Prefix>,
                suffix: super::Suffix<'prefix>,
            }

            #[rental]
            pub struct GbmSurface<'a> {
                surface: Box<Surface<'a, framebuffer::Info>>,
                buffers: super::GbmBuffers<'surface>,
            }
        }
    }

    pub use self::rental::{NativeGraphics, GbmSurface};
}

pub struct DrmBackend {
    drm: DrmDevice,
    crtc: crtc::Handle,
    mode: Mode,
    connectors: Vec<connector::Handle>,
    graphics: rental::NativeGraphics,
    logger: ::slog::Logger,
}

pub enum ModeError {
    ModeNotSuitable
}

static DEFAULT_CURSOR: &[u8; 16384] = include_bytes!("../../resources/cursor.rgba");

impl DrmBackend {
    pub fn new<I, L>(
        drm: DrmDevice,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: I,
        gbm: GbmDevice,
        logger: L
    ) -> Result<DrmBackend, CreationError>
    where
        I: Into<Vec<connector::Handle>>,
        L: Into<Option<::slog::Logger>>
    {
        DrmBackend::new_with_gl_attr(
            drm,
            crtc,
            mode,
            connectors,
            gbm,
            GlAttributes {
                version: None,
                profile: None,
                debug: cfg!(debug_assertions),
                vsync: true
            },
            logger
        )
    }

    pub fn new_with_gl_attr<I, L>(
        drm: DrmDevice,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: I,
        gbm: GbmDevice,
        attributes: GlAttributes,
        logger: L
    ) -> Result<DrmBackend, CreationError>
        where
            I: Into<Vec<connector::Handle>>,
            L: Into<Option<::slog::Logger>>,
    {
        let log = ::slog_or_stdlog(logger).new(o!("smithay_module" => "backend_drm"));
        info!(log, "Initializing a drm backend");

        let connectors = connectors.into();

        for connector in connectors.iter() {
            if !connector::Info::load_from_device(&drm, *connector).unwrap().modes().contains(&mode) {
                panic!("Mode not available for all given connectors");
                //TODO: return Err(ModeError::ModeNotSuitable);
            }
        }

        /* GBM will load a dri driver, but even though they need symbols from
         * libglapi, in some version of Mesa they are not linked to it. Since
         * only the gl-renderer module links to it, the call above won't make
         * these symbols globally available, and loading the DRI driver fails.
         * Workaround this by dlopen()'ing libglapi with RTLD_GLOBAL.
        */
        unsafe { nix::libc::dlopen("libglapi.so.0".as_ptr() as *const _, nix::libc::RTLD_LAZY | nix::libc::RTLD_GLOBAL); }

        let context = match unsafe { EGLContext::new(
            NativeDisplay::Gbm(gbm.as_raw() as *const _),
            attributes,
            PixelFormatRequirements {
                hardware_accelerated: Some(true),
                color_bits: Some(24),
                alpha_bits: Some(8),
                ..Default::default()
            },
            log.clone()
        )} {
            Ok(context) => context,
            Err(err) => {
                error!(log, "EGLContext creation failed: \n {}", err);
                return Err(err);
            }
        };
        debug!(log, "GBM EGLContext initialized");

        let (w, h) = mode.size();

        let graphics = rental::NativeGraphics::new(Box::new(rental::Prefix {
            gbm,
            egl: context,
        }), |prefix| {
            let gbm_surface = prefix.gbm.create_surface(w as u32, h as u32, GbmFormat::XRGB8888, &[BufferObjectFlags::Scanout, BufferObjectFlags::Rendering]).unwrap();

            let egl_surface = unsafe { prefix.egl.create_surface(NativeSurface::Gbm(gbm_surface.as_raw() as *const _)).unwrap() };
            unsafe { egl_surface.make_current().unwrap() };
            egl_surface.swap_buffers().unwrap();

            let surface = rental::GbmSurface::new(Box::new(gbm_surface), |surface| {
                let mut front_bo = surface.lock_front_buffer().unwrap();
                debug!(log, "FrontBuffer color format: {:?}", front_bo.format());
                let fb = framebuffer::create(&drm, &*front_bo).unwrap();
                crtc::set(&drm, crtc, fb.handle(),& connectors, (0, 0), Some(mode)).unwrap();
                front_bo.set_userdata(fb);

                rental::GbmBuffers {
                    front_buffer: Cell::new(front_bo),
                    next_buffer: Cell::new(None),
                }
            });

            let mut cursor = prefix.gbm.create_buffer_object(64, 64, GbmFormat::ARGB8888, &[BufferObjectFlags::Cursor, BufferObjectFlags::Write]).unwrap();
            cursor.write(DEFAULT_CURSOR).unwrap();
            if crtc::set_cursor2(&drm, crtc, Buffer::handle(&cursor), (cursor.width(), cursor.height()), (18, 6)).is_err() {
                crtc::set_cursor(&drm, crtc, Buffer::handle(&cursor), (cursor.width(), cursor.height())).expect("Could not set cursor");
            }

            rental::Suffix {
                gbm: rental::GbmTypes {
                    cursor,
                    surface,
                },
                egl_surface,
            }
        });

        Ok(DrmBackend {
            drm,
            crtc,
            mode,
            connectors,
            graphics,
            logger: log.clone(),
        })
    }

    pub fn register<H: DrmHandler + 'static>(&self, event_loop: &mut EventLoop, handler: H) -> Result<FdEventSource, IoError> {
        let fd = self.drm.as_raw_fd();
        let id = event_loop.add_handler(DrmFdHandler(handler));
        let event_source = event_loop.add_fd_event_source::<DrmFdHandler<H>>(fd, id, READ)?;

        debug!(self.logger, "Registered drm device on event loop");

        self.swap_buffers();
        self.finish_rendering();

        Ok(event_source)
    }

    pub fn prepare_rendering(&self) {
        self.graphics.rent(|suffix| {
            suffix.gbm.surface.rent(|buffers| {
                let next_bo = buffers.next_buffer.replace(None);

                if let Some(next_buffer) = next_bo {
                    buffers.front_buffer.set(next_buffer);
                    // drop and release the old buffer
                } else {
                    unreachable!();
                }
            })
        });
    }

    pub fn finish_rendering(&self) {
        self.graphics.rent(|suffix| {
            suffix.gbm.surface.rent_all(|gbm| {
                let mut next_bo = gbm.surface.lock_front_buffer().unwrap();
                let maybe_fb = next_bo.userdata().cloned();
                let fb = if let Some(info) = maybe_fb {
                    info
                } else {
                    let fb = framebuffer::create(&self.drm, &*next_bo).unwrap();
                    next_bo.set_userdata(fb);
                    fb
                };
                gbm.buffers.next_buffer.set(Some(next_bo));

                trace!(self.logger, "Page flip queued");

                crtc::page_flip(&self.drm, self.crtc, fb.handle(), &[crtc::PageFlipFlags::PageFlipEvent], None::<()>)
            })
        }).unwrap();
    }

    pub fn add_connector(&mut self, connector: connector::Handle) -> Result<(), ModeError> {
        let info = connector::Info::load_from_device(&self.drm, connector).unwrap();
        if info.modes().contains(&self.mode) {
            self.connectors.push(connector);
            Ok(())
        } else {
            Err(ModeError::ModeNotSuitable)
        }
    }

    pub fn used_connectors(&self) -> &[connector::Handle] {
        &self.connectors
    }

    pub fn remove_connector(&mut self, connector: connector::Handle) {
        self.connectors.retain(|x| *x != connector);
    }

    pub fn use_mode(&mut self, mode: Mode) -> Result<(), ModeError> {
        for connector in self.connectors.iter() {
            if !connector::Info::load_from_device(&self.drm, *connector).unwrap().modes().contains(&mode) {
                return Err(ModeError::ModeNotSuitable);
            }
        }

        let drm_ref = &self.drm;
        let crtc = self.crtc;
        let connectors_ref = &self.connectors;

        self.graphics.rent_all_mut(move |graphics| {
            let (w, h) = mode.size();

            let surface = graphics.prefix.gbm.create_surface(w as u32, h as u32, GbmFormat::XRGB8888, &[BufferObjectFlags::Scanout, BufferObjectFlags::Rendering]).unwrap();

            graphics.suffix.egl_surface = unsafe { graphics.prefix.egl.create_surface(NativeSurface::Gbm(surface.as_raw() as *const _)).unwrap() };

            graphics.suffix.gbm.surface = rental::GbmSurface::new(Box::new(surface), |surface| {
                let mut front_bo = surface.lock_front_buffer().unwrap();
                let fb = framebuffer::create(drm_ref, &*front_bo).unwrap();
                crtc::set(drm_ref, crtc, fb.handle(), connectors_ref, (0, 0), Some(mode)).unwrap();
                front_bo.set_userdata(fb);

                rental::GbmBuffers {
                    front_buffer: Cell::new(front_bo),
                    next_buffer: Cell::new(None),
                }
            })
        });

        self.mode = mode;
        Ok(())
    }
}

impl GraphicsBackend for DrmBackend {
    type CursorFormat = (Id, (u32, u32));
    type Error = DrmError;

    fn set_cursor_position(&mut self, x: u32, y: u32) -> Result<(), DrmError> {
        crtc::move_cursor(&self.drm, self.crtc, (x as i32, y as i32))
    }

    fn set_cursor_representation(&mut self, handle: (Id, (u32, u32)), hotspot: (u32, u32)) -> Result<(), DrmError> {
        if crtc::set_cursor2(&self.drm, self.crtc, handle.0, handle.1, (hotspot.0 as i32, hotspot.1 as i32)).is_err() {
            crtc::set_cursor(&self.drm, self.crtc, handle.0, handle.1)
        } else {
            Ok(())
        }
    }
}

impl EGLGraphicsBackend for DrmBackend {
    fn swap_buffers(&self) -> Result<(), SwapBuffersError> {
        self.graphics.rent(|suffix| suffix.egl_surface.swap_buffers())
    }

    unsafe fn get_proc_address(&self, symbol: &str) -> *const c_void {
        self.graphics.head().egl.get_proc_address(symbol)
    }

    fn get_framebuffer_dimensions(&self) -> (u32, u32) {
        let (w, h) = self.mode.size();
        (w as u32, h as u32)
    }

    fn is_current(&self) -> bool {
        self.graphics.head().egl.is_current()
    }

    unsafe fn make_current(&self) -> Result<(), SwapBuffersError> {
        self.graphics.rent(|suffix| suffix.egl_surface.make_current())
    }

    fn get_pixel_format(&self) -> PixelFormat {
        self.graphics.head().egl.get_pixel_format()
    }
}
