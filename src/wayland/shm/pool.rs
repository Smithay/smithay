use nix::{
    libc,
    sys::{
        mman,
        signal::{self, SigAction, SigHandler, Signal},
    },
    unistd,
};
use std::{
    cell::Cell,
    io,
    ops::Deref,
    os::unix::{io::RawFd, prelude::AsRawFd},
    ptr::{self, NonNull},
    slice,
    sync::{Mutex, Once},
};

use slog::{debug, trace};

// Maintainers note: The initializer must NEVER panic or allocate memory.
thread_local!(static SIGBUS_GUARD: Cell<(*const MemMap, bool)> = Cell::new((ptr::null_mut(), false)));

static SIGBUS_INIT: Once = Once::new();
static mut OLD_SIGBUS_HANDLER: *mut SigAction = 0 as *mut SigAction;

#[derive(Debug)]
pub struct Pool {
    // The Mutex wrapping the MemMap is very important.
    //
    // In version 1 of wl_shm, there is nothing which tells the server how the memmap will be used. 99% of the
    // time the memmap is only written by the client and read by the server (a wl_shm backed wl_buffer). This
    // would mean guarding the memmap with a RwLock would typically be fine.
    //
    // However some protocols will have the server write write and the client read (zwlr_screencopy_frame_v1::copy).
    // Therefore we must guard the MemMap with a Mutex.
    map: Mutex<MemMap>,
    fd: RawFd,
    log: ::slog::Logger,
}

// SAFETY: Transferring the MemMap is safe as long as the pointer is not accessed outside the pool.
unsafe impl Send for Pool {}
// SAFETY: The underlying MemMap is guarded by a Mutex, ensuring there is no data race when the server reads
// or writes from the MemMap.
unsafe impl Sync for Pool {}

pub enum ResizeError {
    InvalidSize,
    MremapFailed,
}

impl Pool {
    /// This function takes ownership of the file descriptor.
    pub fn new(fd: RawFd, size: usize, log: ::slog::Logger) -> io::Result<Pool> {
        let memmap = match MemMap::new(fd, size) {
            Ok(memmap) => memmap,
            // TODO: Use inspect_err when stabilized
            Err(err) => {
                trace!(log, "Failed to map shm pool"; "fd" => fd as i32, "size" => size, "error" => &err);
                // Close the file descriptor since we own it.
                let _ = unistd::close(fd);
                return Err(err);
            }
        };

        trace!(log, "Creating new shm pool"; "fd" => fd as i32, "size" => size);
        Ok(Pool {
            map: Mutex::new(memmap),
            fd,
            log,
        })
    }

    pub fn resize(&self, newsize: i32) -> Result<(), ResizeError> {
        let mut guard = self.map.lock().unwrap();
        let oldsize = guard.size();

        if newsize <= 0 || oldsize > (newsize as usize) {
            return Err(ResizeError::InvalidSize);
        }

        trace!(self.log, "Resizing shm pool"; "fd" => self.fd as i32, "oldsize" => oldsize, "newsize" => newsize);

        if let Err(err) = guard.remap(newsize as usize) {
            debug!(
                self.log,
                "SHM pool resize failed"; "fd" => self.fd as i32, "oldsize" => oldsize, "newsize" => newsize, "error" => err
            );
            return Err(ResizeError::MremapFailed);
        }

        Ok(())
    }

    pub fn size(&self) -> usize {
        let guard = self.map.lock().unwrap();
        match *guard {
            MemMap::Mapping { size, .. } => size,
            MemMap::Invalid => 0,
        }
    }

    pub fn with_data_slice<T, F: FnOnce(&[u8]) -> T>(&self, f: F) -> Result<T, ()> {
        // Place the sigbus handler
        SIGBUS_INIT.call_once(|| unsafe {
            place_sigbus_handler();
        });

        let pool_guard = self.map.lock().unwrap();

        trace!(self.log, "Buffer access on shm pool"; "fd" => self.fd as i32);

        // Prepare the access
        SIGBUS_GUARD.with(|guard| {
            let (p, _) = guard.get();
            if !p.is_null() {
                // Recursive call of this method is not supported
                panic!("Recursive access to a SHM pool content is not supported.");
            }
            guard.set((&*pool_guard as *const MemMap, false))
        });

        let slice = &pool_guard[..];
        let t = f(slice);

        // Cleanup Post-access
        SIGBUS_GUARD.with(|guard| {
            let (_, triggered) = guard.get();
            guard.set((ptr::null_mut(), false));
            if triggered {
                debug!(self.log, "SIGBUS caught on access on shm pool"; "fd" => self.fd);
                Err(())
            } else {
                Ok(t)
            }
        })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        trace!(self.log, "Deleting SHM pool"; "fd" => self.fd);
        let _ = unistd::close(self.fd);
    }
}

#[derive(Debug)]
enum MemMap {
    /// A valid mapping.
    Mapping {
        ptr: NonNull<u8>,
        /// Do not close, this file descriptor is owned by the parent Pool.
        fd: RawFd,
        size: usize,
    },

    /// The invalid state.
    ///
    /// This may be set if remapping an shm pool fails after a resize.
    Invalid,
}

impl MemMap {
    fn new(fd: RawFd, size: usize) -> io::Result<MemMap> {
        let ptr = unsafe { map(fd, size) }?;
        // map must return Ok if the Err branch isn't taken
        let ptr = NonNull::new(ptr).unwrap();
        Ok(MemMap::Mapping { ptr, fd, size })
    }

    fn remap(&mut self, newsize: usize) -> io::Result<()> {
        match self {
            MemMap::Mapping {
                ptr: mapping_ptr,
                fd,
                size,
            } => {
                // memunmap cannot fail, as we are unmapping a pre-existing map
                let _ = unsafe { unmap(mapping_ptr.as_ptr(), *size) };

                // remap the fd with the new size
                match unsafe { map(fd.as_raw_fd(), newsize) } {
                    Ok(ptr) => {
                        // update the parameters
                        *mapping_ptr = NonNull::new(ptr).unwrap();
                        *size = newsize;
                        Ok(())
                    }

                    Err(err) => {
                        // set ourselves in an empty state
                        *self = MemMap::Invalid;
                        Err(err.into())
                    }
                }
            }

            // Previous remap has failed.
            MemMap::Invalid => Err(io::Error::new(
                io::ErrorKind::Other,
                "Cannot remap because previous remap has failed.",
            )),
        }
    }

    fn size(&self) -> usize {
        match self {
            MemMap::Mapping { size, .. } => *size,
            MemMap::Invalid => 0,
        }
    }

    fn contains(&self, ptr: *mut u8) -> bool {
        match self {
            MemMap::Mapping {
                ptr: mapping_ptr,
                size,
                ..
            } => ptr >= mapping_ptr.as_ptr() && ptr < unsafe { mapping_ptr.as_ptr().add(*size) },

            // An invalid mapping could never contain a pointer
            MemMap::Invalid => false,
        }
    }

    /// Nullifies the current memory mapping.
    ///
    /// Nullification replaces the existing memory mapping with an anonymous mapping.
    ///
    /// # Safety
    ///
    /// The caller must ensure exclusive access to the memory mapping.
    unsafe fn nullify(&self) -> nix::Result<()> {
        match self {
            MemMap::Mapping { ptr, size, .. } => nullify_map(ptr.as_ptr(), *size),

            // Can't nullify an invalid mapping
            MemMap::Invalid => Ok(()),
        }
    }

    /// # SAFETY:
    ///
    /// The mmap'd memory must support the PROT_WRITE flag.
    #[allow(dead_code)]
    unsafe fn get_mut(&mut self) -> &mut [u8] {
        match self {
            // SAFETY: Writers are required to mutably borrow the MemMap, and the lifetime of the return value
            // is elided to the lifetime of the MemMap. The borrow checker therefore can assert the contents
            // are not accessed until the returned mutable reference is dropped.
            MemMap::Mapping { ptr, size, .. } => slice::from_raw_parts_mut(ptr.as_ptr(), *size),
            MemMap::Invalid => &mut [],
        }
    }
}

impl Deref for MemMap {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        match self {
            // SAFETY: Readers are required to borrow the MemMap, and the lifetime of the return value is
            // elided to the lifetime of the MemMap. The borrow checker therefore can assert the contents
            // are not mutated until the returned mutable reference is dropped.
            //
            // The mmap'd memory must also be readable since PROT_WRITE implies PROT_READ.
            MemMap::Mapping { ptr, size, .. } => unsafe { slice::from_raw_parts(ptr.as_ptr(), *size) },
            MemMap::Invalid => &[],
        }
    }
}

impl Drop for MemMap {
    fn drop(&mut self) {
        if let MemMap::Mapping { ptr, size, .. } = self {
            let _ = unsafe { unmap(ptr.as_ptr(), *size) };
        }
    }
}

unsafe fn map(fd: RawFd, size: usize) -> nix::Result<*mut u8> {
    mman::mmap(
        ptr::null_mut(),
        size,
        mman::ProtFlags::PROT_READ,
        mman::MapFlags::MAP_SHARED,
        fd,
        0,
    )
    .map(|p| p as *mut u8)
}

unsafe fn unmap(ptr: *mut u8, size: usize) -> nix::Result<()> {
    mman::munmap(ptr as *mut _, size)
}

unsafe fn nullify_map(ptr: *mut u8, size: usize) -> nix::Result<()> {
    mman::mmap(
        ptr as *mut _,
        size,
        mman::ProtFlags::PROT_READ,
        mman::MapFlags::MAP_ANONYMOUS
            | mman::MapFlags::MAP_PRIVATE
            // Require the os to place the mapping at the specified address.
            | mman::MapFlags::MAP_FIXED,
        // mmap(2): > some implementations require fd to be -1 if MAP_ANONYMOUS (or MAP_ANON) is specified
        -1,
        0,
    )?;

    Ok(())
}

unsafe fn place_sigbus_handler() {
    // create our sigbus handler
    let action = SigAction::new(
        SigHandler::SigAction(sigbus_handler),
        signal::SaFlags::SA_NODEFER,
        signal::SigSet::empty(),
    );
    match signal::sigaction(Signal::SIGBUS, &action) {
        Ok(old_signal) => {
            OLD_SIGBUS_HANDLER = Box::into_raw(Box::new(old_signal));
        }
        Err(e) => panic!("sigaction failed for SIGBUS handler: {:?}", e),
    }
}

/// Safety:
///
/// The old sigbus handler must be initialized.
unsafe fn reraise_sigbus() {
    // reset the old sigaction
    let _ = signal::sigaction(Signal::SIGBUS, &*OLD_SIGBUS_HANDLER);
    let _ = signal::raise(Signal::SIGBUS);
}

/// The sigbus handler for wl_shm memory.
///
/// ## What can I do in a signal handler?
///
/// You'd need to read the manpages for your OS, but these general rules apply:
/// - No memory allocation
///   - This means any function which can panic is unsafe.
///   - This means even catch_unwind is unsafe because the panic machinery allocates memory.
/// - All invoked libc functions must be async-signal-safe
///
/// There are a few things that are thankfully async-signal-safe:
/// - Calling sigaction and raise (we can reraise the old signal handler)
/// - The vast majority of system calls, even if not listed need to be usable in async-signal-unsafe contexts
///   on any reasonable operating system.
extern "C" fn sigbus_handler(_signum: libc::c_int, info: *mut libc::siginfo_t, _context: *mut libc::c_void) {
    // SAFETY: The info is valid for the lifetime of this signal handler.
    let info = unsafe { &*info };

    // TODO: Check si_code to tell clients what they did wrong:
    // Linux: truncating a mmap'd file means si_code == BUS_ADRERR
    // NetBSD: https://man.netbsd.org/siginfo.2
    //
    // TODO: Could also be useful to test for BUS_OBJERR.

    // Get the faulty address that was read.
    let faulty_ptr = unsafe { info.si_addr() } as *mut u8;

    // SAFETY: The sigbus guard will never panic, (which is unsafe in signal handlers) because the initializer
    // cannot panic.
    //
    // Now wait isn't pthread_getspecific not async-signal-safe?
    //
    // Yes, it is unsound if all of the following are false:
    // - The target supports the #[thread_local] attribute
    // - The target is Linux with glibc (since pthread_getspecific is effectively async-signal-safe with glibc).
    //   Although this is redundant since Linux with glibc supports #[thread_local]
    // - The pthread_getspecific implementation is async-signal-safe (you'll need to browse the libc source
    //   code for the target)
    // - The implementation has static thread locals (wasm and platforms with no atomics)
    let result = SIGBUS_GUARD.try_with(|guard| {
        let (memmap, _) = guard.get();

        if let Some(memmap) = unsafe { memmap.as_ref() } {
            if memmap.contains(faulty_ptr) {
                // We are certain the SIGBUS originates from the memory mapping. Remember that the access was faulty.
                guard.set((memmap, true));

                // Nullify the pool
                //
                // SAFETY:
                // - The guard is set and therefore the current thread has exclusive access to the memory mapping.
                // - mmap is a system call and should be fine to call in an async-signal-unsafe context on any
                //   reasonable operating system.
                if unsafe { memmap.nullify() }.is_err() {
                    // something terrible occurred!
                    unsafe { reraise_sigbus() }
                }

                return;
            }
        }

        // We are not responsible for the sigbus, raise the default sigbus handler.
        unsafe { reraise_sigbus() }
    });

    // TLS was accessed during or after destruction.
    if result.is_err() {
        unsafe { reraise_sigbus() }
    }
}
