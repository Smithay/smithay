//! Protocol for confining the pointer.
//!
//! This provides a way for the client to request that the pointer is confined to a region or
//! locked in place.
use std::{
    collections::{hash_map, HashMap},
    ops,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

use wayland_protocols::wp::pointer_constraints::zv1::server::{
    zwp_confined_pointer_v1::{self, ZwpConfinedPointerV1},
    zwp_locked_pointer_v1::{self, ZwpLockedPointerV1},
    zwp_pointer_constraints_v1::{self, Lifetime, ZwpPointerConstraintsV1},
};
use wayland_server::{
    backend::GlobalId, protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle,
    GlobalDispatch, New, Resource, WEnum,
};

use super::compositor::{self, RegionAttributes};
use crate::{
    input::{pointer::PointerHandle, SeatHandler},
    utils::{Logical, Point},
    wayland::seat::PointerUserData,
};

const VERSION: u32 = 1;

/// Handler for pointer constraints
pub trait PointerConstraintsHandler: SeatHandler {
    /// Pointer lock or confinement constraint created for `pointer` on `surface`
    ///
    /// Use [`with_pointer_constraint`] to access the constraint.
    fn new_constraint(&mut self, surface: &WlSurface, pointer: &PointerHandle<Self>);

    /// The client holding a LockedPointer has commited a cursor position hint.
    ///
    /// This is emitted upon a surface commit if the cursor position hint has been updated.
    ///
    /// Use [`with_pointer_constraint`] to access the constraint and check if it is active.
    fn cursor_position_hint(
        &mut self,
        surface: &WlSurface,
        pointer: &PointerHandle<Self>,
        location: Point<f64, Logical>,
    );
}

/// Constraint confining pointer to a region of the surface
#[derive(Debug)]
pub struct ConfinedPointer {
    handle: zwp_confined_pointer_v1::ZwpConfinedPointerV1,
    region: Option<RegionAttributes>,
    pending_region: Option<RegionAttributes>,
    lifetime: WEnum<Lifetime>,
    active: AtomicBool,
}

impl ConfinedPointer {
    /// Region in which to confine the pointer
    pub fn region(&self) -> Option<&RegionAttributes> {
        self.region.as_ref()
    }
}

/// Constraint locking pointer in place
#[derive(Debug)]
pub struct LockedPointer {
    handle: zwp_locked_pointer_v1::ZwpLockedPointerV1,
    region: Option<RegionAttributes>,
    pending_region: Option<RegionAttributes>,
    lifetime: WEnum<Lifetime>,
    cursor_position_hint: Option<Point<f64, Logical>>,
    pending_cursor_position_hint: Option<Point<f64, Logical>>,
    active: AtomicBool,
}

impl LockedPointer {
    /// Region in which to activate the lock
    pub fn region(&self) -> Option<&RegionAttributes> {
        self.region.as_ref()
    }

    /// Position the client is rendering a cursor, if any
    pub fn cursor_position_hint(&self) -> Option<Point<f64, Logical>> {
        self.cursor_position_hint
    }
}

/// A constraint imposed on the pointer instance
#[derive(Debug)]
pub enum PointerConstraint {
    /// Pointer is confined to a region of the surface
    Confined(ConfinedPointer),
    /// Pointer is locked in place
    Locked(LockedPointer),
}

/// A reference to a pointer constraint that can be activated or deactivated.
///
/// The derefs to `[PointerConstraint]`.
#[derive(Debug)]
pub struct PointerConstraintRef<'a, D: SeatHandler + 'static> {
    entry: hash_map::OccupiedEntry<'a, PointerHandle<D>, PointerConstraint>,
}

impl<'a, D: SeatHandler + 'static> ops::Deref for PointerConstraintRef<'a, D> {
    type Target = PointerConstraint;

    fn deref(&self) -> &Self::Target {
        self.entry.get()
    }
}

impl<'a, D: SeatHandler + 'static> PointerConstraintRef<'a, D> {
    /// Send `locked`/`unlocked`
    ///
    /// This is not sent automatically since compositors may have different
    /// policies about when to allow and activate constraints.
    pub fn activate(&self) {
        match self.entry.get() {
            PointerConstraint::Confined(confined) => {
                confined.handle.confined();
                confined.active.store(true, Ordering::SeqCst);
            }
            PointerConstraint::Locked(locked) => {
                locked.handle.locked();
                locked.active.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Send `unlocked`/`unconfined`
    ///
    /// For oneshot constraints, will destroy the constraint.
    ///
    /// This is sent automatically when the surface loses pointer focus, but
    /// may also be invoked while the surface is focused.
    pub fn deactivate(self) {
        match self.entry.get() {
            PointerConstraint::Confined(confined) => {
                confined.handle.unconfined();
                confined.active.store(false, Ordering::SeqCst);
            }
            PointerConstraint::Locked(locked) => {
                locked.handle.unlocked();
                locked.active.store(false, Ordering::SeqCst);
            }
        }

        if self.lifetime() == WEnum::Value(Lifetime::Oneshot) {
            self.entry.remove_entry();
        }
    }
}

impl PointerConstraint {
    /// Constraint is active
    pub fn is_active(&self) -> bool {
        match self {
            PointerConstraint::Confined(confined) => &confined.active,
            PointerConstraint::Locked(locked) => &locked.active,
        }
        .load(Ordering::SeqCst)
    }

    /// Region in which to lock or confine the pointer
    pub fn region(&self) -> Option<&RegionAttributes> {
        match self {
            PointerConstraint::Confined(confined) => confined.region(),
            PointerConstraint::Locked(locked) => locked.region(),
        }
    }

    fn lifetime(&self) -> WEnum<Lifetime> {
        match self {
            PointerConstraint::Confined(confined) => confined.lifetime,
            PointerConstraint::Locked(locked) => locked.lifetime,
        }
    }

    /// Commits the pending state of the constraint, and returns the cursor position hint if it has changed.
    fn commit(&mut self) -> Option<Point<f64, Logical>> {
        match self {
            Self::Confined(confined) => {
                confined.region.clone_from(&confined.pending_region);
                None
            }
            Self::Locked(locked) => {
                locked.region.clone_from(&locked.pending_region);
                locked.pending_cursor_position_hint.take().map(|hint| {
                    locked.cursor_position_hint = Some(hint);
                    hint
                })
            }
        }
    }
}

/// Pointer constraints state.
#[derive(Debug)]
pub struct PointerConstraintsState {
    global: GlobalId,
}

impl PointerConstraintsState {
    /// Create a new pointer constraints global
    pub fn new<D>(display: &DisplayHandle) -> Self
    where
        D: GlobalDispatch<ZwpPointerConstraintsV1, ()>,
        D: Dispatch<ZwpPointerConstraintsV1, ()>,
        D: Dispatch<ZwpConfinedPointerV1, PointerConstraintUserData<D>>,
        D: Dispatch<ZwpLockedPointerV1, PointerConstraintUserData<D>>,
        D: SeatHandler,
        D: 'static,
    {
        let global = display.create_global::<D, ZwpPointerConstraintsV1, _>(VERSION, ());

        Self { global }
    }

    /// Get the id of ZwpPointerConstraintsV1 global
    pub fn global(&self) -> GlobalId {
        self.global.clone()
    }
}

#[doc(hidden)]
#[derive(Debug)]
pub struct PointerConstraintUserData<D: SeatHandler> {
    surface: WlSurface,
    pointer: Option<PointerHandle<D>>,
}

struct PointerConstraintData<D: SeatHandler + 'static> {
    constraints: HashMap<PointerHandle<D>, PointerConstraint>,
}

// TODO Public method to get current constraints for surface/seat
/// Get constraint for surface and pointer, if any
pub fn with_pointer_constraint<
    D: SeatHandler + 'static,
    T,
    F: FnOnce(Option<PointerConstraintRef<'_, D>>) -> T,
>(
    surface: &WlSurface,
    pointer: &PointerHandle<D>,
    f: F,
) -> T {
    with_constraint_data::<D, _, _>(surface, |data| {
        let constraint = data.and_then(|data| match data.constraints.entry(pointer.clone()) {
            hash_map::Entry::Occupied(entry) => Some(PointerConstraintRef { entry }),
            hash_map::Entry::Vacant(_) => None,
        });
        f(constraint)
    })
}

fn commit_hook<D: SeatHandler + PointerConstraintsHandler + 'static>(
    state: &mut D,
    _dh: &DisplayHandle,
    surface: &WlSurface,
) {
    // `with_constraint_data` locks the pointer constraints,
    // so we collect the hints first into a Vec, then release the mutex
    // and only once the mutex is released, we call the handler method.
    //
    // This is to avoid deadlocks when the handler method might try to access the constraints again.
    // It's not a hypothetical, it bit me while implementing the position hint functionality.
    let position_hints = with_constraint_data::<D, _, _>(surface, |data| {
        let data = data.unwrap();
        data.constraints
            .iter_mut()
            .filter_map(|(pointer, constraint)| constraint.commit().map(|hint| (pointer.clone(), hint)))
            .collect::<Vec<_>>()
    });

    for (pointer, hint) in position_hints {
        state.cursor_position_hint(surface, &pointer, hint);
    }
}

/// Get `PointerConstraintData` associated with a surface, if any.
fn with_constraint_data<
    D: SeatHandler + 'static,
    T,
    F: FnOnce(Option<&mut PointerConstraintData<D>>) -> T,
>(
    surface: &WlSurface,
    f: F,
) -> T {
    compositor::with_states(surface, |states| {
        let data = states.data_map.get::<Mutex<PointerConstraintData<D>>>();
        if let Some(data) = data {
            f(Some(&mut data.lock().unwrap()))
        } else {
            f(None)
        }
    })
}

/// Add constraint for surface, or raise protocol error if one exists
fn add_constraint<D: SeatHandler + PointerConstraintsHandler + 'static>(
    pointer_constraints: &ZwpPointerConstraintsV1,
    surface: &WlSurface,
    pointer: &PointerHandle<D>,
    constraint: PointerConstraint,
) {
    let mut added = false;
    compositor::with_states(surface, |states| {
        added = states.data_map.insert_if_missing_threadsafe(|| {
            Mutex::new(PointerConstraintData::<D> {
                constraints: HashMap::new(),
            })
        });
        let data = states.data_map.get::<Mutex<PointerConstraintData<D>>>().unwrap();
        let mut data = data.lock().unwrap();

        if data.constraints.contains_key(pointer) {
            pointer_constraints.post_error(
                zwp_pointer_constraints_v1::Error::AlreadyConstrained,
                "pointer constrait already exists for surface and seat",
            );
        } else {
            data.constraints.insert(pointer.clone(), constraint);
        }
    });

    if added {
        compositor::add_post_commit_hook(surface, commit_hook::<D>);
    }
}

fn remove_constraint<D: SeatHandler + 'static>(surface: &WlSurface, pointer: &PointerHandle<D>) {
    with_constraint_data::<D, _, _>(surface, |data| {
        if let Some(data) = data {
            data.constraints.remove(pointer);
        }
    });
}

impl<D> Dispatch<ZwpPointerConstraintsV1, (), D> for PointerConstraintsState
where
    D: Dispatch<ZwpPointerConstraintsV1, ()>,
    D: Dispatch<ZwpConfinedPointerV1, PointerConstraintUserData<D>>,
    D: Dispatch<ZwpLockedPointerV1, PointerConstraintUserData<D>>,
    D: SeatHandler,
    D: PointerConstraintsHandler,
    D: 'static,
{
    fn request(
        state: &mut D,
        _client: &wayland_server::Client,
        pointer_constraints: &ZwpPointerConstraintsV1,
        request: zwp_pointer_constraints_v1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        match request {
            zwp_pointer_constraints_v1::Request::LockPointer {
                id,
                surface,
                pointer,
                region,
                lifetime,
            } => {
                let region = region.as_ref().map(compositor::get_region_attributes);
                let pointer = pointer.data::<PointerUserData<D>>().unwrap().handle.clone();
                let handle = data_init.init(
                    id,
                    PointerConstraintUserData {
                        surface: surface.clone(),
                        pointer: pointer.clone(),
                    },
                );
                if let Some(pointer) = pointer {
                    add_constraint(
                        pointer_constraints,
                        &surface,
                        &pointer,
                        PointerConstraint::Locked(LockedPointer {
                            handle,
                            region: region.clone(),
                            pending_region: region,
                            lifetime,
                            cursor_position_hint: None,
                            pending_cursor_position_hint: None,
                            active: AtomicBool::new(false),
                        }),
                    );
                    state.new_constraint(&surface, &pointer);
                }
            }
            zwp_pointer_constraints_v1::Request::ConfinePointer {
                id,
                surface,
                pointer,
                region,
                lifetime,
            } => {
                let region = region.as_ref().map(compositor::get_region_attributes);
                let pointer = pointer.data::<PointerUserData<D>>().unwrap().handle.clone();
                let handle = data_init.init(
                    id,
                    PointerConstraintUserData {
                        surface: surface.clone(),
                        pointer: pointer.clone(),
                    },
                );
                if let Some(pointer) = pointer {
                    add_constraint(
                        pointer_constraints,
                        &surface,
                        &pointer,
                        PointerConstraint::Confined(ConfinedPointer {
                            handle,
                            region: region.clone(),
                            pending_region: region,
                            lifetime,
                            active: AtomicBool::new(false),
                        }),
                    );
                    state.new_constraint(&surface, &pointer);
                }
            }
            zwp_pointer_constraints_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D> GlobalDispatch<ZwpPointerConstraintsV1, (), D> for PointerConstraintsState
where
    D: GlobalDispatch<ZwpPointerConstraintsV1, ()>
        + Dispatch<ZwpPointerConstraintsV1, ()>
        + SeatHandler
        + 'static,
{
    fn bind(
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwpPointerConstraintsV1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, ());
    }
}

impl<D> Dispatch<ZwpConfinedPointerV1, PointerConstraintUserData<D>, D> for PointerConstraintsState
where
    D: Dispatch<ZwpConfinedPointerV1, PointerConstraintUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _confined_pointer: &ZwpConfinedPointerV1,
        request: zwp_confined_pointer_v1::Request,
        data: &PointerConstraintUserData<D>,
        _dh: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let Some(pointer) = &data.pointer else {
            return;
        };

        match request {
            zwp_confined_pointer_v1::Request::SetRegion { region } => {
                with_pointer_constraint(&data.surface, pointer, |constraint| {
                    if let Some(PointerConstraint::Confined(confined)) =
                        constraint.map(|x| x.entry.into_mut())
                    {
                        confined.pending_region = region.as_ref().map(compositor::get_region_attributes);
                    }
                });
            }
            zwp_confined_pointer_v1::Request::Destroy => {
                remove_constraint(&data.surface, pointer);
            }
            _ => unreachable!(),
        }
    }
}

impl<D> Dispatch<ZwpLockedPointerV1, PointerConstraintUserData<D>, D> for PointerConstraintsState
where
    D: Dispatch<ZwpLockedPointerV1, PointerConstraintUserData<D>>,
    D: SeatHandler,
    D: 'static,
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        _locked_pointer: &ZwpLockedPointerV1,
        request: zwp_locked_pointer_v1::Request,
        data: &PointerConstraintUserData<D>,
        _dh: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let Some(pointer) = &data.pointer else {
            return;
        };

        match request {
            zwp_locked_pointer_v1::Request::SetCursorPositionHint { surface_x, surface_y } => {
                with_pointer_constraint(&data.surface, pointer, |constraint| {
                    if let Some(PointerConstraint::Locked(locked)) = constraint.map(|x| x.entry.into_mut()) {
                        locked.pending_cursor_position_hint = Some((surface_x, surface_y).into());
                    }
                });
            }
            zwp_locked_pointer_v1::Request::SetRegion { region } => {
                with_pointer_constraint(&data.surface, pointer, |constraint| {
                    if let Some(PointerConstraint::Locked(locked)) = constraint.map(|x| x.entry.into_mut()) {
                        locked.pending_region = region.as_ref().map(compositor::get_region_attributes);
                    }
                });
            }
            zwp_locked_pointer_v1::Request::Destroy => {
                remove_constraint(&data.surface, pointer);
            }
            _ => unreachable!(),
        }
    }
}

#[allow(missing_docs)]
#[macro_export]
macro_rules! delegate_pointer_constraints {
    ($(@<$( $lt:tt $( : $clt:tt $(+ $dlt:tt )* )? ),+>)? $ty: ty) => {
        $crate::reexports::wayland_server::delegate_global_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::pointer_constraints::zv1::server::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1: ()
        ] => $crate::wayland::pointer_constraints::PointerConstraintsState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::pointer_constraints::zv1::server::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1: ()
        ] => $crate::wayland::pointer_constraints::PointerConstraintsState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::pointer_constraints::zv1::server::zwp_confined_pointer_v1::ZwpConfinedPointerV1: $crate::wayland::pointer_constraints::PointerConstraintUserData<Self>
        ] => $crate::wayland::pointer_constraints::PointerConstraintsState);
        $crate::reexports::wayland_server::delegate_dispatch!($(@< $( $lt $( : $clt $(+ $dlt )* )? ),+ >)? $ty: [
            $crate::reexports::wayland_protocols::wp::pointer_constraints::zv1::server::zwp_locked_pointer_v1::ZwpLockedPointerV1: $crate::wayland::pointer_constraints::PointerConstraintUserData<Self>
        ] => $crate::wayland::pointer_constraints::PointerConstraintsState);
    };
}
