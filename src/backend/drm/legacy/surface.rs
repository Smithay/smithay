use drm::buffer::Buffer;
use drm::control::{
    connector, crtc, dumbbuffer::DumbBuffer, encoder, framebuffer, Device as ControlDevice, Mode,
    PageFlipFlags,
};
use drm::Device as BasicDevice;

use std::cell::Cell;
use std::collections::HashSet;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;
use std::sync::{RwLock, atomic::Ordering};

use crate::backend::drm::{common::Error, DevPath, RawSurface, Surface};
use crate::backend::graphics::CursorBackend;
use crate::backend::graphics::SwapBuffersError;

use super::Dev;

use failure::{Fail, ResultExt};

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct State {
    pub mode: Mode,
    pub connectors: HashSet<connector::Handle>,
}

pub(super) struct LegacyDrmSurfaceInternal<A: AsRawFd + 'static> {
    pub(super) dev: Rc<Dev<A>>,
    pub(super) crtc: crtc::Handle,
    pub(super) state: RwLock<State>,
    pub(super) pending: RwLock<State>,
    pub(super) logger: ::slog::Logger,
    init_buffer: Cell<Option<(DumbBuffer, framebuffer::Handle)>>,
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
        trace!(self.logger, "Setting the new imported cursor");

        if self
            .set_cursor2(self.crtc, Some(buffer), (hotspot.0 as i32, hotspot.1 as i32))
            .is_err()
        {
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
        let mut pending = self.pending.write().unwrap();

        if self.check_connector(conn, &pending.mode)? {
            pending.connectors.insert(conn);
        }

        Ok(())
    }

    fn remove_connector(&self, connector: connector::Handle) -> Result<(), Error> {
        self.pending.write().unwrap().connectors.remove(&connector);
        Ok(())
    }

    fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Self::Error> {
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
        let mut current = self.state.write().unwrap();
        let pending = self.pending.read().unwrap();

        {
            let removed = current.connectors.difference(&pending.connectors);
            let added = pending.connectors.difference(&current.connectors);

            let mut conn_removed = false;
            for conn in removed {
                if let Ok(info) = self.get_connector(*conn) {
                    info!(self.logger, "Removing connector: {:?}", info.interface());
                } else {
                    info!(self.logger, "Removing unknown connector");
                }
                // if the connector was mapped to our crtc, we need to ack the disconnect.
                // the graphics pipeline will not be freed otherwise
                conn_removed = true;
            }

            if conn_removed {
                // We need to do a null commit to free graphics pipelines
                self.set_crtc(self.crtc, None, (0, 0), &[], None)
                    .compat()
                    .map_err(|source| Error::Access {
                        errmsg: "Error setting crtc",
                        dev: self.dev_path(),
                        source,
                    })?;
            }

            for conn in added {
                if let Ok(info) = self.get_connector(*conn) {
                    info!(self.logger, "Adding connector: {:?}", info.interface());
                } else {
                    info!(self.logger, "Adding unknown connector");
                }
            }

            if current.mode != pending.mode {
                info!(
                    self.logger,
                    "Setting new mode: {:?}",
                    pending.mode.name()
                );
            }
        }

        debug!(self.logger, "Setting screen");
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

    fn page_flip(&self, framebuffer: framebuffer::Handle) -> ::std::result::Result<(), SwapBuffersError> {
        trace!(self.logger, "Queueing Page flip");

        ControlDevice::page_flip(
            self,
            self.crtc,
            framebuffer,
            &[PageFlipFlags::PageFlipEvent],
            None,
        )
        .map_err(|_| SwapBuffersError::ContextLost)
    }
}

impl<A: AsRawFd + 'static> LegacyDrmSurfaceInternal<A> {
    pub(crate) fn new(dev: Rc<Dev<A>>, crtc: crtc::Handle, mode: Mode, connectors: &[connector::Handle], logger: ::slog::Logger) -> Result<LegacyDrmSurfaceInternal<A>, Error> {
        // Try to enumarate the current state to set the initial state variable correctly
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
        let state = State { mode: current_mode.unwrap_or_else(|| unsafe { std::mem::zeroed() }), connectors: current_connectors };
        let pending = State { mode, connectors: connectors.into_iter().copied().collect() };
        
        let surface = LegacyDrmSurfaceInternal {
            dev,
            crtc,
            state: RwLock::new(state),
            pending: RwLock::new(pending),
            logger,
            init_buffer: Cell::new(None),
        };

        Ok(surface)
    }

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
        if let Some((db, fb)) = self.init_buffer.take() {
            let _ = self.destroy_framebuffer(fb);
            let _ = self.destroy_dumb_buffer(db);
        }

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
        for conn in current.connectors.iter() {
            if let Ok(info) = self.get_connector(*conn) {
                if info.state() == connector::State::Connected {
                    if let Ok(props) = self.get_properties(*conn) {
                        let (handles, _) = props.as_props_and_values();
                        for handle in handles {
                            if let Ok(info) = self.get_property(*handle) {
                                if info.name().to_str().map(|x| x == "DPMS").unwrap_or(false) {
                                    let _ = self.set_property(*conn, *handle, 3/*DRM_MODE_DPMS_OFF*/);
                                }
                            }
                        }
                    }
                }
            }
        }

        // null commit
        let _ = self.set_crtc(self.crtc, None, (0, 0), &[], None);
    }
}

/// Open raw crtc utilizing legacy mode-setting
pub struct LegacyDrmSurface<A: AsRawFd + 'static>(pub(super) Rc<LegacyDrmSurfaceInternal<A>>);

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

    fn page_flip(&self, framebuffer: framebuffer::Handle) -> ::std::result::Result<(), SwapBuffersError> {
        RawSurface::page_flip(&*self.0, framebuffer)
    }
}
