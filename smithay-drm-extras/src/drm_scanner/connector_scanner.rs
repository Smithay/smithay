use std::{
    collections::HashMap,
    iter::{Chain, Map},
};

use drm::control::{connector, Device as ControlDevice};

/// Responsible for tracking connected/disconnected events.
///
/// ### Example
/// ```no_run
/// # mod helpers { include!("../docs/doctest_helpers.rs"); };
/// # let drm_device: helpers::FakeDevice = todo!();
/// use smithay_drm_extras::drm_scanner::{ConnectorScanner, ConnectorScanEvent};
///
/// let mut scanner = ConnectorScanner::new();
///
/// for event in scanner.scan(&drm_device) {
///     match event {
///         ConnectorScanEvent::Connected(conn) => {},
///         ConnectorScanEvent::Disconnected(conn) => {},
///     }
/// }
#[derive(Debug, Default)]
pub struct ConnectorScanner {
    connectors: HashMap<connector::Handle, connector::Info>,
}

impl ConnectorScanner {
    /// Create new [`ConnectorScanner`]
    pub fn new() -> Self {
        Default::default()
    }

    /// Should be called on every device changed event
    pub fn scan(&mut self, drm: &impl ControlDevice) -> ConnectorScanResult {
        let res_handles = drm.resource_handles().unwrap();
        let connector_handles = res_handles.connectors();

        let mut added = Vec::new();
        let mut removed = Vec::new();

        for conn in connector_handles
            .iter()
            .filter_map(|conn| drm.get_connector(*conn, true).ok())
        {
            let curr_state = conn.state();

            use connector::State;
            if let Some(old) = self.connectors.insert(conn.handle(), conn.clone()) {
                match (old.state(), curr_state) {
                    (State::Connected, State::Disconnected) => removed.push(conn),
                    (State::Disconnected | State::Unknown, State::Connected) => added.push(conn),
                    //
                    (State::Connected, State::Connected) => {}
                    (State::Disconnected, State::Disconnected) => {}
                    //
                    (State::Unknown, _) => {}
                    (_, State::Unknown) => {}
                }
            } else if curr_state == State::Connected {
                added.push(conn)
            }
        }

        ConnectorScanResult {
            connected: added,
            disconnected: removed,
        }
    }

    /// Get map of all connectors, connected and disconnected ones.
    pub fn connectors(&self) -> &HashMap<connector::Handle, connector::Info> {
        &self.connectors
    }
}

/// Result of [`ConnectorScanner::scan`]
///
/// You can use `added` and `removed` fields of this result manually,
/// or you can just iterate (using [`IntoIterator`] or [`ConnectorScanResult::iter`])
/// over this result to get [`ConnectorScanEvent`].
#[derive(Debug, Default, Clone)]
pub struct ConnectorScanResult {
    /// Connectors that got plugged in since last scan
    pub connected: Vec<connector::Info>,
    /// Connectors that got unplugged since last scan
    pub disconnected: Vec<connector::Info>,
}

/// Created from [`ConnectorScanResult`], informs about connector events.
#[derive(Debug, Clone)]
pub enum ConnectorScanEvent {
    /// A new connector got plugged in since last scan
    Connected(connector::Info),
    /// A connector got unplugged in since last scan
    Disconnected(connector::Info),
}

impl ConnectorScanResult {
    /// Creates event iterator for this result
    ///
    /// Internally this clones the data so it is equivalent to [`IntoIterator`]
    pub fn iter(&self) -> impl Iterator<Item = ConnectorScanEvent> {
        self.clone().into_iter()
    }
}

type ConnectorScanItemToEvent = fn(connector::Info) -> ConnectorScanEvent;

impl IntoIterator for ConnectorScanResult {
    type Item = ConnectorScanEvent;
    type IntoIter = Chain<
        Map<std::vec::IntoIter<connector::Info>, ConnectorScanItemToEvent>,
        Map<std::vec::IntoIter<connector::Info>, ConnectorScanItemToEvent>,
    >;

    fn into_iter(self) -> Self::IntoIter {
        self.disconnected
            .into_iter()
            .map(ConnectorScanEvent::Disconnected as ConnectorScanItemToEvent)
            .chain(
                self.connected
                    .into_iter()
                    .map(ConnectorScanEvent::Connected as ConnectorScanItemToEvent),
            )
    }
}
