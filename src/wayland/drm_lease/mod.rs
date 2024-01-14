//! DRM Lease protocol
//!
//! This module provides helpers to handle the wp_drm_lease_v1 protocol,
//! which allows clients to lease DRM resources from the compositor.
//!
//! This is in particular useful for VR applications, that would like to take over
//! a directly attached VR display from the DRM device otherwise controlled by the compositor.
//! In theory however any sorts of DRM resources can be lend out to less privileged clients via this protocol.
//!
//! ## How to use
//!
//! To setup the drm_lease global, you will need to first provide the `DrmNode` you want to lease resources from.
//! You can usually get that from your [`DrmDevice`]. Once the global is up,
//! you can advertise connectors as available through [`DrmLeaseState::add_connector`].
//! Should a connector become unavailable or is used by the compositor, you may remove it again using [`DrmLeaseState::withdraw_connector`].
//!
//! Any client requests will be issued through [`DrmLeaseHandler::lease_request`], which
//! allows you to add additional needed DRM resources to the lease and accept or decline the request.
//!
//! ```no_run
//! # use smithay::delegate_drm_lease;
//! # use smithay::wayland::drm_lease::*;
//! # use smithay::backend::drm::{DrmDevice, DrmNode};
//!
//! pub struct State {
//!     drm_lease_state: DrmLeaseState,
//!     active_leases: Vec<DrmLease>,
//! }
//!
//! # impl State {
//! #     fn get_device_for_node(&self, node: &DrmNode) -> &DrmDevice { todo!() }
//! # }
//!
//! impl DrmLeaseHandler for State {
//!   fn drm_lease_state(&mut self, node: DrmNode) -> &mut DrmLeaseState { &mut self.drm_lease_state }
//!   fn lease_request(&mut self, node: DrmNode, request: DrmLeaseRequest) -> Result<DrmLeaseBuilder, LeaseRejected> {
//!      let device = self.get_device_for_node(&node);
//!      let mut builder = DrmLeaseBuilder::new(device);
//!      for connector in request.connectors {
//!         builder.add_connector(connector);
//! #       let some_free_crtc = todo!();
//! #       let primary_plane_for_connector = todo!();
//!         builder.add_crtc(some_free_crtc);
//!         builder.add_plane(primary_plane_for_connector);
//!      }
//!      Ok(builder)
//!   }
//!   fn new_active_lease(&mut self, node: DrmNode, lease: DrmLease) { self.active_leases.push(lease); }
//!   fn lease_destroyed(&mut self, node: DrmNode, lease_id: u32) { self.active_leases.retain(|l| l.id() != lease_id); }
//! }
//!
//! delegate_drm_lease!(State);
//!
//! # let mut display = wayland_server::Display::<State>::new().unwrap();
//! # let display_handle = display.handle();
//! # let drm_device: DrmDevice = todo!();
//! let mut drm_lease_state = DrmLeaseState::new::<State>(
//!     &display.handle(),
//!     &DrmNode::from_file(
//!         drm_device.device_fd()
//!     ).unwrap()
//! ).unwrap();
//!
//! // Add some connectors
//! # let some_connector = todo!();
//! drm_lease_state.add_connector::<State>(some_connector, "Unknown".into(), "Unknown".into());
//!
//! let state = State {
//!    drm_lease_state,
//!    active_leases: Vec::new(),
//! };
//!
//! // Rest of the compositor goes here
//! ```

use std::{
    collections::HashSet,
    fmt, io,
    num::NonZeroU32,
    os::unix::{io::OwnedFd, prelude::AsFd},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
};

use drm::control::{connector, crtc, plane, Device, RawResourceHandle};
use rustix::fs::OFlags;
use wayland_protocols::wp::drm_lease::v1::server::*;
use wayland_server::backend::GlobalId;
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use crate::backend::drm::{DrmDevice, DrmDeviceFd, DrmNode, NodeType};

/// Delegate type for a drm_lease global
#[derive(Debug)]
pub struct DrmLeaseState {
    node: DrmNode,
    dh: DisplayHandle,
    global: Option<GlobalId>,
    connectors: Vec<DrmLeaseConnector>,
    known_lease_devices: Vec<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1>,
    active_leases: Vec<DrmLeaseRef>,
}

#[derive(Debug)]
struct DrmLeaseConnector {
    name: String,
    description: String,
    node: DrmNode,
    handle: connector::Handle,
    enabled: bool,
    known_instances: Vec<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1>,
}

/// Data attached to wp_drm_lease_request_v1 objects
#[derive(Debug)]
pub struct DrmLeaseRequestData {
    node: DrmNode,
    connectors: Mutex<Vec<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1>>,
}

/// DRM lease request containing a set of requested connectors
#[derive(Debug)]
pub struct DrmLeaseRequest {
    /// requested connectors
    pub connectors: Vec<connector::Handle>,
}

/// Builder struct to collect DRM resources to be leased
#[derive(Debug)]
pub struct DrmLeaseBuilder {
    drm: DrmDeviceFd,
    planes: HashSet<plane::Handle>,
    connectors: HashSet<connector::Handle>,
    crtcs: HashSet<crtc::Handle>,
}

impl DrmLeaseBuilder {
    /// Create a new builder from a DRM Device
    pub fn new(drm: &DrmDevice) -> DrmLeaseBuilder {
        DrmLeaseBuilder {
            drm: drm.device_fd().clone(),
            planes: HashSet::new(),
            connectors: HashSet::new(),
            crtcs: HashSet::new(),
        }
    }

    /// Add a CRTC to the to be leased resources
    pub fn add_crtc(&mut self, crtc: crtc::Handle) {
        self.crtcs.insert(crtc);
    }

    /// Add a connector to the to be leased resources
    pub fn add_connector(&mut self, conn: connector::Handle) {
        self.connectors.insert(conn);
    }

    /// Add a plane to the to be leased resources
    pub fn add_plane(&mut self, plane: plane::Handle) {
        self.planes.insert(plane);
    }

    fn build(self) -> io::Result<DrmLease> {
        let objects: Vec<RawResourceHandle> = self
            .planes
            .iter()
            .cloned()
            .map(Into::into)
            .chain(self.connectors.iter().cloned().map(Into::into))
            .chain(self.crtcs.iter().cloned().map(Into::into))
            .collect();
        let (id, fd) = self.drm.create_lease(&objects, OFlags::CLOEXEC.bits())?;

        Ok(DrmLease {
            drm: self.drm.clone(),
            planes: self.planes,
            connectors: self.connectors,
            crtcs: self.crtcs,
            lease_id: id,
            obj: Arc::new(Mutex::new(None)),
            fd: Arc::new(Mutex::new(Some(fd))),
            revoked: Arc::new(AtomicBool::new(false)),
        })
    }
}

/// Active DRM Lease
///
/// Dropping will revoke the lease
#[derive(Debug, Clone)]
pub struct DrmLease {
    drm: DrmDeviceFd,
    planes: HashSet<plane::Handle>,
    connectors: HashSet<connector::Handle>,
    crtcs: HashSet<crtc::Handle>,
    lease_id: NonZeroU32,
    obj: Arc<Mutex<Option<wp_drm_lease_v1::WpDrmLeaseV1>>>,
    fd: Arc<Mutex<Option<OwnedFd>>>,
    revoked: Arc<AtomicBool>,
}

impl DrmLease {
    /// CRTCs being leased
    pub fn crtcs(&self) -> impl Iterator<Item = &crtc::Handle> {
        self.crtcs.iter()
    }
    /// Connectors being leased
    pub fn connectors(&self) -> impl Iterator<Item = &connector::Handle> {
        self.connectors.iter()
    }
    /// Planes being leased
    pub fn planes(&self) -> impl Iterator<Item = &plane::Handle> {
        self.planes.iter()
    }
    /// Lease Id
    pub fn id(&self) -> u32 {
        self.lease_id.get()
    }

    fn set_obj(&self, obj: wp_drm_lease_v1::WpDrmLeaseV1) {
        *self.obj.lock().unwrap() = Some(obj);
    }
    fn take_fd(&mut self) -> Option<OwnedFd> {
        self.fd.lock().unwrap().take()
    }
}

#[derive(Debug)]
struct DrmLeaseRef {
    drm: DrmDeviceFd,
    obj: Weak<Mutex<Option<wp_drm_lease_v1::WpDrmLeaseV1>>>,
    lease_id: NonZeroU32,
    revoked: Arc<AtomicBool>,
    connectors: HashSet<connector::Handle>,
}

impl Drop for DrmLease {
    fn drop(&mut self) {
        if let Some(obj) = &self.obj.lock().unwrap().take() {
            obj.finished();
        }
        if !self.revoked.swap(true, Ordering::SeqCst) {
            tracing::info!(?self.lease_id, "Revoking lease");
            if let Err(err) = self.drm.revoke_lease(self.lease_id) {
                tracing::warn!(?err, "Error revoking lease");
            };
        }
    }
}

impl DrmLeaseRef {
    fn force_close(&self) {
        if let Some(obj) = Weak::upgrade(&self.obj).and_then(|obj| obj.lock().unwrap().take()) {
            obj.finished();
        }
        if !self.revoked.swap(true, Ordering::SeqCst) {
            tracing::info!(?self.lease_id, "Revoking lease");
            if let Err(err) = self.drm.revoke_lease(self.lease_id) {
                tracing::warn!(?err, "Error revoking lease");
            };
        }
    }
}

/// Data attached to wp_drm_lease_v1 objects
#[derive(Debug)]
pub struct DrmLeaseData {
    id: u32,
    node: DrmNode,
}

/// DRM lease was rejected by the compositor.
#[derive(Debug, Default)]
pub struct LeaseRejected(Option<Box<dyn std::error::Error + 'static>>);

impl fmt::Display for LeaseRejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Some(reason) => f.write_fmt(format_args!("Lease rejected, reason: {}", &**reason)),
            None => f.write_str("Lease rejected"),
        }
    }
}

impl std::error::Error for LeaseRejected {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.0.as_deref()
    }
}

impl LeaseRejected {
    /// Wrap any error type in a `LeaseRejected` error to be returned from [`DrmLeaseHandler::lease_request`].
    pub fn with_cause<T: std::error::Error + 'static>(err: T) -> Self {
        LeaseRejected(Some(Box::new(err)))
    }
}

/// Handler trait for drm leasing from the compositor.
pub trait DrmLeaseHandler:
    GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData>
    + Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, Self>
    + Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, Self>
    + Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, Self>
    + Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, Self>
    + 'static
{
    /// Returns a mutable reference to the [`DrmLeaseState`] delegate type
    fn drm_lease_state(&mut self, node: DrmNode) -> &mut DrmLeaseState;
    /// A client has issued a new request.
    ///
    /// The request only contains connectors and the compositor is lease any resources it may seem fit.
    /// It is recommended however to lease the requested connectors and any resources necessary to drive them
    /// (free CRTCs per connector and at least their primary plane) or reject the request.
    ///
    /// To reject the request return `Err(LeaseRejected::default())`, otherwise return a [`DrmLeaseBuilder`] with all the resources added.
    fn lease_request(
        &mut self,
        node: DrmNode,
        request: DrmLeaseRequest,
    ) -> Result<DrmLeaseBuilder, LeaseRejected>;
    /// A new DRM lease is active. Dropping the provided [`DrmLease`] will revoke the lease.
    fn new_active_lease(&mut self, node: DrmNode, lease: DrmLease);
    /// A DRM lease was destroyed, the previously given [`DrmLease`] should be cleaned up, if not revoked already.
    fn lease_destroyed(&mut self, node: DrmNode, lease_id: u32);
}

/// Data attached to a wp_drm_lease_device_v1 global
pub struct DrmLeaseDeviceGlobalData {
    filter: Box<dyn for<'c> Fn(&'c Client) -> bool + Send + Sync>,
    path: PathBuf,
    node: DrmNode,
}

impl fmt::Debug for DrmLeaseDeviceGlobalData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DrmLeaseDeviceGlobalData")
            .field("path", &self.path)
            .field("node", &self.node)
            .finish()
    }
}

/// Errors thrown by the DRM lease global
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Unable to figure out the path of the drm device used
    #[error("Unable to get modesetting node for drm device")]
    UnableToGetPath,
    /// Unable to get a new file descriptor for the drm device used
    #[error("Unable to get file descriptor for drm device")]
    UnableToGetFd,
    /// Unable to open a new file descriptor for the drm device used
    #[error("Unable to open new file descriptor for drm device")]
    UnableToOpenNode(#[source] rustix::io::Errno),
    /// Unable to drop DRM Master on a new file descriptor
    #[error("Unable to drop master status on file descriptor for drm device")]
    UnableToDropMaster(#[source] io::Error),
}

fn get_non_master_fd<P: AsRef<Path>>(path: P) -> Result<OwnedFd, Error> {
    let fd = rustix::fs::open(
        path.as_ref(),
        OFlags::RDWR | OFlags::CLOEXEC,
        rustix::fs::Mode::empty(),
    )
    .map_err(Error::UnableToOpenNode)?;

    // check if the fd has master
    if drm_ffi::get_client(fd.as_fd(), 0)
        .map(|client| client.auth == 1)
        .unwrap_or(false)
    {
        drm_ffi::auth::release_master(fd.as_fd()).map_err(Error::UnableToDropMaster)?;
    }

    Ok(fd)
}

impl DrmLeaseState {
    /// Create a new DRM lease global for a given [`DrmNode`].
    pub fn new<D>(display: &DisplayHandle, drm_node: &DrmNode) -> Result<DrmLeaseState, Error>
    where
        D: DrmLeaseHandler
            + GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData>
            + Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, D>
            + Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, D>
            + Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, D>
            + Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, D>
            + 'static,
    {
        Self::new_with_filter::<D, _>(display, drm_node, |_| true)
    }

    /// Create a new DRM lease global for a given [`DrmNode`] and a filter.
    ///
    /// Filters can be used to limit visibility of a global to certain clients.
    pub fn new_with_filter<D, F>(
        display: &DisplayHandle,
        drm_node: &DrmNode,
        filter: F,
    ) -> Result<DrmLeaseState, Error>
    where
        D: DrmLeaseHandler
            + GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData>
            + Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, D>
            + Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, D>
            + Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, D>
            + Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, D>
            + 'static,
        F: for<'c> Fn(&'c Client) -> bool + Send + Sync + 'static,
    {
        let path = drm_node
            .dev_path_with_type(NodeType::Primary)
            .ok_or(Error::UnableToGetPath)?;

        // test once if we can get a non-master fd before initializing the global
        let _ = get_non_master_fd(&path)?;

        let data = DrmLeaseDeviceGlobalData {
            filter: Box::new(filter),
            path,
            node: *drm_node,
        };

        let global = display.create_global::<D, wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, _>(1, data);

        Ok(DrmLeaseState {
            node: *drm_node,
            dh: display.clone(),
            global: Some(global),
            connectors: Vec::new(),
            known_lease_devices: Vec::new(),
            active_leases: Vec::new(),
        })
    }

    /// Add a connector, that is free to be leased to clients.
    pub fn add_connector<D>(&mut self, connector: connector::Handle, name: String, description: String)
    where
        D: DrmLeaseHandler
            + GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData>
            + Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, D>
            + Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, D>
            + Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, D>
            + Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, D>
            + 'static,
    {
        if self.connectors.iter().any(|conn| conn.handle == connector) {
            return;
        }

        let mut instances = Vec::new();
        for instance in &self.known_lease_devices {
            if let Ok(client) = self.dh.get_client(instance.id()) {
                if let Ok(lease_connector) = client
                    .create_resource::<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, _, D>(
                        &self.dh, 1, self.node,
                    )
                {
                    instance.connector(&lease_connector);
                    lease_connector.name(name.clone());
                    lease_connector.description(description.clone());
                    lease_connector.connector_id(connector.into());
                    instances.push(lease_connector);
                    instance.done();
                }
            }
        }

        self.connectors.push(DrmLeaseConnector {
            name,
            description,
            node: self.node,
            handle: connector,
            enabled: true,
            known_instances: instances,
        });
    }

    /// Withdraw a connector from the set of connectors available for leasing
    pub fn withdraw_connector(&mut self, connector: connector::Handle) {
        let mut clients = HashSet::new();
        if let Some(pos) = self.connectors.iter().position(|conn| conn.handle == connector) {
            let lease_connector = self.connectors.remove(pos);
            for instance in &lease_connector.known_instances {
                instance.withdrawn();
                if let Some(client) = instance.client() {
                    clients.insert(client.id());
                }
            }
        }
        for instance in &self.known_lease_devices {
            if let Some(client) = instance.client() {
                if clients.contains(&client.id()) {
                    instance.done();
                }
            }
        }

        self.active_leases
            .retain(|lease| !lease.connectors.contains(&connector));
    }

    /// Suspend all connectors temporarily (e.g. upon loosing DRM master as the session becomes inactive)
    pub fn suspend(&mut self) {
        self.suspend_internal(None);
    }

    fn suspend_internal(&mut self, connectors: Option<&HashSet<connector::Handle>>) {
        let mut clients = HashSet::new();
        for connector in self.connectors.iter_mut().filter(|c| {
            connectors
                .map(|filter| filter.contains(&c.handle))
                .unwrap_or(true)
        }) {
            for instance in connector.known_instances.drain(..) {
                instance.withdrawn();
                if let Some(client) = instance.client() {
                    clients.insert(client.id());
                }
            }
            connector.enabled = false;
        }
        for (instance, client) in self
            .known_lease_devices
            .iter()
            .flat_map(|instance| instance.client().map(|client| (instance, client)))
        {
            if clients.contains(&client.id()) {
                instance.done();
            }
        }
    }

    /// Resume all connectors temporarily (e.g. upon gaining DRM master as the session becomes active)
    pub fn resume<D>(&mut self)
    where
        D: DrmLeaseHandler
            + GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData>
            + Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, D>
            + Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, D>
            + Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, D>
            + Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, D>
            + 'static,
    {
        self.resume_internal::<D>(None);
    }

    fn resume_internal<D>(&mut self, connectors: Option<&HashSet<connector::Handle>>)
    where
        D: DrmLeaseHandler
            + GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData>
            + Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, D>
            + Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, D>
            + Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, D>
            + Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, D>
            + 'static,
    {
        for (instance, client) in self
            .known_lease_devices
            .iter()
            .flat_map(|instance| instance.client().map(|client| (instance, client)))
        {
            let mut once = false;
            for connector in self.connectors.iter_mut().filter(|c| {
                connectors
                    .map(|filter| filter.contains(&c.handle))
                    .unwrap_or(true)
            }) {
                if let Some(lease_connector) = connector.new_instance::<D>(&self.dh, &client) {
                    instance.connector(&lease_connector);
                    connector.send_info(&lease_connector);
                    once = true;
                }
            }
            if once {
                instance.done();
            }
        }
        for connector in self.connectors.iter_mut().filter(|c| {
            connectors
                .map(|filter| filter.contains(&c.handle))
                .unwrap_or(true)
        }) {
            connector.enabled = true;
        }
    }

    fn remove_lease<D>(&mut self, id: u32) -> Option<DrmLeaseRef>
    where
        D: DrmLeaseHandler
            + GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData>
            + Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, D>
            + Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, D>
            + Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, D>
            + Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, D>
            + 'static,
    {
        let lease_ref = {
            if let Some(pos) = self
                .active_leases
                .iter()
                .position(|lease| lease.lease_id.get() == id)
            {
                let lease = self.active_leases.remove(pos);
                self.resume_internal::<D>(Some(&lease.connectors));
                lease
            } else {
                return None;
            }
        };
        lease_ref.force_close();
        Some(lease_ref)
    }

    /// [`DrmNode`] belonging to this DRM lease global
    pub fn node(&self) -> DrmNode {
        self.node
    }

    /// Disable the global, it will no longer be advertised to new clients
    pub fn disable_global<D>(&mut self)
    where
        D: DrmLeaseHandler
            + GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData>
            + Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, D>
            + Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, D>
            + Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, D>
            + Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, D>
            + 'static,
    {
        if let Some(global) = self.global.take() {
            self.dh.disable_global::<D>(global);
        }
    }
}

impl Drop for DrmLeaseState {
    fn drop(&mut self) {
        // End all leases
        self.active_leases.clear();

        // withdraw all connectors
        for connector in &self.connectors {
            for instance in &connector.known_instances {
                instance.withdrawn();
            }
        }

        // end all devices
        for device in &*self.known_lease_devices {
            device.released();
        }
    }
}

impl DrmLeaseConnector {
    fn new_instance<D>(
        &mut self,
        dh: &DisplayHandle,
        client: &Client,
    ) -> Option<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1>
    where
        D: DrmLeaseHandler
            + GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData>
            + Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, D>
            + Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, D>
            + Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, D>
            + Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, D>
            + 'static,
    {
        if let Ok(lease_connector) =
            client.create_resource::<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, _, D>(dh, 1, self.node)
        {
            self.known_instances.push(lease_connector.clone());

            Some(lease_connector)
        } else {
            None
        }
    }

    fn send_info(&self, connector: &wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1) {
        connector.name(self.name.clone());
        connector.description(self.description.clone());
        connector.connector_id(self.handle.into());
        connector.done();
    }
}

impl<D> GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData, D>
    for DrmLeaseState
where
    D: DrmLeaseHandler
        + GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData>
        + Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, D>
        + Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, D>
        + Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, D>
        + Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, D>
        + 'static,
{
    fn bind(
        state: &mut D,
        dh: &DisplayHandle,
        client: &Client,
        resource: New<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1>,
        global_data: &DrmLeaseDeviceGlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let drm_lease_state = state.drm_lease_state(global_data.node);
        let wp_drm_lease_device = data_init.init(resource, global_data.node);

        let Ok(fd) = get_non_master_fd(&global_data.path) else {
            // nothing we can do
            wp_drm_lease_device.released();
            return;
        };

        wp_drm_lease_device.drm_fd(fd.as_fd());
        for connector in drm_lease_state.connectors.iter_mut().filter(|c| c.enabled) {
            if let Some(id) = connector.new_instance::<D>(dh, client) {
                wp_drm_lease_device.connector(&id);
                connector.send_info(&id);
            }
        }
        wp_drm_lease_device.done();

        drm_lease_state.known_lease_devices.push(wp_drm_lease_device);
    }

    fn can_view(client: Client, global_data: &DrmLeaseDeviceGlobalData) -> bool {
        (global_data.filter)(&client)
    }
}

impl<D> Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, D> for DrmLeaseState
where
    D: DrmLeaseHandler
        + GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData>
        + Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, D>
        + Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, D>
        + Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, D>
        + Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, D>
        + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &wp_drm_lease_device_v1::WpDrmLeaseDeviceV1,
        request: <wp_drm_lease_device_v1::WpDrmLeaseDeviceV1 as Resource>::Request,
        data: &DrmNode,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_drm_lease_device_v1::Request::CreateLeaseRequest { id } => {
                data_init.init(
                    id,
                    DrmLeaseRequestData {
                        node: *data,
                        connectors: Mutex::new(Vec::new()),
                    },
                );
            }
            wp_drm_lease_device_v1::Request::Release => {
                let drm_lease_state = state.drm_lease_state(*data);
                drm_lease_state
                    .known_lease_devices
                    .retain(|device| device != resource);
                resource.released();
            }
            _ => {}
        }
    }
}

impl<D> Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, D> for DrmLeaseState
where
    D: DrmLeaseHandler
        + GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData>
        + Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, D>
        + Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, D>
        + Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, D>
        + Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, D>
        + 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1,
        _request: <wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1 as Resource>::Request,
        _data: &DrmNode,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }

    fn destroyed(
        state: &mut D,
        _client: wayland_server::backend::ClientId,
        resource: &wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1,
        data: &DrmNode,
    ) {
        let drm_lease_state = state.drm_lease_state(*data);
        drm_lease_state.connectors.iter_mut().for_each(|connector| {
            connector.known_instances.retain(|obj| obj != resource);
        });
    }
}

impl<D> Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, D> for DrmLeaseState
where
    D: DrmLeaseHandler
        + GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData>
        + Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, D>
        + Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, D>
        + Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, D>
        + Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, D>
        + 'static,
{
    fn request(
        state: &mut D,
        _client: &Client,
        resource: &wp_drm_lease_request_v1::WpDrmLeaseRequestV1,
        request: <wp_drm_lease_request_v1::WpDrmLeaseRequestV1 as Resource>::Request,
        data: &DrmLeaseRequestData,
        _dhandle: &DisplayHandle,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            wp_drm_lease_request_v1::Request::RequestConnector { connector } => {
                data.connectors.lock().unwrap().push(connector);
            }
            wp_drm_lease_request_v1::Request::Submit { id } => {
                let drm_lease_state = state.drm_lease_state(data.node);

                let mut connector_handles = Vec::new();
                for connector in &*data.connectors.lock().unwrap() {
                    if let Some(conn) = drm_lease_state
                        .connectors
                        .iter()
                        .find(|conn| conn.known_instances.iter().any(|obj| obj == connector))
                    {
                        if connector_handles.iter().any(|handle| handle == &conn.handle) {
                            resource.post_error(
                                wp_drm_lease_request_v1::Error::DuplicateConnector,
                                format!("Duplicate connector: {}", conn.name),
                            );
                            return;
                        } else {
                            connector_handles.push(conn.handle);
                        }
                    } else {
                        resource.post_error(
                            wp_drm_lease_request_v1::Error::WrongDevice,
                            "Requested lease for wrong device",
                        );
                        return;
                    }
                }

                if connector_handles.is_empty() {
                    resource.post_error(
                        wp_drm_lease_request_v1::Error::EmptyLease,
                        "Lease doesn't contain any connectors",
                    );
                }

                match state.lease_request(
                    data.node,
                    DrmLeaseRequest {
                        connectors: connector_handles,
                    },
                ) {
                    Ok(builder) => match builder.build() {
                        Ok(mut lease) => {
                            let lease_obj = data_init.init(
                                id,
                                DrmLeaseData {
                                    id: lease.lease_id.get(),
                                    node: data.node,
                                },
                            );
                            lease.set_obj(lease_obj.clone());
                            let fd = lease.take_fd().unwrap();
                            lease_obj.lease_fd(fd.as_fd());

                            let lease_ref = DrmLeaseRef {
                                drm: lease.drm.clone(),
                                obj: Arc::downgrade(&lease.obj),
                                lease_id: lease.lease_id,
                                connectors: lease.connectors.clone(),
                                revoked: lease.revoked.clone(),
                            };

                            let drm_lease_state = state.drm_lease_state(data.node);
                            drm_lease_state.suspend_internal(Some(&lease.connectors));
                            drm_lease_state.active_leases.push(lease_ref);

                            state.new_active_lease(data.node, lease);
                        }
                        Err(err) => {
                            tracing::error!(?err, "Failed to create lease");
                            let lease_obj = data_init.init(
                                id,
                                DrmLeaseData {
                                    id: 0,
                                    node: data.node,
                                },
                            );
                            lease_obj.finished();
                        }
                    },
                    Err(err) => {
                        tracing::debug!(?err, "Compositor denied lease request");
                        let lease_obj = data_init.init(
                            id,
                            DrmLeaseData {
                                id: 0,
                                node: data.node,
                            },
                        );
                        lease_obj.finished();
                    }
                }
            }
            _ => {}
        }
    }
}

impl<D> Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, D> for DrmLeaseState
where
    D: DrmLeaseHandler
        + GlobalDispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmLeaseDeviceGlobalData>
        + Dispatch<wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1, DrmNode, D>
        + Dispatch<wp_drm_lease_device_v1::WpDrmLeaseDeviceV1, DrmNode, D>
        + Dispatch<wp_drm_lease_request_v1::WpDrmLeaseRequestV1, DrmLeaseRequestData, D>
        + Dispatch<wp_drm_lease_v1::WpDrmLeaseV1, DrmLeaseData, D>
        + 'static,
{
    fn request(
        _state: &mut D,
        _client: &Client,
        _resource: &wp_drm_lease_v1::WpDrmLeaseV1,
        _request: <wp_drm_lease_v1::WpDrmLeaseV1 as Resource>::Request,
        _data: &DrmLeaseData,
        _dhandle: &DisplayHandle,
        _data_init: &mut DataInit<'_, D>,
    ) {
    }

    fn destroyed(
        state: &mut D,
        _client: wayland_server::backend::ClientId,
        _resource: &wp_drm_lease_v1::WpDrmLeaseV1,
        data: &DrmLeaseData,
    ) {
        let drm_lease_state = state.drm_lease_state(data.node);
        if let Some(lease_ref) = drm_lease_state.remove_lease::<D>(data.id) {
            state.lease_destroyed(data.node, lease_ref.lease_id.get());
        }
    }
}

/// Macro to delegate implementation of the drm-lease protocol to [`DrmLeaseState`].
///
/// You must also implement [`DrmLeaseHandler`] to use this.
#[macro_export]
macro_rules! delegate_drm_lease {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        type __WpDrmLeaseDeviceV1 =
            $crate::reexports::wayland_protocols::wp::drm_lease::v1::server::wp_drm_lease_device_v1::WpDrmLeaseDeviceV1;
        type __WpDrmLeaseConnectorV1 =
            $crate::reexports::wayland_protocols::wp::drm_lease::v1::server::wp_drm_lease_connector_v1::WpDrmLeaseConnectorV1;
        type __WpDrmLeaseRequestV1 =
            $crate::reexports::wayland_protocols::wp::drm_lease::v1::server::wp_drm_lease_request_v1::WpDrmLeaseRequestV1;
        type __WpDrmLeaseV1 =
            $crate::reexports::wayland_protocols::wp::drm_lease::v1::server::wp_drm_lease_v1::WpDrmLeaseV1;

        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __WpDrmLeaseDeviceV1: $crate::wayland::drm_lease::DrmLeaseDeviceGlobalData
        ] => $crate::wayland::drm_lease::DrmLeaseState);

        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __WpDrmLeaseConnectorV1: $crate::backend::drm::DrmNode
        ] => $crate::wayland::drm_lease::DrmLeaseState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __WpDrmLeaseDeviceV1: $crate::backend::drm::DrmNode
        ] => $crate::wayland::drm_lease::DrmLeaseState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __WpDrmLeaseRequestV1: $crate::wayland::drm_lease::DrmLeaseRequestData
        ] => $crate::wayland::drm_lease::DrmLeaseState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            __WpDrmLeaseV1: $crate::wayland::drm_lease::DrmLeaseData
        ] => $crate::wayland::drm_lease::DrmLeaseState);

    };
}
