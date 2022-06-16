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
    os::unix::io::RawFd,
    ptr,
    sync::{Once, RwLock},
};

use slog::{debug, trace};

thread_local!(static SIGBUS_GUARD: Cell<(*const MemMap, bool)> = Cell::new((ptr::null_mut(), false)));

static SIGBUS_INIT: Once = Once::new();
static mut OLD_SIGBUS_HANDLER: *mut SigAction = 0 as *mut SigAction;

#[derive(Debug)]
pub struct Pool {
    // TODO: We may need to change this to a Mutex in order to support protocols like wlr screencopy which may
    // require we write to a memmap.
    map: RwLock<MemMap>,
    fd: RawFd,
    log: ::slog::Logger,
}

// SAFETY: The memmap is owned by the pool and content is only accessible via a reference.
unsafe impl Send for Pool {}
// SAFETY: The memmap is guarded by a RwLock, meaning no writers may mutate the memmap when it is being read.
unsafe impl Sync for Pool {}

pub enum ResizeError {
    InvalidSize,
    MremapFailed,
}

impl Pool {
    pub fn new(fd: RawFd, size: usize, log: ::slog::Logger) -> Result<Pool, ()> {
        let memmap = MemMap::new(fd, size)?;
        trace!(log, "Creating new shm pool"; "fd" => fd as i32, "size" => size);
        Ok(Pool {
            map: RwLock::new(memmap),
            fd,
            log,
        })
    }

    pub fn resize(&self, newsize: i32) -> Result<(), ResizeError> {
        let mut guard = self.map.write().unwrap();
        let oldsize = guard.size();

        if newsize <= 0 || oldsize > (newsize as usize) {
            return Err(ResizeError::InvalidSize);
        }

        trace!(self.log, "Resizing shm pool"; "fd" => self.fd as i32, "oldsize" => oldsize, "newsize" => newsize);

        guard.remap(newsize as usize).map_err(|()| {
            debug!(self.log, "SHM pool resize failed"; "fd" => self.fd as i32, "oldsize" => oldsize, "newsize" => newsize);
            ResizeError::MremapFailed
        })
    }

    pub fn size(&self) -> usize {
        self.map.read().unwrap().size
    }

    pub fn with_data_slice<T, F: FnOnce(&[u8]) -> T>(&self, f: F) -> Result<T, ()> {
        // Place the sigbus handler
        SIGBUS_INIT.call_once(|| unsafe {
            place_sigbus_handler();
        });

        let pool_guard = self.map.read().unwrap();

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

        let slice = pool_guard.get_slice();
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
struct MemMap {
    ptr: *mut u8,
    fd: RawFd,
    size: usize,
}

impl MemMap {
    fn new(fd: RawFd, size: usize) -> Result<MemMap, ()> {
        Ok(MemMap {
            ptr: unsafe { map(fd, size) }?,
            fd,
            size,
        })
    }

    fn remap(&mut self, newsize: usize) -> Result<(), ()> {
        if self.ptr.is_null() {
            return Err(());
        }
        // memunmap cannot fail, as we are unmapping a pre-existing map
        let _ = unsafe { unmap(self.ptr, self.size) };
        // remap the fd with the new size
        match unsafe { map(self.fd, newsize) } {
            Ok(ptr) => {
                // update the parameters
                self.ptr = ptr;
                self.size = newsize;
                Ok(())
            }
            Err(()) => {
                // set ourselves in an empty state
                self.ptr = ptr::null_mut();
                self.size = 0;
                self.fd = -1;
                Err(())
            }
        }
    }

    fn size(&self) -> usize {
        self.size
    }

    fn get_slice(&self) -> &[u8] {
        // if we are in the 'invalid state', self.size == 0 and we return &[]
        // which is perfectly safe even if self.ptr is null
        unsafe { ::std::slice::from_raw_parts(self.ptr, self.size) }
    }

    fn contains(&self, ptr: *mut u8) -> bool {
        ptr >= self.ptr && ptr < unsafe { self.ptr.add(self.size) }
    }

    fn nullify(&self) -> Result<(), ()> {
        unsafe { nullify_map(self.ptr, self.size) }
    }
}

impl Drop for MemMap {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            let _ = unsafe { unmap(self.ptr, self.size) };
        }
    }
}

unsafe fn map(fd: RawFd, size: usize) -> Result<*mut u8, ()> {
    let ret = mman::mmap(
        ptr::null_mut(),
        size,
        mman::ProtFlags::PROT_READ,
        mman::MapFlags::MAP_SHARED,
        fd,
        0,
    );
    ret.map(|p| p as *mut u8).map_err(|_| ())
}

unsafe fn unmap(ptr: *mut u8, size: usize) -> Result<(), ()> {
    let ret = mman::munmap(ptr as *mut _, size);
    ret.map_err(|_| ())
}

unsafe fn nullify_map(ptr: *mut u8, size: usize) -> Result<(), ()> {
    let ret = mman::mmap(
        ptr as *mut _,
        size,
        mman::ProtFlags::PROT_READ,
        mman::MapFlags::MAP_ANONYMOUS | mman::MapFlags::MAP_PRIVATE | mman::MapFlags::MAP_FIXED,
        -1,
        0,
    );
    ret.map(|_| ()).map_err(|_| ())
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
        Err(e) => panic!("sigaction failed sor SIGBUS handler: {:?}", e),
    }
}

unsafe fn reraise_sigbus() {
    // reset the old sigaction
    let _ = signal::sigaction(Signal::SIGBUS, &*OLD_SIGBUS_HANDLER);
    let _ = signal::raise(Signal::SIGBUS);
}

extern "C" fn sigbus_handler(_signum: libc::c_int, info: *mut libc::siginfo_t, _context: *mut libc::c_void) {
    let faulty_ptr = unsafe { siginfo_si_addr(info) } as *mut u8;
    SIGBUS_GUARD.with(|guard| {
        let (memmap, _) = guard.get();
        match unsafe { memmap.as_ref() }.map(|m| (m, m.contains(faulty_ptr))) {
            Some((m, true)) => {
                // we are in a faulty memory pool !
                // remember that it was faulty
                guard.set((memmap, true));
                // nullify the pool
                if m.nullify().is_err() {
                    // something terrible occurred !
                    unsafe { reraise_sigbus() }
                }
            }
            _ => {
                // something else occurred, let's die honorably
                unsafe { reraise_sigbus() }
            }
        }
    });
}

// This was shamelessly stolen from rustc's source
// so I expect it to work whenever rust works
// I guess it's good enough?

#[cfg(any(target_os = "linux", target_os = "android"))]
unsafe fn siginfo_si_addr(info: *mut libc::siginfo_t) -> *mut libc::c_void {
    #[repr(C)]
    struct siginfo_t {
        a: [libc::c_int; 3], // si_signo, si_errno, si_code
        si_addr: *mut libc::c_void,
    }

    (*(info as *const siginfo_t)).si_addr
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
unsafe fn siginfo_si_addr(info: *mut libc::siginfo_t) -> *mut libc::c_void {
    (*info).si_addr
}
