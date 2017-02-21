use std::os::unix::io::RawFd;
use std::sync::RwLock;
use std::ptr;

use nix::sys::mman;

pub struct Pool {
    map: RwLock<MemMap>
}

pub enum ResizeError {
    InvalidSize,
    MremapFailed
}

impl Pool {
    pub fn new(fd: RawFd, size: usize) -> Result<Pool,()> {
        let memmap = MemMap::new(fd, size)?;
        Ok(Pool {
            map: RwLock::new(memmap)
        })
    }

    pub fn resize(&self, newsize: i32) -> Result<(),ResizeError> {
        let mut guard = self.map.write().unwrap();
        if newsize <= 0 || guard.size() > (newsize as usize) {
            return Err(ResizeError::InvalidSize)
        }
        guard.remap(newsize as usize).map_err(|()| ResizeError::MremapFailed)
    }
    
    pub fn with_data_slice<F: FnOnce(&[u8])>(&self, f: F) -> Result<(),()> {
        // TODO: handle SIGBUS
        let guard = self.map.read().unwrap();

        let slice = guard.get_slice();
        f(slice);

        Ok(())
    }
}

struct MemMap {
    ptr: *mut u8,
    fd: RawFd,
    size: usize
}

impl MemMap {
    fn new(fd: RawFd, size: usize) -> Result<MemMap,()> {
        Ok(MemMap {
            ptr: unsafe { map(fd, size) }?,
            fd: fd,
            size: size
        })
    }

    fn remap(&mut self, newsize: usize) -> Result<(),()> {
        if self.ptr.is_null() {
            return Err(())
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
            },
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
}

impl Drop for MemMap {
    fn drop(&mut self) {
        if !self.ptr.is_null() {
            let _ = unsafe { unmap(self.ptr, self.size) };
        }
    }
}

// mman::mmap should really be unsafe... why isn't it?
unsafe fn map(fd: RawFd, size: usize) -> Result<*mut u8, ()> {
    let ret = mman::mmap(
        ptr::null_mut(),
        size,
        mman::PROT_READ,
        mman::MAP_SHARED,
        fd,
        0
    );
    ret.map(|p| p as *mut u8).map_err(|_| ())
}

// mman::munmap should really be unsafe... why isn't it?
unsafe fn unmap(ptr: *mut u8, size: usize) -> Result<(),()> {
    let ret = mman::munmap(ptr as *mut _, size);
    ret.map_err(|_| ())
}
