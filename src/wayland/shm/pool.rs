#![forbid(unsafe_op_in_unsafe_fn)]

use std::{
    cell::Cell,
    mem,
    num::NonZeroUsize,
    os::unix::io::{AsFd, BorrowedFd, OwnedFd},
    ptr,
    sync::{
        mpsc::{sync_channel, SyncSender},
        Once, RwLock,
    },
    thread,
};

use once_cell::sync::Lazy;
use rustix::mm;
use tracing::{debug, instrument, trace};

// Dropping Pool is actually pretty slow. Unmapping the memory can take 1-2 ms, but the real
// offender is closing the file descriptor, which I've seen take up to 6 ms. It's waiting on some
// spinlock in the kernel.
//
// Blocking the main thread for 6 ms is quite bad. In fact, 6 ms is almost the entire time budget
// for a 165 Hz frame. To make matters worse, some clients will cause repeated creation and
// dropping of shm pools, like Firefox during a focus-out animation. This results in dropped
// frames.
//
// To work around this problem, we spawn a separate thread whose sole purpose is dropping stuff we
// send it through a channel. Conveniently, Pool is already Send, so there's no problem doing this.
//
// We use SyncSender because the regular Sender only got Sync in 1.72 which is above our MSRV.
static DROP_THIS: Lazy<SyncSender<InnerPool>> = Lazy::new(|| {
    let (tx, rx) = sync_channel(16);
    thread::Builder::new()
        .name("Shm dropping thread".to_owned())
        .spawn(move || {
            while let Ok(x) = rx.recv() {
                profiling::scope!("dropping Pool");
                drop(x);
            }
        })
        .unwrap();
    tx
});

thread_local!(static SIGBUS_GUARD: Cell<(*const MemMap, bool)> = Cell::new((ptr::null_mut(), false)));

/// SAFETY:
/// This will be only set in the `SIGBUS_INIT` closure, hence only once!
static mut OLD_SIGBUS_HANDLER: Option<libc::sigaction> = None;
static SIGBUS_INIT: Once = Once::new();

#[derive(Debug)]
pub struct Pool {
    inner: Option<InnerPool>,
}

#[derive(Debug)]
struct InnerPool {
    map: RwLock<MemMap>,
    fd: OwnedFd,
}

// SAFETY: The memmap is owned by the pool and content is only accessible via a reference.
unsafe impl Send for InnerPool {}
// SAFETY: The memmap is guarded by a RwLock, meaning no writers may mutate the memmap when it is being read.
unsafe impl Sync for InnerPool {}

pub enum ResizeError {
    InvalidSize,
    MremapFailed,
}

impl InnerPool {
    #[instrument(level = "trace", skip_all, name = "wayland_shm")]
    pub fn new(fd: OwnedFd, size: NonZeroUsize) -> Result<InnerPool, OwnedFd> {
        let memmap = match MemMap::new(fd.as_fd(), size) {
            Ok(memmap) => memmap,
            Err(_) => {
                return Err(fd);
            }
        };
        trace!(fd = ?fd, size = ?size, "Creating new shm pool");
        Ok(InnerPool {
            map: RwLock::new(memmap),
            fd,
        })
    }

    pub fn resize(&self, newsize: NonZeroUsize) -> Result<(), ResizeError> {
        let mut guard = self.map.write().unwrap();
        let oldsize = guard.size();

        if oldsize > usize::from(newsize) {
            return Err(ResizeError::InvalidSize);
        }

        trace!(fd = ?self.fd, oldsize = oldsize, newsize = ?newsize, "Resizing shm pool");
        guard.remap(self.fd.as_fd(), newsize).map_err(|()| {
            debug!(fd = ?self.fd, oldsize = oldsize, newsize = ?newsize, "SHM pool resize failed");
            ResizeError::MremapFailed
        })
    }

    pub fn size(&self) -> usize {
        self.map.read().unwrap().size
    }

    #[instrument(level = "trace", skip_all, name = "wayland_shm")]
    pub fn with_data<T, F: FnOnce(*const u8, usize) -> T>(&self, f: F) -> Result<T, ()> {
        // Place the sigbus handler
        SIGBUS_INIT.call_once(|| unsafe {
            place_sigbus_handler();
        });

        let pool_guard = self.map.read().unwrap();

        trace!(fd = ?self.fd, "Buffer access on shm pool");

        // Prepare the access
        SIGBUS_GUARD.with(|guard| {
            let (p, _) = guard.get();
            if !p.is_null() {
                // Recursive call of this method is not supported
                panic!("Recursive access to a SHM pool content is not supported.");
            }
            guard.set((&*pool_guard as *const MemMap, false))
        });

        let t = f(pool_guard.ptr as *const _, pool_guard.size);

        // Cleanup Post-access
        SIGBUS_GUARD.with(|guard| {
            let (_, triggered) = guard.get();
            guard.set((ptr::null_mut(), false));
            if triggered {
                debug!(fd = ?self.fd, "SIGBUS caught on access on shm pool");
                Err(())
            } else {
                Ok(t)
            }
        })
    }

    #[instrument(level = "trace", skip_all, name = "wayland_shm")]
    pub fn with_data_mut<T, F: FnOnce(*mut u8, usize) -> T>(&self, f: F) -> Result<T, ()> {
        // Place the sigbus handler
        SIGBUS_INIT.call_once(|| unsafe {
            place_sigbus_handler();
        });

        let pool_guard = self.map.write().unwrap();

        trace!(fd = ?self.fd, "Mutable buffer access on shm pool");

        // Prepare the access
        SIGBUS_GUARD.with(|guard| {
            let (p, _) = guard.get();
            if !p.is_null() {
                // Recursive call of this method is not supported
                panic!("Recursive access to a SHM pool content is not supported.");
            }
            guard.set((&*pool_guard as *const MemMap, false))
        });

        let t = f(pool_guard.ptr, pool_guard.size);

        // Cleanup Post-access
        SIGBUS_GUARD.with(|guard| {
            let (_, triggered) = guard.get();
            guard.set((ptr::null_mut(), false));
            if triggered {
                debug!(fd = ?self.fd, "SIGBUS caught on access on shm pool");
                Err(())
            } else {
                Ok(t)
            }
        })
    }
}

impl Pool {
    pub fn new(fd: OwnedFd, size: NonZeroUsize) -> Result<Self, OwnedFd> {
        InnerPool::new(fd, size).map(|p| Self { inner: Some(p) })
    }

    pub fn resize(&self, newsize: NonZeroUsize) -> Result<(), ResizeError> {
        self.inner.as_ref().unwrap().resize(newsize)
    }

    pub fn size(&self) -> usize {
        self.inner.as_ref().unwrap().size()
    }

    pub fn with_data<T, F: FnOnce(*const u8, usize) -> T>(&self, f: F) -> Result<T, ()> {
        self.inner.as_ref().unwrap().with_data(f)
    }

    pub fn with_data_mut<T, F: FnOnce(*mut u8, usize) -> T>(&self, f: F) -> Result<T, ()> {
        self.inner.as_ref().unwrap().with_data_mut(f)
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        let _ = DROP_THIS.send(self.inner.take().unwrap());
    }
}

#[derive(Debug)]
struct MemMap {
    ptr: *mut u8,
    size: usize,
}

impl MemMap {
    fn new(fd: BorrowedFd<'_>, size: NonZeroUsize) -> Result<MemMap, ()> {
        Ok(MemMap {
            ptr: unsafe { map(fd, size) }?,
            size: size.into(),
        })
    }

    fn remap(&mut self, fd: BorrowedFd<'_>, newsize: NonZeroUsize) -> Result<(), ()> {
        if self.ptr.is_null() {
            return Err(());
        }
        // memunmap cannot fail, as we are unmapping a pre-existing map
        let _ = unsafe { unmap(self.ptr, self.size) };
        // remap the fd with the new size
        match unsafe { map(fd, newsize) } {
            Ok(ptr) => {
                // update the parameters
                self.ptr = ptr;
                self.size = usize::from(newsize);
                Ok(())
            }
            Err(()) => {
                // set ourselves in an empty state
                self.ptr = ptr::null_mut();
                self.size = 0;
                Err(())
            }
        }
    }

    fn size(&self) -> usize {
        self.size
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

/// A simple wrapper with some default arguments for `nix::mman::mmap`.
unsafe fn map(fd: BorrowedFd<'_>, size: NonZeroUsize) -> Result<*mut u8, ()> {
    let ret = unsafe {
        mm::mmap(
            ptr::null_mut(),
            size.into(),
            mm::ProtFlags::READ | mm::ProtFlags::WRITE,
            mm::MapFlags::SHARED,
            fd,
            0,
        )
    };
    ret.map(|p| p as *mut u8).map_err(|_| ())
}

/// A simple wrapper for `nix::mman::munmap`.
#[profiling::function]
unsafe fn unmap(ptr: *mut u8, size: usize) -> Result<(), ()> {
    let ret = unsafe { mm::munmap(ptr as *mut _, size) };
    ret.map_err(|_| ())
}

unsafe fn nullify_map(ptr: *mut u8, size: usize) -> Result<(), ()> {
    let ret = unsafe {
        mm::mmap_anonymous(
            ptr as *mut std::ffi::c_void,
            size,
            mm::ProtFlags::READ,
            mm::MapFlags::PRIVATE | mm::MapFlags::FIXED,
        )
    };
    ret.map(|_| ()).map_err(|_| ())
}

/// SAFETY: This function will be called only ONCE and that is in the closure of
/// `SIGBUS_INIT`.
unsafe fn place_sigbus_handler() {
    // create our sigbus handler
    unsafe {
        let action = libc::sigaction {
            sa_sigaction: sigbus_handler as _,
            sa_flags: libc::SA_SIGINFO | libc::SA_NODEFER,
            ..mem::zeroed()
        };
        let old_action = OLD_SIGBUS_HANDLER.insert(mem::zeroed());
        if libc::sigaction(libc::SIGBUS, &action, old_action) == -1 {
            let e = rustix::io::Errno::from_raw_os_error(errno::errno().0);
            panic!("sigaction failed for SIGBUS handler: {:?}", e);
        }
    }
}

unsafe fn reraise_sigbus() {
    // reset the old sigaction
    unsafe {
        libc::sigaction(
            libc::SIGBUS,
            OLD_SIGBUS_HANDLER.as_ref().unwrap(),
            ptr::null_mut(),
        );
        libc::raise(libc::SIGBUS);
    }
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

/// This was shamelessly stolen from rustc's source
/// so I expect it to work whenever rust works
/// I guess it's good enough?
///
/// SAFETY:
/// The returned pointer points to a struct. Make sure that you use it
/// appropriately.
#[cfg(any(target_os = "linux", target_os = "android"))]
unsafe fn siginfo_si_addr(info: *mut libc::siginfo_t) -> *mut libc::c_void {
    #[repr(C)]
    #[allow(non_camel_case_types)]
    struct siginfo_t {
        a: [libc::c_int; 3], // si_signo, si_errno, si_code
        si_addr: *mut libc::c_void,
    }

    unsafe { (*(info as *const siginfo_t)).si_addr }
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
unsafe fn siginfo_si_addr(info: *mut libc::siginfo_t) -> *mut libc::c_void {
    unsafe { (*info).si_addr as _ }
}
