use drm::buffer::Buffer;
use drm::control::{
    connector, crtc, dumbbuffer::DumbBuffer, encoder, framebuffer, Device as ControlDevice, Mode,
    PageFlipFlags,
};
use drm::Device as BasicDevice;

use std::collections::HashSet;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::{atomic::Ordering, Arc, RwLock};

use crate::backend::drm::{common::Error, DevPath, RawSurface, Surface};
use crate::backend::graphics::CursorBackend;

use super::Dev;

use failure::{Fail, ResultExt};

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct State {
    pub mode: Mode,
    pub connectors: HashSet<connector::Handle>,
}

pub(in crate::backend::drm) struct LegacyDrmSurfaceInternal<A: AsRawFd + 'static> {
    pub(super) dev: Arc<Dev<A>>,
    pub(in crate::backend::drm) crtc: crtc::Handle,
    pub(super) state: RwLock<State>,
    pub(super) pending: RwLock<State>,
    pub(super) logger: ::slog::Logger,
}

impl<A: AsRawFd + 'static> AsRawFd for LegacyDrmSurfaceInternal<A> {
    fn as_raw_fd(&self) -> RawFd {
        self.dev.as_raw_fd()
    }
}

impl<A: AsRawFd + 'static> BasicDevice for LegacyDrmSurfaceInternal<A> {}
impl<A: AsRawFd + 'static> ControlDevice for LegacyDrmSurfaceInternal<A> {}

impl<A: AsRawFd + 'static> CursorBackend for LegacyDrmSurfaceInternal<A> {
    type CursorFormat = dyn Buffer;
    type Error = Error;

    fn set_cursor_position(&self, x: u32, y: u32) -> Result<(), Error> {
        if !self.dev.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        trace!(self.logger, "Move the cursor to {},{}", x, y);
        self.move_cursor(self.crtc, (x as i32, y as i32))
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Error moving cursor",
                dev: self.dev_path(),
                source,
            })
    }

    fn set_cursor_representation(
        &self,
        buffer: &Self::CursorFormat,
        hotspot: (u32, u32),
    ) -> Result<(), Error> {
        if !self.dev.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        trace!(self.logger, "Setting the new imported cursor");

        // set_cursor2 allows us to set the hotspot, but is not supported by every implementation.
        if self
            .set_cursor2(self.crtc, Some(buffer), (hotspot.0 as i32, hotspot.1 as i32))
            .is_err()
        {
            // the cursor will be slightly misplaced, when using the function for hotspots other then (0, 0),
            // but that is still better then no cursor.
            self.set_cursor(self.crtc, Some(buffer))
                .compat()
                .map_err(|source| Error::Access {
                    errmsg: "Failed to set cursor",
                    dev: self.dev_path(),
                    source,
                })?;
        }

        Ok(())
    }

    fn clear_cursor_representation(&self) -> Result<(), Error> {
        self.set_cursor(self.crtc, Option::<&DumbBuffer>::None)
            .compat()    
            .map_err(|source| Error::Access {
                errmsg: "Failed to set cursor",
                dev: self.dev_path(),
                source,
            })
    }
}

impl<A: AsRawFd + 'static> Surface for LegacyDrmSurfaceInternal<A> {
    type Error = Error;
    type Connectors = HashSet<connector::Handle>;

    fn crtc(&self) -> crtc::Handle {
        self.crtc
    }

    fn current_connectors(&self) -> Self::Connectors {
        self.state.read().unwrap().connectors.clone()
    }

    fn pending_connectors(&self) -> Self::Connectors {
        self.pending.read().unwrap().connectors.clone()
    }

    fn current_mode(&self) -> Mode {
        self.state.read().unwrap().mode
    }

    fn pending_mode(&self) -> Mode {
        self.pending.read().unwrap().mode
    }

    fn add_connector(&self, conn: connector::Handle) -> Result<(), Error> {
        if !self.dev.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let mut pending = self.pending.write().unwrap();

        if self.check_connector(conn, &pending.mode)? {
            pending.connectors.insert(conn);
        }

        Ok(())
    }

    fn remove_connector(&self, connector: connector::Handle) -> Result<(), Error> {
        let mut pending = self.pending.write().unwrap();

        if pending.connectors.contains(&connector) && pending.connectors.len() == 1 {
            return Err(Error::SurfaceWithoutConnectors(self.crtc));
        }

        pending.connectors.remove(&connector);
        Ok(())
    }

    fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Self::Error> {
        if connectors.is_empty() {
            return Err(Error::SurfaceWithoutConnectors(self.crtc));
        }

        if !self.dev.active.load(Ordering::SeqCst) {
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

    fn use_mode(&self, mode: Mode) -> Result<(), Error> {
        if !self.dev.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let mut pending = self.pending.write().unwrap();

        // check the connectors to see if this mode is supported
        for connector in &pending.connectors {
            if !self
                .get_connector(*connector)
                .compat()
                .map_err(|source| Error::Access {
                    errmsg: "Error loading connector info",
                    dev: self.dev_path(),
                    source,
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
}

impl<A: AsRawFd + 'static> RawSurface for LegacyDrmSurfaceInternal<A> {
    fn commit_pending(&self) -> bool {
        *self.pending.read().unwrap() != *self.state.read().unwrap()
    }

    fn commit(&self, framebuffer: framebuffer::Handle) -> Result<(), Error> {
        if !self.dev.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let mut current = self.state.write().unwrap();
        let pending = self.pending.read().unwrap();

        {
            let removed = current.connectors.difference(&pending.connectors);
            let added = pending.connectors.difference(&current.connectors);

            let mut conn_removed = false;
            for conn in removed.clone() {
                if let Ok(info) = self.get_connector(*conn) {
                    info!(self.logger, "Removing connector: {:?}", info.interface());
                } else {
                    info!(self.logger, "Removing unknown connector");
                }
                // if the connector was mapped to our crtc, we need to ack the disconnect.
                // the graphics pipeline will not be freed otherwise
                conn_removed = true;
            }
            self.dev.set_connector_state(removed.copied(), false)?;

            if conn_removed {
                // null commit (necessary to trigger removal on the kernel side with the legacy api.)
                self.set_crtc(self.crtc, None, (0, 0), &[], None)
                    .compat()
                    .map_err(|source| Error::Access {
                        errmsg: "Error setting crtc",
                        dev: self.dev_path(),
                        source,
                    })?;
            }

            for conn in added.clone() {
                if let Ok(info) = self.get_connector(*conn) {
                    info!(self.logger, "Adding connector: {:?}", info.interface());
                } else {
                    info!(self.logger, "Adding unknown connector");
                }
            }
            self.dev.set_connector_state(added.copied(), true)?;

            if current.mode != pending.mode {
                info!(self.logger, "Setting new mode: {:?}", pending.mode.name());
            }
        }

        debug!(self.logger, "Setting screen");
        // do a modeset and attach the given framebuffer
        self.set_crtc(
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
        .compat()
        .map_err(|source| Error::Access {
            errmsg: "Error setting crtc",
            dev: self.dev_path(),
            source,
        })?;

        *current = pending.clone();

        // set crtc does not trigger page_flip events, so we immediately queue a flip
        // with the same framebuffer.
        // this will result in wasting a frame, because this flip will need to wait
        // for `set_crtc`, but is necessary to drive the event loop and thus provide
        // a more consistent api.
        ControlDevice::page_flip(
            self,
            self.crtc,
            framebuffer,
            &[PageFlipFlags::PageFlipEvent],
            None,
        )
        .map_err(|source| Error::Access {
            errmsg: "Failed to queue page flip",
            dev: self.dev_path(),
            source: source.compat(),
        })
    }

    fn page_flip(&self, framebuffer: framebuffer::Handle) -> Result<(), Error> {
        trace!(self.logger, "Queueing Page flip");

        if !self.dev.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        ControlDevice::page_flip(
            self,
            self.crtc,
            framebuffer,
            &[PageFlipFlags::PageFlipEvent],
            None,
        )
        .compat()
        .map_err(|source| Error::Access {
            errmsg: "Failed to page flip",
            dev: self.dev_path(),
            source,
        })
    }
}

impl<A: AsRawFd + 'static> LegacyDrmSurfaceInternal<A> {
    pub(crate) fn new(
        dev: Arc<Dev<A>>,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: &[connector::Handle],
        logger: ::slog::Logger,
    ) -> Result<LegacyDrmSurfaceInternal<A>, Error> {
        info!(
            logger,
            "Initializing drm surface with mode {:?} and connectors {:?}", mode, connectors
        );

        // Try to enumarate the current state to set the initial state variable correctly.
        // We need an accurate state to handle `commit_pending`.
        let crtc_info = dev.get_crtc(crtc).compat().map_err(|source| Error::Access {
            errmsg: "Error loading crtc info",
            dev: dev.dev_path(),
            source,
        })?;

        let current_mode = crtc_info.mode();

        let mut current_connectors = HashSet::new();
        let res_handles = ControlDevice::resource_handles(&*dev)
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Error loading drm resources",
                dev: dev.dev_path(),
                source,
            })?;
        for &con in res_handles.connectors() {
            let con_info = dev.get_connector(con).compat().map_err(|source| Error::Access {
                errmsg: "Error loading connector info",
                dev: dev.dev_path(),
                source,
            })?;
            if let Some(enc) = con_info.current_encoder() {
                let enc_info = dev.get_encoder(enc).compat().map_err(|source| Error::Access {
                    errmsg: "Error loading encoder info",
                    dev: dev.dev_path(),
                    source,
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
        let state = State {
            mode: current_mode.unwrap_or_else(|| unsafe { std::mem::zeroed() }),
            connectors: current_connectors,
        };
        let pending = State {
            mode,
            connectors: connectors.iter().copied().collect(),
        };

        let surface = LegacyDrmSurfaceInternal {
            dev,
            crtc,
            state: RwLock::new(state),
            pending: RwLock::new(pending),
            logger,
        };

        Ok(surface)
    }

    // we use this function to verify, if a certain connector/mode combination
    // is valid on our crtc. We do this with the most basic information we have:
    // - is there a matching encoder
    // - does the connector support the provided Mode.
    //
    // Better would be some kind of test commit to ask the driver,
    // but that only exists for the atomic api.
    fn check_connector(&self, conn: connector::Handle, mode: &Mode) -> Result<bool, Error> {
        let info = self
            .get_connector(conn)
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Error loading connector info",
                dev: self.dev_path(),
                source,
            })?;

        // check if the connector can handle the current mode
        if info.modes().contains(mode) {
            // check if there is a valid encoder
            let encoders = info
                .encoders()
                .iter()
                .filter(|enc| enc.is_some())
                .map(|enc| enc.unwrap())
                .map(|encoder| {
                    self.get_encoder(encoder)
                        .compat()
                        .map_err(|source| Error::Access {
                            errmsg: "Error loading encoder info",
                            dev: self.dev_path(),
                            source,
                        })
                })
                .collect::<Result<Vec<encoder::Info>, _>>()?;

            // and if any encoder supports the selected crtc
            let resource_handles = self.resource_handles().compat().map_err(|source| Error::Access {
                errmsg: "Error loading resources",
                dev: self.dev_path(),
                source,
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
}

impl<A: AsRawFd + 'static> Drop for LegacyDrmSurfaceInternal<A> {
    fn drop(&mut self) {
        // ignore failure at this point

        if !self.dev.active.load(Ordering::SeqCst) {
            // the device is gone or we are on another tty
            // old state has been restored, we shouldn't touch it.
            // if we are on another tty the connectors will get disabled
            // by the device, when switching back
            return;
        }

        let _ = self.set_cursor(self.crtc, Option::<&DumbBuffer>::None);
        // disable connectors again
        let current = self.state.read().unwrap();
        if self
            .dev
            .set_connector_state(current.connectors.iter().copied(), false)
            .is_ok()
        {
            // null commit
            let _ = self.set_crtc(self.crtc, None, (0, 0), &[], None);
        }
    }
}

/// Open raw crtc utilizing legacy mode-setting
pub struct LegacyDrmSurface<A: AsRawFd + 'static>(
    pub(in crate::backend::drm) Arc<LegacyDrmSurfaceInternal<A>>,
);

impl<A: AsRawFd + 'static> AsRawFd for LegacyDrmSurface<A> {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl<A: AsRawFd + 'static> BasicDevice for LegacyDrmSurface<A> {}
impl<A: AsRawFd + 'static> ControlDevice for LegacyDrmSurface<A> {}

impl<A: AsRawFd + 'static> CursorBackend for LegacyDrmSurface<A> {
    type CursorFormat = dyn Buffer;
    type Error = Error;

    fn set_cursor_position(&self, x: u32, y: u32) -> Result<(), Error> {
        self.0.set_cursor_position(x, y)
    }

    fn set_cursor_representation(
        &self,
        buffer: &Self::CursorFormat,
        hotspot: (u32, u32),
    ) -> Result<(), Error> {
        self.0.set_cursor_representation(buffer, hotspot)
    }

    fn clear_cursor_representation(&self) -> Result<(), Self::Error> {
        self.0.clear_cursor_representation()
    }
}

impl<A: AsRawFd + 'static> Surface for LegacyDrmSurface<A> {
    type Error = Error;
    type Connectors = HashSet<connector::Handle>;

    fn crtc(&self) -> crtc::Handle {
        self.0.crtc()
    }

    fn current_connectors(&self) -> Self::Connectors {
        self.0.current_connectors()
    }

    fn pending_connectors(&self) -> Self::Connectors {
        self.0.pending_connectors()
    }

    fn current_mode(&self) -> Mode {
        self.0.current_mode()
    }

    fn pending_mode(&self) -> Mode {
        self.0.pending_mode()
    }

    fn add_connector(&self, connector: connector::Handle) -> Result<(), Error> {
        self.0.add_connector(connector)
    }

    fn remove_connector(&self, connector: connector::Handle) -> Result<(), Error> {
        self.0.remove_connector(connector)
    }

    fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Self::Error> {
        self.0.set_connectors(connectors)
    }

    fn use_mode(&self, mode: Mode) -> Result<(), Error> {
        self.0.use_mode(mode)
    }
}

impl<A: AsRawFd + 'static> RawSurface for LegacyDrmSurface<A> {
    fn commit_pending(&self) -> bool {
        self.0.commit_pending()
    }

    fn commit(&self, framebuffer: framebuffer::Handle) -> Result<(), Error> {
        self.0.commit(framebuffer)
    }

    fn page_flip(&self, framebuffer: framebuffer::Handle) -> Result<(), Error> {
        RawSurface::page_flip(&*self.0, framebuffer)
    }
}

#[cfg(test)]
mod test {
    use super::LegacyDrmSurface;
    use std::fs::File;

    fn is_send<S: Send>() {}

    #[test]
    fn surface_is_send() {
        is_send::<LegacyDrmSurface<File>>();
    }
}
