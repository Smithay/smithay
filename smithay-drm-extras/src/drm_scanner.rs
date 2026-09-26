//! # Drm Scanner
//!
//! - [`ConnectorScanner`] is responsible for tracking connected/disconnected events.
//! - [`CrtcMapper`] trait and [`SimpleCrtcMapper`] are meant for mapping crtc to connector.
//! - [`DrmScanner`] combines two above into single abstraction.
//!   If it does not fit your needs you can always drop down to using [`ConnectScanner`] alone.
//!
//! ### Example
//! ```no_run
//! # mod helpers { include!("./docs/doctest_helpers.rs"); };
//! # let drm_device: helpers::FakeDevice = todo!();
//! use smithay_drm_extras::drm_scanner::{DrmScanner, DrmScanEvent};
//!
//! let mut scanner: DrmScanner = DrmScanner::new();
//!
//! for event in scanner.scan_connectors(&drm_device).expect("failed to scan connectors") {
//!     match event {
//!         DrmScanEvent::Connected { .. } => {},
//!         DrmScanEvent::Disconnected { .. } => {},
//!         DrmScanEvent::Changed { .. } => {},
//!     }
//! }
//! ```
use std::collections::{HashMap, HashSet};

use drm::control::{Device as ControlDevice, connector, crtc};

mod connector_scanner;
pub use connector_scanner::{ConnectorScanEvent, ConnectorScanResult, ConnectorScanner};

mod crtc_mapper;
pub use crtc_mapper::{CrtcMapper, SimpleCrtcMapper};

fn newly_mapped_connectors(
    previously_mapped: &HashSet<connector::Handle>,
    connected_events: &HashSet<connector::Handle>,
    current_mappings: impl Iterator<Item = (connector::Handle, Option<crtc::Handle>)>,
) -> Vec<(connector::Handle, crtc::Handle)> {
    current_mappings
        .filter_map(|(connector, crtc)| {
            let crtc = crtc?;
            (!previously_mapped.contains(&connector) && !connected_events.contains(&connector))
                .then_some((connector, crtc))
        })
        .collect()
}

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
    /// println!("Plugged {} connectors", res.connected.len());
    /// println!("Unplugged {} connectors", res.disconnected.len());
    ///
    /// // Or simply iterate over it
    /// for event in res {
    ///     match event {
    ///         DrmScanEvent::Connected { .. } => {},
    ///         DrmScanEvent::Disconnected { .. } => {},
    ///         DrmScanEvent::Changed { .. } => {},
    ///     }
    /// }
    /// ```
    pub fn scan_connectors(&mut self, drm: &impl ControlDevice) -> std::io::Result<DrmScanResult> {
        let scan = self.connectors.scan(drm)?;
        let previously_mapped: HashSet<_> = self
            .connectors
            .connectors()
            .keys()
            .filter(|connector| self.crtc_mapper.crtc_for_connector(connector).is_some())
            .copied()
            .collect();

        let removed = scan
            .disconnected
            .into_iter()
            .map(|info| {
                let crtc = self.crtc_mapper.crtc_for_connector(&info.handle());
                (info, crtc)
            })
            .collect();

        self.crtc_mapper.map(drm, self.connectors.connectors().values());

        let mut added: Vec<_> = scan
            .connected
            .into_iter()
            .map(|info| {
                let crtc = self.crtc_mapper.crtc_for_connector(&info.handle());
                (info, crtc)
            })
            .collect();
        let connected_events: HashSet<_> = added.iter().map(|(info, _)| info.handle()).collect();

        // A connected connector can remain unmapped when every compatible CRTC is occupied. If a
        // later scan releases a CRTC, the mapper can assign it without a corresponding connector
        // state change. Report that connector now so consumers can activate the new mapping.
        let newly_mapped = newly_mapped_connectors(
            &previously_mapped,
            &connected_events,
            self.connectors
                .connectors()
                .values()
                .filter(|info| info.state() == connector::State::Connected)
                .map(|info| (info.handle(), self.crtc_mapper.crtc_for_connector(&info.handle()))),
        );

        added.extend(newly_mapped.into_iter().filter_map(|(connector, crtc)| {
            self.connectors
                .connectors()
                .get(&connector)
                .cloned()
                .map(|info| (info, Some(crtc)))
        }));

        let changed = scan
            .changed
            .into_iter()
            .map(|info| {
                let crtc = self.crtc_mapper.crtc_for_connector(&info.handle());
                (info, crtc)
            })
            .collect();

        Ok(DrmScanResult {
            disconnected: removed,
            connected: added,
            changed,
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
    /// Connectors that got plugged in or became mapped to a CRTC since last scan
    pub connected: Vec<DrmScanItem>,
    /// Connectors that got unplugged since last scan
    pub disconnected: Vec<DrmScanItem>,
    /// Connectors whose mode list changed while staying connected
    pub changed: Vec<DrmScanItem>,
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
    /// A connector got plugged in or became mapped to a CRTC since last scan
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
    /// The connector's mode list changed while staying connected
    Changed {
        /// Info about the connector whose modes changed
        connector: connector::Info,
        /// Crtc that is mapped to this connector
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

    fn changed((connector, crtc): (connector::Info, Option<crtc::Handle>)) -> Self {
        DrmScanEvent::Changed { connector, crtc }
    }
}

impl IntoIterator for DrmScanResult {
    type Item = DrmScanEvent;
    type IntoIter = std::vec::IntoIter<DrmScanEvent>;

    fn into_iter(self) -> Self::IntoIter {
        self.disconnected
            .into_iter()
            .map(DrmScanEvent::disconnected)
            .chain(self.connected.into_iter().map(DrmScanEvent::connected))
            .chain(self.changed.into_iter().map(DrmScanEvent::changed))
            .collect::<Vec<_>>()
            .into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reports_connector_when_crtc_mapping_becomes_available() {
        let already_mapped = drm::control::from_u32::<connector::Handle>(1).unwrap();
        let newly_mapped = drm::control::from_u32::<connector::Handle>(2).unwrap();
        let newly_connected = drm::control::from_u32::<connector::Handle>(3).unwrap();
        let still_unmapped = drm::control::from_u32::<connector::Handle>(4).unwrap();
        let first_crtc = drm::control::from_u32::<crtc::Handle>(5).unwrap();
        let second_crtc = drm::control::from_u32::<crtc::Handle>(6).unwrap();
        let third_crtc = drm::control::from_u32::<crtc::Handle>(7).unwrap();
        let previously_mapped = HashSet::from([already_mapped]);
        let connected_events = HashSet::from([newly_connected]);
        let current_mappings = [
            (already_mapped, Some(first_crtc)),
            (newly_mapped, Some(second_crtc)),
            (newly_connected, Some(third_crtc)),
            (still_unmapped, None),
        ];

        assert_eq!(
            newly_mapped_connectors(
                &previously_mapped,
                &connected_events,
                current_mappings.into_iter(),
            ),
            vec![(newly_mapped, second_crtc)]
        );
    }
}
