//! # Smithay DRM Extras
//!
//! This crate contains some extra abstractions and helpers over DRM
//!
//! - [`edid`] is responsible for extraction of information from DRM connectors
//! - [`drm_scanner`] is responsible for detecting connector connected and
//!   disconnected events, as well as mapping CRTC to them.
//!
//! ### Features
//! - `generate-hwdata` - If enabled [hwdata](https://github.com/vcrhonek/hwdata) code will be regenerated using `hwdata` system package

#![warn(missing_docs, missing_debug_implementations)]

pub mod drm_scanner;
pub mod edid;
mod hwdata;

use drm::control::connector;

pub fn format_connector_name(connector_info: &connector::Info) -> String {
    let interface_id = connector_info.interface_id();

    // TODO: Remove once supported in drm-rs
    use connector::Interface;
    let interface_short_name = match connector_info.interface() {
        Interface::Unknown => "Unknown",
        Interface::VGA => "VGA",
        Interface::DVII => "DVI-I",
        Interface::DVID => "DVI-D",
        Interface::DVIA => "DVI-A",
        Interface::Composite => "Composite",
        Interface::SVideo => "SVIDEO",
        Interface::LVDS => "LVDS",
        Interface::Component => "Component",
        Interface::NinePinDIN => "DIN",
        Interface::DisplayPort => "DP",
        Interface::HDMIA => "HDMI-A",
        Interface::HDMIB => "HDMI-B",
        Interface::TV => "TV",
        Interface::EmbeddedDisplayPort => "eDP",
        Interface::Virtual => "Virtual",
        Interface::DSI => "DSI",
        Interface::DPI => "DPI",
    };

    format!("{interface_short_name}-{interface_id}")
}
