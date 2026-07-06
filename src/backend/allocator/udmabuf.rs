//! Module containing an implementation of the [`Allocator`]-trait using linux `udmabuf` api.

#![cfg(target_os = "linux")]

use std::{
    io,
    os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd, RawFd},
    sync::LazyLock,
};

use super::{
    Allocator, Fourcc, Modifier,
    dmabuf::{Dmabuf, DmabufFlags},
    format::get_bpp,
};
use crate::utils::Size;

use rustix::{
    fs::{MemfdFlags, Mode, OFlags, SealFlags, fcntl_add_seals, ftruncate, memfd_create},
    ioctl::{Ioctl, Opcode, ioctl, opcode},
    param::page_size,
};

static PAGE_SIZE: LazyLock<usize> = LazyLock::new(page_size);

/// [`Allocator`] creating [`Dmabuf`]s for system memory via the `udmabuf` uapi.
///
/// Note: [`Allocator::create_buffer`] will always return linear buffers for a
/// [`UdmabufAllocator`] and fail if [`Modifier::Linear`] is not in the list
/// of passed modifiers.
#[derive(Debug)]
pub struct UdmabufAllocator {
    dev: OwnedFd,
}

/// Minimum alignment requirement for the stride for `radv` (which seems to be the highest among drivers).
pub const STRIDE_ALIGN: usize = 256;

impl UdmabufAllocator {
    /// Tries to open `/dev/udmabuf` and create a [`UdmabufAllocator`].
    pub fn new() -> io::Result<UdmabufAllocator> {
        let fd = rustix::fs::open("/dev/udmabuf", OFlags::RDONLY | OFlags::CLOEXEC, Mode::empty())?;
        Ok(UdmabufAllocator { dev: fd })
    }

    /// Creates a [`Dmabuf`] from an existing allocation represented by a memfd.
    ///
    /// For importing to succeed a few additional requirements need to be satisfied:
    /// - the width and height cannot exceed 65535.
    /// - the stride needs to be aligned to [`STRIDE_ALIGN`].
    /// - size and offset need to be aligned to the platforms page size.
    ///
    /// Note: dmabufs created through this interface will always be interpreted as linear.
    #[allow(clippy::too_many_arguments)]
    pub fn create_buffer_from_memfd(
        &self,
        mem_fd: impl AsFd,
        offset: usize,
        size: usize,
        format: Fourcc,
        width: u32,
        height: u32,
        stride: u32,
    ) -> Result<Dmabuf, io::Error> {
        if width >= u16::MAX as u32 || height >= u16::MAX as u32 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        if !(stride as usize).is_multiple_of(STRIDE_ALIGN) {
            return Err(io::ErrorKind::InvalidData.into());
        }

        if !size.is_multiple_of(*PAGE_SIZE) || size < (height * stride) as usize {
            return Err(io::ErrorKind::InvalidData.into());
        }

        let dma_fd = udmabuf_from_memfd(self.dev.as_fd(), mem_fd.as_fd(), offset as u64, size as u64)?;
        let mut dmabuf = Dmabuf::builder(
            Size::new(width as i32, height as i32),
            format,
            Modifier::Linear,
            DmabufFlags::empty(),
        );
        dmabuf.add_plane(dma_fd, 0, stride);
        dmabuf.build().ok_or_else(|| io::ErrorKind::Unsupported.into())
    }
}

impl Allocator for UdmabufAllocator {
    type Buffer = Dmabuf;
    type Error = io::Error;

    fn create_buffer(
        &mut self,
        width: u32,
        height: u32,
        fourcc: Fourcc,
        modifiers: &[Modifier],
    ) -> Result<Self::Buffer, Self::Error> {
        if width >= u16::MAX as u32 || height >= u16::MAX as u32 {
            return Err(io::ErrorKind::InvalidData.into());
        }
        if !modifiers.contains(&Modifier::Linear) {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let stride = (width as usize * get_bpp(fourcc).ok_or(io::ErrorKind::InvalidData)?)
            .next_multiple_of(STRIDE_ALIGN);
        let size = (height as usize * stride).next_multiple_of(*PAGE_SIZE);

        let mem_fd = memfd_create("udmabuf", MemfdFlags::ALLOW_SEALING | MemfdFlags::CLOEXEC)?;
        ftruncate(&mem_fd, size as u64)?;
        fcntl_add_seals(&mem_fd, SealFlags::SHRINK)?;
        self.create_buffer_from_memfd(mem_fd, 0, size, fourcc, width, height, stride as u32)
    }
}

fn udmabuf_from_memfd(
    fd: BorrowedFd<'_>,
    mem_fd: BorrowedFd<'_>,
    offset: u64,
    size: u64,
) -> io::Result<OwnedFd> {
    #[repr(C)]
    #[derive(Debug)]
    struct udmabuf_create {
        memfd: u32,
        flags: u32,
        offset: u64,
        size: u64,
    }
    const UDMABUF_FLAGS_CLOEXEC: u32 = 0x01;

    unsafe impl Ioctl for udmabuf_create {
        type Output = RawFd;

        const IS_MUTATING: bool = false;

        fn opcode(&self) -> Opcode {
            opcode::write::<udmabuf_create>(b'u', 0x42)
        }

        fn as_ptr(&mut self) -> *mut rustix::ffi::c_void {
            self as *mut Self as *mut _
        }

        unsafe fn output_from_ptr(
            out: rustix::ioctl::IoctlOutput,
            _extract_output: *mut rustix::ffi::c_void,
        ) -> rustix::io::Result<Self::Output> {
            if out < 0 {
                Err(rustix::io::Errno::from_raw_os_error(!out))
            } else {
                Ok(out)
            }
        }
    }

    let args = udmabuf_create {
        memfd: mem_fd.as_raw_fd() as u32,
        flags: UDMABUF_FLAGS_CLOEXEC,
        offset,
        size,
    };

    unsafe {
        ioctl(fd, args)
            .map(|fd| OwnedFd::from_raw_fd(fd))
            .map_err(|err| io::Error::from_raw_os_error(err.raw_os_error()))
    }
}
