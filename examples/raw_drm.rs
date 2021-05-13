#![warn(rust_2018_idioms)]

#[macro_use]
extern crate slog;

use slog::Drain;
use smithay::{
    backend::{
        allocator::{dumb::DumbBuffer, Format, Fourcc, Modifier, Slot, Swapchain},
        drm::{device_bind, DeviceHandler, DrmDevice, DrmError, DrmSurface},
    },
    reexports::{
        calloop::EventLoop,
        drm::control::{connector::State as ConnectorState, crtc, framebuffer, Device as ControlDevice},
    },
};
use std::{
    fs::{File, OpenOptions},
    io::Error as IoError,
    os::unix::io::{AsRawFd, RawFd},
    rc::Rc,
    sync::Mutex,
};

#[derive(Clone)]
struct FdWrapper {
    file: Rc<File>,
}

impl AsRawFd for FdWrapper {
    fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

fn main() {
    let log = slog::Logger::root(Mutex::new(slog_term::term_full().fuse()).fuse(), o!());

    /*
     * Initialize the drm backend
     */

    // "Find" a suitable drm device
    let mut options = OpenOptions::new();
    options.read(true);
    options.write(true);
    let fd = FdWrapper {
        file: Rc::new(options.open("/dev/dri/card0").unwrap()),
    };

    let mut device = DrmDevice::new(fd.clone(), true, log.clone()).unwrap();

    // Get a set of all modesetting resource handles (excluding planes):
    let res_handles = ControlDevice::resource_handles(&device).unwrap();

    // Use first connected connector
    let connector_info = res_handles
        .connectors()
        .iter()
        .map(|conn| device.get_connector(*conn).unwrap())
        .find(|conn| conn.state() == ConnectorState::Connected)
        .unwrap();

    // Use the first encoder
    let encoder = connector_info
        .encoders()
        .iter()
        .filter_map(|&e| e)
        .next()
        .unwrap();
    let encoder_info = device.get_encoder(encoder).unwrap();

    // use the connected crtc if any
    let crtc = encoder_info
        .crtc()
        // or use the first one that is compatible with the encoder
        .unwrap_or_else(|| res_handles.filter_crtcs(encoder_info.possible_crtcs())[0]);

    // Assuming we found a good connector and loaded the info into `connector_info`
    let mode = connector_info.modes()[0]; // Use first mode (usually highest resoltion, but in reality you should filter and sort and check and match with other connectors, if you use more then one.)

    // We just use one plane, the primary one
    let plane = device.planes(&crtc).unwrap().primary;

    // Initialize the hardware backend
    let surface = Rc::new(
        device
            .create_surface(crtc, plane, mode, &[connector_info.handle()])
            .unwrap(),
    );

    /*
     * Lets create buffers and framebuffers.
     * We use drm-rs DumbBuffers, because they always work and require little to no setup.
     * But they are very slow, this is just for demonstration purposes.
     */
    let (w, h) = mode.size();
    let allocator = DrmDevice::new(fd, false, log.clone()).unwrap();
    let mut swapchain = Swapchain::new(
        allocator,
        w.into(),
        h.into(),
        Format {
            code: Fourcc::Argb8888,
            modifier: Modifier::Invalid,
        },
    );
    let first_buffer: Slot<DumbBuffer<FdWrapper>, _> = swapchain.acquire().unwrap().unwrap();
    let framebuffer = surface.add_framebuffer(first_buffer.handle(), 32, 32).unwrap();
    first_buffer.set_userdata(framebuffer);

    // Get the device as an allocator into the
    device.set_handler(DrmHandlerImpl {
        swapchain,
        current: first_buffer,
        surface: surface.clone(),
    });

    /*
     * Register the DrmDevice on the EventLoop
     */
    let mut event_loop = EventLoop::<()>::new().unwrap();
    let _source = device_bind(&event_loop.handle(), device)
        .map_err(|err| -> IoError { err.into() })
        .unwrap();

    // Start rendering
    surface.commit(framebuffer, true).unwrap();

    // Run
    event_loop.run(None, &mut (), |_| {}).unwrap();
}

pub struct DrmHandlerImpl {
    swapchain:
        Swapchain<DrmDevice<FdWrapper>, DumbBuffer<FdWrapper>, framebuffer::Handle>,
    current: Slot<DumbBuffer<FdWrapper>, framebuffer::Handle>,
    surface: Rc<DrmSurface<FdWrapper>>,
}

impl DeviceHandler for DrmHandlerImpl {
    fn vblank(&mut self, _crtc: crtc::Handle) {
        {
            // Next buffer
            let next = self.swapchain.acquire().unwrap().unwrap();
            if next.userdata().is_none() {
                let fb = self.surface.add_framebuffer(next.handle(), 32, 32).unwrap();
                next.set_userdata(fb);
            }

            // now we could render to the mapping via software rendering.
            // this example just sets some grey color

            {
                let mut db = *next.handle();
                let mut mapping = self.surface.map_dumb_buffer(&mut db).unwrap();
                for x in mapping.as_mut() {
                    *x = 128;
                }
            }
            self.current = next;
        }

        let fb = self.current.userdata().unwrap();
        self.surface.page_flip(fb, true).unwrap();
    }

    fn error(&mut self, error: DrmError) {
        panic!("{:?}", error);
    }
}
