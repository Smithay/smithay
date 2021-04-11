use std::convert::TryInto;
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use std::ops::Deref;

use crate::backend::allocator::{Allocator, Buffer, Format};

pub const SLOT_CAP: usize = 4;

pub struct Swapchain<A: Allocator<B>, B: Buffer + TryInto<B>, D: Buffer = B> {
    allocator: A,
    _original_buffer_format: std::marker::PhantomData<B>,

    width: u32,
    height: u32,
    format: Format,

    slots: [Slot<D>; SLOT_CAP],
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

#[derive(Debug, thiserror::Error)]
pub enum SwapchainError<E1, E2>
where
    E1: std::error::Error + 'static,
    E2: std::error::Error + 'static,
{
    #[error("Failed to allocate a new buffer: {0}")]
    AllocationError(#[source] E1),
    #[error("Failed to convert a new buffer: {0}")]
    ConversionError(#[source] E2),
}

impl<A, B, D, E1, E2> Swapchain<A, B, D>
where
    A: Allocator<B, Error=E1>,
    B: Buffer + TryInto<D, Error=E2>,
    D: Buffer,
    E1: std::error::Error + 'static,
    E2: std::error::Error + 'static,
{
    pub fn new(allocator: A, width: u32, height: u32, format: Format) -> Swapchain<A, B, D> {
        Swapchain {
            allocator,
            _original_buffer_format: std::marker::PhantomData,
            width,
            height,
            format,
            slots: Default::default(),
        }
    }

    pub fn acquire(&mut self) -> Result<Option<Slot<D>>, SwapchainError<E1, E2>> {
        if let Some(free_slot) = self.slots.iter_mut().filter(|s| !s.acquired.load(Ordering::SeqCst)).next() {
            if free_slot.buffer.is_none() {
                free_slot.buffer = Arc::new(Some(
                    self.allocator
                    .create_buffer(self.width, self.height, self.format).map_err(SwapchainError::AllocationError)?
                    .try_into().map_err(SwapchainError::ConversionError)?
                ));
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
        self.slots = Default::default();
    }
}