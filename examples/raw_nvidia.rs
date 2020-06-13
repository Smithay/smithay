#![warn(rust_2018_idioms)]

#[macro_use]
extern crate slog;

use glium::Surface as GliumSurface;
use slog::Drain;
use smithay::{
    backend::{
        drm::{
            atomic::{AtomicDrmDevice, AtomicDrmSurface},
            common::fallback::{EitherError, FallbackDevice, FallbackSurface},
            common::Error as DrmError,
            device_bind,
            egl::{EglDevice, EglSurface, Error as EglError},
            eglstream::{
                egl::EglStreamDeviceBackend, EglStreamDevice, EglStreamSurface, Error as EglStreamError,
            },
            legacy::{LegacyDrmDevice, LegacyDrmSurface},
            Device, DeviceHandler,
        },
        graphics::glium::GliumGraphicsBackend,
    },
    reexports::{
        calloop::EventLoop,
        drm::control::{connector::State as ConnectorState, crtc},
    },
};
use std::{
    fs::{File, OpenOptions},
    io::Error as IoError,
    os::unix::io::{AsRawFd, RawFd},
    rc::Rc,
    sync::Mutex,
};

pub struct ClonableFile {
    file: File,
}

impl AsRawFd for ClonableFile {
    fn as_raw_fd(&self) -> RawFd {
        self.file.as_raw_fd()
    }
}

impl Clone for ClonableFile {
    fn clone(&self) -> Self {
        ClonableFile {
            file: self.file.try_clone().expect("Unable to clone file"),
        }
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
    let file = ClonableFile {
        file: options.open("/dev/dri/card1").expect("Failed to open card1"),
    };
    let mut device = EglDevice::new(
        EglStreamDevice::new(
            FallbackDevice::<AtomicDrmDevice<_>, LegacyDrmDevice<_>>::new(file, true, log.clone())
                .expect("Failed to initialize drm device"),
            log.clone(),
        )
        .expect("Failed to initialize egl stream device"),
        log.clone(),
    )
    .expect("Failed to initialize egl device");

    // Get a set of all modesetting resource handles (excluding planes):
    let res_handles = Device::resource_handles(&device).unwrap();

    // Use first connected connector
    let connector_info = res_handles
        .connectors()
        .iter()
        .map(|conn| Device::get_connector_info(&device, *conn).unwrap())
        .find(|conn| conn.state() == ConnectorState::Connected)
        .unwrap();
    println!("Conn: {:?}", connector_info.interface());

    // Use the first encoder
    let encoder_info =
        Device::get_encoder_info(&device, connector_info.encoders()[0].expect("expected encoder")).unwrap();

    // use the connected crtc if any
    let crtc = encoder_info
        .crtc()
        // or use the first one that is compatible with the encoder
        .unwrap_or_else(|| {
            *res_handles
                .filter_crtcs(encoder_info.possible_crtcs())
                .iter()
                .next()
                .unwrap()
        });
    println!("Crtc {:?}", crtc);

    // Assuming we found a good connector and loaded the info into `connector_info`
    let mode = connector_info.modes()[0]; // Use first mode (usually highest resolution, but in reality you should filter and sort and check and match with other connectors, if you use more then one.)
    println!("Mode: {:?}", mode);

    // Initialize the hardware backend
    let surface = device
        .create_surface(crtc, mode, &[connector_info.handle()])
        .expect("Failed to create surface");

    let backend: Rc<GliumGraphicsBackend<_>> = Rc::new(surface.into());
    device.set_handler(DrmHandlerImpl {
        surface: backend.clone(),
    });

    /*
     * Register the DrmDevice on the EventLoop
     */
    let mut event_loop = EventLoop::<()>::new().unwrap();
    let _source = device_bind(&event_loop.handle(), device)
        .map_err(|err| -> IoError { err.into() })
        .unwrap();

    // Start rendering
    {
        if let Err(smithay::backend::graphics::SwapBuffersError::ContextLost(err)) = backend.draw().finish() {
            println!("{}", err);
            return;
        };
    }

    // Run
    event_loop.run(None, &mut (), |_| {}).unwrap();
}

pub struct DrmHandlerImpl {
    surface: Rc<
        GliumGraphicsBackend<
            EglSurface<
                EglStreamSurface<
                    FallbackSurface<AtomicDrmSurface<ClonableFile>, LegacyDrmSurface<ClonableFile>>,
                >,
            >,
        >,
    >,
}

impl DeviceHandler for DrmHandlerImpl {
    type Device = EglDevice<
        EglStreamDeviceBackend<FallbackDevice<AtomicDrmDevice<ClonableFile>, LegacyDrmDevice<ClonableFile>>>,
        EglStreamDevice<FallbackDevice<AtomicDrmDevice<ClonableFile>, LegacyDrmDevice<ClonableFile>>>,
    >;

    fn vblank(&mut self, _crtc: crtc::Handle) {
        {
            println!("Vblank");
            let mut frame = self.surface.draw();
            frame.clear(None, Some((0.8, 0.8, 0.9, 1.0)), false, Some(1.0), None);
            if let Err(smithay::backend::graphics::SwapBuffersError::ContextLost(err)) = frame.finish() {
                panic!("Failed to swap: {}", err);
            }
        }
    }

    fn error(&mut self, error: EglError<EglStreamError<EitherError<DrmError, DrmError>>>) {
        panic!("{:?}", error);
    }
}
