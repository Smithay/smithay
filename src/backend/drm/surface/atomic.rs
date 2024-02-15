use drm::control::atomic::AtomicModeReq;
use drm::control::Device as ControlDevice;
use drm::control::{
    connector, crtc, dumbbuffer::DumbBuffer, framebuffer, plane, property, AtomicCommitFlags, Mode, PlaneType,
};

use std::collections::HashSet;
use std::os::unix::io::AsRawFd;
use std::sync::Mutex;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, RwLock,
};

use crate::backend::drm::error::AccessError;
use crate::utils::{Coordinate, Point, Rectangle, Transform};
use crate::{
    backend::{
        allocator::format::{get_bpp, get_depth},
        drm::{
            device::atomic::{map_props, Mapping},
            device::DrmDeviceInternal,
            error::Error,
            plane_type, DrmDeviceFd,
        },
    },
    utils::DevPath,
};

use tracing::{debug, info, info_span, instrument, trace, warn};

use super::{PlaneConfig, PlaneState};

#[derive(Debug, Clone)]
pub struct State {
    pub active: bool,
    pub mode: Mode,
    pub blob: property::Value<'static>,
    pub connectors: HashSet<connector::Handle>,
}

impl PartialEq for State {
    fn eq(&self, other: &Self) -> bool {
        self.active == other.active && self.mode == other.mode && self.connectors == other.connectors
    }
}

impl State {
    fn current_state<A: DevPath + ControlDevice>(
        fd: &A,
        crtc: crtc::Handle,
        prop_mapping: &mut Mapping,
    ) -> Result<Self, Error> {
        let crtc_info = fd.get_crtc(crtc).map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Error loading crtc info",
                dev: fd.dev_path(),
                source,
            })
        })?;

        // If we have no current mode, we create a fake one, which will not match (and thus gets overridden on the commit below).
        // A better fix would probably be making mode an `Option`, but that would mean
        // we need to be sure, we require a mode to always be set without relying on the compiler.
        // So we cheat, because it works and is easier to handle later.
        let current_mode = crtc_info.mode().unwrap_or_else(|| unsafe { std::mem::zeroed() });
        let current_blob = match crtc_info.mode() {
            Some(mode) => fd.create_property_blob(&mode).map_err(|source| {
                Error::Access(AccessError {
                    errmsg: "Failed to create Property Blob for mode",
                    dev: fd.dev_path(),
                    source,
                })
            })?,
            None => property::Value::Unknown(0),
        };

        let res_handles = fd.resource_handles().map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Error loading drm resources",
                dev: fd.dev_path(),
                source,
            })
        })?;

        // the current set of connectors are those, that already have the correct `CRTC_ID` set.
        // so we collect them for `current_state` and set the user-given once in `pending_state`.
        //
        // If they don't match, `commit_pending` will return true and they will be changed on the next `commit`.
        let mut current_connectors = HashSet::new();
        // make sure the mapping is up to date
        map_props(fd, res_handles.connectors(), &mut prop_mapping.0)?;
        for conn in res_handles.connectors() {
            let crtc_prop = prop_mapping
                .0
                .get(conn)
                .expect("Unknown handle")
                .get("CRTC_ID")
                .ok_or_else(|| Error::UnknownProperty {
                    handle: (*conn).into(),
                    name: "CRTC_ID",
                })
                .copied()?;
            if let (Ok(crtc_prop_info), Ok(props)) = (fd.get_property(crtc_prop), fd.get_properties(*conn)) {
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

        // Get the current active (dpms) state of the CRTC
        //
        // Changing a CRTC to active might require a modeset
        let mut active = None;
        if let Ok(props) = fd.get_properties(crtc) {
            let active_prop = prop_mapping.1.get(&crtc).and_then(|m| m.get("ACTIVE"));
            let (ids, vals) = props.as_props_and_values();
            for (&id, &val) in ids.iter().zip(vals.iter()) {
                if Some(&id) == active_prop {
                    active = property::ValueType::Boolean.convert_value(val).as_boolean();
                    break;
                }
            }
        }

        Ok(State {
            // If we don't know the active state we just assume off.
            // This is highly unlikely, but having a false negative should do no harm.
            active: active.unwrap_or(false),
            mode: current_mode,
            blob: current_blob,
            connectors: current_connectors,
        })
    }

    fn clear(&mut self) {
        self.mode = unsafe { std::mem::zeroed() };
        self.blob = property::Value::Unknown(0);
        self.connectors.clear();
        self.active = false;
    }
}

#[derive(Debug)]
pub struct AtomicDrmSurface {
    pub(in crate::backend::drm) fd: Arc<DrmDeviceInternal>,
    pub(super) active: Arc<AtomicBool>,
    crtc: crtc::Handle,
    plane: plane::Handle,
    used_planes: Mutex<HashSet<plane::Handle>>,
    prop_mapping: RwLock<Mapping>,
    state: RwLock<State>,
    pending: RwLock<State>,
    pub(super) span: tracing::Span,
}

impl AtomicDrmSurface {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        fd: Arc<DrmDeviceInternal>,
        active: Arc<AtomicBool>,
        crtc: crtc::Handle,
        plane: plane::Handle,
        mut prop_mapping: Mapping,
        mode: Mode,
        connectors: &[connector::Handle],
    ) -> Result<Self, Error> {
        let span = info_span!("drm_atomic", crtc = ?crtc);
        let _guard = span.enter();
        info!(
            "Initializing drm surface ({:?}:{:?}) with mode {:?} and connectors {:?}",
            crtc, plane, mode, connectors
        );

        let state = State::current_state(&*fd, crtc, &mut prop_mapping)?;
        let blob = fd.create_property_blob(&mode).map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Failed to create Property Blob for mode",
                dev: fd.dev_path(),
                source,
            })
        })?;
        let pending = State {
            active: true,
            mode,
            blob,
            connectors: connectors.iter().copied().collect(),
        };

        drop(_guard);
        let surface = AtomicDrmSurface {
            fd,
            active,
            crtc,
            plane,
            used_planes: Mutex::new(HashSet::new()),
            prop_mapping: RwLock::new(prop_mapping),
            state: RwLock::new(state),
            pending: RwLock::new(pending),
            span,
        };

        Ok(surface)
    }

    // we need a framebuffer to do test commits, which we use to verify our pending state.
    // here we create a dumbbuffer for that purpose.
    #[profiling::function]
    fn create_test_buffer(&self, size: (u16, u16), plane: plane::Handle) -> Result<TestBuffer, Error> {
        let (w, h) = size;
        let needs_alpha = plane_type(&*self.fd, plane)? != PlaneType::Primary;
        let format = if needs_alpha {
            crate::backend::allocator::Fourcc::Argb8888
        } else {
            crate::backend::allocator::Fourcc::Xrgb8888
        };

        let db = self
            .fd
            .create_dumb_buffer((w as u32, h as u32), format, get_bpp(format).unwrap() as u32)
            .map_err(|source| {
                Error::Access(AccessError {
                    errmsg: "Failed to create dumb buffer",
                    dev: self.fd.dev_path(),
                    source,
                })
            })?;
        let fb_result = self
            .fd
            .add_framebuffer(
                &db,
                get_depth(format).unwrap() as u32,
                get_bpp(format).unwrap() as u32,
            )
            .map_err(|source| {
                Error::Access(AccessError {
                    errmsg: "Failed to create framebuffer",
                    dev: self.fd.dev_path(),
                    source,
                })
            });

        match fb_result {
            Ok(fb) => Ok(TestBuffer {
                fd: self.fd.clone(),
                db,
                fb,
            }),
            Err(err) => {
                let _ = self.fd.destroy_dumb_buffer(db);
                Err(err)
            }
        }
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

    fn ensure_props_known(&self, conns: &[connector::Handle]) -> Result<(), Error> {
        let mapping_exists = {
            let prop_mapping = self.prop_mapping.read().unwrap();
            conns.iter().all(|conn| prop_mapping.0.get(conn).is_some())
        };
        if !mapping_exists {
            map_props(
                &*self.fd,
                self.fd
                    .resource_handles()
                    .map_err(|source| {
                        Error::Access(AccessError {
                            errmsg: "Error loading connector info",
                            dev: self.fd.dev_path(),
                            source,
                        })
                    })?
                    .connectors(),
                &mut self.prop_mapping.write().unwrap().0,
            )?;
        }
        Ok(())
    }

    #[instrument(parent = &self.span, skip(self))]
    pub fn add_connector(&self, conn: connector::Handle) -> Result<(), Error> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        self.ensure_props_known(&[conn])?;
        let info = self.fd.get_connector(conn, false).map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Error loading connector info",
                dev: self.fd.dev_path(),
                source,
            })
        })?;

        let mut pending = self.pending.write().unwrap();

        // check if the connector can handle the current mode
        if info.modes().contains(&pending.mode) {
            let test_buffer = self.create_test_buffer(pending.mode.size(), self.plane)?;

            // check if config is supported
            let req = self.build_request(
                &mut [conn].iter(),
                &mut [].iter(),
                [&PlaneState {
                    handle: self.plane,
                    config: Some(PlaneConfig {
                        src: Rectangle::from_loc_and_size(Point::default(), pending.mode.size()).to_f64(),
                        dst: Rectangle::from_loc_and_size(
                            Point::default(),
                            (pending.mode.size().0 as i32, pending.mode.size().1 as i32),
                        ),
                        transform: Transform::Normal,
                        alpha: 1.0,
                        damage_clips: None,
                        fb: test_buffer.fb,
                        fence: None,
                    }),
                }],
                Some(pending.blob),
            )?;
            self.fd
                .atomic_commit(
                    AtomicCommitFlags::ALLOW_MODESET | AtomicCommitFlags::TEST_ONLY,
                    req,
                )
                .map_err(|_| Error::TestFailed(self.crtc))?;

            // seems to be, lets add the connector
            pending.connectors.insert(conn);

            Ok(())
        } else {
            Err(Error::ModeNotSuitable(pending.mode))
        }
    }

    #[instrument(parent = &self.span, skip(self))]
    pub fn remove_connector(&self, conn: connector::Handle) -> Result<(), Error> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let mut pending = self.pending.write().unwrap();

        // the test would also prevent this, but the error message is far less helpful
        if pending.connectors.contains(&conn) && pending.connectors.len() == 1 {
            return Err(Error::SurfaceWithoutConnectors(self.crtc));
        }

        // check if new config is supported (should be)
        let test_buffer = self.create_test_buffer(pending.mode.size(), self.plane)?;

        let req = self.build_request(
            &mut [].iter(),
            &mut [conn].iter(),
            [&PlaneState {
                handle: self.plane,
                config: Some(PlaneConfig {
                    src: Rectangle::from_loc_and_size(Point::default(), pending.mode.size()).to_f64(),
                    dst: Rectangle::from_loc_and_size(
                        Point::default(),
                        (pending.mode.size().0 as i32, pending.mode.size().1 as i32),
                    ),
                    transform: Transform::Normal,
                    alpha: 1.0,
                    damage_clips: None,
                    fb: test_buffer.fb,
                    fence: None,
                }),
            }],
            Some(pending.blob),
        )?;
        self.fd
            .atomic_commit(
                AtomicCommitFlags::ALLOW_MODESET | AtomicCommitFlags::TEST_ONLY,
                req,
            )
            .map_err(|_| Error::TestFailed(self.crtc))?;

        // seems to be, lets remove the connector
        pending.connectors.remove(&conn);

        Ok(())
    }

    #[instrument(parent = &self.span, skip(self))]
    pub fn set_connectors(&self, connectors: &[connector::Handle]) -> Result<(), Error> {
        // the test would also prevent this, but the error message is far less helpful
        if connectors.is_empty() {
            return Err(Error::SurfaceWithoutConnectors(self.crtc));
        }

        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let current = self.state.write().unwrap();
        let mut pending = self.pending.write().unwrap();

        self.ensure_props_known(connectors)?;
        let conns = connectors.iter().cloned().collect::<HashSet<_>>();
        let mut added = conns.difference(&current.connectors);
        let mut removed = current.connectors.difference(&conns);

        let test_buffer = self.create_test_buffer(pending.mode.size(), self.plane)?;

        let req = self.build_request(
            &mut added,
            &mut removed,
            [&PlaneState {
                handle: self.plane,
                config: Some(PlaneConfig {
                    src: Rectangle::from_loc_and_size(Point::default(), pending.mode.size()).to_f64(),
                    dst: Rectangle::from_loc_and_size(
                        Point::default(),
                        (pending.mode.size().0 as i32, pending.mode.size().1 as i32),
                    ),
                    transform: Transform::Normal,
                    alpha: 1.0,
                    damage_clips: None,
                    fb: test_buffer.fb,
                    fence: None,
                }),
            }],
            Some(pending.blob),
        )?;

        self.fd
            .atomic_commit(
                AtomicCommitFlags::ALLOW_MODESET | AtomicCommitFlags::TEST_ONLY,
                req,
            )
            .map_err(|_| Error::TestFailed(self.crtc))?;

        pending.connectors = conns;

        Ok(())
    }

    #[instrument(level = "debug", parent = &self.span, skip(self))]
    pub fn use_mode(&self, mode: Mode) -> Result<(), Error> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let mut pending = self.pending.write().unwrap();

        // check if new config is supported
        let new_blob = self.fd.create_property_blob(&mode).map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Failed to create Property Blob for mode",
                dev: self.fd.dev_path(),
                source,
            })
        })?;

        let test_buffer = self.create_test_buffer(mode.size(), self.plane)?;

        let req = self.build_request(
            &mut pending.connectors.iter(),
            &mut [].iter(),
            [&PlaneState {
                handle: self.plane,
                config: Some(PlaneConfig {
                    src: Rectangle::from_loc_and_size(Point::default(), mode.size()).to_f64(),
                    dst: Rectangle::from_loc_and_size(
                        Point::default(),
                        (mode.size().0 as i32, mode.size().1 as i32),
                    ),
                    transform: Transform::Normal,
                    alpha: 1.0,
                    damage_clips: None,
                    fb: test_buffer.fb,
                    fence: None,
                }),
            }],
            Some(new_blob),
        )?;
        if let Err(err) = self
            .fd
            .atomic_commit(
                AtomicCommitFlags::ALLOW_MODESET | AtomicCommitFlags::TEST_ONLY,
                req,
            )
            .map_err(|_| Error::TestFailed(self.crtc))
        {
            let _ = self.fd.destroy_property_blob(new_blob.into());
            return Err(err);
        }

        // seems to be, lets change the mode
        pending.mode = mode;
        pending.blob = new_blob;

        Ok(())
    }

    pub fn commit_pending(&self) -> bool {
        *self.pending.read().unwrap() != *self.state.read().unwrap()
    }

    #[instrument(level = "trace", parent = &self.span, skip(self, planes))]
    #[profiling::function]
    pub fn test_state<'a>(
        &self,
        planes: impl IntoIterator<Item = PlaneState<'a>>,
        allow_modeset: bool,
    ) -> Result<(), Error> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let planes = planes.into_iter().collect::<Vec<_>>();

        let current = self.state.read().unwrap();
        let pending = self.pending.read().unwrap();

        let current_conns = current.connectors.clone();
        let pending_conns = pending.connectors.clone();
        let mut removed = current_conns.difference(&pending_conns);
        let mut added = pending_conns.difference(&current_conns);

        let req = self.build_request(&mut added, &mut removed, &*planes, Some(pending.blob))?;

        let flags = if allow_modeset {
            AtomicCommitFlags::ALLOW_MODESET | AtomicCommitFlags::TEST_ONLY
        } else {
            AtomicCommitFlags::TEST_ONLY
        };
        self.fd.atomic_commit(flags, req).map_err(|source| {
            Error::Access(AccessError {
                errmsg: "Error testing state",
                dev: self.fd.dev_path(),
                source,
            })
        })
    }

    #[instrument(level = "trace", parent = &self.span, skip(self, planes))]
    #[profiling::function]
    pub fn commit<'a>(
        &self,
        planes: impl IntoIterator<Item = PlaneState<'a>>,
        event: bool,
    ) -> Result<(), Error> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let planes = planes.into_iter().collect::<Vec<_>>();
        let mut current = self.state.write().unwrap();
        let mut used_planes = self.used_planes.lock().unwrap();
        let pending = self.pending.write().unwrap();

        debug!(current = ?*current, pending = ?*pending, ?planes, "Preparing Commit",);

        // we need the differences to know, which connectors need to change properties
        let current_conns = current.connectors.clone();
        let pending_conns = pending.connectors.clone();
        let mut removed = current_conns.difference(&pending_conns);
        let mut added = pending_conns.difference(&current_conns);

        for conn in removed.clone() {
            if let Ok(info) = self.fd.get_connector(*conn, false) {
                info!("Removing connector: {:?}", info.interface());
            } else {
                info!("Removing unknown connector");
            }
        }

        for conn in added.clone() {
            if let Ok(info) = self.fd.get_connector(*conn, false) {
                info!("Adding connector: {:?}", info.interface());
            } else {
                info!("Adding unknown connector");
            }
        }

        if current.mode != pending.mode {
            info!("Setting new mode: {:?}", pending.mode.name());
        }

        trace!("Testing screen config");

        // test the new config and return the request if it would be accepted by the driver.
        let req = {
            let req = self.build_request(&mut added, &mut removed, &*planes, Some(pending.blob))?;

            if let Err(err) = self.fd.atomic_commit(
                AtomicCommitFlags::ALLOW_MODESET | AtomicCommitFlags::TEST_ONLY,
                req.clone(),
            ) {
                warn!("New screen configuration invalid!:\n\t{:#?}\n\t{}\n", req, err);

                return Err(Error::TestFailed(self.crtc));
            } else {
                if current.mode != pending.mode {
                    if let Err(err) = self.fd.destroy_property_blob(current.blob.into()) {
                        warn!("Failed to destroy old mode property blob: {}", err);
                    }
                }

                // new config
                req
            }
        };

        debug!("Setting screen: {:?}", req);
        let result = self
            .fd
            .atomic_commit(
                if event {
                    // on the atomic api we can modeset and trigger a page_flip event on the same call!
                    AtomicCommitFlags::PAGE_FLIP_EVENT | AtomicCommitFlags::ALLOW_MODESET
                    // we also *should* not need to wait for completion, like with `set_crtc`,
                    // because we have tested this exact commit already, so we do not expect any errors later down the line.
                    //
                    // but there is always an exception and `amdgpu` can fail in interesting ways with this flag set...
                    // https://gitlab.freedesktop.org/drm/amd/-/issues?scope=all&utf8=%E2%9C%93&state=opened&search=drm_atomic_helper_wait_for_flip_done
                    //
                    // so we skip this flag:
                    // AtomicCommitFlags::Nonblock,
                } else {
                    AtomicCommitFlags::ALLOW_MODESET
                },
                req,
            )
            .map_err(|source| {
                Error::Access(AccessError {
                    errmsg: "Error setting crtc",
                    dev: self.fd.dev_path(),
                    source,
                })
            });

        if result.is_ok() {
            *current = pending.clone();
            for plane in planes.iter() {
                if plane.config.is_some() {
                    used_planes.insert(plane.handle);
                } else {
                    used_planes.remove(&plane.handle);
                }
            }
        }

        result
    }

    #[instrument(level = "trace", parent = &self.span, skip(self, planes))]
    #[profiling::function]
    pub fn page_flip<'a>(
        &self,
        planes: impl IntoIterator<Item = PlaneState<'a>>,
        event: bool,
    ) -> Result<(), Error> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let mut used_planes = self.used_planes.lock().unwrap();
        let planes = planes.into_iter().collect::<Vec<_>>();

        // page flips work just like commits with fewer parameters..
        let req = self.build_request(&mut [].iter(), &mut [].iter(), &*planes, None)?;

        // .. and without `AtomicCommitFlags::AllowModeset`.
        // If we would set anything here, that would require a modeset, this would fail,
        // indicating a problem in our assumptions.
        trace!(?planes, "Queueing page flip: {:?}", req);
        let res = self
            .fd
            .atomic_commit(
                if event {
                    AtomicCommitFlags::PAGE_FLIP_EVENT | AtomicCommitFlags::NONBLOCK
                } else {
                    AtomicCommitFlags::NONBLOCK
                },
                req,
            )
            .map_err(|source| {
                Error::Access(AccessError {
                    errmsg: "Page flip commit failed",
                    dev: self.fd.dev_path(),
                    source,
                })
            });

        if res.is_ok() {
            for plane in planes.iter() {
                if plane.config.is_some() {
                    used_planes.insert(plane.handle);
                } else {
                    used_planes.remove(&plane.handle);
                }
            }
        }

        res
    }

    // If a mode is set a matching blob needs to be set (the inverse is not true)
    #[allow(clippy::too_many_arguments)]
    #[profiling::function]
    pub fn build_request<'a>(
        &self,
        new_connectors: &mut dyn Iterator<Item = &connector::Handle>,
        removed_connectors: &mut dyn Iterator<Item = &connector::Handle>,
        planes: impl IntoIterator<Item = &'a PlaneState<'a>>,
        blob: Option<property::Value<'static>>,
    ) -> Result<AtomicModeReq, Error> {
        let prop_mapping = self.prop_mapping.read().unwrap();

        // okay, here we build the actual requests used by the surface.
        let mut req = AtomicModeReq::new();

        // requests consist out of a set of properties and their new values
        // for different drm objects (crtc, plane, connector, ...).

        // for every connector that is new, we need to set our crtc_id
        for conn in new_connectors {
            req.add_property(
                *conn,
                conn_prop_handle(&prop_mapping, *conn, "CRTC_ID")?,
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
                conn_prop_handle(&prop_mapping, *conn, "CRTC_ID")?,
                property::Value::CRTC(None),
            );
        }

        // we need to set the new mode, if there is one
        if let Some(blob) = blob {
            req.add_property(
                self.crtc,
                crtc_prop_handle(&prop_mapping, self.crtc, "MODE_ID")?,
                blob,
            );
        }

        // we also need to set this crtc active
        req.add_property(
            self.crtc,
            crtc_prop_handle(&prop_mapping, self.crtc, "ACTIVE")?,
            property::Value::Boolean(true),
        );

        for plane_state in planes.into_iter() {
            let handle = &plane_state.handle;

            if let Some(config) = plane_state.config.as_ref() {
                // connect the plane to the CRTC
                req.add_property(
                    *handle,
                    plane_prop_handle(&prop_mapping, *handle, "CRTC_ID")?,
                    property::Value::CRTC(Some(self.crtc)),
                );

                // Set the fb for the plane
                req.add_property(
                    *handle,
                    plane_prop_handle(&prop_mapping, *handle, "FB_ID")?,
                    property::Value::Framebuffer(Some(config.fb)),
                );

                req.add_property(
                    *handle,
                    plane_prop_handle(&prop_mapping, *handle, "SRC_X")?,
                    // these are 16.16. fixed point
                    property::Value::UnsignedRange(to_fixed(config.src.loc.x) as u64),
                );
                req.add_property(
                    *handle,
                    plane_prop_handle(&prop_mapping, *handle, "SRC_Y")?,
                    // these are 16.16. fixed point
                    property::Value::UnsignedRange(to_fixed(config.src.loc.y) as u64),
                );
                req.add_property(
                    *handle,
                    plane_prop_handle(&prop_mapping, *handle, "SRC_W")?,
                    // these are 16.16. fixed point
                    property::Value::UnsignedRange(to_fixed(config.src.size.w) as u64),
                );
                req.add_property(
                    *handle,
                    plane_prop_handle(&prop_mapping, *handle, "SRC_H")?,
                    // these are 16.16. fixed point
                    property::Value::UnsignedRange(to_fixed(config.src.size.h) as u64),
                );

                req.add_property(
                    *handle,
                    plane_prop_handle(&prop_mapping, *handle, "CRTC_X")?,
                    property::Value::SignedRange(config.dst.loc.x as i64),
                );
                req.add_property(
                    *handle,
                    plane_prop_handle(&prop_mapping, *handle, "CRTC_Y")?,
                    property::Value::SignedRange(config.dst.loc.y as i64),
                );
                req.add_property(
                    *handle,
                    plane_prop_handle(&prop_mapping, *handle, "CRTC_W")?,
                    property::Value::UnsignedRange(config.dst.size.w as u64),
                );
                req.add_property(
                    *handle,
                    plane_prop_handle(&prop_mapping, *handle, "CRTC_H")?,
                    property::Value::UnsignedRange(config.dst.size.h as u64),
                );
                if let Ok(prop) = plane_prop_handle(&prop_mapping, *handle, "rotation") {
                    req.add_property(
                        *handle,
                        prop,
                        property::Value::Bitmask(DrmRotation::from(config.transform).bits() as u64),
                    );
                } else if config.transform != Transform::Normal {
                    // if we are missing the rotation property we can no rely on
                    // the driver to report a non working configuration and can
                    // only guarantee that Transform::Normal (no rotation) will
                    // work
                    return Err(Error::UnknownProperty {
                        handle: (*handle).into(),
                        name: "rotation",
                    });
                }
                if let Ok(prop) = plane_prop_handle(&prop_mapping, *handle, "alpha") {
                    req.add_property(
                        *handle,
                        prop,
                        property::Value::UnsignedRange((config.alpha * u16::MAX as f32).round() as u64),
                    );
                } else if config.alpha != 1.0 {
                    // if we are missing the alpha property we can not display any transparent alpha values
                    return Err(Error::UnknownProperty {
                        handle: (*handle).into(),
                        name: "alpha",
                    });
                }
                if let Ok(prop) = plane_prop_handle(&prop_mapping, *handle, "FB_DAMAGE_CLIPS") {
                    if let Some(damage) = config.damage_clips.as_ref() {
                        req.add_property(*handle, prop, *damage);
                    } else {
                        req.add_property(*handle, prop, property::Value::Blob(0));
                    }
                }
                if let Ok(prop) = plane_prop_handle(&prop_mapping, *handle, "IN_FENCE_FD") {
                    if let Some(fence) = config.fence.as_ref().map(|f| f.as_raw_fd()) {
                        req.add_property(*handle, prop, property::Value::SignedRange(fence as i64));
                    } else {
                        req.add_property(*handle, prop, property::Value::SignedRange(-1));
                    }
                } else if config.fence.is_some() {
                    return Err(Error::UnknownProperty {
                        handle: (*handle).into(),
                        name: "IN_FENCE_FD",
                    });
                }
            } else {
                self.append_reset_plane_state(&mut req, *handle)?;
            }
        }

        Ok(req)
    }

    // this helper function disconnects the plane.
    // this is mostly used to remove the contents quickly, e.g. on tty switch,
    // as other compositors might not make use of other planes,
    // leaving our e.g. cursor or overlays as a relict of a better time on the screen.
    pub fn clear_plane(&self, plane: plane::Handle) -> Result<(), Error> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let mut req = AtomicModeReq::new();
        self.append_reset_plane_state(&mut req, plane)?;

        let result = self
            .fd
            .atomic_commit(AtomicCommitFlags::empty(), req)
            .map_err(|source| {
                Error::Access(AccessError {
                    errmsg: "Failed to commit on clear_plane",
                    dev: self.fd.dev_path(),
                    source,
                })
            });

        if result.is_ok() {
            self.used_planes.lock().unwrap().remove(&plane);
        }

        result
    }

    #[profiling::function]
    fn clear_state(&self) -> Result<(), Error> {
        if !self.active.load(Ordering::SeqCst) {
            return Err(Error::DeviceInactive);
        }

        let _guard = self.span.enter();
        let mut req = AtomicModeReq::new();
        // reset all planes we used
        for plane in self.used_planes.lock().unwrap().iter() {
            self.append_reset_plane_state(&mut req, *plane)?;
        }

        // disable connectors again
        let current = self.state.read().unwrap();
        let prop_mapping = self.prop_mapping.read().unwrap();
        for conn in current.connectors.iter() {
            let prop = prop_mapping
                .0
                .get(conn)
                .expect("Unknown Handle")
                .get("CRTC_ID")
                .expect("Unknown property CRTC_ID");
            req.add_property(*conn, *prop, property::Value::CRTC(None));
        }
        let active_prop = prop_mapping
            .1
            .get(&self.crtc)
            .expect("Unknown Handle")
            .get("ACTIVE")
            .expect("Unknown property ACTIVE");
        let mode_prop = prop_mapping
            .1
            .get(&self.crtc)
            .expect("Unknown Handle")
            .get("MODE_ID")
            .expect("Unknown property MODE_ID");

        req.add_property(self.crtc, *active_prop, property::Value::Boolean(false));
        req.add_property(self.crtc, *mode_prop, property::Value::Unknown(0));
        std::mem::drop(current);

        let res = self
            .fd
            .atomic_commit(AtomicCommitFlags::ALLOW_MODESET, req)
            .map_err(|source| {
                Error::Access(AccessError {
                    errmsg: "Failed to commit on clear_state",
                    dev: self.fd.dev_path(),
                    source,
                })
            });

        if res.is_ok() {
            self.used_planes.lock().unwrap().clear();
            self.state.write().unwrap().clear();
        }

        res
    }

    fn append_reset_plane_state(&self, req: &mut AtomicModeReq, plane: plane::Handle) -> Result<(), Error> {
        let prop_mapping = self.prop_mapping.read().unwrap();

        req.add_property(
            plane,
            plane_prop_handle(&prop_mapping, plane, "CRTC_ID")?,
            property::Value::CRTC(None),
        );

        req.add_property(
            plane,
            plane_prop_handle(&prop_mapping, plane, "FB_ID")?,
            property::Value::Framebuffer(None),
        );

        // reset the plane properties
        req.add_property(
            plane,
            plane_prop_handle(&prop_mapping, plane, "SRC_X")?,
            // these are 16.16. fixed point
            property::Value::UnsignedRange(0u64),
        );
        req.add_property(
            plane,
            plane_prop_handle(&prop_mapping, plane, "SRC_Y")?,
            // these are 16.16. fixed point
            property::Value::UnsignedRange(0u64),
        );
        req.add_property(
            plane,
            plane_prop_handle(&prop_mapping, plane, "SRC_W")?,
            // these are 16.16. fixed point
            property::Value::UnsignedRange(0u64),
        );
        req.add_property(
            plane,
            plane_prop_handle(&prop_mapping, plane, "SRC_H")?,
            // these are 16.16. fixed point
            property::Value::UnsignedRange(0u64),
        );

        req.add_property(
            plane,
            plane_prop_handle(&prop_mapping, plane, "CRTC_X")?,
            property::Value::SignedRange(0i64),
        );
        req.add_property(
            plane,
            plane_prop_handle(&prop_mapping, plane, "CRTC_Y")?,
            property::Value::SignedRange(0i64),
        );
        req.add_property(
            plane,
            plane_prop_handle(&prop_mapping, plane, "CRTC_W")?,
            property::Value::UnsignedRange(0u64),
        );
        req.add_property(
            plane,
            plane_prop_handle(&prop_mapping, plane, "CRTC_H")?,
            property::Value::UnsignedRange(0u64),
        );
        if let Ok(prop) = plane_prop_handle(&prop_mapping, plane, "rotation") {
            req.add_property(
                plane,
                prop,
                property::Value::Bitmask(DrmRotation::from(Transform::Normal).bits() as u64),
            );
        }
        if let Ok(prop) = plane_prop_handle(&prop_mapping, plane, "alpha") {
            req.add_property(plane, prop, property::Value::UnsignedRange(0xffff));
        }
        if let Ok(prop) = plane_prop_handle(&prop_mapping, plane, "FB_DAMAGE_CLIPS") {
            req.add_property(plane, prop, property::Value::Blob(0));
        }
        if let Ok(prop) = plane_prop_handle(&prop_mapping, plane, "IN_FENCE_FD") {
            req.add_property(plane, prop, property::Value::SignedRange(-1));
        }

        Ok(())
    }

    pub(crate) fn reset_state<B: DevPath + ControlDevice + 'static>(
        &self,
        fd: Option<&B>,
    ) -> Result<(), Error> {
        *self.state.write().unwrap() = if let Some(fd) = fd {
            State::current_state(fd, self.crtc, &mut self.prop_mapping.write().unwrap())?
        } else {
            State::current_state(&*self.fd, self.crtc, &mut self.prop_mapping.write().unwrap())?
        };
        Ok(())
    }

    pub(crate) fn device_fd(&self) -> &DrmDeviceFd {
        self.fd.device_fd()
    }
}

struct TestBuffer {
    fd: Arc<DrmDeviceInternal>,
    db: DumbBuffer,
    fb: framebuffer::Handle,
}

impl AsRef<framebuffer::Handle> for TestBuffer {
    fn as_ref(&self) -> &framebuffer::Handle {
        &self.fb
    }
}

impl Drop for TestBuffer {
    fn drop(&mut self) {
        let _ = self.fd.destroy_framebuffer(self.fb);
        let _ = self.fd.destroy_dumb_buffer(self.db);
    }
}

impl Drop for AtomicDrmSurface {
    fn drop(&mut self) {
        if !self.active.load(Ordering::SeqCst) {
            // the device is gone or we are on another tty
            // old state has been restored, we shouldn't touch it.
            // if we are on another tty the connectors will get disabled
            // by the device, when switching back
            return;
        }

        let _guard = self.span.enter();
        if let Err(err) = self.clear_state() {
            warn!("Unable to clear state: {}", err);
        }
    }
}

pub(crate) fn conn_prop_handle(
    prop_mapping: &Mapping,
    handle: connector::Handle,
    name: &'static str,
) -> Result<property::Handle, Error> {
    prop_mapping
        .0
        .get(&handle)
        .expect("Unknown handle")
        .get(name)
        .ok_or_else(|| Error::UnknownProperty {
            handle: handle.into(),
            name,
        })
        .copied()
}

pub(crate) fn crtc_prop_handle(
    prop_mapping: &Mapping,
    handle: crtc::Handle,
    name: &'static str,
) -> Result<property::Handle, Error> {
    prop_mapping
        .1
        .get(&handle)
        .expect("Unknown handle")
        .get(name)
        .ok_or_else(|| Error::UnknownProperty {
            handle: handle.into(),
            name,
        })
        .copied()
}

pub(crate) fn plane_prop_handle(
    prop_mapping: &Mapping,
    handle: plane::Handle,
    name: &'static str,
) -> Result<property::Handle, Error> {
    prop_mapping
        .2
        .get(&handle)
        .expect("Unknown handle")
        .get(name)
        .ok_or_else(|| Error::UnknownProperty {
            handle: handle.into(),
            name,
        })
        .copied()
}

#[inline]
fn to_fixed<N: Coordinate>(n: N) -> u32 {
    f64::round(n.to_f64() * (1 << 16) as f64) as u32
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
    struct DrmRotation: u8 {
        const ROTATE_0      =   0b00000001;
        const ROTATE_90     =   0b00000010;
        const ROTATE_180    =   0b00000100;
        const ROTATE_270    =   0b00001000;
        const REFLECT_X     =   0b00010000;
        const REFLECT_Y     =   0b00100000;
    }
}

impl From<Transform> for DrmRotation {
    fn from(transform: Transform) -> Self {
        match transform {
            Transform::Normal => DrmRotation::ROTATE_0,
            Transform::_90 => DrmRotation::ROTATE_90,
            Transform::_180 => DrmRotation::ROTATE_180,
            Transform::_270 => DrmRotation::ROTATE_270,
            Transform::Flipped => DrmRotation::REFLECT_Y,
            Transform::Flipped90 => DrmRotation::REFLECT_Y | DrmRotation::ROTATE_90,
            Transform::Flipped180 => DrmRotation::REFLECT_Y | DrmRotation::ROTATE_180,
            Transform::Flipped270 => DrmRotation::REFLECT_Y | DrmRotation::ROTATE_270,
        }
    }
}

#[cfg(test)]
mod test {
    use crate::{
        backend::drm::surface::atomic::to_fixed,
        utils::{Physical, Rectangle},
    };

    use super::AtomicDrmSurface;

    fn is_send<S: Send>() {}

    #[test]
    fn surface_is_send() {
        is_send::<AtomicDrmSurface>();
    }

    #[test]
    fn test_fixed_point() {
        let geometry: Rectangle<f64, Physical> = Rectangle::from_loc_and_size((0.0, 0.0), (1920.0, 1080.0));
        let fixed = to_fixed(geometry.size.w) as u64;
        assert_eq!(125829120, fixed);
    }

    #[test]
    fn test_fractional_fixed_point() {
        let geometry: Rectangle<f64, Physical> = Rectangle::from_loc_and_size((0.0, 0.0), (1920.1, 1080.0));
        let fixed = to_fixed(geometry.size.w) as u64;
        assert_eq!(125835674, fixed);
    }
}
