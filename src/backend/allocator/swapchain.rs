use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::ops::Deref;

use crate::backend::allocator::{Allocator, Buffer, Format};

pub const SLOT_CAP: usize = 3;

pub struct Swapchain<A: Allocator<B>, B: Buffer> {
    allocator: A,

    width: u32,
    height: u32,
    format: Format,

    slots: [Slot<B>; SLOT_CAP],
}

pub struct Slot<B: Buffer> {
    buffer: Arc<Option<B>>,
    acquired: Arc<AtomicBool>,
}

impl<B: Buffer> Default for Slot<B> {
    fn default() -> Self {
        Slot {
            buffer: Arc::new(None),
            acquired: Arc::new(AtomicBool::new(false)),
        }
    }
}

impl<B: Buffer> Clone for Slot<B> {
    fn clone(&self) -> Self {
        Slot {
            buffer: self.buffer.clone(),
            acquired: self.acquired.clone(),
        }
    }
}

impl<B: Buffer> Deref for Slot<B> {
    type Target = B;
    fn deref(&self) -> &B {
        Option::as_ref(&*self.buffer).unwrap()
    }
}

impl<B: Buffer> Drop for Slot<B> {
    fn drop(&mut self) {
        self.acquired.store(false, Ordering::AcqRel);
    }
}

impl<A: Allocator<B>, B: Buffer> Swapchain<A, B> {
    pub fn new(allocator: A, width: u32, height: u32, format: Format) -> Swapchain<A, B> {
        Swapchain {
            allocator,
            width,
            height,
            format,
            slots: Default::default(),
        }
    }

    pub fn acquire(&mut self) -> Result<Option<Slot<B>>, A::Error> {
        if let Some(free_slot) = self.slots.iter_mut().filter(|s| !s.acquired.load(Ordering::SeqCst)).next() {
            if free_slot.buffer.is_none() {
                free_slot.buffer = Arc::new(Some(self.allocator.create_buffer(self.width, self.height, self.format)?));
            }
            assert!(!free_slot.buffer.is_some());

            if !free_slot.acquired.swap(true, Ordering::AcqRel) {
                return Ok(Some(free_slot.clone()));
            }

        }

        // no free slots
        Ok(None)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if self.width == width && self.height == height {
            return;
        }

        self.width = width;
        self.height = height;

        for mut slot in &mut self.slots {
            let _ = std::mem::replace(&mut slot, &mut Slot::default());
        }
    }
}