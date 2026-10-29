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
    ioctl::{Ioctl, Opcode, Updater, ioctl, opcode},
    param::page_size,
};
use tracing::{debug, info, warn};

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

    /// Whether the gpu of the given drm device can read udmabufs reliably.
    ///
    /// `SMITHAY_UDMABUF_IMPORT` overrides the result.
    pub fn is_supported_by(device: impl AsFd) -> bool {
        static OVERRIDE: LazyLock<Option<bool>> = LazyLock::new(|| {
            let value = std::env::var("SMITHAY_UDMABUF_IMPORT").ok()?;
            let enabled = match value.to_lowercase().as_str() {
                "1" | "true" | "yes" | "y" => true,
                "0" | "false" | "no" | "n" => false,
                _ => {
                    warn!(value, "Ignoring invalid SMITHAY_UDMABUF_IMPORT");
                    return None;
                }
            };
            info!(enabled, "SMITHAY_UDMABUF_IMPORT is set");
            Some(enabled)
        });
        if let Some(enabled) = *OVERRIDE {
            return enabled;
        }

        let device = device.as_fd();
        let driver = match drm_driver_name(device) {
            Ok(driver) => driver,
            Err(err) => {
                debug!(?err, "Failed to query drm driver, not using udmabuf");
                return false;
            }
        };
        let supported = match driver.as_str() {
            // Sampling sometimes returns stale data
            // https://forums.developer.nvidia.com/t/imported-cpu-cached-dma-buf-mapped-non-coherent-gpu-reads-stale-data-egl-dma-buf-import-615-71-09/384944
            "nvidia-drm" => false,
            // Without llc, GPU reads bypass CPU caches and i915 flushes them only on the first import
            "i915" => i915_has_llc(device).unwrap_or(false),
            // Importing breaks the virgl context on the host
            "virtio_gpu" => false,
            // kernel driver leaks imported dma-bufs
            "vmwgfx" => false,
            _ => true,
        };
        debug!(driver, supported, "Checked udmabuf support");
        supported
    }

    /// Creates a [`Dmabuf`] from an existing allocation represented by a memfd.
    ///
    /// For importing to succeed a few additional requirements need to be satisfied:
    /// - the width and height cannot exceed 65535.
    /// - the stride and the offset within its page need to be supported by the importing driver,
    ///   multiples of [`STRIDE_ALIGN`] are supported by all.
    /// - size needs to be at least `height * stride`.
    /// - the memfd needs to contain all pages touched by `offset..offset + size`.
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
        if size < (height * stride) as usize {
            return Err(io::ErrorKind::InvalidData.into());
        }

        let plane_offset = offset % *PAGE_SIZE;
        let start = offset - plane_offset;
        let udmabuf_size = (plane_offset + size).next_multiple_of(*PAGE_SIZE);

        let dma_fd = udmabuf_from_memfd(
            self.dev.as_fd(),
            mem_fd.as_fd(),
            start as u64,
            udmabuf_size as u64,
        )?;
        let mut dmabuf = Dmabuf::builder(
            Size::new(width as i32, height as i32),
            format,
            Modifier::Linear,
            DmabufFlags::empty(),
        );
        dmabuf.add_plane(dma_fd, plane_offset as u32, stride);
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

// drm-ffi is only available with `backend_drm`, udmabuf works without it.
fn drm_driver_name(device: BorrowedFd<'_>) -> io::Result<String> {
    #[repr(C)]
    struct drm_version {
        version_major: i32,
        version_minor: i32,
        version_patchlevel: i32,
        name_len: usize,
        name: *mut u8,
        date_len: usize,
        date: *mut u8,
        desc_len: usize,
        desc: *mut u8,
    }
    const DRM_IOCTL_VERSION: Opcode = opcode::read_write::<drm_version>(b'd', 0x00);

    let mut name = [0u8; 64];
    let mut version = drm_version {
        version_major: 0,
        version_minor: 0,
        version_patchlevel: 0,
        name_len: name.len(),
        name: name.as_mut_ptr(),
        date_len: 0,
        date: std::ptr::null_mut(),
        desc_len: 0,
        desc: std::ptr::null_mut(),
    };
    // SAFETY: `drm_version` matches the kernel's struct, and `name` has room for `name_len` bytes.
    unsafe { ioctl(device, Updater::<DRM_IOCTL_VERSION, _>::new(&mut version))? };
    let len = version.name_len.min(name.len());
    Ok(String::from_utf8_lossy(&name[..len]).into_owned())
}

fn i915_has_llc(device: BorrowedFd<'_>) -> io::Result<bool> {
    #[repr(C)]
    struct drm_i915_getparam {
        param: i32,
        value: *mut i32,
    }
    // DRM_COMMAND_BASE + DRM_I915_GETPARAM
    const DRM_IOCTL_I915_GETPARAM: Opcode = opcode::read_write::<drm_i915_getparam>(b'd', 0x40 + 0x06);
    const I915_PARAM_HAS_LLC: i32 = 17;

    let mut value = 0;
    let mut param = drm_i915_getparam {
        param: I915_PARAM_HAS_LLC,
        value: &mut value,
    };
    // SAFETY: `drm_i915_getparam` matches the kernel's struct, and `value` points to a writable i32.
    unsafe { ioctl(device, Updater::<DRM_IOCTL_I915_GETPARAM, _>::new(&mut param))? };
    Ok(value != 0)
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
