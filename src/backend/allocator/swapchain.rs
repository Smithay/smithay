use std::{
    fmt,
    ops::Deref,
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc,
    },
};

use tracing::instrument;

use crate::backend::allocator::{Allocator, Buffer, Fourcc, Modifier};
use crate::utils::user_data::UserDataMap;

use super::dmabuf::{AsDmabuf, Dmabuf};

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
pub struct Swapchain<A: Allocator> {
    /// Allocator used by the swapchain
    pub allocator: A,

    width: u32,
    height: u32,
    fourcc: Fourcc,
    modifiers: Vec<Modifier>,

    slots: [Arc<InternalSlot<A::Buffer>>; SLOT_CAP],
}

impl<A: Allocator> fmt::Debug for Swapchain<A> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Swapchain")
            .field("width", &self.width)
            .field("height", &self.height)
            .field("fourcc", &self.fourcc)
            .field("modifiers", &self.modifiers)
            .finish_non_exhaustive()
    }
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

impl<B: Buffer + AsDmabuf> AsDmabuf for Slot<B> {
    type Error = <B as AsDmabuf>::Error;

    fn export(&self) -> Result<super::dmabuf::Dmabuf, Self::Error> {
        let maybe_dmabuf = self.userdata().get::<Dmabuf>();
        if maybe_dmabuf.is_none() {
            let dmabuf = (**self).export()?;
            self.userdata().insert_if_missing(|| dmabuf);
        }

        Ok(self.userdata().get::<Dmabuf>().cloned().unwrap())
    }
}

impl<B: Buffer> Drop for Slot<B> {
    fn drop(&mut self) {
        self.0.acquired.store(false, Ordering::SeqCst);
    }
}

impl<A> Swapchain<A>
where
    A: Allocator,
{
    /// Create a new swapchain with the desired allocator, dimensions and pixel format for the created buffers.
    pub fn new(
        allocator: A,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        modifiers: Vec<Modifier>,
    ) -> Swapchain<A> {
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
    #[instrument(level = "trace", skip_all, err)]
    #[profiling::function]
    pub fn acquire(&mut self) -> Result<Option<Slot<A::Buffer>>, A::Error> {
        if let Some(free_slot) = self
            .slots
            .iter_mut()
            .find(|s| !s.acquired.swap(true, Ordering::SeqCst))
        {
            if free_slot.buffer.is_none() {
                let free_slot = Arc::get_mut(free_slot).expect("Acquired was false, but Arc is not unique?");
                match self
                    .allocator
                    .create_buffer(self.width, self.height, self.fourcc, &self.modifiers)
                {
                    Ok(buffer) => free_slot.buffer = Some(buffer),
                    Err(err) => {
                        free_slot.acquired.store(false, Ordering::SeqCst);
                        return Err(err);
                    }
                }
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
    /// the buffer may not be used for rendering anymore.
    /// You may hold on to it, if you require keeping it alive.
    ///
    /// Buffers can always just be safely discarded by dropping them, but not
    /// calling this function before may affect performance characteristics
    /// (e.g. by not tracking the buffer age).
    pub fn submitted(&mut self, slot: &Slot<A::Buffer>) {
        // don't mess up the state, if the user submitted and old buffer, after e.g. a resize
        if !self.slots.iter().any(|other| Arc::ptr_eq(&slot.0, other)) {
            return;
        }

        slot.0.age.store(1, Ordering::SeqCst);
        for other_slot in &mut self.slots {
            if !Arc::ptr_eq(other_slot, &slot.0) && other_slot.buffer.is_some() {
                let res = other_slot
                    .age
                    .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |age| {
                        if age > 0 {
                            age.checked_add(1)
                        } else {
                            Some(0)
                        }
                    });
                // If the age overflows the slot was not used for a long time. Lets clear it
                if res.is_err() {
                    *other_slot = Default::default();
                }
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

    /// Remove all internally cached buffers.
    pub fn reset_buffers(&mut self) {
        for slot in &mut self.slots {
            *slot = Default::default();
        }
    }

    /// Reset the age for each buffer.
    ///
    /// Resetting the buffer age will discard all damage information and force a
    /// full redraw for the next frame.
    pub fn reset_buffer_ages(&mut self) {
        for slot in &mut self.slots {
            match Arc::get_mut(slot) {
                Some(slot) => slot.age = AtomicU8::new(0),
                None => *slot = Default::default(),
            }
        }
    }

    /// Get set format
    pub fn format(&self) -> Fourcc {
        self.fourcc
    }
}
