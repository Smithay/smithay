use std::collections::HashMap;

use drm::control::{connector, crtc, Device as ControlDevice};

/// CRTC Mapper trait
///
/// It exists to allow custom mappers in [`super::DrmScanner`].
///
/// It is responsible for mapping CRTCs to connectors.
/// For each connector it has to pick suitable CRTC.
pub trait CrtcMapper {
    /// Request mapping of CRTCs to supplied connectors
    ///
    /// Usually called in response to udev device changed event,
    /// or on device init.
    fn map<'a>(
        &mut self,
        drm: &impl ControlDevice,
        connectors: impl Iterator<Item = &'a connector::Info> + Clone,
    );

    /// Query CRTC mapped to supplied connector
    fn crtc_for_connector(&self, connector: &connector::Handle) -> Option<crtc::Handle>;
}

/// Simple CRTC Mapper
///
/// This is basic mapper that simply chooses one CRTC for every connector.
///
/// It is also capable of recovering mappings that were used by display manger or tty
/// before the compositor was started up.
#[derive(Debug, Default)]
pub struct SimpleCrtcMapper {
    crtcs: HashMap<connector::Handle, crtc::Handle>,
}

impl SimpleCrtcMapper {
    /// Create new [`SimpleCrtcMapper`]
    pub fn new() -> Self {
        Self::default()
    }

    fn is_taken(&self, crtc: &crtc::Handle) -> bool {
        self.crtcs.values().any(|v| v == crtc)
    }

    fn is_available(&self, crtc: &crtc::Handle) -> bool {
        !self.is_taken(crtc)
    }

    fn restored_for_connector(
        &self,
        drm: &impl ControlDevice,
        connector: &connector::Info,
    ) -> Option<crtc::Handle> {
        let encoder = connector.current_encoder()?;
        let encoder = drm.get_encoder(encoder).ok()?;
        let crtc = encoder.crtc()?;

        self.is_available(&crtc).then_some(crtc)
    }

    fn pick_next_avalible_for_connector(
        &self,
        drm: &impl ControlDevice,
        connector: &connector::Info,
    ) -> Option<crtc::Handle> {
        let res_handles = drm.resource_handles().ok()?;

        connector
            .encoders()
            .iter()
            .flat_map(|encoder_handle| drm.get_encoder(*encoder_handle))
            .find_map(|encoder_info| {
                res_handles
                    .filter_crtcs(encoder_info.possible_crtcs())
                    .into_iter()
                    .find(|crtc| self.is_available(crtc))
            })
    }
}

impl super::CrtcMapper for SimpleCrtcMapper {
    fn map<'a>(
        &mut self,
        drm: &impl ControlDevice,
        connectors: impl Iterator<Item = &'a connector::Info> + Clone,
    ) {
        for connector in connectors
            .clone()
            .filter(|conn| conn.state() != connector::State::Connected)
        {
            self.crtcs.remove(&connector.handle());
        }

        let mut needs_crtc: Vec<&connector::Info> = connectors
            .filter(|conn| conn.state() == connector::State::Connected)
            .filter(|conn| !self.crtcs.contains_key(&conn.handle()))
            .collect();

        needs_crtc.retain(|connector| {
            if let Some(crtc) = self.restored_for_connector(drm, connector) {
                self.crtcs.insert(connector.handle(), crtc);

                // This connector no longer needs crtc so let's remove it
                false
            } else {
                true
            }
        });

        for connector in needs_crtc {
            if let Some(crtc) = self.pick_next_avalible_for_connector(drm, connector) {
                self.crtcs.insert(connector.handle(), crtc);
            }
        }
    }

    fn crtc_for_connector(&self, connector: &connector::Handle) -> Option<crtc::Handle> {
        self.crtcs.get(connector).copied()
    }
}
