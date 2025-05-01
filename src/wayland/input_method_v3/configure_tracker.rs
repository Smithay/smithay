/*! Tracks serial number assignment */
use crate::utils::{Serial, SERIAL_COUNTER};

/// Tracks states updated via configure sequences involving serials
#[derive(Debug)]
pub struct ConfigureTracker<State> {
    /// An ordered sequence of the configures the server has sent out to the client waiting to be
    /// acknowledged by the client. All pending configures that are older than
    /// the acknowledged one will be discarded during processing
    /// layer_surface.ack_configure.
    /// The newest configure has the highest index.
    pending_configures: Vec<(State, Serial)>,

    /// Holds the last server_pending state that has been acknowledged by the
    /// client. This state should be cloned to the current during a commit.
    last_acked: Option<State>,
}

impl<State> Default for ConfigureTracker<State> {
    fn default() -> Self {
        Self {
            pending_configures: Vec::new(),
            last_acked: None,
        }
    }
}

impl<State: Clone> ConfigureTracker<State> {
    /// Assigns a new pending state and returns its serial
    pub fn assign_serial(&mut self, state: State) -> Serial {
        let serial = SERIAL_COUNTER.next_serial();
        self.pending_configures.push((state, serial));
        serial
    }

    /// Marks that the user accepted the serial and returns the state associated with it.
    /// If the serial is not currently pending, returns None.
    pub fn ack_serial(&mut self, serial: Serial) -> Option<State> {
        let (state, _) = self
            .pending_configures
            .iter()
            .find(|(_, c_serial)| *c_serial == serial)
            .cloned()?;

        self.pending_configures.retain(|(_, c_serial)| *c_serial > serial);
        self.last_acked = Some(state.clone());
        Some(state)
    }

    /// Last state sent to the client but not acknowledged
    pub fn last_pending_state(&self) -> Option<&State> {
        self.pending_configures.last().map(|(s, _)| s)
    }
}
