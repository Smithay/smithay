//! Shared bookkeeping for the per-device grab stack.
//!
//! Every input device keeps an ordered list of grabs instead of a single slot. An event enters at
//! the top of the list; the inner handle a grab receives stands for *the rest of the list*, so
//! whatever a grab forwards reaches the next grab below it, and the default grab is the floor.
//!
//! Which grabs are in the list at the same time, and in which order, follows from how they got
//! there, not from a table of grab types:
//!
//! - An **explicit grab** is installed through the device handle and stands for an interaction the
//!   user is in the middle of: an open popup, a drag, a window being moved. These come first, the
//!   most recent one on top. The serial a grab is installed with names the interaction it belongs
//!   to. If a grab with that serial is active, the new grab is the next stage of that interaction
//!   and takes its place, and everything else stays: this is how a drag that starts from a press
//!   inside a popup ends up above the popup's grab. Otherwise it is a new interaction and
//!   replaces the ones in progress, which is what installing a grab has always done.
//! - A **persistent grab** is an explicit grab that belongs to a protocol object rather than to
//!   an interaction (see e.g. `KeyboardGrab::persistent`). These come after the interactions,
//!   whichever was installed first. They neither replace interactions nor are replaced by them,
//!   and the blanket queries and the blanket `unset_grab` of the device handles do not see them.
//! - The **implicit grab** is the one the default grab installs when a button goes down (the
//!   click grab). There is at most one, it comes last, and it stands in for the default grab as
//!   the floor for as long as it lasts.

use std::any::TypeId;

use crate::utils::Serial;

/// Bookkeeping for one installed grab.
pub(crate) struct GrabMeta<F> {
    pub(crate) serial: Serial,
    /// Installation order, larger is more recent.
    pub(crate) seq: u64,
    pub(crate) type_id: TypeId,
    pub(crate) implicit: bool,
    pub(crate) persistent: bool,
    /// Set when the grab ended while a dispatch was in flight. The entry is skipped from then on
    /// and swept once the dispatch is over.
    pub(crate) removed: bool,
    /// The last focus candidate that reached this grab, which is what it re-forwards when it ends
    /// and asks for the focus to be restored.
    pub(crate) pending_focus: F,
}

pub(crate) struct GrabEntry<G: ?Sized, F> {
    pub(crate) meta: GrabMeta<F>,
    pub(crate) handler: Box<G>,
}

/// A grab that is about to be installed.
pub(crate) struct NewGrab<G: ?Sized, F> {
    pub(crate) serial: Serial,
    pub(crate) type_id: TypeId,
    pub(crate) persistent: bool,
    pub(crate) pending_focus: F,
    pub(crate) handler: Box<G>,
}

/// A change to the stack requested from inside a grab, applied once the dispatch is over.
pub(crate) enum Deferred<G: ?Sized, F> {
    /// Put this grab where the (already ended) grab with the given `seq` was.
    Replace(u64, NewGrab<G, F>),
    /// Install the implicit grab. Requested by the default grab.
    Implicit(NewGrab<G, F>),
    /// Install an explicit grab on top.
    Top(NewGrab<G, F>),
}

/// Where an inner handle stands in the stack.
pub(crate) enum Position<'a, F> {
    /// Above every grab, where events enter.
    Root,
    /// At an installed grab.
    Grab(&'a mut GrabMeta<F>),
    /// At the default grab, below every installed grab.
    Default,
}

pub(crate) struct GrabStack<G: ?Sized, F> {
    /// In dispatch order: interactions, most recent first, then persistent grabs, then the implicit grab.
    pub(crate) entries: Vec<GrabEntry<G, F>>,
    next_seq: u64,
}

impl<G: ?Sized, F> std::fmt::Debug for GrabStack<G, F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list()
            .entries(
                self.entries
                    .iter()
                    .map(|e| (e.meta.serial, e.meta.implicit, e.meta.persistent)),
            )
            .finish()
    }
}

impl<G: ?Sized, F> GrabStack<G, F> {
    pub(crate) fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_seq: 0,
        }
    }

    fn entry(&mut self, grab: NewGrab<G, F>, implicit: bool) -> GrabEntry<G, F> {
        self.next_seq += 1;
        GrabEntry {
            meta: GrabMeta {
                serial: grab.serial,
                seq: self.next_seq,
                type_id: grab.type_id,
                implicit,
                persistent: grab.persistent,
                removed: false,
                pending_focus: grab.pending_focus,
            },
            handler: grab.handler,
        }
    }

    /// Remove and return every entry matching `pred`. The caller is expected to call `unset` on
    /// the returned grabs.
    pub(crate) fn take_where(
        &mut self,
        mut pred: impl FnMut(&GrabEntry<G, F>) -> bool,
    ) -> Vec<GrabEntry<G, F>> {
        let mut taken = Vec::new();
        let mut i = 0;
        while i < self.entries.len() {
            if pred(&self.entries[i]) {
                taken.push(self.entries.remove(i));
            } else {
                i += 1;
            }
        }
        taken
    }

    /// The grabs that installing an explicit grab displaces.
    ///
    /// The caller is expected to call `unset` on them.
    pub(crate) fn take_displaced(
        &mut self,
        serial: Serial,
        type_id: TypeId,
        persistent: bool,
    ) -> Vec<GrabEntry<G, F>> {
        if persistent {
            return self.take_where(|e| e.meta.type_id == type_id);
        }

        // Is this the next stage of an interaction that is in progress, or a new one?
        let continues = self
            .entries
            .iter()
            .any(|e| !e.meta.persistent && e.meta.serial == serial);
        self.take_where(|e| {
            !e.meta.persistent
                && (!continues || e.meta.serial == serial || e.meta.type_id == type_id || e.meta.implicit)
        })
    }

    /// Install an explicit grab: an interaction on top, a persistent grab below the interactions.
    ///
    /// Call [`Self::take_displaced`] first.
    pub(crate) fn push(&mut self, grab: NewGrab<G, F>) -> u64 {
        let at = if grab.persistent {
            self.entries
                .iter()
                .position(|e| e.meta.persistent || e.meta.implicit)
                .unwrap_or(self.entries.len())
        } else {
            0
        };
        let entry = self.entry(grab, false);
        let seq = entry.meta.seq;
        self.entries.insert(at, entry);
        seq
    }

    /// Apply what grabs asked for during a dispatch and sweep the ones that ended.
    ///
    /// Returns the grabs that were displaced in the process, for the caller to `unset`.
    pub(crate) fn finish(&mut self, deferred: Vec<Deferred<G, F>>) -> Vec<GrabEntry<G, F>> {
        let mut displaced = Vec::new();
        for op in deferred {
            match op {
                Deferred::Replace(seq, grab) => {
                    if let Some(i) = self.entries.iter().position(|e| e.meta.seq == seq) {
                        let implicit = self.entries[i].meta.implicit;
                        let entry = self.entry(grab, implicit);
                        self.entries[i] = entry;
                    } else {
                        displaced.extend(self.take_displaced(grab.serial, grab.type_id, grab.persistent));
                        self.push(grab);
                    }
                }
                Deferred::Implicit(grab) => {
                    displaced.extend(self.take_where(|e| e.meta.implicit && !e.meta.removed));
                    let entry = self.entry(grab, true);
                    self.entries.push(entry);
                }
                Deferred::Top(grab) => {
                    displaced.extend(self.take_displaced(grab.serial, grab.type_id, grab.persistent));
                    self.push(grab);
                }
            }
        }
        self.entries.retain(|e| !e.meta.removed);
        displaced
    }

    /// The most recently installed grab that is a user interaction, which is what the
    /// single-grab accessors of the device handles report.
    pub(crate) fn current(&self) -> Option<&GrabEntry<G, F>> {
        self.entries
            .iter()
            .filter(|e| !e.meta.persistent)
            .max_by_key(|e| e.meta.seq)
    }

    /// Split the stack at the grab with the given `seq`.
    pub(crate) fn split_at_seq(&mut self, seq: u64) -> Option<(&mut GrabMeta<F>, &mut [GrabEntry<G, F>])> {
        let i = self.entries.iter().position(|e| e.meta.seq == seq)?;
        let (head, below) = self.entries.split_at_mut(i + 1);
        Some((&mut head[i].meta, below))
    }
}

/// The next grab to visit in `entries` and what lies below it.
pub(crate) fn split_next<G: ?Sized, F>(
    mut entries: &mut [GrabEntry<G, F>],
    skip_persistent: bool,
) -> Option<(&mut GrabMeta<F>, &mut G, &mut [GrabEntry<G, F>])> {
    loop {
        let (entry, below) = std::mem::take(&mut entries).split_first_mut()?;
        if entry.meta.removed || (skip_persistent && entry.meta.persistent) {
            entries = below;
            continue;
        }
        return Some((&mut entry.meta, &mut *entry.handler, below));
    }
}
