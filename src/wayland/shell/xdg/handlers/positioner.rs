use std::sync::Mutex;

use crate::{utils::Rectangle, wayland::Serial};

use wayland_protocols::xdg_shell::server::{xdg_positioner, xdg_positioner::XdgPositioner};

use wayland_server::{
    backend::{ClientId, ObjectId},
    DataInit, DelegateDispatch, DelegateDispatchBase, DestructionNotify, Dispatch, DisplayHandle, Resource,
    WEnum,
};

use super::{PositionerState, XdgShellHandler, XdgShellState};

/*
 * xdg_positioner
 */

/// User data for Xdg Positioner
#[derive(Default, Debug)]
pub struct XdgPositionerUserData {
    pub(crate) inner: Mutex<PositionerState>,
}

impl DestructionNotify for XdgPositionerUserData {
    fn object_destroyed(&self, _client_id: ClientId, _object_id: ObjectId) {}
}

impl DelegateDispatchBase<XdgPositioner> for XdgShellState {
    type UserData = XdgPositionerUserData;
}

impl<D> DelegateDispatch<XdgPositioner, D> for XdgShellState
where
    D: Dispatch<XdgPositioner, UserData = XdgPositionerUserData>,
    D: XdgShellHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        positioner: &XdgPositioner,
        request: xdg_positioner::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_>,
        _data_init: &mut DataInit<'_, D>,
    ) {
        let mut state = data.inner.lock().unwrap();
        match request {
            xdg_positioner::Request::SetSize { width, height } => {
                if width < 1 || height < 1 {
                    positioner.post_error(
                        cx,
                        xdg_positioner::Error::InvalidInput,
                        "Invalid size for positioner.",
                    );
                } else {
                    state.rect_size = (width, height).into();
                }
            }
            xdg_positioner::Request::SetAnchorRect { x, y, width, height } => {
                if width < 1 || height < 1 {
                    positioner.post_error(
                        cx,
                        xdg_positioner::Error::InvalidInput,
                        "Invalid size for positioner's anchor rectangle.",
                    );
                } else {
                    state.anchor_rect = Rectangle::from_loc_and_size((x, y), (width, height));
                }
            }
            xdg_positioner::Request::SetAnchor { anchor } => {
                if let WEnum::Value(anchor) = anchor {
                    state.anchor_edges = anchor;
                }
            }
            xdg_positioner::Request::SetGravity { gravity } => {
                if let WEnum::Value(gravity) = gravity {
                    state.gravity = gravity;
                }
            }
            xdg_positioner::Request::SetConstraintAdjustment {
                constraint_adjustment,
            } => {
                let constraint_adjustment =
                    xdg_positioner::ConstraintAdjustment::from_bits_truncate(constraint_adjustment);
                state.constraint_adjustment = constraint_adjustment;
            }
            xdg_positioner::Request::SetOffset { x, y } => {
                state.offset = (x, y).into();
            }
            xdg_positioner::Request::SetReactive => {
                state.reactive = true;
            }
            xdg_positioner::Request::SetParentSize {
                parent_width,
                parent_height,
            } => {
                state.parent_size = Some((parent_width, parent_height).into());
            }
            xdg_positioner::Request::SetParentConfigure { serial } => {
                state.parent_configure = Some(Serial::from(serial));
            }
            xdg_positioner::Request::Destroy => {
                // handled by destructor
            }
            _ => unreachable!(),
        }
    }
}
