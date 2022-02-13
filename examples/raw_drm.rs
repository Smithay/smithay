#![warn(rust_2018_idioms)]

#[macro_use]
extern crate slog;

use slog::Drain;
use smithay::{
    backend::{
        allocator::{dumb::DumbBuffer, Fourcc, Slot, Swapchain},
        drm::{DrmDevice, DrmEvent, DrmSurface},
    },
    reexports::{
        calloop::EventLoop,
        drm::control::{connector::State as ConnectorState, crtc, framebuffer, Device as ControlDevice},
    },
};
use std::{
    fs::{File, OpenOptions},
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

    let device = DrmDevice::new(fd.clone(), true, log.clone()).unwrap();

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

    // Initialize the hardware backend
    let surface = Rc::new(
        device
            .create_surface(crtc, mode, &[connector_info.handle()])
            .unwrap(),
    );

    /*
     * Lets create buffers and framebuffers.
     * We use drm-rs DumbBuffers, because they always work and require little to no setup.
     * But they are very slow, this is just for demonstration purposes.
     */
    let (w, h) = mode.size();
    let allocator = DrmDevice::new(fd, false, log.clone()).unwrap();
    let mods = surface
        .supported_formats(surface.plane())
        .expect("Unable to readout formats for surface")
        .iter()
        .filter_map(|format| {
            if format.code == Fourcc::Argb8888 {
                Some(format.modifier)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let mut swapchain = Swapchain::new(allocator, w.into(), h.into(), Fourcc::Argb8888, mods);
    let first_buffer: Slot<DumbBuffer<FdWrapper>> = swapchain.acquire().unwrap().unwrap();
    let framebuffer = surface.add_framebuffer(first_buffer.handle(), 32, 32).unwrap();
    first_buffer.userdata().insert_if_missing(|| framebuffer);

    // Get the device as an allocator into the
    let mut vblank_handler = VBlankHandler {
        swapchain,
        current: first_buffer,
        surface: surface.clone(),
    };

    /*
     * Register the DrmDevice on the EventLoop
     */
    let mut event_loop = EventLoop::<()>::try_new().unwrap();
    event_loop
        .handle()
        .insert_source(device, move |event, _: &mut _, _: &mut ()| match event {
            DrmEvent::VBlank(crtc) => vblank_handler.vblank(crtc),
            DrmEvent::Error(e) => panic!("{}", e),
        })
        .unwrap();
    // Start rendering
    surface
        .commit([(framebuffer, surface.plane())].iter(), true)
        .unwrap();

    // Run
    event_loop.run(None, &mut (), |_| {}).unwrap();
}

pub struct VBlankHandler {
    swapchain: Swapchain<DrmDevice<FdWrapper>, DumbBuffer<FdWrapper>>,
    current: Slot<DumbBuffer<FdWrapper>>,
    surface: Rc<DrmSurface<FdWrapper>>,
}

impl VBlankHandler {
    fn vblank(&mut self, _crtc: crtc::Handle) {
        {
            // Next buffer
            let next = self.swapchain.acquire().unwrap().unwrap();
            if next.userdata().get::<framebuffer::Handle>().is_none() {
                let fb = self.surface.add_framebuffer(next.handle(), 32, 32).unwrap();
                next.userdata().insert_if_missing(|| fb);
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

        let fb = *self.current.userdata().get::<framebuffer::Handle>().unwrap();
        self.surface
            .page_flip([(fb, self.surface.plane())].iter(), true)
            .unwrap();
    }
}
