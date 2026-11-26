//! Protocol for confining the pointer.
//!
//! This provides a way for the client to request that the pointer is confined to a region or
//! locked in place.
use std::{
    collections::{HashMap, hash_map},
    ops,
    sync::{
        Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
};

use wayland_protocols::wp::pointer_constraints::zv1::server::{
    zwp_confined_pointer_v1::{self, ZwpConfinedPointerV1},
    zwp_locked_pointer_v1::{self, ZwpLockedPointerV1},
    zwp_pointer_constraints_v1::{self, Lifetime, ZwpPointerConstraintsV1},
};
use wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource, WEnum,
    backend::{GlobalId, ObjectId},
    protocol::wl_surface::WlSurface,
};

use super::compositor::{self, RegionAttributes};
use crate::{
    input::{
        SeatHandler,
        pointer::{PointerHandle, PointerRc},
    },
    utils::{Logical, Point},
    wayland::{Dispatch2, GlobalData, GlobalDispatch2, seat::PointerUserData},
};

const VERSION: u32 = 1;

/// Reason for Constraint remove
#[derive(Debug)]
pub enum ConstraintRemove {
    /// Client call destroy
    Destroyed(PointerConstraint),
    /// Compositor pointer leave surface
    PointerLeave(Option<RegionAttributes>),
}

/// Handler for pointer constraints
pub trait PointerConstraintsHandler: SeatHandler {
    /// Pointer lock or confinement constraint created for `pointer` on `surface`
    ///
    /// Use [`with_pointer_constraint`] to access the constraint.
    fn new_constraint(&mut self, _surface: &WlSurface, _pointer: &PointerHandle<Self>) {}

    /// Pointer constraint removed for `pointer` on `surface`
    ///
    /// Use [`with_pointer_constraint`] to access the constraint.
    fn remove_constraint(
        &mut self,
        _surface: &WlSurface,
        _pointer: &PointerHandle<Self>,
        _constraint_remove: ConstraintRemove,
    ) {
    }

    /// The client holding a LockedPointer has committed a cursor position hint.
    ///
    /// This is emitted upon a surface commit if the cursor position hint has been updated.
    ///
    /// Use [`with_pointer_constraint`] to access the constraint and check if it is active.
    fn cursor_position_hint(
        &mut self,
        _surface: &WlSurface,
        _pointer: &PointerHandle<Self>,
        _location: Point<f64, Logical>,
    ) {
    }
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

impl<D: SeatHandler + 'static> ops::Deref for PointerConstraintRef<'_, D> {
    type Target = PointerConstraint;

    fn deref(&self) -> &Self::Target {
        self.entry.get()
    }
}

impl<D: SeatHandler + PointerConstraintsHandler + 'static> PointerConstraintRef<'_, D> {
    /// Send `locked`/`unlocked`
    ///
    /// This is not sent automatically since compositors may have different
    /// policies about when to allow and activate constraints.
    pub fn activate(&self) {
        match self.entry.get() {
            PointerConstraint::Confined(confined) => {
                if !confined.active.swap(true, Ordering::SeqCst) {
                    confined.handle.confined();
                }
            }
            PointerConstraint::Locked(locked) => {
                if !locked.active.swap(true, Ordering::SeqCst) {
                    locked.handle.locked();
                }
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
        let deactivated = match self.entry.get() {
            PointerConstraint::Confined(confined) => {
                if confined.active.swap(false, Ordering::SeqCst) {
                    confined.handle.unconfined();
                    true
                } else {
                    false
                }
            }
            PointerConstraint::Locked(locked) => {
                if locked.active.swap(false, Ordering::SeqCst) {
                    locked.handle.unlocked();
                    true
                } else {
                    false
                }
            }
        };

        if deactivated && self.lifetime() == WEnum::Value(Lifetime::Oneshot) {
            self.entry.remove_entry();
        }
    }
}

impl PointerConstraint {
    fn id(&self) -> ObjectId {
        match self {
            PointerConstraint::Confined(confined) => confined.handle.id(),
            PointerConstraint::Locked(locked) => locked.handle.id(),
        }
    }

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
                locked.pending_cursor_position_hint.take().inspect(|hint| {
                    locked.cursor_position_hint = Some(*hint);
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
        D: GlobalDispatch<ZwpPointerConstraintsV1, GlobalData>,
        D: Dispatch<ZwpPointerConstraintsV1, GlobalData>,
        D: Dispatch<ZwpConfinedPointerV1, PointerConstraintUserData<D>>,
        D: Dispatch<ZwpLockedPointerV1, PointerConstraintUserData<D>>,
        D: SeatHandler,
        D: 'static,
    {
        let global = display.create_global::<D, ZwpPointerConstraintsV1, _>(VERSION, GlobalData);

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
    arc: Weak<PointerRc<D>>,
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

/// Access the constraint for `(surface, pointer)` only when it matches `constraint_id`.
fn with_matching_constraint<
    D: SeatHandler + 'static,
    T,
    F: FnOnce(hash_map::OccupiedEntry<'_, PointerHandle<D>, PointerConstraint>) -> T,
>(
    surface: &WlSurface,
    pointer: &PointerHandle<D>,
    constraint_id: &ObjectId,
    f: F,
) -> Option<T> {
    with_constraint_data::<D, _, _>(surface, |data| match data?.constraints.entry(pointer.clone()) {
        hash_map::Entry::Occupied(entry) if entry.get().id() == *constraint_id => Some(f(entry)),
        _ => None,
    })
}

/// Add constraint for surface, or raise protocol error if one exists.
///
/// Returns `true` if the constraint was inserted.
fn add_constraint<D: SeatHandler + PointerConstraintsHandler + 'static>(
    pointer_constraints: &ZwpPointerConstraintsV1,
    surface: &WlSurface,
    pointer: &PointerHandle<D>,
    constraint: PointerConstraint,
) -> bool {
    let mut data_added = false;
    let mut inserted = false;
    compositor::with_states(surface, |states| {
        data_added = states.data_map.insert_if_missing_threadsafe(|| {
            Mutex::new(PointerConstraintData::<D> {
                constraints: HashMap::new(),
            })
        });
        let data = states.data_map.get::<Mutex<PointerConstraintData<D>>>().unwrap();
        let mut data = data.lock().unwrap();

        if !data.constraints.contains_key(pointer) {
            data.constraints.insert(pointer.clone(), constraint);
            inserted = true;
        }
    });

    if data_added {
        compositor::add_post_commit_hook(surface, commit_hook::<D>);
    }

    if !inserted {
        pointer_constraints.post_error(
            zwp_pointer_constraints_v1::Error::AlreadyConstrained,
            "pointer constraint already exists for surface and seat",
        );
    }

    inserted
}

fn remove_constraint<D: SeatHandler + PointerConstraintsHandler + 'static>(
    state: &mut D,
    surface: &WlSurface,
    pointer: &PointerHandle<D>,
    constraint_id: &ObjectId,
) {
    if let Some(constraint) =
        with_matching_constraint(surface, pointer, constraint_id, |entry| entry.remove())
    {
        state.remove_constraint(surface, pointer, ConstraintRemove::Destroyed(constraint));
    }
}

impl<D> Dispatch2<ZwpPointerConstraintsV1, D> for GlobalData
where
    D: Dispatch<ZwpConfinedPointerV1, PointerConstraintUserData<D>>,
    D: Dispatch<ZwpLockedPointerV1, PointerConstraintUserData<D>>,
    D: SeatHandler,
    D: PointerConstraintsHandler,
    D: 'static,
{
    fn request(
        &self,
        state: &mut D,
        _client: &wayland_server::Client,
        pointer_constraints: &ZwpPointerConstraintsV1,
        request: zwp_pointer_constraints_v1::Request,
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
                let arc = pointer.data::<PointerUserData<D>>().unwrap().arc.clone();
                let handle = data_init.init(
                    id,
                    PointerConstraintUserData {
                        surface: surface.clone(),
                        arc: arc.clone(),
                    },
                );
                if let Some(arc) = arc.upgrade() {
                    let pointer = PointerHandle { arc };
                    if add_constraint(
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
                    ) {
                        state.new_constraint(&surface, &pointer);
                    }
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
                let arc = pointer.data::<PointerUserData<D>>().unwrap().arc.clone();
                let handle = data_init.init(
                    id,
                    PointerConstraintUserData {
                        surface: surface.clone(),
                        arc: arc.clone(),
                    },
                );
                if let Some(arc) = arc.upgrade() {
                    let pointer = PointerHandle { arc };
                    if add_constraint(
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
                    ) {
                        state.new_constraint(&surface, &pointer);
                    }
                }
            }
            zwp_pointer_constraints_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }
}

impl<D> GlobalDispatch2<ZwpPointerConstraintsV1, D> for GlobalData
where
    D: Dispatch<ZwpPointerConstraintsV1, GlobalData> + SeatHandler + 'static,
{
    fn bind(
        &self,
        _state: &mut D,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<ZwpPointerConstraintsV1>,
        data_init: &mut DataInit<'_, D>,
    ) {
        data_init.init(resource, GlobalData);
    }
}

impl<D> Dispatch2<ZwpConfinedPointerV1, D> for PointerConstraintUserData<D>
where
    D: SeatHandler,
    D: PointerConstraintsHandler,
    D: 'static,
{
    fn request(
        &self,
        _state: &mut D,
        _client: &wayland_server::Client,
        confined_pointer: &ZwpConfinedPointerV1,
        request: zwp_confined_pointer_v1::Request,
        _dh: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let Some(pointer) = &self.arc.upgrade().map(|arc| PointerHandle { arc }) else {
            return;
        };

        match request {
            zwp_confined_pointer_v1::Request::SetRegion { region } => {
                with_matching_constraint(&self.surface, pointer, &confined_pointer.id(), |entry| {
                    if let PointerConstraint::Confined(confined) = entry.into_mut() {
                        confined.pending_region = region.as_ref().map(compositor::get_region_attributes);
                    }
                });
            }
            zwp_confined_pointer_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(
        &self,
        state: &mut D,
        _client: wayland_server::backend::ClientId,
        resource: &ZwpConfinedPointerV1,
    ) {
        let Some(pointer) = &self.arc.upgrade().map(|arc| PointerHandle { arc }) else {
            return;
        };

        remove_constraint(state, &self.surface, pointer, &resource.id());
    }
}

impl<D> Dispatch2<ZwpLockedPointerV1, D> for PointerConstraintUserData<D>
where
    D: SeatHandler,
    D: PointerConstraintsHandler,
    D: 'static,
{
    fn request(
        &self,
        _state: &mut D,
        _client: &wayland_server::Client,
        locked_pointer: &ZwpLockedPointerV1,
        request: zwp_locked_pointer_v1::Request,
        _dh: &DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let Some(pointer) = &self.arc.upgrade().map(|arc| PointerHandle { arc }) else {
            return;
        };

        match request {
            zwp_locked_pointer_v1::Request::SetCursorPositionHint { surface_x, surface_y } => {
                with_matching_constraint(&self.surface, pointer, &locked_pointer.id(), |entry| {
                    if let PointerConstraint::Locked(locked) = entry.into_mut() {
                        locked.pending_cursor_position_hint = Some((surface_x, surface_y).into());
                    }
                });
            }
            zwp_locked_pointer_v1::Request::SetRegion { region } => {
                with_matching_constraint(&self.surface, pointer, &locked_pointer.id(), |entry| {
                    if let PointerConstraint::Locked(locked) = entry.into_mut() {
                        locked.pending_region = region.as_ref().map(compositor::get_region_attributes);
                    }
                });
            }
            zwp_locked_pointer_v1::Request::Destroy => {}
            _ => unreachable!(),
        }
    }

    fn destroyed(
        &self,
        state: &mut D,
        _client: wayland_server::backend::ClientId,
        resource: &ZwpLockedPointerV1,
    ) {
        let Some(pointer) = &self.arc.upgrade().map(|arc| PointerHandle { arc }) else {
            return;
        };

        remove_constraint(state, &self.surface, pointer, &resource.id());
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{ErrorKind, Read, Write},
        os::unix::net::UnixStream,
        sync::Arc,
    };

    use wayland_server::{
        Display,
        backend::{ClientData, ClientId, DisconnectReason},
    };

    use super::*;
    use crate::{
        input::SeatState,
        utils::Rectangle,
        wayland::compositor::{CompositorClientState, CompositorHandler, CompositorState},
    };

    struct TestClientData {
        compositor: CompositorClientState,
        on_disconnect: Mutex<Option<(WlSurface, PointerHandle<TestState>)>>,
    }

    impl ClientData for TestClientData {
        fn initialized(&self, _client_id: ClientId) {}
        fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {
            if let Some((surface, pointer)) = self.on_disconnect.lock().unwrap().as_ref() {
                assert!(with_pointer_constraint(surface, pointer, |c| c.is_some()));
            }
        }
    }

    struct TestState {
        compositor: CompositorState,
        seat_state: SeatState<Self>,
        _pointer_constraints: PointerConstraintsState,
        surfaces: Vec<WlSurface>,
        new_constraints: usize,
        destroyed_constraints: Vec<ObjectId>,
        hints: Vec<Point<f64, Logical>>,
    }

    impl CompositorHandler for TestState {
        fn compositor_state(&mut self) -> &mut CompositorState {
            &mut self.compositor
        }

        fn client_compositor_state<'a>(&self, client: &'a Client) -> &'a CompositorClientState {
            &client.get_data::<TestClientData>().unwrap().compositor
        }

        fn commit(&mut self, surface: &WlSurface) {
            if !self.surfaces.contains(surface) {
                self.surfaces.push(surface.clone());
            }
        }
    }

    impl SeatHandler for TestState {
        type KeyboardFocus = WlSurface;
        type PointerFocus = WlSurface;
        type TouchFocus = WlSurface;

        fn seat_state(&mut self) -> &mut SeatState<Self> {
            &mut self.seat_state
        }
    }

    impl PointerConstraintsHandler for TestState {
        fn new_constraint(&mut self, _surface: &WlSurface, _pointer: &PointerHandle<Self>) {
            self.new_constraints += 1;
        }

        fn remove_constraint(
            &mut self,
            _surface: &WlSurface,
            _pointer: &PointerHandle<Self>,
            constraint_remove: ConstraintRemove,
        ) {
            if let ConstraintRemove::Destroyed(constraint) = constraint_remove {
                self.destroyed_constraints.push(constraint.id());
            }
        }

        fn cursor_position_hint(
            &mut self,
            _surface: &WlSurface,
            _pointer: &PointerHandle<Self>,
            location: Point<f64, Logical>,
        ) {
            self.hints.push(location);
        }
    }

    crate::delegate_dispatch2!(TestState);

    struct WireClient {
        stream: UnixStream,
        next_id: u32,
        registry: u32,
        compositor: u32,
        seat: u32,
        constraints: u32,
        pointer: u32,
    }

    impl WireClient {
        fn send(&mut self, object_id: u32, opcode: u16, payload: &[u32]) {
            let size = ((payload.len() + 2) * 4) as u32;
            let mut bytes = Vec::with_capacity(size as usize);
            bytes.extend_from_slice(&object_id.to_ne_bytes());
            bytes.extend_from_slice(&((size << 16) | u32::from(opcode)).to_ne_bytes());
            for word in payload {
                bytes.extend_from_slice(&word.to_ne_bytes());
            }
            self.stream.write_all(&bytes).unwrap();
        }

        fn alloc_id(&mut self) -> u32 {
            let id = self.next_id;
            self.next_id += 1;
            id
        }

        fn bind(&mut self, global_name: u32, interface: &str, version: u32) -> u32 {
            let new_id = self.alloc_id();
            let raw = interface.as_bytes();
            let len_with_nul = raw.len() + 1;
            let padded_words = len_with_nul.div_ceil(4);
            let mut str_bytes = vec![0u8; padded_words * 4];
            str_bytes[..raw.len()].copy_from_slice(raw);

            let mut payload = Vec::with_capacity(3 + padded_words);
            payload.push(global_name);
            payload.push(len_with_nul as u32);
            for chunk in str_bytes.as_chunks::<4>().0 {
                payload.push(u32::from_ne_bytes(*chunk));
            }
            payload.push(version);
            payload.push(new_id);
            self.send(self.registry, 0, &payload);
            new_id
        }

        fn create_surface(&mut self) -> u32 {
            let id = self.alloc_id();
            self.send(self.compositor, 0, &[id]);
            id
        }

        fn create_region(&mut self, rect: Rectangle<i32, Logical>) -> u32 {
            let id = self.alloc_id();
            self.send(self.compositor, 1, &[id]);
            self.send(
                id,
                1,
                &[
                    rect.loc.x as u32,
                    rect.loc.y as u32,
                    rect.size.w as u32,
                    rect.size.h as u32,
                ],
            );
            id
        }

        fn commit(&mut self, surface: u32) {
            self.send(surface, 6, &[]);
        }

        fn lock_pointer(&mut self, surface: u32, region: u32, lifetime: Lifetime) -> u32 {
            let id = self.alloc_id();
            self.send(
                self.constraints,
                1,
                &[id, surface, self.pointer, region, lifetime as u32],
            );
            id
        }

        fn confine_pointer(&mut self, surface: u32, region: u32, lifetime: Lifetime) -> u32 {
            let id = self.alloc_id();
            self.send(
                self.constraints,
                2,
                &[id, surface, self.pointer, region, lifetime as u32],
            );
            id
        }

        fn locked_set_hint(&mut self, locked: u32, x: i32, y: i32) {
            self.send(locked, 1, &[(x * 256) as u32, (y * 256) as u32]);
        }

        fn locked_set_region(&mut self, locked: u32, region: u32) {
            self.send(locked, 2, &[region]);
        }

        fn confined_set_region(&mut self, confined: u32, region: u32) {
            self.send(confined, 1, &[region]);
        }

        fn destroy(&mut self, object_id: u32) {
            self.send(object_id, 0, &[]);
        }

        fn read_messages(&mut self) -> Vec<(u32, u16, Vec<u32>)> {
            let mut raw = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                match self.stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => raw.extend_from_slice(&buf[..n]),
                    Err(err) if err.kind() == ErrorKind::WouldBlock => break,
                    Err(err) => panic!("unexpected socket read error: {err}"),
                }
            }

            let mut out = Vec::new();
            let mut offset = 0;
            while offset + 8 <= raw.len() {
                let obj = u32::from_ne_bytes(raw[offset..offset + 4].try_into().unwrap());
                let header = u32::from_ne_bytes(raw[offset + 4..offset + 8].try_into().unwrap());
                let size = (header >> 16) as usize;
                let opcode = (header & 0xffff) as u16;
                assert!(size >= 8 && size & 3 == 0 && offset + size <= raw.len());
                let payload = raw[offset + 8..offset + size]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|chunk| u32::from_ne_bytes(*chunk))
                    .collect();
                out.push((obj, opcode, payload));
                offset += size;
            }
            out
        }
    }

    struct Harness {
        display: Display<TestState>,
        state: TestState,
        client_data: Arc<TestClientData>,
        client: WireClient,
        pointer: PointerHandle<TestState>,
        surface_id: u32,
        surface: WlSurface,
    }

    impl Harness {
        fn new() -> Self {
            let mut display = Display::<TestState>::new().unwrap();
            let mut dh = display.handle();
            let compositor = CompositorState::new::<TestState>(&dh);
            let mut seat_state = SeatState::new();
            let mut seat = seat_state.new_wl_seat(&dh, "seat0");
            let pointer = seat.add_pointer();
            let pointer_constraints = PointerConstraintsState::new::<TestState>(&dh);

            let (server_stream, client_stream) = UnixStream::pair().unwrap();
            client_stream.set_nonblocking(true).unwrap();
            let client_data = Arc::new(TestClientData {
                compositor: CompositorClientState::default(),
                on_disconnect: Mutex::new(None),
            });
            dh.insert_client(server_stream, client_data.clone()).unwrap();

            let mut state = TestState {
                compositor,
                seat_state,
                _pointer_constraints: pointer_constraints,
                surfaces: Vec::new(),
                new_constraints: 0,
                destroyed_constraints: Vec::new(),
                hints: Vec::new(),
            };

            let mut client = WireClient {
                stream: client_stream,
                next_id: 2,
                registry: 0,
                compositor: 0,
                seat: 0,
                constraints: 0,
                pointer: 0,
            };
            client.registry = client.alloc_id();
            client.send(1, 1, &[client.registry]);
            display.dispatch_clients(&mut state).unwrap();
            display.flush_clients().unwrap();

            let mut globals = HashMap::new();
            for (obj, opcode, payload) in client.read_messages() {
                if obj == client.registry && opcode == 0 && payload.len() >= 3 {
                    let name = payload[0];
                    let str_len = payload[1] as usize;
                    let str_bytes: Vec<u8> = payload[2..payload.len() - 1]
                        .iter()
                        .flat_map(|w| w.to_ne_bytes())
                        .collect();
                    let iface = std::str::from_utf8(&str_bytes[..str_len - 1]).unwrap();
                    globals.insert(iface.to_owned(), name);
                }
            }

            client.compositor = client.bind(globals["wl_compositor"], "wl_compositor", 5);
            client.seat = client.bind(globals["wl_seat"], "wl_seat", 8);
            client.constraints = client.bind(
                globals["zwp_pointer_constraints_v1"],
                "zwp_pointer_constraints_v1",
                1,
            );
            client.pointer = client.alloc_id();
            client.send(client.seat, 0, &[client.pointer]);
            let surface_id = client.create_surface();
            client.commit(surface_id);

            display.dispatch_clients(&mut state).unwrap();
            let surface = state.surfaces[0].clone();

            Self {
                display,
                state,
                client_data,
                client,
                pointer,
                surface_id,
                surface,
            }
        }

        fn dispatch(&mut self) {
            self.display.dispatch_clients(&mut self.state).unwrap();
            let _ = self.display.flush_clients();
        }

        fn current_id(&self) -> Option<ObjectId> {
            with_pointer_constraint(&self.surface, &self.pointer, |c| c.map(|c| c.id()))
        }

        fn activate_and_deactivate(&self) {
            with_pointer_constraint(&self.surface, &self.pointer, |c| {
                let c = c.expect("constraint should exist");
                c.activate();
                c.deactivate();
            });
        }
    }

    #[test]
    fn defunct_oneshot_locked_cannot_mutate_or_remove_replacement_lock() {
        let mut h = Harness::new();

        let lock1 = h.client.lock_pointer(h.surface_id, 0, Lifetime::Oneshot);
        h.dispatch();
        assert_eq!(h.state.new_constraints, 1);

        h.activate_and_deactivate();
        assert!(h.current_id().is_none());

        // Replacement created while defunct oneshot resource is still alive.
        let lock2 = h.client.lock_pointer(h.surface_id, 0, Lifetime::Persistent);
        h.dispatch();
        assert_eq!(h.state.new_constraints, 2);
        let lock2_id = h.current_id().expect("replacement lock should exist");

        // Stale requests from lock1 must not overwrite lock2.
        let stale_region = h.client.create_region(Rectangle::from_size((10, 10).into()));
        h.client.locked_set_hint(lock1, 11, 22);
        h.client.locked_set_region(lock1, stale_region);
        h.client.commit(h.surface_id);
        h.dispatch();

        assert!(h.state.hints.is_empty());
        with_pointer_constraint(&h.surface, &h.pointer, |c| {
            let c = c.expect("replacement lock should remain");
            assert_eq!(c.id(), lock2_id);
            assert!(c.region().is_none());
        });

        // Destroying defunct lock1 must not remove lock2.
        h.client.destroy(lock1);
        h.dispatch();
        assert!(h.state.destroyed_constraints.is_empty());
        assert_eq!(h.current_id(), Some(lock2_id.clone()));

        // Valid updates on the current inactive lock2 must still apply.
        let live_region = h.client.create_region(Rectangle::from_size((30, 40).into()));
        h.client.locked_set_hint(lock2, 50, 60);
        h.client.locked_set_region(lock2, live_region);
        h.client.commit(h.surface_id);
        h.dispatch();

        assert_eq!(h.state.hints, vec![Point::from((50., 60.))]);
        with_pointer_constraint(&h.surface, &h.pointer, |c| {
            let c = c.expect("replacement lock should remain");
            let region = c.region().expect("replacement lock region should be set");
            assert!(region.contains((29, 39)));
            assert!(!region.contains((30, 40)));
        });

        // Destroying lock2 itself removes it and emits Destroyed.
        h.client.destroy(lock2);
        h.dispatch();
        assert!(h.current_id().is_none());
        assert_eq!(h.state.destroyed_constraints, vec![lock2_id]);
    }

    #[test]
    fn defunct_oneshot_confined_cannot_mutate_or_remove_replacement_and_cross_kind_destroy() {
        let mut h = Harness::new();

        // Confined oneshot -> replaced by confined oneshot.
        let confine1 = h.client.confine_pointer(h.surface_id, 0, Lifetime::Oneshot);
        h.dispatch();
        assert_eq!(h.state.new_constraints, 1);
        h.activate_and_deactivate();

        let confine2 = h.client.confine_pointer(h.surface_id, 0, Lifetime::Oneshot);
        h.dispatch();
        assert_eq!(h.state.new_constraints, 2);
        let confine2_id = h.current_id().expect("replacement confine should exist");

        // Stale SetRegion and destroy on defunct confine1 must not affect confine2.
        let stale_region = h.client.create_region(Rectangle::from_size((15, 15).into()));
        h.client.confined_set_region(confine1, stale_region);
        h.client.commit(h.surface_id);
        h.client.destroy(confine1);
        h.dispatch();

        assert!(h.state.destroyed_constraints.is_empty());
        with_pointer_constraint(&h.surface, &h.pointer, |c| {
            let c = c.expect("replacement confine should remain");
            assert_eq!(c.id(), confine2_id);
            assert!(c.region().is_none());
        });

        // Live SetRegion on the current (inactive) confinement must apply on commit.
        let live_region = h.client.create_region(Rectangle::from_size((25, 35).into()));
        h.client.confined_set_region(confine2, live_region);
        h.client.commit(h.surface_id);
        h.dispatch();
        with_pointer_constraint(&h.surface, &h.pointer, |c| {
            let c = c.expect("replacement confine should remain");
            let region = c.region().expect("replacement confine region should be set");
            assert!(region.contains((24, 34)));
            assert!(!region.contains((25, 35)));
        });

        // Cross-kind: deactivate confined oneshot, replace with locked, destroy old confined.
        h.activate_and_deactivate();
        let lock = h.client.lock_pointer(h.surface_id, 0, Lifetime::Persistent);
        h.dispatch();
        assert_eq!(h.state.new_constraints, 3);
        let lock_id = h.current_id().expect("replacement lock should exist");

        h.client.destroy(confine2);
        h.dispatch();
        assert!(h.state.destroyed_constraints.is_empty());
        assert_eq!(h.current_id(), Some(lock_id.clone()));

        h.client.destroy(lock);
        h.dispatch();
        assert!(h.current_id().is_none());
        assert_eq!(h.state.destroyed_constraints, vec![lock_id]);

        // Destroying a live confinement removes it and emits Destroyed.
        let confine3 = h.client.confine_pointer(h.surface_id, 0, Lifetime::Persistent);
        h.dispatch();
        assert_eq!(h.state.new_constraints, 4);
        let confine3_id = h.current_id().expect("live confine should exist");
        h.client.destroy(confine3);
        h.dispatch();
        assert!(h.current_id().is_none());
        assert_eq!(h.state.destroyed_constraints.last(), Some(&confine3_id));
    }

    #[test]
    fn duplicate_constraint_posts_already_constrained_without_deadlock_or_spurious_callback() {
        for confine_duplicate in [false, true] {
            let mut h = Harness::new();

            let _lock1 = h.client.lock_pointer(h.surface_id, 0, Lifetime::Persistent);
            h.dispatch();
            assert_eq!(h.state.new_constraints, 1);
            let lock1_id = h.current_id().expect("initial lock should exist");

            // Drain earlier server events and arm disconnect-time constraint inspection to
            // ensure post_error does not hold the surface or constraint mutex.
            let _ = h.client.read_messages();
            *h.client_data.on_disconnect.lock().unwrap() = Some((h.surface.clone(), h.pointer.clone()));

            // A second constraint on the same surface and seat posts AlreadyConstrained, which
            // disconnects the client. It must not deadlock, must not fire a second
            // new_constraint callback, and teardown must emit Destroyed only once for lock1.
            if confine_duplicate {
                h.client.confine_pointer(h.surface_id, 0, Lifetime::Persistent);
            } else {
                h.client.lock_pointer(h.surface_id, 0, Lifetime::Persistent);
            }
            h.dispatch();

            assert_eq!(h.state.new_constraints, 1);
            assert!(h.current_id().is_none());
            assert_eq!(h.state.destroyed_constraints, vec![lock1_id]);

            let protocol_errors: Vec<(u32, u32)> = h
                .client
                .read_messages()
                .into_iter()
                .filter(|(obj, opcode, payload)| *obj == 1 && *opcode == 0 && payload.len() >= 2)
                .map(|(_, _, payload)| (payload[0], payload[1]))
                .collect();
            assert_eq!(
                protocol_errors,
                vec![(
                    h.client.constraints,
                    zwp_pointer_constraints_v1::Error::AlreadyConstrained as u32
                )]
            );
        }
    }
}
