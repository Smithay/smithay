use std::ops::Deref;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard,
};

use crate::backend::allocator::{Allocator, Buffer, Format};

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
/// If you have associated resources for each buffer, that can also be re-used (e.g. framebuffer Handles for a `DrmDevice`),
/// you can store then in the buffer slots userdata, where it gets freed, if the buffer gets allocated, but
/// is still valid, if the buffer was just re-used. So instead of creating a framebuffer handle for each new
/// buffer, you can skip creation, if the userdata already contains a framebuffer handle.
pub struct Swapchain<A: Allocator<B>, B: Buffer, U: 'static> {
    /// Allocator used by the swapchain
    pub allocator: A,

    width: u32,
    height: u32,
    format: Format,

    slots: [Slot<B, U>; SLOT_CAP],
}

/// Slot of a swapchain containing an allocated buffer and its userdata.
///
/// Can be cloned and passed around freely, the buffer is marked for re-use
/// once all copies are dropped. Holding on to this struct will block the
/// buffer in the swapchain.
pub struct Slot<B: Buffer, U: 'static> {
    buffer: Arc<Option<B>>,
    acquired: Arc<AtomicBool>,
    userdata: Arc<Mutex<Option<U>>>,
}

impl<B: Buffer, U: 'static> Slot<B, U> {
    /// Set userdata for this slot.
    pub fn set_userdata(&self, data: U) -> Option<U> {
        self.userdata.lock().unwrap().replace(data)
    }

    /// Retrieve userdata for this slot.
    pub fn userdata(&self) -> MutexGuard<'_, Option<U>> {
        self.userdata.lock().unwrap()
    }

    /// Clear userdata contained in this slot.
    pub fn clear_userdata(&self) -> Option<U> {
        self.userdata.lock().unwrap().take()
    }
}

impl<B: Buffer, U: 'static> Default for Slot<B, U> {
    fn default() -> Self {
        Slot {
            buffer: Arc::new(None),
            acquired: Arc::new(AtomicBool::new(false)),
            userdata: Arc::new(Mutex::new(None)),
        }
    }
}

impl<B: Buffer, U: 'static> Clone for Slot<B, U> {
    fn clone(&self) -> Self {
        Slot {
            buffer: self.buffer.clone(),
            acquired: self.acquired.clone(),
            userdata: self.userdata.clone(),
        }
    }
}

impl<B: Buffer, U: 'static> Deref for Slot<B, U> {
    type Target = B;
    fn deref(&self) -> &B {
        Option::as_ref(&*self.buffer).unwrap()
    }
}

impl<B: Buffer, U: 'static> Drop for Slot<B, U> {
    fn drop(&mut self) {
        self.acquired.store(false, Ordering::SeqCst);
    }
}

impl<A, B, U> Swapchain<A, B, U>
where
    A: Allocator<B>,
    B: Buffer,
    U: 'static,
{
    /// Create a new swapchain with the desired allocator and dimensions and pixel format for the created buffers.
    pub fn new(allocator: A, width: u32, height: u32, format: Format) -> Swapchain<A, B, U> {
        Swapchain {
            allocator,
            width,
            height,
            format,
            slots: Default::default(),
        }
    }

    /// Acquire a new slot from the swapchain, if one is still free.
    ///
    /// The swapchain has an internal maximum of four re-usable buffers.
    /// This function returns the first free one.
    pub fn acquire(&mut self) -> Result<Option<Slot<B, U>>, A::Error> {
        if let Some(free_slot) = self.slots.iter_mut().find(|s| !s.acquired.load(Ordering::SeqCst)) {
            if free_slot.buffer.is_none() {
                free_slot.buffer = Arc::new(Some(self.allocator.create_buffer(
                    self.width,
                    self.height,
                    self.format,
                )?));
            }
            assert!(free_slot.buffer.is_some());

            if !free_slot.acquired.swap(true, Ordering::SeqCst) {
                return Ok(Some(free_slot.clone()));
            }
        }

        // no free slots
        Ok(None)
    }

    /// Change the dimensions of newly returned buffers.
    ///
    /// Already optained buffers are unaffected and will be cleaned up on drop.
    pub fn resize(&mut self, width: u32, height: u32) {
        if self.width == width && self.height == height {
            return;
        }

        self.width = width;
        self.height = height;
        self.slots = Default::default();
    }
}
