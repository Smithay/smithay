extern crate smithay;
#[macro_use]
extern crate slog;
extern crate slog_term;

use slog::Drain;
use smithay::{
    backend::drm::{
        device_bind,
        legacy::{error::Error, LegacyDrmDevice, LegacyDrmSurface},
        Device, DeviceHandler, RawSurface, Surface,
    },
    reexports::{
        drm::{
            buffer::PixelFormat,
            control::{
                connector::{self, State as ConnectorState},
                crtc,
                dumbbuffer::DumbBuffer,
                encoder, framebuffer, Device as ControlDevice, ResourceInfo,
            },
        },
        wayland_server::calloop::EventLoop,
    },
};
use std::{
    fs::{File, OpenOptions},
    io::Error as IoError,
    rc::Rc,
    sync::Mutex,
};

fn main() {
    let log = slog::Logger::root(Mutex::new(slog_term::term_full().fuse()).fuse(), o!());

    /*
     * Initialize the drm backend
     */

    // "Find" a suitable drm device
    let mut options = OpenOptions::new();
    options.read(true);
    options.write(true);
    let mut device = LegacyDrmDevice::new(options.open("/dev/dri/card0").unwrap(), log.clone()).unwrap();

    // Get a set of all modesetting resource handles (excluding planes):
    let res_handles = Device::resource_handles(&device).unwrap();

    // Use first connected connector
    let connector_info = res_handles
        .connectors()
        .iter()
        .map(|conn| Device::resource_info::<connector::Info>(&device, *conn).unwrap())
        .find(|conn| conn.connection_state() == ConnectorState::Connected)
        .unwrap();

    // Use the first encoder
    let encoder_info = Device::resource_info::<encoder::Info>(&device, connector_info.encoders()[0]).unwrap();

    // use the connected crtc if any
    let crtc = encoder_info
        .current_crtc()
        // or use the first one that is compatible with the encoder
        .unwrap_or_else(|| {
            *res_handles
                .filter_crtcs(encoder_info.possible_crtcs())
                .iter()
                .next()
                .unwrap()
        });

    // Assuming we found a good connector and loaded the info into `connector_info`
    let mode = connector_info.modes()[0]; // Use first mode (usually highest resoltion, but in reality you should filter and sort and check and match with other connectors, if you use more then one.)

    // Initialize the hardware backend
    let surface = Rc::new(device.create_surface(crtc).unwrap());

    surface.use_mode(Some(mode)).unwrap();
    for conn in surface.current_connectors().into_iter() {
        if conn != connector_info.handle() {
            surface.remove_connector(conn).unwrap();
        }
    }
    surface.add_connector(connector_info.handle()).unwrap();

    /*
     * Lets create buffers and framebuffers.
     * We use drm-rs DumbBuffers, because they always work and require little to no setup.
     * But they are very slow, this is just for demonstration purposes.
     */
    let (w, h) = mode.size();
    let front_buffer =
        DumbBuffer::create_from_device(&device, (w as u32, h as u32), PixelFormat::XRGB8888).unwrap();
    let front_framebuffer = device.create_framebuffer(&front_buffer).unwrap();
    let back_buffer =
        DumbBuffer::create_from_device(&device, (w as u32, h as u32), PixelFormat::XRGB8888).unwrap();
    let back_framebuffer = device.create_framebuffer(&back_buffer).unwrap();

    device.set_handler(DrmHandlerImpl {
        current: front_framebuffer.handle(),
        front: (front_buffer, front_framebuffer.clone()),
        back: (back_buffer, back_framebuffer),
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
    if surface.commit_pending() {
        surface.commit(front_framebuffer.handle()).unwrap();
    }
    surface.page_flip(front_framebuffer.handle()).unwrap();

    // Run
    event_loop.run(None, &mut (), |_| {}).unwrap();
}

pub struct DrmHandlerImpl {
    front: (DumbBuffer, framebuffer::Info),
    back: (DumbBuffer, framebuffer::Info),
    current: framebuffer::Handle,
    surface: Rc<LegacyDrmSurface<File>>,
}

impl DeviceHandler for DrmHandlerImpl {
    type Device = LegacyDrmDevice<File>;

    fn vblank(&mut self, _crtc: crtc::Handle) {
        {
            // Swap and map buffer
            let mut mapping = if self.current == self.front.1.handle() {
                self.current = self.back.1.handle();
                self.back.0.map(&*self.surface).unwrap()
            } else {
                self.current = self.front.1.handle();
                self.front.0.map(&*self.surface).unwrap()
            };

            // now we could render to the mapping via software rendering.
            // this example just sets some grey color

            for mut x in mapping.as_mut() {
                *x = 128;
            }
        }
        self.surface.page_flip(self.current).unwrap();
    }

    fn error(&mut self, error: Error) {
        panic!("{:?}", error);
    }
}
