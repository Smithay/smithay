//! # EDID - Extended Display Identification Data
//!
//! This module is meant to help with extraction of EDID data from connectors
//!
//! ```no_run
//! # mod helpers { include!("./docs/doctest_helpers.rs"); };
//! # let drm_device: helpers::FakeDevice = todo!();
//! # let connector = todo!();
//! use smithay_drm_extras::edid::EdidInfo;
//!
//! let info = EdidInfo::for_connector(&drm_device, connector).unwrap();
//!
//! println!("Monitor name: {}", info.model);
//! println!("Manufacturer name: {}", info.manufacturer);
//! ```

use drm::control::{connector, Device as ControlDevice, PropertyValueSet};
use edid_rs::MonitorDescriptor;

use super::hwdata;

/// Information about monitor, acquired from EDID
#[derive(Debug, Clone)]
pub struct EdidInfo {
    /// Monitor name
    pub model: String,
    /// Name of manufacturer of this monitor
    pub manufacturer: String,
}

impl EdidInfo {
    /// Get EDID info from supplied connector
    pub fn for_connector(device: &impl ControlDevice, connector: connector::Handle) -> Option<EdidInfo> {
        device
            .get_properties(connector)
            .ok()
            .and_then(|props| get_edid(device, &props))
            .map(|edid| EdidInfo {
                model: get_monitor_name(&edid),
                manufacturer: get_manufacturer_name(&edid),
            })
    }
}

fn get_edid(device: &impl ControlDevice, props: &PropertyValueSet) -> Option<edid_rs::EDID> {
    let (info, value) = props
        .into_iter()
        .filter_map(|(handle, value)| {
            let info = device.get_property(*handle).ok()?;

            Some((info, value))
        })
        .find(|(info, _)| info.name().to_str() == Ok("EDID"))?;

    let blob = info.value_type().convert_value(*value).as_blob()?;
    let data = device.get_property_blob(blob).ok()?;

    let mut reader = std::io::Cursor::new(data);

    edid_rs::parse(&mut reader).ok()
}

fn get_manufacturer_name(edid: &edid_rs::EDID) -> String {
    let id = edid.product.manufacturer_id;
    let code = [id.0, id.1, id.2];

    hwdata::pnp_id_to_name(&code)
        .map(|name| name.to_string())
        .unwrap_or_else(|| {
            code.into_iter()
                .map(|v| (v as u8).to_string())
                .collect::<Vec<String>>()
                .join("_")
        })
}

fn get_monitor_name(edid: &edid_rs::EDID) -> String {
    edid.descriptors
        .0
        .iter()
        .find_map(|desc| {
            if let MonitorDescriptor::MonitorName(name) = desc {
                Some(name.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| edid.product.product_code.to_string())
}
