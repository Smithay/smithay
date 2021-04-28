use std::convert::TryInto;
use std::ops::Deref;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard,
};

use crate::backend::allocator::{Allocator, Buffer, Format};

pub const SLOT_CAP: usize = 4;

pub struct Swapchain<A: Allocator<B>, B: Buffer + TryInto<B>, U: 'static, D: Buffer = B> {
    pub allocator: A,
    _original_buffer_format: std::marker::PhantomData<B>,

    width: u32,
    height: u32,
    format: Format,

    slots: [Slot<D, U>; SLOT_CAP],
}

pub struct Slot<B: Buffer, U: 'static> {
    buffer: Arc<Option<B>>,
    acquired: Arc<AtomicBool>,
    userdata: Arc<Mutex<Option<U>>>,
}

impl<B: Buffer, U: 'static> Slot<B, U> {
    pub fn set_userdata(&self, data: U) -> Option<U> {
        self.userdata.lock().unwrap().replace(data)
    }

    pub fn userdata(&self) -> MutexGuard<'_, Option<U>> {
        self.userdata.lock().unwrap()
    }

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

impl<A, B, D, U, E1, E2> Swapchain<A, B, U, D>
where
    A: Allocator<B, Error = E1>,
    B: Buffer + TryInto<D, Error = E2>,
    D: Buffer,
    E1: std::error::Error + 'static,
    E2: std::error::Error + 'static,
    U: 'static,
{
    pub fn new(allocator: A, width: u32, height: u32, format: Format) -> Swapchain<A, B, U, D> {
        Swapchain {
            allocator,
            _original_buffer_format: std::marker::PhantomData,
            width,
            height,
            format,
            slots: Default::default(),
        }
    }

    pub fn acquire(&mut self) -> Result<Option<Slot<D, U>>, SwapchainError<E1, E2>> {
        if let Some(free_slot) = self.slots.iter_mut().find(|s| !s.acquired.load(Ordering::SeqCst)) {
            if free_slot.buffer.is_none() {
                free_slot.buffer = Arc::new(Some(
                    self.allocator
                        .create_buffer(self.width, self.height, self.format)
                        .map_err(SwapchainError::AllocationError)?
                        .try_into()
                        .map_err(SwapchainError::ConversionError)?,
                ));
            }
            assert!(free_slot.buffer.is_some());

            if !free_slot.acquired.swap(true, Ordering::SeqCst) {
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
