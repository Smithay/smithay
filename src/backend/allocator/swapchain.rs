use std::ops::Deref;
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc,
};

use crate::backend::allocator::{Allocator, Buffer, Fourcc, Modifier};
use crate::utils::user_data::UserDataMap;

pub const SLOT_CAP: usize = 4;

/// Swapchain handling a fixed set of re-usable buffers e.g. for scan-out.
///
/// ## How am I supposed to use this?
///
/// To do proper buffer management, most compositors do so called double-buffering.
/// Which means you use two buffers, one that is currently presented (the front buffer)
/// and one that is currently rendered to (the back buffer). After each rendering operation
/// you swap the buffers around, the old front buffer becomes the new back buffer, while
/// the new front buffer is displayed to the user. This avoids showing the user rendering
/// artifacts doing rendering.
///
/// There are also reasons to do triple-buffering, e.g. if you swap operation takes a
/// unspecified amount of time. In that case you have one buffer, that is currently
/// displayed, one that is done drawing and about to be swapped in and another one,
/// which you can use to render currently.
///
/// Re-using and managing these buffers becomes increasingly complex the more buffers you
/// introduce, which is where `Swapchain` comes into play.
///
/// `Swapchain` allocates buffers for you and transparently re-created them, e.g. when resizing.
/// All you tell the swapchain is: *"Give me the next free buffer"* (by calling [`acquire`](Swapchain::acquire)).
/// You then hold on to the returned buffer during rendering and swapping and free it once it is displayed.
/// Efficient re-use of the buffers is done by the swapchain.
///
/// If you have associated resources for each buffer that can be reused (e.g. framebuffer `Handle`s for a `DrmDevice`),
/// you can store then in the `Slot`s userdata field. If a buffer is re-used, its userdata is preserved for the next time
/// it is returned by `acquire()`.
#[derive(Debug)]
pub struct Swapchain<A: Allocator<B>, B: Buffer> {
    /// Allocator used by the swapchain
    pub allocator: A,

    width: u32,
    height: u32,
    fourcc: Fourcc,
    modifiers: Vec<Modifier>,

    slots: [Arc<InternalSlot<B>>; SLOT_CAP],
}

/// Slot of a swapchain containing an allocated buffer and its userdata.
///
/// The buffer is marked for re-use once all copies are dropped.
/// Holding on to this struct will block the buffer in the swapchain.
#[derive(Debug)]
pub struct Slot<B: Buffer>(Arc<InternalSlot<B>>);

#[derive(Debug)]
struct InternalSlot<B: Buffer> {
    buffer: Option<B>,
    acquired: AtomicBool,
    age: AtomicU8,
    userdata: UserDataMap,
}

impl<B: Buffer> Slot<B> {
    /// Retrieve userdata for this slot.
    pub fn userdata(&self) -> &UserDataMap {
        &self.0.userdata
    }

    /// Retrieve the age of the buffer
    pub fn age(&self) -> u8 {
        self.0.age.load(Ordering::SeqCst)
    }
}

impl<B: Buffer> Default for InternalSlot<B> {
    fn default() -> Self {
        InternalSlot {
            buffer: None,
            acquired: AtomicBool::new(false),
            age: AtomicU8::new(0),
            userdata: UserDataMap::new(),
        }
    }
}

impl<B: Buffer> Deref for Slot<B> {
    type Target = B;
    fn deref(&self) -> &B {
        Option::as_ref(&self.0.buffer).unwrap()
    }
}

impl<B: Buffer> Drop for Slot<B> {
    fn drop(&mut self) {
        self.0.acquired.store(false, Ordering::SeqCst);
    }
}

impl<A, B> Swapchain<A, B>
where
    A: Allocator<B>,
    B: Buffer,
{
    /// Create a new swapchain with the desired allocator, dimensions and pixel format for the created buffers.
    pub fn new(
        allocator: A,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        modifiers: Vec<Modifier>,
    ) -> Swapchain<A, B> {
        Swapchain {
            allocator,
            width,
            height,
            fourcc,
            modifiers,
            slots: Default::default(),
        }
    }

    /// Acquire a new slot from the swapchain, if one is still free.
    ///
    /// The swapchain has an internal maximum of four re-usable buffers.
    /// This function returns the first free one.
    pub fn acquire(&mut self) -> Result<Option<Slot<B>>, A::Error> {
        if let Some(free_slot) = self
            .slots
            .iter_mut()
            .find(|s| !s.acquired.swap(true, Ordering::SeqCst))
        {
            if free_slot.buffer.is_none() {
                let mut free_slot =
                    Arc::get_mut(free_slot).expect("Acquired was false, but Arc is not unique?");
                free_slot.buffer = Some(self.allocator.create_buffer(
                    self.width,
                    self.height,
                    self.fourcc,
                    &self.modifiers,
                )?);
            }
            assert!(free_slot.buffer.is_some());
            return Ok(Some(Slot(free_slot.clone())));
        }

        // no free slots
        Ok(None)
    }

    /// Mark a given buffer as submitted.
    ///
    /// This might effect internal data (e.g. buffer age) and may only be called,
    /// if the buffer was actually used for display.
    /// Buffers can always just be safely discarded by dropping them, but not
    /// calling this function may affect performance characteristics
    /// (e.g. by not tracking the buffer age).
    pub fn submitted(&self, slot: Slot<B>) {
        // don't mess up the state, if the user submitted and old buffer, after e.g. a resize
        if !self.slots.iter().any(|other| Arc::ptr_eq(&slot.0, other)) {
            return;
        }

        slot.0.age.store(1, Ordering::SeqCst);
        for other_slot in &self.slots {
            if !Arc::ptr_eq(other_slot, &slot.0) && other_slot.buffer.is_some() {
                assert!(other_slot
                    .age
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |age| {
                        if age > 0 {
                            Some(age + 1)
                        } else {
                            Some(0)
                        }
                    })
                    .is_ok());
            }
        }
    }

    /// Change the dimensions of newly returned buffers.
    ///
    /// Already obtained buffers are unaffected and will be cleaned up on drop.
    pub fn resize(&mut self, width: u32, height: u32) {
        if self.width == width && self.height == height {
            return;
        }

        self.width = width;
        self.height = height;
        self.slots = Default::default();
    }

    /// Remove all internally cached buffers to e.g. reset age values
    pub fn reset_buffers(&mut self) {
        for slot in &mut self.slots {
            if let Some(internal_slot) = Arc::get_mut(slot) {
                *internal_slot = InternalSlot {
                    buffer: internal_slot.buffer.take(),
                    ..Default::default()
                };
            } else {
                *slot = Default::default();
            }
        }
    }
}
