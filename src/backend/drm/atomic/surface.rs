use drm::buffer::Buffer;
use drm::control::atomic::AtomicModeReq;
use drm::control::Device as ControlDevice;
use drm::control::{
    connector, crtc, dumbbuffer::DumbBuffer, framebuffer, plane, property, AtomicCommitFlags, Mode, PlaneType,
};
use drm::Device as BasicDevice;

use std::collections::HashSet;
use std::os::unix::io::{AsRawFd, RawFd};
use std::sync::{atomic::Ordering, Arc, Mutex, RwLock};

use failure::ResultExt as FailureResultExt;

use super::Dev;
use crate::backend::drm::{common::Error, DevPath, RawSurface, Surface};
use crate::backend::graphics::CursorBackend;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorState {
    pub position: Option<(u32, u32)>,
    pub hotspot: (u32, u32),
    pub framebuffer: Option<framebuffer::Handle>,
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

#[derive(Debug)]
pub(in crate::backend::drm) struct AtomicDrmSurfaceInternal<A: AsRawFd + 'static> {
    pub(super) dev: Arc<Dev<A>>,
    pub(in crate::backend::drm) crtc: crtc::Handle,
    pub(super) cursor: Mutex<CursorState>,
    pub(in crate::backend::drm) planes: Planes,
    pub(super) state: RwLock<State>,
    pub(super) pending: RwLock<State>,
    pub(super) logger: ::slog::Logger,
    pub(super) test_buffer: Mutex<Option<(DumbBuffer, framebuffer::Handle)>>,
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
        dev: Arc<Dev<A>>,
        crtc: crtc::Handle,
        mode: Mode,
        connectors: &[connector::Handle],
        logger: ::slog::Logger,
    ) -> Result<Self, Error> {
        info!(
            logger,
            "Initializing drm surface with mode {:?} and connectors {:?}", mode, connectors
        );

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

        // the current set of connectors are those, that already have the correct `CRTC_ID` set.
        // so we collect them for `current_state` and set the user-given once in `pending_state`.
        //
        // If they don't match, `commit_pending` will return true and they will be changed on the next `commit`.
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

        // we need to find planes for this crtc.
        // (cursor and primary planes are usually available once for every crtc,
        //  so this is a very naive algorithm.)
        let (primary, cursor) =
            AtomicDrmSurfaceInternal::find_planes(&dev, crtc).ok_or(Error::NoSuitablePlanes {
                crtc,
                dev: dev.dev_path(),
            })?;
        let surface = AtomicDrmSurfaceInternal {
            dev,
            crtc,
            cursor: Mutex::new(CursorState {
                position: None,
                framebuffer: None,
                hotspot: (0, 0),
            }),
            planes: Planes { primary, cursor },
            state: RwLock::new(state),
            pending: RwLock::new(pending),
            logger,
            test_buffer: Mutex::new(None),
        };

        Ok(surface)
    }

    // we need a framebuffer to do test commits, which we use to verify our pending state.
    // here we create a dumbbuffer for that purpose.
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

        let mut test_buffer = self.test_buffer.lock().unwrap();
        if let Some((old_db, old_fb)) = test_buffer.take() {
            let _ = self.destroy_framebuffer(old_fb);
            let _ = self.destroy_dumb_buffer(old_db);
        };
        *test_buffer = Some((db, fb));

        Ok(fb)
    }
}

impl<A: AsRawFd + 'static> Drop for AtomicDrmSurfaceInternal<A> {
    fn drop(&mut self) {
        if let Some((db, fb)) = self.test_buffer.lock().unwrap().take() {
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
        // the test would also prevent this, but the error message is far less helpful
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
        let pending = self.pending.write().unwrap();

        debug!(
            self.logger,
            "Preparing Commit.\n\tCurrent: {:?}\n\tPending: {:?}\n", *current, *pending
        );

        // we need the differences to know, which connectors need to change properties
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

        // test the new config and return the request if it would be accepted by the driver.
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

                return Err(err);
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
                    // on the atomic api we can modeset and trigger a page_flip event on the same call!
                    AtomicCommitFlags::PageFlipEvent,
                    AtomicCommitFlags::AllowModeset,
                    // we also do not need to wait for completion, like with `set_crtc`.
                    // and have tested this already, so we do not expect any errors later down the line.
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

        // page flips work just like commits with fewer parameters..
        let req = self.build_request(
            &mut [].iter(),
            &mut [].iter(),
            &self.planes,
            Some(framebuffer),
            None,
            None,
        )?;

        // .. and without `AtomicCommitFlags::AllowModeset`.
        // If we would set anything here, that would require a modeset, this would fail,
        // indicating a problem in our assumptions.
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

// this whole implementation just queues the cursor state for the next commit.
impl<A: AsRawFd + 'static> CursorBackend for AtomicDrmSurfaceInternal<A> {
    type CursorFormat = dyn Buffer;
    type Error = Error;

    fn set_cursor_position(&self, x: u32, y: u32) -> Result<(), Error> {
        if !self.dev.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        trace!(self.logger, "New cursor position ({},{}) pending", x, y);
        self.cursor.lock().unwrap().position = Some((x, y));
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

        let mut cursor = self.cursor.lock().unwrap();

        if let Some(fb) = cursor.framebuffer.take() {
            let _ = self.destroy_framebuffer(fb);
        }

        cursor.framebuffer = Some(self.add_planar_framebuffer(buffer, &[0; 4], 0).compat().map_err(
            |source| Error::Access {
                errmsg: "Failed to import cursor",
                dev: self.dev_path(),
                source,
            },
        )?);
        cursor.hotspot = hotspot;

        Ok(())
    }

    fn clear_cursor_representation(&self) -> Result<(), Self::Error> {
        let mut cursor = self.cursor.lock().unwrap();
        if let Some(fb) = cursor.framebuffer.take() {
            let _ = self.destroy_framebuffer(fb);
        }

        self.clear_plane(self.planes.cursor)
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
        // okay, here we build the actual requests used by the surface.
        let mut req = AtomicModeReq::new();

        // requests consist out of a set of properties and their new values
        // for different drm objects (crtc, plane, connector, ...).

        // for every connector that is new, we need to set our crtc_id
        for conn in new_connectors {
            req.add_property(
                *conn,
                self.conn_prop_handle(*conn, "CRTC_ID")?,
                property::Value::CRTC(Some(self.crtc)),
            );
        }

        // for every connector that got removed, we need to set no crtc_id.
        // (this is a bit problematic, because this means we need to remove, commit, add, commit
        // in the right order to move a connector to another surface. otherwise we disable the
        // the connector here again...)
        for conn in removed_connectors {
            req.add_property(
                *conn,
                self.conn_prop_handle(*conn, "CRTC_ID")?,
                property::Value::CRTC(None),
            );
        }

        // we need to set the new mode, if there is one
        if let Some(blob) = blob {
            req.add_property(self.crtc, self.crtc_prop_handle(self.crtc, "MODE_ID")?, blob);
        }

        // we also need to set this crtc active
        req.add_property(
            self.crtc,
            self.crtc_prop_handle(self.crtc, "ACTIVE")?,
            property::Value::Boolean(true),
        );

        // and we need to set the framebuffer for our primary plane
        if let Some(fb) = framebuffer {
            req.add_property(
                planes.primary,
                self.plane_prop_handle(planes.primary, "FB_ID")?,
                property::Value::Framebuffer(Some(fb)),
            );
        }

        // if there is a new mode, we shoudl also make sure the plane is connected
        if let Some(mode) = mode {
            req.add_property(
                planes.primary,
                self.plane_prop_handle(planes.primary, "CRTC_ID")?,
                property::Value::CRTC(Some(self.crtc)),
            );
            // we can take different parts of a plane ...
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
                // these are 16.16. fixed point
                property::Value::UnsignedRange((mode.size().0 as u64) << 16),
            );
            req.add_property(
                planes.primary,
                self.plane_prop_handle(planes.primary, "SRC_H")?,
                property::Value::UnsignedRange((mode.size().1 as u64) << 16),
            );
            // .. onto different coordinated on the crtc, but we just use a 1:1 mapping.
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

        // if there is a cursor, we add the cursor plane to the request as well.
        // this synchronizes cursor movement with rendering, which reduces flickering.
        let mut cursor = self.cursor.lock().unwrap();
        if let (Some(pos), Some(fb)) = (cursor.position, cursor.framebuffer) {
            match self.get_framebuffer(fb).compat().map_err(|source| Error::Access {
                errmsg: "Error getting cursor fb",
                dev: self.dev_path(),
                source,
            }) {
                Ok(cursor_info) => {
                    let hotspot = cursor.hotspot;

                    // again like the primary plane we need to set crtc and framebuffer.
                    req.add_property(
                        planes.cursor,
                        self.plane_prop_handle(planes.cursor, "CRTC_ID")?,
                        property::Value::CRTC(Some(self.crtc)),
                    );
                    // copy the whole plane
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
                    // but this time add this at some very specific coordinates of the crtc
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
                    cursor.framebuffer = None;
                }
            }
        }

        Ok(req)
    }

    // primary and cursor planes are almost always unique to a crtc.
    // otherwise we would be in trouble and would need to figure this out
    // on the device level to find the best plane combination.
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

    // this helper function clears the contents of a single plane.
    // this is mostly used to remove the cursor, e.g. on tty switch,
    // as other compositors might not make use of other planes,
    // leaving our cursor as a relict of a better time on the screen.
    pub(super) fn clear_plane(&self, plane: plane::Handle) -> Result<(), Error> {
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
#[derive(Debug)]
pub struct AtomicDrmSurface<A: AsRawFd + 'static>(
    pub(in crate::backend::drm) Arc<AtomicDrmSurfaceInternal<A>>,
);

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

    fn clear_cursor_representation(&self) -> Result<(), Self::Error> {
        self.0.clear_cursor_representation()
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

#[cfg(test)]
mod test {
    use super::AtomicDrmSurface;
    use std::fs::File;

    fn is_send<S: Send>() {}

    #[test]
    fn surface_is_send() {
        is_send::<AtomicDrmSurface<File>>();
    }
}
