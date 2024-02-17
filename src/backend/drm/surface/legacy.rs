use drm::control::{connector, crtc, encoder, framebuffer, Device as ControlDevice, Mode, PageFlipFlags};

use std::collections::HashSet;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, RwLock,
};

use crate::backend::drm::error::AccessError;
use crate::{
    backend::drm::{
        device::legacy::set_connector_state, device::DrmDeviceInternal, error::Error, DrmDeviceFd,
    },
    utils::DevPath,
};

use tracing::{debug, info, info_span, instrument, trace};

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct State {
    pub active: bool,
    pub mode: Mode,
    pub connectors: HashSet<connector::Handle>,
}

impl State {
    fn current_state<A: DevPath + ControlDevice>(fd: &A, crtc: crtc::Handle) -> Result<Self, Error> {
        // Try to enumarate the current state to set the initial state variable correctly.
        // We need an accurate state to handle `commit_pending`.
        let crtc_info = fd.get_crtc(crtc).map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Error loading crtc info",
                dev: fd.dev_path(),
                source,
            })
        })?;

        let current_mode = crtc_info.mode();

        let mut current_connectors = HashSet::new();
        let res_handles = fd.resource_handles().map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Error loading drm resources",
                dev: fd.dev_path(),
                source,
            })
        })?;
        for &con in res_handles.connectors() {
            let con_info = fd.get_connector(con, false).map_err(|source| {
                Error::Access(AccessError {
                    errmsg: "Error loading connector info",
                    dev: fd.dev_path(),
                    source,
                })
            })?;
            if let Some(enc) = con_info.current_encoder() {
                let enc_info = fd.get_encoder(enc).map_err(|source| {
                    Error::Access(AccessError {
                        errmsg: "Error loading encoder info",
                        dev: fd.dev_path(),
                        source,
                    })
                })?;
                if let Some(current_crtc) = enc_info.crtc() {
                    if crtc == current_crtc {
                        current_connectors.insert(con);
                    }
                }
            }
        }

        // If we have no current mode, we create a fake one, which will not match (and thus gets overriden on the commit below).
        // A better fix would probably be making mode an `Option`, but that would mean
        // we need to be sure, we require a mode to always be set without relying on the compiler.
        // So we cheat, because it works and is easier to handle later.
        Ok(State {
            // On legacy there is not (reliable) way to read-back the dpms state.
            // So we just always assume it is off.
            active: false,
            mode: current_mode.unwrap_or_else(|| unsafe { std::mem::zeroed() }),
            connectors: current_connectors,
        })
    }
}

#[derive(Debug)]
pub struct LegacyDrmSurface {
    pub(super) fd: Arc<DrmDeviceInternal>,
    pub(super) active: Arc<AtomicBool>,
    crtc: crtc::Handle,
    state: RwLock<State>,
    pending: RwLock<State>,
    pub(super) span: tracing::Span,
    supports_async_page_flips: bool,
}

impl LegacyDrmSurface {
    pub fn new(
        fd: Arc<DrmDeviceInternal>,
        active: Arc<AtomicBool>,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: &[connector::Handle],
        supports_tearing_page_flips: bool,
    ) -> Result<Self, Error> {
        let span = info_span!("drm_legacy", crtc = ?crtc);
        let _guard = span.enter();
        info!(?mode, ?connectors, ?crtc, "Initializing drm surface",);

        let state = State::current_state(&*fd, crtc)?;
        let pending = State {
            active: true,
            mode,
            connectors: connectors.iter().copied().collect(),
        };

        drop(_guard);
        let surface = LegacyDrmSurface {
            fd,
            active,
            crtc,
            state: RwLock::new(state),
            pending: RwLock::new(pending),
            span,
            supports_async_page_flips: supports_tearing_page_flips,
        };

        Ok(surface)
    }

    pub fn current_connectors(&self) -> HashSet<connector::Handle> {
        self.state.read().unwrap().connectors.clone()
    }

    pub fn pending_connectors(&self) -> HashSet<connector::Handle> {
        self.pending.read().unwrap().connectors.clone()
    }

    pub fn current_mode(&self) -> Mode {
        self.state.read().unwrap().mode
    }

    pub fn pending_mode(&self) -> Mode {
        self.pending.read().unwrap().mode
    }

    #[instrument(parent = &self.span, skip(self))]
    pub fn add_connector(&self, conn: connector::Handle) -> Result<(), Error> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let mut pending = self.pending.write().unwrap();

        if self.check_connector(conn, &pending.mode)? {
            pending.connectors.insert(conn);
        }

        Ok(())
    }

    #[instrument(parent = &self.span, skip(self))]
    pub fn remove_connector(&self, connector: connector::Handle) -> Result<(), Error> {
        let mut pending = self.pending.write().unwrap();

        if pending.connectors.contains(&connector) && pending.connectors.len() == 1 {
            return Err(Error::SurfaceWithoutConnectors(self.crtc));
        }

        pending.connectors.remove(&connector);
        Ok(())
    }

    #[instrument(parent = &self.span, skip(self))]
    pub fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Error> {
        if connectors.is_empty() {
            return Err(Error::SurfaceWithoutConnectors(self.crtc));
        }

        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let mut pending = self.pending.write().unwrap();

        if connectors
            .iter()
            .map(|conn| self.check_connector(*conn, &pending.mode))
            .collect::<Result<Vec<bool>, _>>()?
            .iter()
            .all(|v| *v)
        {
            pending.connectors = connectors.iter().cloned().collect();
        }

        Ok(())
    }

    #[instrument(level = "debug", parent = &self.span, skip(self))]
    pub fn use_mode(&self, mode: Mode) -> Result<(), Error> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let mut pending = self.pending.write().unwrap();

        // check the connectors to see if this mode is supported
        for connector in &pending.connectors {
            if !self
                .fd
                .get_connector(*connector, false)
                .map_err(|source| {
                    Error::Access(AccessError {
                        errmsg: "Error loading connector info",
                        dev: self.fd.dev_path(),
                        source,
                    })
                })?
                .modes()
                .contains(&mode)
            {
                return Err(Error::ModeNotSuitable(mode));
            }
        }

        pending.mode = mode;

        Ok(())
    }

    pub fn commit_pending(&self) -> bool {
        *self.pending.read().unwrap() != *self.state.read().unwrap()
    }

    fn flip_flags(&self, mut flip_flags: PageFlipFlags) -> PageFlipFlags {
        if !self.supports_async_page_flips {
            flip_flags.remove(PageFlipFlags::ASYNC);
        }
        flip_flags
    }

    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    pub fn commit(&self, framebuffer: framebuffer::Handle, flip_flags: PageFlipFlags) -> Result<(), Error> {
        let flip_flags = self.flip_flags(flip_flags);

        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let mut current = self.state.write().unwrap();
        let pending = self.pending.read().unwrap();

        {
            let removed = current.connectors.difference(&pending.connectors);
            let added = pending.connectors.difference(&current.connectors);

            let mut conn_removed = false;
            for conn in removed.clone() {
                if let Ok(info) = self.fd.get_connector(*conn, false) {
                    info!("Removing connector: {:?}", info.interface());
                } else {
                    info!("Removing unknown connector");
                }
                // if the connector was mapped to our crtc, we need to ack the disconnect.
                // the graphics pipeline will not be freed otherwise
                conn_removed = true;
            }
            set_connector_state(&*self.fd, removed.copied(), false)?;

            if conn_removed {
                // null commit (necessary to trigger removal on the kernel side with the legacy api.)
                self.fd
                    .set_crtc(self.crtc, None, (0, 0), &[], None)
                    .map_err(|source| {
                        Error::Access(AccessError {
                            errmsg: "Error setting crtc",
                            dev: self.fd.dev_path(),
                            source,
                        })
                    })?;
            }

            for conn in added.clone() {
                if let Ok(info) = self.fd.get_connector(*conn, false) {
                    info!("Adding connector: {:?}", info.interface());
                } else {
                    info!("Adding unknown connector");
                }
            }
            set_connector_state(&*self.fd, added.copied(), true)?;

            if current.mode != pending.mode {
                info!("Setting new mode: {:?}", pending.mode.name());
            }
        }

        debug!("Setting screen");
        // do a modeset and attach the given framebuffer
        self.fd
            .set_crtc(
                self.crtc,
                Some(framebuffer),
                (0, 0),
                &pending
                    .connectors
                    .iter()
                    .copied()
                    .collect::<Vec<connector::Handle>>(),
                Some(pending.mode),
            )
            .map_err(|source| {
                Error::Access(AccessError {
                    errmsg: "Error setting crtc",
                    dev: self.fd.dev_path(),
                    source,
                })
            })?;

        *current = pending.clone();

        if flip_flags.contains(PageFlipFlags::EVENT) {
            // set crtc does not trigger page_flip events, so we immediately queue a flip
            // with the same framebuffer.
            // this will result in wasting a frame, because this flip will need to wait
            // for `set_crtc`, but is necessary to drive the event loop and thus provide
            // a more consistent api.
            ControlDevice::page_flip(&*self.fd, self.crtc, framebuffer, flip_flags, None).map_err(
                |source| {
                    Error::Access(AccessError {
                        errmsg: "Failed to queue page flip",
                        dev: self.fd.dev_path(),
                        source,
                    })
                },
            )?;
        }

        Ok(())
    }

    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    pub fn page_flip(
        &self,
        framebuffer: framebuffer::Handle,
        flip_flags: PageFlipFlags,
    ) -> Result<(), Error> {
        let flip_flags = self.flip_flags(flip_flags);

        trace!("Queueing Page flip");

        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        ControlDevice::page_flip(&*self.fd, self.crtc, framebuffer, flip_flags, None).map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Failed to page flip",
                dev: self.fd.dev_path(),
                source,
            })
        })
    }

    #[instrument(level = "trace", parent = &self.span, skip(self))]
    #[profiling::function]
    pub fn test_buffer(&self, fb: framebuffer::Handle, mode: &Mode) -> Result<(), Error> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let pending = self.pending.read().unwrap();

        debug!("Setting screen for buffer *testing*");
        self.fd
            .set_crtc(
                self.crtc,
                Some(fb),
                (0, 0),
                &pending
                    .connectors
                    .iter()
                    .copied()
                    .collect::<Vec<connector::Handle>>(),
                Some(*mode),
            )
            .map_err(|source| {
                Error::Access(AccessError {
                    errmsg: "Failed to test buffer",
                    dev: self.fd.dev_path(),
                    source,
                })
            })
    }

    // we use this function to verify, if a certain connector/mode combination
    // is valid on our crtc. We do this with the most basic information we have:
    // - is there a matching encoder
    // - does the connector support the provided Mode.
    //
    // Better would be some kind of test commit to ask the driver,
    // but that only exists for the atomic api.
    fn check_connector(&self, conn: connector::Handle, mode: &Mode) -> Result<bool, Error> {
        let info = self.fd.get_connector(conn, false).map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Error loading connector info",
                dev: self.fd.dev_path(),
                source,
            })
        })?;

        // check if the connector can handle the current mode
        if info.modes().contains(mode) {
            // check if there is a valid encoder
            let encoders = info
                .encoders()
                .iter()
                .map(|encoder| {
                    self.fd.get_encoder(*encoder).map_err(|source| {
                        Error::Access(AccessError {
                            errmsg: "Error loading encoder info",
                            dev: self.fd.dev_path(),
                            source,
                        })
                    })
                })
                .collect::<Result<Vec<encoder::Info>, _>>()?;

            // and if any encoder supports the selected crtc
            let resource_handles = self.fd.resource_handles().map_err(|source| {
                Error::Access(AccessError {
                    errmsg: "Error loading resources",
                    dev: self.fd.dev_path(),
                    source,
                })
            })?;
            if !encoders
                .iter()
                .map(|encoder| encoder.possible_crtcs())
                .all(|crtc_list| resource_handles.filter_crtcs(crtc_list).contains(&self.crtc))
            {
                Ok(false)
            } else {
                Ok(true)
            }
        } else {
            Ok(false)
        }
    }

    pub(crate) fn reset_state<B: DevPath + ControlDevice + 'static>(
        &self,
        fd: Option<&B>,
    ) -> Result<(), Error> {
        *self.state.write().unwrap() = if let Some(fd) = fd {
            State::current_state(fd, self.crtc)?
        } else {
            State::current_state(&*self.fd, self.crtc)?
        };
        Ok(())
    }

    pub(crate) fn device_fd(&self) -> &DrmDeviceFd {
        self.fd.device_fd()
    }
}

impl Drop for LegacyDrmSurface {
    fn drop(&mut self) {
        let _guard = self.span.enter();
        // ignore failure at this point

        if !self.active.load(Ordering::SeqCst) {
            // the device is gone or we are on another tty
            // old state has been restored, we shouldn't touch it.
            // if we are on another tty the connectors will get disabled
            // by the device, when switching back
            return;
        }

        // disable connectors again
        let current = self.state.read().unwrap();
        if set_connector_state(&*self.fd, current.connectors.iter().copied(), false).is_ok() {
            // null commit
            let _ = self.fd.set_crtc(self.crtc, None, (0, 0), &[], None);
        }
    }
}

#[cfg(test)]
mod test {
    use super::LegacyDrmSurface;

    fn is_send<S: Send>() {}

    #[test]
    fn surface_is_send() {
        is_send::<LegacyDrmSurface>();
    }
}
