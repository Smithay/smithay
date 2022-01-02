use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::{cell::RefCell, ops::Deref as _, sync::Mutex};

use crate::wayland::compositor;
use crate::wayland::delegate::{
    DelegateDispatch, DelegateDispatchBase, DelegateGlobalDispatch, DelegateGlobalDispatchBase,
};
use crate::wayland::shell::xdg::{PopupState, XDG_POPUP_ROLE, XDG_TOPLEVEL_ROLE};
use crate::wayland::Serial;
use wayland_protocols::unstable::xdg_decoration::v1::server::zxdg_toplevel_decoration_v1;
use wayland_protocols::xdg_shell::server::xdg_popup::XdgPopup;
use wayland_protocols::xdg_shell::server::xdg_positioner::XdgPositioner;
use wayland_protocols::xdg_shell::server::xdg_surface::XdgSurface;
use wayland_protocols::xdg_shell::server::xdg_toplevel::XdgToplevel;
use wayland_protocols::xdg_shell::server::xdg_wm_base::XdgWmBase;
use wayland_protocols::xdg_shell::server::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_server::backend::{ClientId, ObjectId};
use wayland_server::protocol::wl_surface;
use wayland_server::{
    DataInit, DestructionNotify, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
};

use crate::utils::Rectangle;

use super::{
    InnerState, PopupConfigure, PositionerState, ShellClient, ShellClientData, SurfaceCachedState,
    ToplevelConfigure, XdgPopupSurfaceRoleAttributes, XdgRequest, XdgShellDispatch, XdgShellHandler,
    XdgToplevelSurfaceRoleAttributes,
};

impl<D, H: XdgShellHandler<D>> DelegateGlobalDispatchBase<XdgWmBase> for XdgShellDispatch<'_, D, H> {
    type GlobalData = ();
}

impl<D, H> DelegateGlobalDispatch<XdgWmBase, D> for XdgShellDispatch<'_, D, H>
where
    D: GlobalDispatch<XdgWmBase, GlobalData = ()>
        + Dispatch<XdgWmBase, UserData = XdgWmBaseUserData>
        + Dispatch<XdgSurface, UserData = XdgSurfaceUserData>
        + Dispatch<XdgPositioner, UserData = XdgPositionerUserData>
        + 'static,
    H: XdgShellHandler<D>,
{
    fn bind(
        &mut self,
        cx: &mut DisplayHandle<'_, D>,
        _client: &wayland_server::Client,
        resource: New<XdgWmBase>,
        _global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, D>,
    ) {
        let shell = data_init.init(resource, XdgWmBaseUserData::default());

        self.1.request(
            cx,
            XdgRequest::NewClient {
                client: ShellClient::new(&shell),
            },
        );
    }
}

impl<D, H: XdgShellHandler<D>> DelegateDispatchBase<XdgWmBase> for XdgShellDispatch<'_, D, H> {
    type UserData = XdgWmBaseUserData;
}

impl<D, H> DelegateDispatch<XdgWmBase, D> for XdgShellDispatch<'_, D, H>
where
    D: Dispatch<XdgWmBase, UserData = XdgWmBaseUserData>
        + Dispatch<XdgSurface, UserData = XdgSurfaceUserData>
        + Dispatch<XdgPositioner, UserData = XdgPositionerUserData>
        + 'static,
    H: XdgShellHandler<D>,
{
    fn request(
        &mut self,
        _client: &wayland_server::Client,
        shell: &XdgWmBase,
        request: xdg_wm_base::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, D>,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xdg_wm_base::Request::CreatePositioner { id } => {
                data_init.init(id, Default::default());
            }
            xdg_wm_base::Request::GetXdgSurface { id, surface } => {
                // Do not assign a role to the surface here
                // xdg_surface is not role, only xdg_toplevel and
                // xdg_popup are defined as roles

                data_init.init(
                    id,
                    XdgSurfaceUserData {
                        wl_surface: surface,
                        wm_base: shell.deref().clone(),
                        has_active_role: AtomicBool::new(false),
                    },
                );
            }
            xdg_wm_base::Request::Pong { serial } => {
                let serial = Serial::from(serial);
                let valid = {
                    let mut guard = data.client_data.lock().unwrap();
                    if guard.pending_ping == Some(serial) {
                        guard.pending_ping = None;
                        true
                    } else {
                        false
                    }
                };
                if valid {
                    self.1.request(
                        cx,
                        XdgRequest::ClientPong {
                            client: ShellClient::new(&shell),
                        },
                    );
                }
            }
            xdg_wm_base::Request::Destroy => {
                // all is handled by destructor
            }
            _ => unreachable!(),
        }
    }
}

/*
 * xdg_shell
 */

/// User data for Xdg Wm Base
#[derive(Default, Debug)]
pub struct XdgWmBaseUserData {
    pub(crate) client_data: Mutex<ShellClientData>,
}

impl DestructionNotify for XdgWmBaseUserData {
    fn object_destroyed(&self, _client_id: ClientId, _object_id: ObjectId) {}
}

/*
 * xdg_positioner
 */

/// User data for Xdg Positioner
#[derive(Default, Debug)]
pub struct XdgPositionerUserData {
    inner: Mutex<PositionerState>,
}

impl DestructionNotify for XdgPositionerUserData {
    fn object_destroyed(&self, _client_id: ClientId, _object_id: ObjectId) {}
}

impl<D, H: XdgShellHandler<D>> DelegateDispatchBase<XdgPositioner> for XdgShellDispatch<'_, D, H> {
    type UserData = XdgPositionerUserData;
}

impl<D, H> DelegateDispatch<XdgPositioner, D> for XdgShellDispatch<'_, D, H>
where
    D: Dispatch<XdgPositioner, UserData = XdgPositionerUserData> + 'static,
    H: XdgShellHandler<D>,
{
    fn request(
        &mut self,
        _client: &wayland_server::Client,
        positioner: &XdgPositioner,
        request: xdg_positioner::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, D>,
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

/*
 * xdg_surface
 */

/// User data of XdgSurface
#[derive(Debug)]
pub struct XdgSurfaceUserData {
    wl_surface: wl_surface::WlSurface,
    wm_base: xdg_wm_base::XdgWmBase,
    has_active_role: AtomicBool,
}

impl<D, H: XdgShellHandler<D>> DelegateDispatchBase<XdgSurface> for XdgShellDispatch<'_, D, H> {
    type UserData = XdgSurfaceUserData;
}

impl<D, H> DelegateDispatch<XdgSurface, D> for XdgShellDispatch<'_, D, H>
where
    D: Dispatch<XdgSurface, UserData = XdgSurfaceUserData>
        + Dispatch<XdgToplevel, UserData = XdgShellSurfaceUserData>
        + Dispatch<XdgPopup, UserData = XdgShellSurfaceUserData>
        + 'static,
    H: XdgShellHandler<D>,
{
    fn request(
        &mut self,
        _client: &wayland_server::Client,
        xdg_surface: &XdgSurface,
        request: xdg_surface::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, D>,
        data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xdg_surface::Request::Destroy => {
                // all is handled by our destructor
            }
            xdg_surface::Request::GetToplevel { id } => {
                // We now can assign a role to the surface
                let surface = &data.wl_surface;
                let shell = &data.wm_base;

                if compositor::give_role(cx, surface, XDG_TOPLEVEL_ROLE).is_err() {
                    shell.post_error(cx, xdg_wm_base::Error::Role, "Surface already has a role.");
                    return;
                }

                data.has_active_role.store(true, Ordering::Release);

                compositor::with_states::<D, _, _>(surface, |states| {
                    states.data_map.insert_if_missing_threadsafe(|| {
                        Mutex::new(XdgToplevelSurfaceRoleAttributes::default())
                    })
                })
                .unwrap();

                compositor::add_commit_hook(cx, surface, super::ToplevelSurface::commit_hook::<D>);

                let toplevel = data_init.init(
                    id,
                    XdgShellSurfaceUserData {
                        kind: SurfaceKind::Toplevel,
                        shell_data: self.0.inner.clone(),
                        wl_surface: data.wl_surface.clone(),
                        xdg_surface: xdg_surface.clone(),
                        wm_base: data.wm_base.clone(),
                        decoration: Default::default(),
                    },
                );

                self.0
                    .inner
                    .lock()
                    .unwrap()
                    .known_toplevels
                    .push(make_toplevel_handle(&toplevel));

                let handle = make_toplevel_handle(&toplevel);
                self.1.request(cx, XdgRequest::NewToplevel { surface: handle });
            }
            xdg_surface::Request::GetPopup {
                id,
                parent,
                positioner,
            } => {
                let positioner_data = *positioner
                    .data::<XdgPositionerUserData>()
                    .unwrap()
                    .inner
                    .lock()
                    .unwrap();

                let parent_surface = parent.map(|parent| {
                    let parent_data = parent.data::<XdgSurfaceUserData>().unwrap();
                    parent_data.wl_surface.clone()
                });

                // We now can assign a role to the surface
                let surface = &data.wl_surface;
                let shell = &data.wm_base;

                let attributes = XdgPopupSurfaceRoleAttributes {
                    parent: parent_surface,
                    server_pending: Some(PopupState {
                        // Set the positioner data as the popup geometry
                        geometry: positioner_data.get_geometry(),
                        positioner: positioner_data,
                    }),
                    ..Default::default()
                };
                if compositor::give_role(cx, surface, XDG_POPUP_ROLE).is_err() {
                    shell.post_error(cx, xdg_wm_base::Error::Role, "Surface already has a role.");
                    return;
                }

                data.has_active_role.store(true, Ordering::Release);

                compositor::with_states::<D, _, _>(surface, |states| {
                    states.data_map.insert_if_missing_threadsafe(|| {
                        Mutex::new(XdgPopupSurfaceRoleAttributes::default())
                    });
                    *states
                        .data_map
                        .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                        .unwrap()
                        .lock()
                        .unwrap() = attributes;
                })
                .unwrap();

                compositor::add_commit_hook(cx, surface, super::PopupSurface::commit_hook::<D>);

                let popup = data_init.init(
                    id,
                    XdgShellSurfaceUserData {
                        kind: SurfaceKind::Popup,
                        shell_data: self.0.inner.clone(),
                        wl_surface: data.wl_surface.clone(),
                        xdg_surface: xdg_surface.clone(),
                        wm_base: data.wm_base.clone(),
                        decoration: Default::default(),
                    },
                );

                self.0
                    .inner
                    .lock()
                    .unwrap()
                    .known_popups
                    .push(make_popup_handle(&popup));

                let handle = make_popup_handle(&popup);
                self.1.request(
                    cx,
                    XdgRequest::NewPopup {
                        surface: handle,
                        positioner: positioner_data,
                    },
                );
            }
            xdg_surface::Request::SetWindowGeometry { x, y, width, height } => {
                // Check the role of the surface, this can be either xdg_toplevel
                // or xdg_popup. If none of the role matches the xdg_surface has no role set
                // which is a protocol error.
                let surface = &data.wl_surface;

                let role = compositor::get_role(cx, surface);

                if role.is_none() {
                    xdg_surface.post_error(
                        cx,
                        xdg_surface::Error::NotConstructed,
                        "xdg_surface must have a role.",
                    );
                    return;
                }

                if role != Some(XDG_TOPLEVEL_ROLE) && role != Some(XDG_POPUP_ROLE) {
                    data.wm_base.post_error(
                        cx,
                        xdg_wm_base::Error::Role,
                        "xdg_surface must have a role of xdg_toplevel or xdg_popup.",
                    );
                }

                compositor::with_states::<D, _, _>(surface, |states| {
                    states.cached_state.pending::<SurfaceCachedState>().geometry =
                        Some(Rectangle::from_loc_and_size((x, y), (width, height)));
                })
                .unwrap();
            }
            xdg_surface::Request::AckConfigure { serial } => {
                let serial = Serial::from(serial);
                let surface = &data.wl_surface;

                // Check the role of the surface, this can be either xdg_toplevel
                // or xdg_popup. If none of the role matches the xdg_surface has no role set
                // which is a protocol error.
                if compositor::get_role(cx, surface).is_none() {
                    xdg_surface.post_error(
                        cx,
                        xdg_surface::Error::NotConstructed,
                        "xdg_surface must have a role.",
                    );
                    return;
                }

                // Find the correct configure state for the provided serial
                // discard all configure states that are older than the provided
                // serial.
                // If no matching serial can be found raise a protocol error
                //
                // Invoke the user impl with the found configuration
                // This has to include the serial and the role specific data.
                // - For xdg_popup there is no data.
                // - For xdg_toplevel send the state data including
                //   width, height, min/max size, maximized, fullscreen, resizing, activated
                //
                // This can be used to integrate custom protocol extensions
                let found_configure = compositor::with_states::<D, _, _>(surface, |states| {
                    if states.role == Some(XDG_TOPLEVEL_ROLE) {
                        Ok(states
                            .data_map
                            .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
                            .unwrap()
                            .lock()
                            .unwrap()
                            .ack_configure(serial))
                    } else if states.role == Some(XDG_POPUP_ROLE) {
                        Ok(states
                            .data_map
                            .get::<Mutex<XdgPopupSurfaceRoleAttributes>>()
                            .unwrap()
                            .lock()
                            .unwrap()
                            .ack_configure(serial))
                    } else {
                        Err(())
                    }
                })
                .unwrap();

                let configure = match found_configure {
                    Ok(Some(configure)) => configure,
                    Ok(None) => {
                        data.wm_base.post_error(
                            cx,
                            xdg_wm_base::Error::InvalidSurfaceState,
                            format!("wrong configure serial: {}", <u32>::from(serial)),
                        );
                        return;
                    }
                    Err(()) => {
                        data.wm_base.post_error(
                            cx,
                            xdg_wm_base::Error::Role as u32,
                            "xdg_surface must have a role of xdg_toplevel or xdg_popup.",
                        );
                        return;
                    }
                };

                self.1.request(
                    cx,
                    XdgRequest::AckConfigure {
                        surface: surface.clone(),
                        configure,
                    },
                );
            }
            _ => unreachable!(),
        }
    }
}

impl DestructionNotify for XdgSurfaceUserData {
    fn object_destroyed(&self, _client_id: ClientId, _object_id: ObjectId) {
        // if !self.wl_surface.as_ref().is_alive() {
        //     // the wl_surface is destroyed, this means the client is not
        //     // trying to change the role but it's a cleanup (possibly a
        //     // disconnecting client), ignore the protocol check.
        //     return;
        // }

        // if compositor::get_role(&data.wl_surface).is_none() {
        //     // No role assigned to the surface, we can exit early.
        //     return;
        // }

        // if data.has_active_role.load(Ordering::Acquire) {
        //     data.wm_base.as_ref().post_error(
        //         xdg_wm_base::Error::Role as u32,
        //         "xdg_surface was destroyed before its role object".into(),
        //     );
        // }
    }
}

/*
 * xdg_toplevel
 */

#[derive(Debug)]
enum SurfaceKind {
    Toplevel,
    Popup,
}

/// User data of xdg toplevel surface
#[derive(Debug)]
pub struct XdgShellSurfaceUserData {
    kind: SurfaceKind,
    pub(crate) shell_data: Arc<Mutex<InnerState>>,
    pub(crate) wl_surface: wl_surface::WlSurface,
    pub(crate) wm_base: xdg_wm_base::XdgWmBase,
    pub(crate) xdg_surface: xdg_surface::XdgSurface,
    pub(crate) decoration: Mutex<Option<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>>,
}

impl DestructionNotify for XdgShellSurfaceUserData {
    fn object_destroyed(&self, _client_id: ClientId, object_id: ObjectId) {
        if let Some(data) = self.xdg_surface.data::<XdgSurfaceUserData>() {
            data.has_active_role.store(false, Ordering::Release);
        }

        match &self.kind {
            SurfaceKind::Toplevel => {
                // remove this surface from the known ones (as well as any leftover dead surface)
                self.shell_data
                    .lock()
                    .unwrap()
                    .known_toplevels
                    .retain(|other| other.shell_surface.id() != object_id);
            }
            SurfaceKind::Popup => {
                // remove this surface from the known ones (as well as any leftover dead surface)
                self.shell_data
                    .lock()
                    .unwrap()
                    .known_popups
                    .retain(|other| other.shell_surface.id() != object_id);
            }
        }
    }
}

impl<D, H: XdgShellHandler<D>> DelegateDispatchBase<XdgToplevel> for XdgShellDispatch<'_, D, H> {
    type UserData = XdgShellSurfaceUserData;
}

impl<D, H> DelegateDispatch<XdgToplevel, D> for XdgShellDispatch<'_, D, H>
where
    D: Dispatch<XdgToplevel, UserData = XdgShellSurfaceUserData> + 'static,
    H: XdgShellHandler<D>,
{
    fn request(
        &mut self,
        _client: &wayland_server::Client,
        toplevel: &XdgToplevel,
        request: xdg_toplevel::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, D>,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xdg_toplevel::Request::Destroy => {
                // all it done by the destructor
            }
            xdg_toplevel::Request::SetParent { parent } => {
                let parent_surface = parent.map(|toplevel_surface_parent| {
                    toplevel_surface_parent
                        .data::<XdgShellSurfaceUserData>()
                        .unwrap()
                        .wl_surface
                        .clone()
                });

                // Parent is not double buffered, we can set it directly
                set_parent::<D>(&toplevel, parent_surface);
            }
            xdg_toplevel::Request::SetTitle { title } => {
                // Title is not double buffered, we can set it directly
                with_surface_toplevel_role_data::<D, _, _>(&toplevel, |data| {
                    data.title = Some(title);
                });
            }
            xdg_toplevel::Request::SetAppId { app_id } => {
                // AppId is not double buffered, we can set it directly
                with_surface_toplevel_role_data::<D, _, _>(&toplevel, |role| {
                    role.app_id = Some(app_id);
                });
            }
            xdg_toplevel::Request::ShowWindowMenu { seat, serial, x, y } => {
                // This has to be handled by the compositor
                let handle = make_toplevel_handle(&toplevel);
                let serial = Serial::from(serial);

                self.1.request(
                    cx,
                    XdgRequest::ShowWindowMenu {
                        surface: handle,
                        seat,
                        serial,
                        location: (x, y).into(),
                    },
                );
            }
            xdg_toplevel::Request::Move { seat, serial } => {
                // This has to be handled by the compositor
                let handle = make_toplevel_handle(&toplevel);
                let serial = Serial::from(serial);

                self.1.request(
                    cx,
                    XdgRequest::Move {
                        surface: handle,
                        seat,
                        serial,
                    },
                );
            }
            xdg_toplevel::Request::Resize { seat, serial, edges } => {
                if let WEnum::Value(edges) = edges {
                    // This has to be handled by the compositor
                    let handle = make_toplevel_handle(&toplevel);
                    let serial = Serial::from(serial);

                    self.1.request(
                        cx,
                        XdgRequest::Resize {
                            surface: handle,
                            seat,
                            serial,
                            edges,
                        },
                    );
                }
            }
            xdg_toplevel::Request::SetMaxSize { width, height } => {
                with_toplevel_pending_state::<D, _, _>(data, |toplevel_data| {
                    toplevel_data.max_size = (width, height).into();
                });
            }
            xdg_toplevel::Request::SetMinSize { width, height } => {
                with_toplevel_pending_state::<D, _, _>(data, |toplevel_data| {
                    toplevel_data.min_size = (width, height).into();
                });
            }
            xdg_toplevel::Request::SetMaximized => {
                let handle = make_toplevel_handle(&toplevel);
                self.1.request(cx, XdgRequest::Maximize { surface: handle });
            }
            xdg_toplevel::Request::UnsetMaximized => {
                let handle = make_toplevel_handle(&toplevel);
                self.1.request(cx, XdgRequest::UnMaximize { surface: handle });
            }
            xdg_toplevel::Request::SetFullscreen { output } => {
                let handle = make_toplevel_handle(&toplevel);
                self.1.request(
                    cx,
                    XdgRequest::Fullscreen {
                        surface: handle,
                        output,
                    },
                );
            }
            xdg_toplevel::Request::UnsetFullscreen => {
                let handle = make_toplevel_handle(&toplevel);
                self.1.request(cx, XdgRequest::UnFullscreen { surface: handle });
            }
            xdg_toplevel::Request::SetMinimized => {
                // This has to be handled by the compositor, may not be
                // supported and just ignored
                let handle = make_toplevel_handle(&toplevel);
                self.1.request(cx, XdgRequest::Minimize { surface: handle });
            }
            _ => unreachable!(),
        }
    }
}

// Utility functions allowing to factor out a lot of the upcoming logic
fn with_surface_toplevel_role_data<D, F, T>(toplevel: &xdg_toplevel::XdgToplevel, f: F) -> T
where
    F: FnOnce(&mut XdgToplevelSurfaceRoleAttributes) -> T,
    D: 'static,
{
    let data = toplevel.data::<XdgShellSurfaceUserData>().unwrap();
    compositor::with_states::<D, _, _>(&data.wl_surface, |states| {
        f(&mut *states
            .data_map
            .get::<Mutex<XdgToplevelSurfaceRoleAttributes>>()
            .unwrap()
            .lock()
            .unwrap())
    })
    .unwrap()
}

fn with_toplevel_pending_state<D, F, T>(data: &XdgShellSurfaceUserData, f: F) -> T
where
    F: FnOnce(&mut SurfaceCachedState) -> T,
    D: 'static,
{
    compositor::with_states::<D, _, _>(&data.wl_surface, |states| {
        f(&mut *states.cached_state.pending::<SurfaceCachedState>())
    })
    .unwrap()
}

pub fn send_toplevel_configure<D>(
    cx: &mut DisplayHandle<'_, D>,
    resource: &xdg_toplevel::XdgToplevel,
    configure: ToplevelConfigure,
) {
    let data = resource.data::<XdgShellSurfaceUserData>().unwrap();

    let (width, height) = configure.state.size.unwrap_or_default().into();
    // convert the Vec<State> (which is really a Vec<u32>) into Vec<u8>
    let states = {
        let mut states: Vec<xdg_toplevel::State> =
            configure.state.states.into_filtered_states(resource.version());
        let ptr = states.as_mut_ptr();
        let len = states.len();
        let cap = states.capacity();
        ::std::mem::forget(states);
        unsafe { Vec::from_raw_parts(ptr as *mut u8, len * 4, cap * 4) }
    };
    let serial = configure.serial;

    // Send the toplevel configure
    resource.configure(cx, width, height, states);

    // Send the base xdg_surface configure event to mark
    // The configure as finished
    data.xdg_surface.configure(cx, serial.into());
}

fn make_toplevel_handle(resource: &xdg_toplevel::XdgToplevel) -> super::ToplevelSurface {
    let data = resource.data::<XdgShellSurfaceUserData>().unwrap();
    super::ToplevelSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: resource.clone(),
    }
}

/*
 * xdg_popup
 */
pub(crate) fn send_popup_configure<D>(
    cx: &mut DisplayHandle<'_, D>,
    resource: &xdg_popup::XdgPopup,
    configure: PopupConfigure,
) {
    let data = resource.data::<XdgShellSurfaceUserData>().unwrap();

    let serial = configure.serial;
    let geometry = configure.state.geometry;

    // Send repositioned if token is set
    if let Some(token) = configure.reposition_token {
        resource.repositioned(cx, token);
    }

    // Send the popup configure
    resource.configure(
        cx,
        geometry.loc.x,
        geometry.loc.y,
        geometry.size.w,
        geometry.size.h,
    );

    // Send the base xdg_surface configure event to mark
    // the configure as finished
    data.xdg_surface.configure(cx, serial.into());
}

fn make_popup_handle(resource: &xdg_popup::XdgPopup) -> super::PopupSurface {
    let data = resource.data::<XdgShellSurfaceUserData>().unwrap();
    super::PopupSurface {
        wl_surface: data.wl_surface.clone(),
        shell_surface: resource.clone(),
    }
}

impl<D, H: XdgShellHandler<D>> DelegateDispatchBase<XdgPopup> for XdgShellDispatch<'_, D, H> {
    type UserData = XdgShellSurfaceUserData;
}

impl<D, H> DelegateDispatch<XdgPopup, D> for XdgShellDispatch<'_, D, H>
where
    D: Dispatch<XdgPopup, UserData = XdgShellSurfaceUserData> + 'static,
    H: XdgShellHandler<D>,
{
    fn request(
        &mut self,
        _client: &wayland_server::Client,
        popup: &XdgPopup,
        request: xdg_popup::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, D>,
        _data_init: &mut DataInit<'_, D>,
    ) {
        match request {
            xdg_popup::Request::Destroy => {
                // all is handled by our destructor
            }
            xdg_popup::Request::Grab { seat, serial } => {
                let handle = super::PopupSurface {
                    wl_surface: data.wl_surface.clone(),
                    shell_surface: popup.clone(),
                };

                let serial = Serial::from(serial);

                self.1.request(
                    cx,
                    XdgRequest::Grab {
                        surface: handle,
                        seat,
                        serial,
                    },
                );
            }
            xdg_popup::Request::Reposition { positioner, token } => {
                let handle = super::PopupSurface {
                    wl_surface: data.wl_surface.clone(),
                    shell_surface: popup.clone(),
                };

                let positioner_data = *positioner
                    .data::<XdgPositionerUserData>()
                    .unwrap()
                    .inner
                    .lock()
                    .unwrap();

                self.1.request(
                    cx,
                    XdgRequest::RePosition {
                        surface: handle,
                        positioner: positioner_data,
                        token,
                    },
                );
            }
            _ => unreachable!(),
        }
    }
}

// fn destroy_popup(popup: xdg_popup::XdgPopup) {
// }

pub(crate) fn get_parent<D: 'static>(toplevel: &xdg_toplevel::XdgToplevel) -> Option<wl_surface::WlSurface> {
    with_surface_toplevel_role_data::<D, _, _>(toplevel, |data| data.parent.clone())
}

/// Sets the parent of the specified toplevel surface.
///
/// The parent must be a toplevel surface.
///
/// The parent of a surface is not double buffered and therefore may be set directly.
///
/// If the parent is `None`, the parent-child relationship is removed.
pub(crate) fn set_parent<D: 'static>(
    toplevel: &xdg_toplevel::XdgToplevel,
    parent: Option<wl_surface::WlSurface>,
) {
    with_surface_toplevel_role_data::<D, _, _>(toplevel, |data| {
        data.parent = parent;
    });
}
