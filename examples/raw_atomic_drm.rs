#![warn(rust_2018_idioms)]

#[macro_use]
extern crate slog;

use slog::Drain;
use smithay::{
    backend::drm::{
        atomic::{AtomicDrmDevice, AtomicDrmSurface},
        common::Error,
        device_bind, Device, DeviceHandler, RawSurface, Surface,
    },
    reexports::{
        calloop::EventLoop,
        drm::{
            buffer::format::PixelFormat,
            control::{
                connector::State as ConnectorState, crtc, dumbbuffer::DumbBuffer, framebuffer, property,
                Device as ControlDevice, ResourceHandle,
            },
        },
    },
};
use std::{
    fs::{File, OpenOptions},
    io::Error as IoError,
    rc::Rc,
    sync::Mutex,
};

fn get_property_by_name<'a, D: ControlDevice, T: ResourceHandle>(
    dev: &'a D,
    handle: T,
    name: &'static str,
) -> Option<(property::ValueType, property::RawValue)> {
    let props = dev.get_properties(handle).expect("Could not get props");
    let (ids, vals) = props.as_props_and_values();
    for (&id, &val) in ids.iter().zip(vals.iter()) {
        let info = dev.get_property(id).unwrap();
        if info.name().to_str().map(|x| x == name).unwrap_or(false) {
            let val_ty = info.value_type();
            return Some((val_ty, val));
        }
    }
    None
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
    let mut device = AtomicDrmDevice::new(options.open("/dev/dri/card0").unwrap(), true, log.clone()).unwrap();

    // Get a set of all modesetting resource handles (excluding planes):
    let res_handles = Device::resource_handles(&device).unwrap();

    // Use first connected connector
    let connector_info = res_handles
        .connectors()
        .iter()
        .map(|conn| device.get_connector_info(*conn).unwrap())
        .find(|conn| conn.state() == ConnectorState::Connected)
        .unwrap();

    // use the connected crtc if any
    let (val_ty, raw) = get_property_by_name(&device, connector_info.handle(), "CRTC_ID").unwrap();
    let crtc = match val_ty.convert_value(raw) {
        property::Value::CRTC(Some(handle)) => handle,
        property::Value::CRTC(None) => {
            // Use the first encoder
            let encoder = connector_info
                .encoders()
                .iter()
                .filter_map(|&e| e)
                .next()
                .unwrap();
            let encoder_info = device.get_encoder_info(encoder).unwrap();

            *res_handles
                .filter_crtcs(encoder_info.possible_crtcs())
                .iter()
                .next()
                .unwrap()
        }
        _ => unreachable!("CRTC_ID does not return another property type"),
    };

    // Assuming we found a good connector and loaded the info into `connector_info`
    let mode = connector_info.modes()[0]; // Use first mode (usually highest resoltion, but in reality you should filter and sort and check and match with other connectors, if you use more then one.)

    // Initialize the hardware backend
    let surface = Rc::new(device.create_surface(crtc, mode, &[connector_info.handle()]).unwrap());

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
    let front_buffer = device
        .create_dumb_buffer((w as u32, h as u32), PixelFormat::XRGB8888)
        .unwrap();
    let front_framebuffer = device.add_framebuffer(&front_buffer).unwrap();
    let back_buffer = device
        .create_dumb_buffer((w as u32, h as u32), PixelFormat::XRGB8888)
        .unwrap();
    let back_framebuffer = device.add_framebuffer(&back_buffer).unwrap();

    device.set_handler(DrmHandlerImpl {
        current: front_framebuffer,
        front: (front_buffer, front_framebuffer),
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
        surface.commit(front_framebuffer).unwrap();
    }

    // Run
    event_loop.run(None, &mut (), |_| {}).unwrap();
}

pub struct DrmHandlerImpl {
    front: (DumbBuffer, framebuffer::Handle),
    back: (DumbBuffer, framebuffer::Handle),
    current: framebuffer::Handle,
    surface: Rc<AtomicDrmSurface<File>>,
}

impl DeviceHandler for DrmHandlerImpl {
    type Device = AtomicDrmDevice<File>;

    fn vblank(&mut self, _crtc: crtc::Handle) {
        {
            // Swap and map buffer
            let mut mapping = if self.current == self.front.1 {
                self.current = self.back.1;
                self.surface.map_dumb_buffer(&mut self.back.0).unwrap()
            } else {
                self.current = self.front.1;
                self.surface.map_dumb_buffer(&mut self.front.0).unwrap()
            };

            // now we could render to the mapping via software rendering.
            // this example just sets some grey color

            for x in mapping.as_mut() {
                *x = 128;
            }
        }
        RawSurface::page_flip(&*self.surface, self.current).unwrap();
    }

    fn error(&mut self, error: Error) {
        panic!("{:?}", error);
    }
}
