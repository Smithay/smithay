use drm::buffer::Buffer;
use drm::control::atomic::AtomicModeReq;
use drm::control::Device as ControlDevice;
use drm::control::{
    connector, crtc, dumbbuffer::DumbBuffer, framebuffer, plane, property, AtomicCommitFlags, Mode, PlaneType,
};
use drm::Device as BasicDevice;

use std::cell::Cell;
use std::collections::HashSet;
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;
use std::sync::{atomic::Ordering, RwLock};

use failure::ResultExt as FailureResultExt;

use super::Dev;
use crate::backend::drm::{common::Error, DevPath, RawSurface, Surface};
use crate::backend::graphics::CursorBackend;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorState {
    pub position: Cell<Option<(u32, u32)>>,
    pub hotspot: Cell<(u32, u32)>,
    pub framebuffer: Cell<Option<framebuffer::Handle>>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct State {
    pub mode: Mode,
    pub blob: property::Value<'static>,
    pub connectors: HashSet<connector::Handle>,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct Planes {
    pub primary: plane::Handle,
    pub cursor: plane::Handle,
}

pub(super) struct AtomicDrmSurfaceInternal<A: AsRawFd + 'static> {
    pub(super) dev: Rc<Dev<A>>,
    pub(super) crtc: crtc::Handle,
    pub(super) cursor: CursorState,
    pub(super) planes: Planes,
    pub(super) state: RwLock<State>,
    pub(super) pending: RwLock<State>,
    pub(super) logger: ::slog::Logger,
    pub(super) test_buffer: Cell<Option<(DumbBuffer, framebuffer::Handle)>>,
}

impl<A: AsRawFd + 'static> AsRawFd for AtomicDrmSurfaceInternal<A> {
    fn as_raw_fd(&self) -> RawFd {
        self.dev.as_raw_fd()
    }
}

impl<A: AsRawFd + 'static> BasicDevice for AtomicDrmSurfaceInternal<A> {}
impl<A: AsRawFd + 'static> ControlDevice for AtomicDrmSurfaceInternal<A> {}

impl<A: AsRawFd + 'static> AtomicDrmSurfaceInternal<A> {
    pub(crate) fn new(
        dev: Rc<Dev<A>>,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: &[connector::Handle],
        logger: ::slog::Logger,
    ) -> Result<Self, Error> {
        let crtc_info = dev.get_crtc(crtc).compat().map_err(|source| Error::Access {
            errmsg: "Error loading crtc info",
            dev: dev.dev_path(),
            source,
        })?;

        // If we have no current mode, we create a fake one, which will not match (and thus gets overriden on the commit below).
        // A better fix would probably be making mode an `Option`, but that would mean
        // we need to be sure, we require a mode to always be set without relying on the compiler.
        // So we cheat, because it works and is easier to handle later.
        let current_mode = crtc_info.mode().unwrap_or_else(|| unsafe { std::mem::zeroed() });
        let current_blob = match crtc_info.mode() {
            Some(mode) => dev
                .create_property_blob(mode)
                .compat()
                .map_err(|source| Error::Access {
                    errmsg: "Failed to create Property Blob for mode",
                    dev: dev.dev_path(),
                    source,
                })?,
            None => property::Value::Unknown(0),
        };

        let blob = dev
            .create_property_blob(mode)
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Failed to create Property Blob for mode",
                dev: dev.dev_path(),
                source,
            })?;

        let res_handles = ControlDevice::resource_handles(&*dev)
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Error loading drm resources",
                dev: dev.dev_path(),
                source,
            })?;

        let mut current_connectors = HashSet::new();
        for conn in res_handles.connectors() {
            let crtc_prop = dev
                .prop_mapping
                .0
                .get(&conn)
                .expect("Unknown handle")
                .get("CRTC_ID")
                .ok_or_else(|| Error::UnknownProperty {
                    handle: (*conn).into(),
                    name: "CRTC_ID",
                })
                .map(|x| *x)?;
            if let (Ok(crtc_prop_info), Ok(props)) = (dev.get_property(crtc_prop), dev.get_properties(*conn))
            {
                let (ids, vals) = props.as_props_and_values();
                for (&id, &val) in ids.iter().zip(vals.iter()) {
                    if id == crtc_prop {
                        if let property::Value::CRTC(Some(conn_crtc)) =
                            crtc_prop_info.value_type().convert_value(val)
                        {
                            if conn_crtc == crtc {
                                current_connectors.insert(*conn);
                            }
                        }
                        break;
                    }
                }
            }
        }
        let state = State {
            mode: current_mode,
            blob: current_blob,
            connectors: current_connectors,
        };
        let pending = State {
            mode,
            blob,
            connectors: connectors.iter().copied().collect(),
        };

        let (primary, cursor) =
            AtomicDrmSurfaceInternal::find_planes(&dev, crtc).ok_or(Error::NoSuitablePlanes {
                crtc,
                dev: dev.dev_path(),
            })?;
        let surface = AtomicDrmSurfaceInternal {
            dev,
            crtc,
            cursor: CursorState {
                position: Cell::new(None),
                framebuffer: Cell::new(None),
                hotspot: Cell::new((0, 0)),
            },
            planes: Planes { primary, cursor },
            state: RwLock::new(state),
            pending: RwLock::new(pending),
            logger,
            test_buffer: Cell::new(None),
        };

        Ok(surface)
    }

    fn create_test_buffer(&self, mode: &Mode) -> Result<framebuffer::Handle, Error> {
        let (w, h) = mode.size();
        let db = self
            .create_dumb_buffer((w as u32, h as u32), drm::buffer::format::PixelFormat::ARGB8888)
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Failed to create dumb buffer",
                dev: self.dev_path(),
                source,
            })?;
        let fb = self
            .add_framebuffer(&db)
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Failed to create framebuffer",
                dev: self.dev_path(),
                source,
            })?;
        if let Some((old_db, old_fb)) = self.test_buffer.replace(Some((db, fb))) {
            let _ = self.destroy_framebuffer(old_fb);
            let _ = self.destroy_dumb_buffer(old_db);
        };

        Ok(fb)
    }
}

impl<A: AsRawFd + 'static> Drop for AtomicDrmSurfaceInternal<A> {
    fn drop(&mut self) {
        if let Some((db, fb)) = self.test_buffer.take() {
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

        // other ttys that use no cursor, might not clear it themselves.
        // This makes sure our cursor won't stay visible.
        if let Err(err) = self.clear_plane(self.planes.cursor) {
            warn!(
                self.logger,
                "Failed to clear cursor on {:?}: {}", self.planes.cursor, err
            );
        }

        // disable connectors again
        let current = self.state.read().unwrap();
        let mut req = AtomicModeReq::new();
        for conn in current.connectors.iter() {
            let prop = self
                .dev
                .prop_mapping
                .0
                .get(&conn)
                .expect("Unknown Handle")
                .get("CRTC_ID")
                .expect("Unknown property CRTC_ID");
            req.add_property(*conn, *prop, property::Value::CRTC(None));
        }
        let active_prop = self
            .dev
            .prop_mapping
            .1
            .get(&self.crtc)
            .expect("Unknown Handle")
            .get("ACTIVE")
            .expect("Unknown property ACTIVE");
        let mode_prop = self
            .dev
            .prop_mapping
            .1
            .get(&self.crtc)
            .expect("Unknown Handle")
            .get("MODE_ID")
            .expect("Unknown property MODE_ID");

        req.add_property(self.crtc, *active_prop, property::Value::Boolean(false));
        req.add_property(self.crtc, *mode_prop, property::Value::Unknown(0));
        if let Err(err) = self.atomic_commit(&[AtomicCommitFlags::AllowModeset], req) {
            warn!(self.logger, "Unable to disable connectors: {}", err);
        }
    }
}

impl<A: AsRawFd + 'static> Surface for AtomicDrmSurfaceInternal<A> {
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

        let info = self
            .get_connector(conn)
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Error loading connector info",
                dev: self.dev_path(),
                source,
            })?;

        let mut pending = self.pending.write().unwrap();

        // check if the connector can handle the current mode
        if info.modes().contains(&pending.mode) {
            // check if config is supported
            let req = self.build_request(
                &mut [conn].iter(),
                &mut [].iter(),
                &self.planes,
                Some(self.create_test_buffer(&pending.mode)?),
                Some(pending.mode),
                Some(pending.blob),
            )?;
            self.atomic_commit(
                &[AtomicCommitFlags::AllowModeset, AtomicCommitFlags::TestOnly],
                req,
            )
            .compat()
            .map_err(|_| Error::TestFailed(self.crtc))?;

            // seems to be, lets add the connector
            pending.connectors.insert(conn);

            Ok(())
        } else {
            Err(Error::ModeNotSuitable(pending.mode))
        }
    }

    fn remove_connector(&self, conn: connector::Handle) -> Result<(), Error> {
        if !self.dev.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let mut pending = self.pending.write().unwrap();

        // the test would also prevent this, but the error message is far less helpful
        if pending.connectors.contains(&conn) && pending.connectors.len() == 1 {
            return Err(Error::SurfaceWithoutConnectors(self.crtc));
        }

        // check if new config is supported (should be)
        let req = self.build_request(
            &mut [].iter(),
            &mut [conn].iter(),
            &self.planes,
            Some(self.create_test_buffer(&pending.mode)?),
            Some(pending.mode),
            Some(pending.blob),
        )?;
        self.atomic_commit(
            &[AtomicCommitFlags::AllowModeset, AtomicCommitFlags::TestOnly],
            req,
        )
        .compat()
        .map_err(|_| Error::TestFailed(self.crtc))?;

        // seems to be, lets remove the connector
        pending.connectors.remove(&conn);

        Ok(())
    }

    fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Error> {
        if connectors.is_empty() {
            return Err(Error::SurfaceWithoutConnectors(self.crtc));
        }

        if !self.dev.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let current = self.state.write().unwrap();
        let mut pending = self.pending.write().unwrap();

        let conns = connectors.iter().cloned().collect::<HashSet<_>>();
        let mut added = conns.difference(&current.connectors);
        let mut removed = current.connectors.difference(&conns);

        let req = self.build_request(
            &mut added,
            &mut removed,
            &self.planes,
            Some(self.create_test_buffer(&pending.mode)?),
            Some(pending.mode),
            Some(pending.blob),
        )?;

        self.atomic_commit(
            &[AtomicCommitFlags::AllowModeset, AtomicCommitFlags::TestOnly],
            req,
        )
        .map_err(|_| Error::TestFailed(self.crtc))?;

        pending.connectors = conns;

        Ok(())
    }

    fn use_mode(&self, mode: Mode) -> Result<(), Error> {
        if !self.dev.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let mut pending = self.pending.write().unwrap();

        // check if new config is supported
        let new_blob = self
            .create_property_blob(mode)
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Failed to create Property Blob for mode",
                dev: self.dev_path(),
                source,
            })?;

        let test_fb = Some(self.create_test_buffer(&pending.mode)?);
        let req = self.build_request(
            &mut pending.connectors.iter(),
            &mut [].iter(),
            &self.planes,
            test_fb,
            Some(mode),
            Some(new_blob),
        )?;
        if let Err(err) = self
            .atomic_commit(
                &[AtomicCommitFlags::AllowModeset, AtomicCommitFlags::TestOnly],
                req,
            )
            .compat()
            .map_err(|_| Error::TestFailed(self.crtc))
        {
            let _ = self.dev.destroy_property_blob(new_blob.into());
            return Err(err);
        }

        // seems to be, lets change the mode
        pending.mode = mode;
        pending.blob = new_blob;

        Ok(())
    }
}

impl<A: AsRawFd + 'static> RawSurface for AtomicDrmSurfaceInternal<A> {
    fn commit_pending(&self) -> bool {
        *self.pending.read().unwrap() != *self.state.read().unwrap()
    }

    fn commit(&self, framebuffer: framebuffer::Handle) -> Result<(), Error> {
        if !self.dev.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let mut current = self.state.write().unwrap();
        let mut pending = self.pending.write().unwrap();

        debug!(
            self.logger,
            "Preparing Commit.\n\tCurrent: {:?}\n\tPending: {:?}\n", *current, *pending
        );

        let current_conns = current.connectors.clone();
        let pending_conns = pending.connectors.clone();
        let mut removed = current_conns.difference(&pending_conns);
        let mut added = pending_conns.difference(&current_conns);

        for conn in removed.clone() {
            if let Ok(info) = self.get_connector(*conn) {
                info!(self.logger, "Removing connector: {:?}", info.interface());
            } else {
                info!(self.logger, "Removing unknown connector");
            }
        }

        for conn in added.clone() {
            if let Ok(info) = self.get_connector(*conn) {
                info!(self.logger, "Adding connector: {:?}", info.interface());
            } else {
                info!(self.logger, "Adding unknown connector");
            }
        }

        if current.mode != pending.mode {
            info!(self.logger, "Setting new mode: {:?}", pending.mode.name());
        }

        trace!(self.logger, "Testing screen config");

        let req = {
            let req = self.build_request(
                &mut added,
                &mut removed,
                &self.planes,
                Some(framebuffer),
                Some(pending.mode),
                Some(pending.blob),
            )?;

            if let Err(err) = self
                .atomic_commit(
                    &[AtomicCommitFlags::AllowModeset, AtomicCommitFlags::TestOnly],
                    req.clone(),
                )
                .compat()
                .map_err(|_| Error::TestFailed(self.crtc))
            {
                warn!(
                    self.logger,
                    "New screen configuration invalid!:\n\t{:#?}\n\t{}\n", req, err
                );
                info!(self.logger, "Reverting back to last know good state");

                *pending = current.clone();

                self.build_request(
                    &mut [].iter(),
                    &mut [].iter(),
                    &self.planes,
                    Some(framebuffer),
                    Some(current.mode),
                    Some(current.blob),
                )?
            } else {
                if current.mode != pending.mode {
                    if let Err(err) = self.dev.destroy_property_blob(current.blob.into()) {
                        warn!(self.logger, "Failed to destory old mode property blob: {}", err);
                    }
                }

                // new config
                req
            }
        };

        debug!(self.logger, "Setting screen: {:?}", req);
        let result = self
            .atomic_commit(
                &[
                    AtomicCommitFlags::PageFlipEvent,
                    AtomicCommitFlags::AllowModeset,
                    AtomicCommitFlags::Nonblock,
                ],
                req,
            )
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Error setting crtc",
                dev: self.dev_path(),
                source,
            });

        if result.is_ok() {
            *current = pending.clone();
        }

        result
    }

    fn page_flip(&self, framebuffer: framebuffer::Handle) -> Result<(), Error> {
        if !self.dev.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let req = self.build_request(
            &mut [].iter(),
            &mut [].iter(),
            &self.planes,
            Some(framebuffer),
            None,
            None,
        )?;

        trace!(self.logger, "Queueing page flip: {:?}", req);
        self.atomic_commit(
            &[AtomicCommitFlags::PageFlipEvent, AtomicCommitFlags::Nonblock],
            req,
        )
        .compat()
        .map_err(|source| Error::Access {
            errmsg: "Page flip commit failed",
            dev: self.dev_path(),
            source,
        })?;

        Ok(())
    }
}

impl<A: AsRawFd + 'static> CursorBackend for AtomicDrmSurfaceInternal<A> {
    type CursorFormat = dyn Buffer;
    type Error = Error;

    fn set_cursor_position(&self, x: u32, y: u32) -> Result<(), Error> {
        if !self.dev.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        trace!(self.logger, "New cursor position ({},{}) pending", x, y);
        self.cursor.position.set(Some((x, y)));
        Ok(())
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

        if let Some(fb) = self.cursor.framebuffer.get().take() {
            let _ = self.destroy_framebuffer(fb);
        }

        self.cursor.framebuffer.set(Some(
            self.add_planar_framebuffer(buffer, &[0; 4], 0)
                .compat()
                .map_err(|source| Error::Access {
                    errmsg: "Failed to import cursor",
                    dev: self.dev_path(),
                    source,
                })?,
        ));

        self.cursor.hotspot.set(hotspot);

        Ok(())
    }
}

impl<A: AsRawFd + 'static> AtomicDrmSurfaceInternal<A> {
    fn conn_prop_handle(
        &self,
        handle: connector::Handle,
        name: &'static str,
    ) -> Result<property::Handle, Error> {
        (*self.dev)
            .prop_mapping
            .0
            .get(&handle)
            .expect("Unknown handle")
            .get(name)
            .ok_or_else(|| Error::UnknownProperty {
                handle: handle.into(),
                name,
            })
            .map(|x| *x)
    }

    fn crtc_prop_handle(&self, handle: crtc::Handle, name: &'static str) -> Result<property::Handle, Error> {
        (*self.dev)
            .prop_mapping
            .1
            .get(&handle)
            .expect("Unknown handle")
            .get(name)
            .ok_or_else(|| Error::UnknownProperty {
                handle: handle.into(),
                name,
            })
            .map(|x| *x)
    }

    #[allow(dead_code)]
    fn fb_prop_handle(
        &self,
        handle: framebuffer::Handle,
        name: &'static str,
    ) -> Result<property::Handle, Error> {
        (*self.dev)
            .prop_mapping
            .2
            .get(&handle)
            .expect("Unknown handle")
            .get(name)
            .ok_or_else(|| Error::UnknownProperty {
                handle: handle.into(),
                name,
            })
            .map(|x| *x)
    }

    fn plane_prop_handle(
        &self,
        handle: plane::Handle,
        name: &'static str,
    ) -> Result<property::Handle, Error> {
        (*self.dev)
            .prop_mapping
            .3
            .get(&handle)
            .expect("Unknown handle")
            .get(name)
            .ok_or_else(|| Error::UnknownProperty {
                handle: handle.into(),
                name,
            })
            .map(|x| *x)
    }

    // If a mode is set a matching blob needs to be set (the inverse is not true)
    fn build_request(
        &self,
        new_connectors: &mut dyn Iterator<Item = &connector::Handle>,
        removed_connectors: &mut dyn Iterator<Item = &connector::Handle>,
        planes: &Planes,
        framebuffer: Option<framebuffer::Handle>,
        mode: Option<Mode>,
        blob: Option<property::Value<'static>>,
    ) -> Result<AtomicModeReq, Error> {
        let mut req = AtomicModeReq::new();

        for conn in new_connectors {
            req.add_property(
                *conn,
                self.conn_prop_handle(*conn, "CRTC_ID")?,
                property::Value::CRTC(Some(self.crtc)),
            );
        }

        for conn in removed_connectors {
            req.add_property(
                *conn,
                self.conn_prop_handle(*conn, "CRTC_ID")?,
                property::Value::CRTC(None),
            );
        }

        if let Some(blob) = blob {
            req.add_property(self.crtc, self.crtc_prop_handle(self.crtc, "MODE_ID")?, blob);
        }

        req.add_property(
            self.crtc,
            self.crtc_prop_handle(self.crtc, "ACTIVE")?,
            property::Value::Boolean(true),
        );

        if let Some(fb) = framebuffer {
            req.add_property(
                planes.primary,
                self.plane_prop_handle(planes.primary, "FB_ID")?,
                property::Value::Framebuffer(Some(fb)),
            );
        }

        if let Some(mode) = mode {
            req.add_property(
                planes.primary,
                self.plane_prop_handle(planes.primary, "CRTC_ID")?,
                property::Value::CRTC(Some(self.crtc)),
            );
            req.add_property(
                planes.primary,
                self.plane_prop_handle(planes.primary, "SRC_X")?,
                property::Value::UnsignedRange(0),
            );
            req.add_property(
                planes.primary,
                self.plane_prop_handle(planes.primary, "SRC_Y")?,
                property::Value::UnsignedRange(0),
            );
            req.add_property(
                planes.primary,
                self.plane_prop_handle(planes.primary, "SRC_W")?,
                property::Value::UnsignedRange((mode.size().0 as u64) << 16),
            );
            req.add_property(
                planes.primary,
                self.plane_prop_handle(planes.primary, "SRC_H")?,
                property::Value::UnsignedRange((mode.size().1 as u64) << 16),
            );
            req.add_property(
                planes.primary,
                self.plane_prop_handle(planes.primary, "CRTC_X")?,
                property::Value::SignedRange(0),
            );
            req.add_property(
                planes.primary,
                self.plane_prop_handle(planes.primary, "CRTC_Y")?,
                property::Value::SignedRange(0),
            );
            req.add_property(
                planes.primary,
                self.plane_prop_handle(planes.primary, "CRTC_W")?,
                property::Value::UnsignedRange(mode.size().0 as u64),
            );
            req.add_property(
                planes.primary,
                self.plane_prop_handle(planes.primary, "CRTC_H")?,
                property::Value::UnsignedRange(mode.size().1 as u64),
            );
        }

        let cursor_pos = self.cursor.position.get();
        let cursor_fb = self.cursor.framebuffer.get();

        if let (Some(pos), Some(fb)) = (cursor_pos, cursor_fb) {
            match self.get_framebuffer(fb).compat().map_err(|source| Error::Access {
                errmsg: "Error getting cursor fb",
                dev: self.dev_path(),
                source,
            }) {
                Ok(cursor_info) => {
                    let hotspot = self.cursor.hotspot.get();

                    req.add_property(
                        planes.cursor,
                        self.plane_prop_handle(planes.cursor, "CRTC_ID")?,
                        property::Value::CRTC(Some(self.crtc)),
                    );
                    req.add_property(
                        planes.cursor,
                        self.plane_prop_handle(planes.cursor, "SRC_X")?,
                        property::Value::UnsignedRange(0),
                    );
                    req.add_property(
                        planes.cursor,
                        self.plane_prop_handle(planes.cursor, "SRC_Y")?,
                        property::Value::UnsignedRange(0),
                    );
                    req.add_property(
                        planes.cursor,
                        self.plane_prop_handle(planes.cursor, "SRC_W")?,
                        property::Value::UnsignedRange((cursor_info.size().0 as u64) << 16),
                    );
                    req.add_property(
                        planes.cursor,
                        self.plane_prop_handle(planes.cursor, "SRC_H")?,
                        property::Value::UnsignedRange((cursor_info.size().1 as u64) << 16),
                    );
                    req.add_property(
                        planes.cursor,
                        self.plane_prop_handle(planes.cursor, "CRTC_X")?,
                        property::Value::SignedRange(pos.0 as i64 - (hotspot.0 as i64)),
                    );
                    req.add_property(
                        planes.cursor,
                        self.plane_prop_handle(planes.cursor, "CRTC_Y")?,
                        property::Value::SignedRange(pos.1 as i64 - (hotspot.1 as i64)),
                    );
                    req.add_property(
                        planes.cursor,
                        self.plane_prop_handle(planes.cursor, "CRTC_W")?,
                        property::Value::UnsignedRange(cursor_info.size().0 as u64),
                    );
                    req.add_property(
                        planes.cursor,
                        self.plane_prop_handle(planes.cursor, "CRTC_H")?,
                        property::Value::UnsignedRange(cursor_info.size().1 as u64),
                    );
                    req.add_property(
                        planes.cursor,
                        self.plane_prop_handle(planes.cursor, "FB_ID")?,
                        property::Value::Framebuffer(Some(fb)),
                    );
                }
                Err(err) => {
                    warn!(self.logger, "Cursor FB invalid: {}. Skipping.", err);
                    self.cursor.framebuffer.set(None);
                }
            }
        }

        Ok(req)
    }

    fn find_planes(card: &Dev<A>, crtc: crtc::Handle) -> Option<(plane::Handle, plane::Handle)> {
        let res = card.resource_handles().expect("Could not list resources");
        let planes = card.plane_handles().expect("Could not list planes");
        let vec: Vec<(PlaneType, plane::Handle)> = planes
            .planes()
            .iter()
            .copied()
            .filter(|plane| {
                card.get_plane(*plane)
                    .map(|plane_info| {
                        let compatible_crtcs = res.filter_crtcs(plane_info.possible_crtcs());
                        compatible_crtcs.contains(&crtc)
                    })
                    .unwrap_or(false)
            })
            .filter_map(|plane| {
                if let Ok(props) = card.get_properties(plane) {
                    let (ids, vals) = props.as_props_and_values();
                    for (&id, &val) in ids.iter().zip(vals.iter()) {
                        if let Ok(info) = card.get_property(id) {
                            if info.name().to_str().map(|x| x == "type").unwrap_or(false) {
                                if val == (PlaneType::Primary as u32).into() {
                                    return Some((PlaneType::Primary, plane));
                                }
                                if val == (PlaneType::Cursor as u32).into() {
                                    return Some((PlaneType::Cursor, plane));
                                }
                            }
                        }
                    }
                }
                None
            })
            .collect();

        Some((
            vec.iter().find_map(|(plane_type, plane)| {
                if *plane_type == PlaneType::Primary {
                    Some(*plane)
                } else {
                    None
                }
            })?,
            vec.iter().find_map(|(plane_type, plane)| {
                if *plane_type == PlaneType::Cursor {
                    Some(*plane)
                } else {
                    None
                }
            })?,
        ))
    }

    pub(crate) fn clear_plane(&self, plane: plane::Handle) -> Result<(), Error> {
        let mut req = AtomicModeReq::new();

        req.add_property(
            plane,
            self.plane_prop_handle(plane, "CRTC_ID")?,
            property::Value::CRTC(None),
        );

        req.add_property(
            plane,
            self.plane_prop_handle(plane, "FB_ID")?,
            property::Value::Framebuffer(None),
        );

        self.atomic_commit(&[AtomicCommitFlags::TestOnly], req.clone())
            .compat()
            .map_err(|_| Error::TestFailed(self.crtc))?;

        self.atomic_commit(&[AtomicCommitFlags::Nonblock], req)
            .compat()
            .map_err(|source| Error::Access {
                errmsg: "Failed to commit on clear_plane",
                dev: self.dev_path(),
                source,
            })
    }
}

/// Open raw crtc utilizing atomic mode-setting
pub struct AtomicDrmSurface<A: AsRawFd + 'static>(pub(super) Rc<AtomicDrmSurfaceInternal<A>>);

impl<A: AsRawFd + 'static> AsRawFd for AtomicDrmSurface<A> {
    fn as_raw_fd(&self) -> RawFd {
        self.0.as_raw_fd()
    }
}

impl<A: AsRawFd + 'static> BasicDevice for AtomicDrmSurface<A> {}
impl<A: AsRawFd + 'static> ControlDevice for AtomicDrmSurface<A> {}

impl<A: AsRawFd + 'static> CursorBackend for AtomicDrmSurface<A> {
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

impl<A: AsRawFd + 'static> Surface for AtomicDrmSurface<A> {
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

    fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Error> {
        self.0.set_connectors(connectors)
    }

    fn use_mode(&self, mode: Mode) -> Result<(), Error> {
        self.0.use_mode(mode)
    }
}

impl<A: AsRawFd + 'static> RawSurface for AtomicDrmSurface<A> {
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
