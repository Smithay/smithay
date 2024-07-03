//! # Drm Scanner
//!
//! - [`ConnectorScanner`] is responsible for tracking connected/disconnected events.
//! - [`CrtcMapper`] trait and [`SimpleCrtcMapper`] are meant for mapping crtc to connector.
//! - [`DrmScanner`] combines two above into single abstraction.
//!   If it does not fit your needs you can always drop down to using [`ConnectoScanner`] alone.
//!
//! ### Example
//! ```no_run
//! # mod helpers { include!("./docs/doctest_helpers.rs"); };
//! # let drm_device: helpers::FakeDevice = todo!();
//! use smithay_drm_extras::drm_scanner::{DrmScanner, DrmScanEvent};
//!
//! let mut scanner: DrmScanner = DrmScanner::new();
//!
//! for event in scanner.scan_connectors(&drm_device) {
//!     match event {
//!         DrmScanEvent::Connected { .. } => {},
//!         DrmScanEvent::Disconnected { .. } => {},
//!     }
//! }
//! ```
use std::{
    collections::HashMap,
    iter::{Chain, Map},
};

use drm::control::{connector, crtc, Device as ControlDevice};

mod connector_scanner;
pub use connector_scanner::{ConnectorScanEvent, ConnectorScanResult, ConnectorScanner};

mod crtc_mapper;
pub use crtc_mapper::{CrtcMapper, SimpleCrtcMapper};

/// Drm Scanner
///
/// Wrapper over [`ConnectorScanner`] and [`CrtcMapper`]
#[derive(Debug, Default)]
pub struct DrmScanner<Mapper = SimpleCrtcMapper>
where
    Mapper: CrtcMapper,
{
    connectors: ConnectorScanner,
    crtc_mapper: Mapper,
}

impl<M> DrmScanner<M>
where
    M: CrtcMapper + Default,
{
    /// Create new DrmScanner with default CRTC mapper.
    pub fn new() -> Self {
        Self::new_with_mapper(Default::default())
    }
}

impl<M> DrmScanner<M>
where
    M: CrtcMapper,
{
    /// Create new DrmScanner with custom CRTC mapper
    pub fn new_with_mapper(mapper: M) -> Self {
        Self {
            connectors: Default::default(),
            crtc_mapper: mapper,
        }
    }

    /// [`CrtcMapper`] getter
    pub fn crtc_mapper(&self) -> &M {
        &self.crtc_mapper
    }

    /// Muttable [`CrtcMapper`] getter
    pub fn crtc_mapper_mut(&mut self) -> &mut M {
        &mut self.crtc_mapper
    }

    /// Scan connectors to find out what has changed since last call to this method.
    ///
    /// Returns [`DrmScanResult`] that contains added and removed connectors,
    /// and CRTCs that got assigned to them.
    ///
    /// Should be called on every device changed event
    ///
    /// ```no_run
    /// # mod helpers { include!("./docs/doctest_helpers.rs"); };
    /// # let drm_device: helpers::FakeDevice = todo!();
    /// use smithay_drm_extras::drm_scanner::{DrmScanner, DrmScanEvent};
    ///
    /// let mut scanner: DrmScanner = DrmScanner::new();
    /// let res = scanner.scan_connectors(&drm_device).expect("failed to scan connectors");
    ///
    /// // You can extract scan info manually
    /// println!("Plugged {} connectors", res.added.len());
    /// println!("Unplugged {} connectors", res.removed.len());
    ///
    /// // Or simply iterate over it
    /// for event in res {
    ///     match event {
    ///         DrmScanEvent::Connected { .. } => {},
    ///         DrmScanEvent::Disconnected { .. } => {},
    ///     }
    /// }
    /// ```
    pub fn scan_connectors(&mut self, drm: &impl ControlDevice) -> std::io::Result<DrmScanResult> {
        let scan = self.connectors.scan(drm)?;

        let removed = scan
            .disconnected
            .into_iter()
            .map(|info| {
                let crtc = self.crtc_mapper.crtc_for_connector(&info.handle());
                (info, crtc)
            })
            .collect();

        self.crtc_mapper
            .map(drm, self.connectors.connectors().iter().map(|(_, info)| info));

        let added = scan
            .connected
            .into_iter()
            .map(|info| {
                let crtc = self.crtc_mapper.crtc_for_connector(&info.handle());
                (info, crtc)
            })
            .collect();

        Ok(DrmScanResult {
            disconnected: removed,
            connected: added,
        })
    }

    /// Get map of all connectors, connected and disconnected ones.
    pub fn connectors(&self) -> &HashMap<connector::Handle, connector::Info> {
        self.connectors.connectors()
    }

    /// Get CRTC that is mapped to supplied connector
    ///
    /// This will query underlying [`CrtcMapper`]
    pub fn crtc_for_connector(&self, connector: &connector::Handle) -> Option<crtc::Handle> {
        self.crtc_mapper.crtc_for_connector(connector)
    }

    /// Get iterator over all `connector -> CRTC` mappings
    pub fn crtcs(&self) -> impl Iterator<Item = (&connector::Info, crtc::Handle)> {
        self.connectors()
            .iter()
            .filter_map(|(handle, info)| Some((info, self.crtc_for_connector(handle)?)))
    }
}

type DrmScanItem = (connector::Info, Option<crtc::Handle>);

/// Result of [`DrmScanner::scan_connectors`]
///
/// You can use `added` and `removed` fields of this result manually,
/// or you can just iterate (using [`IntoIterator`] or [`DrmScanResult::iter`])
/// over this result to get [`DrmScanEvent`].
#[derive(Debug, Default, Clone)]
pub struct DrmScanResult {
    /// Connectors that got plugged in since last scan
    pub connected: Vec<DrmScanItem>,
    /// Connectors that got unplugged since last scan
    pub disconnected: Vec<DrmScanItem>,
}

impl DrmScanResult {
    /// Creates event iterator for this result
    ///
    /// Internally this clones the data so it is equivalent to [`IntoIterator`]
    pub fn iter(&self) -> impl Iterator<Item = DrmScanEvent> {
        self.clone().into_iter()
    }
}

/// Created from [`DrmScanResult`], informs about connector events.
#[derive(Debug, Clone)]
pub enum DrmScanEvent {
    /// A new connector got plugged in since last scan
    Connected {
        /// Info about connected connector
        connector: connector::Info,
        /// Crtc that got mapped to this connector
        crtc: Option<crtc::Handle>,
    },
    /// A connector got unplugged in since last scan
    Disconnected {
        /// Info about disconnected connector
        connector: connector::Info,
        /// Crtc that is no longer mapped to this connector
        crtc: Option<crtc::Handle>,
    },
}

impl DrmScanEvent {
    fn connected((connector, crtc): (connector::Info, Option<crtc::Handle>)) -> Self {
        DrmScanEvent::Connected { connector, crtc }
    }

    fn disconnected((connector, crtc): (connector::Info, Option<crtc::Handle>)) -> Self {
        DrmScanEvent::Disconnected { connector, crtc }
    }
}

type DrmScanItemToEvent = fn(DrmScanItem) -> DrmScanEvent;

impl IntoIterator for DrmScanResult {
    type Item = DrmScanEvent;
    type IntoIter = Chain<
        Map<std::vec::IntoIter<DrmScanItem>, DrmScanItemToEvent>,
        Map<std::vec::IntoIter<DrmScanItem>, DrmScanItemToEvent>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.disconnected
            .into_iter()
            .map(DrmScanEvent::disconnected as DrmScanItemToEvent)
            .chain(
                self.connected
                    .into_iter()
                    .map(DrmScanEvent::connected as DrmScanItemToEvent),
            )
    }
}
