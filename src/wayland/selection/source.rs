use std::os::fd::BorrowedFd;
use std::os::unix::io::{AsFd, OwnedFd};

use wayland_protocols::ext::data_control::v1::server::ext_data_control_source_v1::ExtDataControlSourceV1;
use wayland_protocols::wp::primary_selection::zv1::server::zwp_primary_selection_source_v1::ZwpPrimarySelectionSourceV1 as PrimarySource;
use wayland_protocols_wlr::data_control::v1::server::zwlr_data_control_source_v1::ZwlrDataControlSourceV1;
use wayland_server::{protocol::wl_data_source::WlDataSource, Resource};

use crate::utils::IsAlive;
use crate::wayland::selection::primary_selection::PrimarySourceUserData;

use super::data_device::DataSourceUserData;
use super::ext_data_control::ExtDataControlSourceUserData;
use super::private::selection_dispatch;
use super::wlr_data_control::DataControlSourceUserData;
use super::SelectionTarget;

/// The source of the selection data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SelectionSource {
    /// The selection source provider.
    pub(crate) provider: SelectionSourceProvider,
}

impl SelectionSource {
    /// Mime types associated with the source.
    pub fn mime_types(&self) -> Vec<String> {
        self.provider.mime_types()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataControlSource {
    Wlr(ZwlrDataControlSourceV1),
    Ext(ExtDataControlSourceV1),
}

impl DataControlSource {
    fn send(&self, mime_type: String, fd: BorrowedFd<'_>) {
        match self {
            Self::Wlr(obj) => obj.send(mime_type, fd),
            Self::Ext(obj) => obj.send(mime_type, fd),
        }
    }

    fn cancelled(&self) {
        match self {
            Self::Wlr(obj) => obj.cancelled(),
            Self::Ext(obj) => obj.cancelled(),
        }
    }

    fn mime_types(&self) -> Vec<String> {
        match self {
            Self::Wlr(source) => {
                let data: &DataControlSourceUserData = source.data().unwrap();
                data.inner.lock().unwrap().mime_types.clone()
            }
            Self::Ext(source) => {
                let data: &ExtDataControlSourceUserData = source.data().unwrap();
                data.inner.lock().unwrap().mime_types.clone()
            }
        }
    }

    fn contains_mime_type(&self, mime_type: &String) -> bool {
        match self {
            Self::Wlr(source) => {
                let data: &DataControlSourceUserData = source.data().unwrap();
                data.inner.lock().unwrap().mime_types.contains(mime_type)
            }
            Self::Ext(source) => {
                let data: &ExtDataControlSourceUserData = source.data().unwrap();
                data.inner.lock().unwrap().mime_types.contains(mime_type)
            }
        }
    }
}

impl IsAlive for DataControlSource {
    #[inline]
    fn alive(&self) -> bool {
        match self {
            Self::Wlr(obj) => obj.alive(),
            Self::Ext(obj) => obj.alive(),
        }
    }
}

/// Provider of the selection data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionSourceProvider {
    /// The source was from the regular data device.
    DataDevice(WlDataSource),
    /// The primary selection was used as a source.
    Primary(PrimarySource),
    /// The data control selection was used as source.
    DataControl(DataControlSource),
}

impl SelectionSourceProvider {
    /// Mark selection source as no longer valid.
    pub fn cancel(&self) {
        selection_dispatch!(self; Self(source) => source.cancelled())
    }

    /// Send the data with the specified `mime_type` using the given `fd`.
    pub fn send(&self, mime_type: String, fd: OwnedFd) {
        selection_dispatch!(self; Self(source) => source.send(mime_type, fd.as_fd()))
    }

    /// Check whether the given selection source contains provided `mime_type`.
    pub fn contains_mime_type(&self, mime_type: &String) -> bool {
        match self {
            Self::DataDevice(source) => {
                let data: &DataSourceUserData = source.data().unwrap();
                data.inner.lock().unwrap().mime_types.contains(mime_type)
            }
            Self::Primary(source) => {
                let data: &PrimarySourceUserData = source.data().unwrap();
                data.inner.lock().unwrap().mime_types.contains(mime_type)
            }
            Self::DataControl(source) => source.contains_mime_type(mime_type),
        }
    }

    /// Get the mime types associated with the given source.
    pub fn mime_types(&self) -> Vec<String> {
        match self {
            Self::DataDevice(source) => {
                let data: &DataSourceUserData = source.data().unwrap();
                data.inner.lock().unwrap().mime_types.clone()
            }
            Self::Primary(source) => {
                let data: &PrimarySourceUserData = source.data().unwrap();
                data.inner.lock().unwrap().mime_types.clone()
            }
            Self::DataControl(source) => source.mime_types(),
        }
    }
}

impl IsAlive for SelectionSourceProvider {
    #[inline]
    fn alive(&self) -> bool {
        selection_dispatch!(self; Self(source) => source.alive())
    }
}

/// The selection used by the compositor UI elements, like EGUI panels.
#[derive(Debug, Clone)]
pub struct CompositorSelectionProvider<U: Clone + Send + Sync + 'static> {
    /// The target used for the end compositor selection.
    pub ty: SelectionTarget,
    pub mime_types: Vec<String>,
    pub user_data: U,
}
