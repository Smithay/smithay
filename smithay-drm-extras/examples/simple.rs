use std::{collections::HashMap, path::PathBuf, time::Duration};

use ::drm::control::{connector, crtc};
use smithay_drm_extras::{
    display_info,
    drm_scanner::{self, DrmScanEvent},
};

use smithay::{
    backend::{
        drm::{self, DrmDeviceFd, DrmNode},
        session::{libseat::LibSeatSession, Session},
        udev::{UdevBackend, UdevEvent},
    },
    reexports::{
        calloop::{timer::Timer, EventLoop, LoopHandle},
        rustix::fs::OFlags,
    },
    utils::DeviceFd,
};

struct State {
    handle: LoopHandle<'static, Self>,
    session: LibSeatSession,
    devices: HashMap<DrmNode, Device>,
}

struct Device {
    drm: drm::DrmDevice,
    drm_scanner: drm_scanner::DrmScanner,
    surfaces: HashMap<crtc::Handle, Surface>,
}

#[derive(Clone)]
struct Surface {
    // Your gbm surface stuff goes here
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut event_loop = EventLoop::<State>::try_new()?;

    let (session, notify) = LibSeatSession::new().unwrap();

    event_loop.handle().insert_source(notify, |_, _, _| {}).unwrap();

    let mut state = State {
        handle: event_loop.handle(),
        session,
        devices: Default::default(),
    };

    init_udev(&mut state);

    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_secs(5)), |_, _, _| {
            panic!("Aborted");
        })
        .unwrap();

    event_loop.run(None, &mut state, |_data| {})?;

    Ok(())
}

fn init_udev(state: &mut State) {
    let backend = UdevBackend::new(state.session.seat()).unwrap();
    for (device_id, path) in backend.device_list() {
        state.on_udev_event(UdevEvent::Added {
            device_id,
            path: path.to_owned(),
        });
    }

    state
        .handle
        .insert_source(backend, |event, _, state| state.on_udev_event(event))
        .unwrap();
}

impl State {
    fn on_udev_event(&mut self, event: UdevEvent) {
        match event {
            UdevEvent::Added { device_id, path } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    self.device_added(node, path);
                }
            }
            UdevEvent::Changed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    self.device_changed(node);
                }
            }
            UdevEvent::Removed { device_id } => {
                if let Ok(node) = DrmNode::from_dev_id(device_id) {
                    self.device_removed(node);
                }
            }
        }
    }

    fn device_added(&mut self, node: DrmNode, path: PathBuf) {
        let fd = self
            .session
            .open(
                &path,
                OFlags::RDWR | OFlags::CLOEXEC | OFlags::NOCTTY | OFlags::NONBLOCK,
            )
            .unwrap();

        let fd = DrmDeviceFd::new(DeviceFd::from(fd));

        let (drm, drm_notifier) = drm::DrmDevice::new(fd, false).unwrap();

        self.handle
            .insert_source(drm_notifier, move |event, _, _| match event {
                drm::DrmEvent::VBlank(_) => {}
                drm::DrmEvent::Error(_) => {}
            })
            .unwrap();

        self.devices.insert(
            node,
            Device {
                drm,
                drm_scanner: Default::default(),
                surfaces: Default::default(),
            },
        );

        self.device_changed(node);
    }

    fn connector_connected(&mut self, node: DrmNode, connector: connector::Info, crtc: crtc::Handle) {
        if let Some(device) = self.devices.get_mut(&node) {
            let name = format!("{}-{}", connector.interface().as_str(), connector.interface_id());

            let display_info = display_info::for_connector(&device.drm, connector.handle());

            let manufacturer = display_info
                .as_ref()
                .and_then(|info| info.make())
                .unwrap_or_else(|| "Unknown".into());

            let model = display_info
                .as_ref()
                .and_then(|info| info.model())
                .unwrap_or_else(|| "Unknown".into());

            println!("Connected:");
            dbg!(name);
            dbg!(manufacturer);
            dbg!(model);

            device.surfaces.insert(crtc, Surface {});
        }
    }

    fn connector_disconnected(&mut self, node: DrmNode, connector: connector::Info, crtc: crtc::Handle) {
        let name = format!("{}-{}", connector.interface().as_str(), connector.interface_id());

        println!("Disconnected:");
        dbg!(name);

        if let Some(device) = self.devices.get_mut(&node) {
            device.surfaces.remove(&crtc);
        }
    }

    fn device_changed(&mut self, node: DrmNode) {
        let device = if let Some(device) = self.devices.get_mut(&node) {
            device
        } else {
            return;
        };

        for event in device
            .drm_scanner
            .scan_connectors(&device.drm)
            .expect("failed to scan connectors")
        {
            match event {
                DrmScanEvent::Connected {
                    connector,
                    crtc: Some(crtc),
                } => {
                    self.connector_connected(node, connector, crtc);
                }
                DrmScanEvent::Disconnected {
                    connector,
                    crtc: Some(crtc),
                } => {
                    self.connector_disconnected(node, connector, crtc);
                }
                _ => {}
            }
        }
    }

    fn device_removed(&mut self, node: DrmNode) {
        let device = if let Some(device) = self.devices.get_mut(&node) {
            device
        } else {
            return;
        };

        let crtcs: Vec<_> = device
            .drm_scanner
            .crtcs()
            .map(|(info, crtc)| (info.clone(), crtc))
            .collect();

        for (connector, crtc) in crtcs {
            self.connector_disconnected(node, connector, crtc);
        }

        self.devices.remove(&node);
    }
}
