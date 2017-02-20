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
            ptr: map(fd, size)?,
            fd: fd,
            size: size
        })
    }

    fn remap(&mut self, newsize: usize) -> Result<(),()> {
        unmap(self.ptr, self.size)?;
        self.ptr = map(self.fd, newsize)?;
        self.size = newsize;
        Ok(())
    }

    fn size(&self) -> usize {
        self.size
    }

    fn get_slice(&self) -> &[u8] {
        unsafe { ::std::slice::from_raw_parts(self.ptr, self.size) }
    }
}

// mman::mmap should really be unsafe... why isn't it?
#[allow(unused_unsafe)]
fn map(fd: RawFd, size: usize) -> Result<*mut u8, ()> {
    let ret = unsafe { mman::mmap(
        ptr::null_mut(),
        size,
        mman::PROT_READ,
        mman::MAP_SHARED,
        fd,
        0
    ) };
    ret.map(|p| p as *mut u8).map_err(|_| ())
}

// mman::munmap should really be unsafe... why isn't it?
#[allow(unused_unsafe)]
fn unmap(ptr: *mut u8, size: usize) -> Result<(),()> {
    let ret = unsafe { mman::munmap(ptr as *mut _, size) };
    ret.map_err(|_| ())
}
