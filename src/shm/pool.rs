use std::os::unix::io::RawFd;

pub struct Pool;

impl Pool {
    pub fn new(fd: RawFd, size: i32) -> Pool {
        unimplemented!()
    }

    pub fn resize(&self, newsize: i32) -> Result<(),()> {
        unimplemented!()
    }
    
    pub fn with_data_slice<F: FnOnce(&[u8])>(&self, f: F) -> Result<(),()> {
        unimplemented!()
    }
}
