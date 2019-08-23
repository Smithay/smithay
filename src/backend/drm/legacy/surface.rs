use drm::buffer::Buffer;
use drm::control::{connector, crtc, dumbbuffer::DumbBuffer, encoder, framebuffer, Device as ControlDevice, Mode, PageFlipFlags};
use drm::Device as BasicDevice;
use failure::ResultExt as FailureResultExt;

use std::collections::HashSet;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;
use std::sync::RwLock;

use crate::backend::drm::{DevPath, RawSurface, Surface};
use crate::backend::graphics::CursorBackend;
use crate::backend::graphics::SwapBuffersError;

use super::{error::*, Dev};

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct State {
    pub mode: Option<Mode>,
    pub connectors: HashSet<connector::Handle>,
}

pub(super) struct LegacyDrmSurfaceInternal<A: AsRawFd + 'static> {
    pub(super) dev: Rc<Dev<A>>,
    pub(super) crtc: crtc::Handle,
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

impl<'a, A: AsRawFd + 'static> CursorBackend<'a> for LegacyDrmSurfaceInternal<A> {
    type CursorFormat = &'a dyn Buffer;
    type Error = Error;

    fn set_cursor_position(&self, x: u32, y: u32) -> Result<()> {
        trace!(self.logger, "Move the cursor to {},{}", x, y);
        self.move_cursor(self.crtc, (x as i32, y as i32))
            .compat()
            .chain_err(|| ErrorKind::DrmDev(format!("Error moving cursor on {:?}", self.dev_path())))
    }

    fn set_cursor_representation<'b>(&'b self, buffer: Self::CursorFormat, hotspot: (u32, u32)) -> Result<()>
    where
        'a: 'b,
    {
        trace!(self.logger, "Setting the new imported cursor");

        if self.set_cursor2(self.crtc, Some(buffer), (hotspot.0 as i32, hotspot.1 as i32)).is_err() {
            self.set_cursor(self.crtc, Some(buffer))
                .compat()
                .chain_err(|| ErrorKind::DrmDev(format!("Failed to set cursor on {:?}", self.dev_path())))?;
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

    fn current_mode(&self) -> Option<Mode> {
        self.state.read().unwrap().mode
    }

    fn pending_mode(&self) -> Option<Mode> {
        self.pending.read().unwrap().mode
    }

    fn add_connector(&self, conn: connector::Handle) -> Result<()> {
        let info = self.get_connector(conn).compat().chain_err(|| {
            ErrorKind::DrmDev(format!("Error loading connector info on {:?}", self.dev_path()))
        })?;

        let mut pending = self.pending.write().unwrap();

        // check if the connector can handle the current mode
        if info.modes().contains(pending.mode.as_ref().unwrap()) {
            // check if there is a valid encoder
            let encoders = info
                .encoders()
                .iter()
                .filter(|enc| enc.is_some())
                .map(|enc| enc.unwrap())
                .map(|encoder| {
                    self.get_encoder(encoder).compat().chain_err(|| {
                        ErrorKind::DrmDev(format!("Error loading encoder info on {:?}", self.dev_path()))
                    })
                })
                .collect::<Result<Vec<encoder::Info>>>()?;

            // and if any encoder supports the selected crtc
            let resource_handles = self.resource_handles().compat().chain_err(|| {
                ErrorKind::DrmDev(format!("Error loading resources on {:?}", self.dev_path()))
            })?;
            if !encoders
                .iter()
                .map(|encoder| encoder.possible_crtcs())
                .all(|crtc_list| resource_handles.filter_crtcs(crtc_list).contains(&self.crtc))
            {
                bail!(ErrorKind::NoSuitableEncoder(info, self.crtc));
            }

            pending.connectors.insert(conn);
            Ok(())
        } else {
            bail!(ErrorKind::ModeNotSuitable(pending.mode.unwrap()));
        }
    }

    fn remove_connector(&self, connector: connector::Handle) -> Result<()> {
        self.pending.write().unwrap().connectors.remove(&connector);
        Ok(())
    }

    fn use_mode(&self, mode: Option<Mode>) -> Result<()> {
        let mut pending = self.pending.write().unwrap();

        // check the connectors to see if this mode is supported
        if let Some(mode) = mode {
            for connector in &pending.connectors {
                if !self.get_connector(*connector)
                    .compat()
                    .chain_err(|| {
                        ErrorKind::DrmDev(format!("Error loading connector info on {:?}", self.dev_path()))
                    })?
                    .modes()
                    .contains(&mode)
                {
                    bail!(ErrorKind::ModeNotSuitable(mode));
                }
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

    fn commit(&self, framebuffer: framebuffer::Handle) -> Result<()> {
        let mut current = self.state.write().unwrap();
        let pending = self.pending.read().unwrap();

        {
            let removed = current.connectors.difference(&pending.connectors);
            let added = pending.connectors.difference(&current.connectors);

            for conn in removed {
                if let Ok(info) = self.get_connector(*conn) {
                    info!(self.logger, "Removing connector: {:?}", info.interface());
                } else {
                    info!(self.logger, "Removing unknown connector");
                }
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
                    pending.mode.as_ref().unwrap().name()
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
            pending.mode,
        )
        .compat()
        .chain_err(|| {
            ErrorKind::DrmDev(format!(
                "Error setting crtc {:?} on {:?}",
                self.crtc,
                self.dev_path()
            ))
        })?;

        *current = pending.clone();

        Ok(())
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
        .map_err(|x| dbg!(x))
        .map_err(|_| SwapBuffersError::ContextLost)
    }
}

impl<A: AsRawFd + 'static> Drop for LegacyDrmSurfaceInternal<A> {
    fn drop(&mut self) {
        // ignore failure at this point
        let _ = self.set_cursor(self.crtc, Option::<&DumbBuffer>::None);
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

impl<'a, A: AsRawFd + 'static> CursorBackend<'a> for LegacyDrmSurface<A> {
    type CursorFormat = &'a dyn Buffer;
    type Error = Error;

    fn set_cursor_position(&self, x: u32, y: u32) -> Result<()> {
        self.0.set_cursor_position(x, y)
    }

    fn set_cursor_representation<'b>(&'b self, buffer: Self::CursorFormat, hotspot: (u32, u32)) -> Result<()>
    where
        'a: 'b,
    {
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

    fn current_mode(&self) -> Option<Mode> {
        self.0.current_mode()
    }

    fn pending_mode(&self) -> Option<Mode> {
        self.0.pending_mode()
    }

    fn add_connector(&self, connector: connector::Handle) -> Result<()> {
        self.0.add_connector(connector)
    }

    fn remove_connector(&self, connector: connector::Handle) -> Result<()> {
        self.0.remove_connector(connector)
    }

    fn use_mode(&self, mode: Option<Mode>) -> Result<()> {
        self.0.use_mode(mode)
    }
}

impl<A: AsRawFd + 'static> RawSurface for LegacyDrmSurface<A> {
    fn commit_pending(&self) -> bool {
        self.0.commit_pending()
    }

    fn commit(&self, framebuffer: framebuffer::Handle) -> Result<()> {
        self.0.commit(framebuffer)
    }

    fn page_flip(&self, framebuffer: framebuffer::Handle) -> ::std::result::Result<(), SwapBuffersError> {
        RawSurface::page_flip(&*self.0, framebuffer)
    }
}
