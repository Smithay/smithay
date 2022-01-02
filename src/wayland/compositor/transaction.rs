// The transaction model for handling surface states in Smithay
//
// The caching logic in `cache.rs` provides surfaces with a queue of
// pending states identified with numeric commit ids, allowing the compositor
// to precisely control *when* a state become active. This file is the second
// half: these identified states are grouped into transactions, which allow the
// synchronization of updates accross surfaces.
//
// There are 2 main cases when the state of multiple surfaces must be updated
// atomically:
// - synchronized subsurface must have their state updated at the same time as their parents
// - The upcoming `wp_transaction` protocol
//
// In these situations, the individual states in a surface queue are grouped into a transaction
// and are all applied atomically when the transaction itself is applied. The logic for creating
// new transactions is currently the following:
//
// - Each surface has an implicit "pending" transaction, into which its newly commited state is
//   recorded
// - Furthermore, on commit, the pending transaction of all synchronized child subsurfaces is merged
//   into the current surface's pending transaction, and a new implicit transaction is started for those
//   children (logic is implemented in `handlers.rs`, in `PrivateSurfaceData::commit`).
// - Then, still on commit, if the surface is not a synchronized subsurface, its pending transaction is
//   directly applied
//
// This last step will change once we have support for explicit synchronization (and further in the future,
// of the wp_transaction protocol). Explicit synchronization introduces a notion of blockers: the transaction
// cannot be applied before all blockers are released, and thus must wait for it to be the case.
//
// For thoses situations, the (currently unused) `TransactionQueue` will come into play. It is a per-client
// queue of transactions, that stores and applies them by both respecting their topological order
// (ensuring that for each surface, states are applied in the correct order) and that all transactions
// wait befor all their blockers are resolved to be merged. If a blocker is cancelled, the whole transaction
// it blocks is cancelled as well, and simply dropped. Thanks to the logic of `Cache::apply_state`, the
// associated state will be applied automatically when the next valid transaction is applied, ensuring
// global coherence.

// A significant part of the logic of this module is not yet used,
// but will be once proper transaction & blockers support is
// added to smithay
#![allow(dead_code)]

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

use wayland_server::{protocol::wl_surface::WlSurface, DisplayHandle, Resource};

use crate::wayland::Serial;

use super::tree::PrivateSurfaceData;

pub trait Blocker {
    fn state(&self) -> BlockerState;
}

pub enum BlockerState {
    Pending,
    Released,
    Cancelled,
}

#[derive(Default)]
struct TransactionState {
    surfaces: Vec<(WlSurface, Serial)>,
    blockers: Vec<Box<dyn Blocker + Send>>,
}

impl TransactionState {
    fn insert(&mut self, surface: WlSurface, id: Serial) {
        if let Some(place) = self.surfaces.iter_mut().find(|place| place.0 == surface) {
            // the surface is already in the list, update the serial
            if place.1 < id {
                place.1 = id;
            }
        } else {
            // the surface is not in the list, insert it
            self.surfaces.push((surface, id));
        }
    }
}

enum TransactionInner {
    Data(TransactionState),
    Fused(Arc<Mutex<TransactionInner>>),
}

pub(crate) struct PendingTransaction {
    inner: Arc<Mutex<TransactionInner>>,
}

impl Default for PendingTransaction {
    fn default() -> Self {
        PendingTransaction {
            inner: Arc::new(Mutex::new(TransactionInner::Data(Default::default()))),
        }
    }
}

impl PendingTransaction {
    fn with_inner_state<T, F: FnOnce(&mut TransactionState) -> T>(&self, f: F) -> T {
        let mut next = self.inner.clone();
        loop {
            let tmp = match *next.lock().unwrap() {
                TransactionInner::Data(ref mut state) => return f(state),
                TransactionInner::Fused(ref into) => into.clone(),
            };
            next = tmp;
        }
    }

    pub(crate) fn insert_state(&self, surface: WlSurface, id: Serial) {
        self.with_inner_state(|state| state.insert(surface, id))
    }

    pub(crate) fn add_blocker<B: Blocker + Send + 'static>(&self, blocker: B) {
        self.with_inner_state(|state| state.blockers.push(Box::new(blocker) as Box<_>))
    }

    pub(crate) fn is_same_as(&self, other: &PendingTransaction) -> bool {
        let ptr1 = self.with_inner_state(|state| state as *const _);
        let ptr2 = other.with_inner_state(|state| state as *const _);
        ptr1 == ptr2
    }

    pub(crate) fn merge_into(&self, into: &PendingTransaction) {
        if self.is_same_as(into) {
            // nothing to do
            return;
        }
        // extract our pending surfaces and change our link
        let mut next = self.inner.clone();
        let my_state;
        loop {
            let tmp = {
                let mut guard = next.lock().unwrap();
                match *guard {
                    TransactionInner::Data(ref mut state) => {
                        my_state = std::mem::take(state);
                        *guard = TransactionInner::Fused(into.inner.clone());
                        break;
                    }
                    TransactionInner::Fused(ref into) => into.clone(),
                }
            };
            next = tmp;
        }
        // fuse our surfaces into our new transaction state
        self.with_inner_state(|state| {
            for (surface, id) in my_state.surfaces {
                state.insert(surface, id);
            }
            state.blockers.extend(my_state.blockers);
        });
    }

    pub(crate) fn finalize(mut self) -> Transaction {
        // When finalizing a transaction, this *must* be the last handle to this transaction
        loop {
            let inner = match Arc::try_unwrap(self.inner) {
                Ok(mutex) => mutex.into_inner().unwrap(),
                Err(_) => panic!("Attempting to finalize a transaction but handle is not the last."),
            };
            match inner {
                TransactionInner::Data(TransactionState {
                    surfaces, blockers, ..
                }) => return Transaction { surfaces, blockers },
                TransactionInner::Fused(into) => self.inner = into,
            }
        }
    }
}
pub(crate) struct Transaction {
    surfaces: Vec<(WlSurface, Serial)>,
    blockers: Vec<Box<dyn Blocker + Send>>,
}

impl Transaction {
    /// Computes the global state of the transaction with regard to its blockers
    ///
    /// The logic is:
    ///
    /// - if at least one blocker is cancelled, the transaction is cancelled
    /// - otherwise, if at least one blocker is pending, the transaction is pending
    /// - otherwise, all blockers are released, and the transaction is also released
    pub(crate) fn state(&self) -> BlockerState {
        use BlockerState::*;
        self.blockers
            .iter()
            .fold(Released, |acc, blocker| match (acc, blocker.state()) {
                (Cancelled, _) | (_, Cancelled) => Cancelled,
                (Pending, _) | (_, Pending) => Pending,
                (Released, Released) => Released,
            })
    }

    pub(crate) fn apply<D: 'static>(self, cx: &mut DisplayHandle<'_, D>) {
        for (surface, id) in self.surfaces {
            PrivateSurfaceData::<D>::with_states(&surface, |states| {
                states.cached_state.apply_state(id, cx);
            })
        }
    }
}

// This queue should be per-client
#[derive(Default)]
pub(crate) struct TransactionQueue {
    transactions: Vec<Transaction>,
    // we keep the hashset around to reuse allocations
    seen_surfaces: HashSet<u32>,
}

impl TransactionQueue {
    pub(crate) fn append(&mut self, t: Transaction) {
        self.transactions.push(t);
    }

    pub(crate) fn apply_ready<D: 'static>(&mut self, cx: &mut DisplayHandle<'_, D>) {
        // this is a very non-optimized implementation
        // we just iterate over the queue of transactions, keeping track of which
        // surface we have seen as they encode transaction dependencies
        self.seen_surfaces.clear();
        // manually iterate as we're going to modify the Vec while iterating on it
        let mut i = 0;
        // the loop will terminate, as at every iteration either i is incremented by 1
        // or the lenght of self.transactions is reduced by 1.
        while i <= self.transactions.len() {
            let mut skip = false;
            // does the transaction have any active blocker?
            match self.transactions[i].state() {
                BlockerState::Cancelled => {
                    // this transaction is cancelled, remove it without further processing
                    self.transactions.remove(i);
                    continue;
                }
                BlockerState::Pending => {
                    skip = true;
                }
                BlockerState::Released => {}
            }
            // if not, does this transaction depend on any previous transaction?
            if !skip {
                for (s, _) in &self.transactions[i].surfaces {
                    // TODO:
                    // if !s.as_ref().is_alive() {
                    //     continue;
                    // }
                    if self.seen_surfaces.contains(&s.id().protocol_id()) {
                        skip = true;
                        break;
                    }
                }
            }

            if skip {
                // this transaction is not yet ready and should be skipped, add its surfaces to our
                // seen list
                for (s, _) in &self.transactions[i].surfaces {
                    // TODO:
                    // if !s.as_ref().is_alive() {
                    //     continue;.
                    // }
                    self.seen_surfaces.insert(s.id().protocol_id());
                }
                i += 1;
            } else {
                // this transaction is to be applied, yay!
                self.transactions.remove(i).apply(cx);
            }
        }
    }
}
