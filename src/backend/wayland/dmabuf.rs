//! DRM device and dmabuf presentation delegate types.

use std::{
    array::TryFromSliceError,
    convert::{TryFrom, TryInto},
    mem::size_of,
    slice,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use drm_fourcc::{DrmFormat, DrmFourcc, DrmModifier, UnrecognizedFourcc};
use memmap2::MmapOptions;
use nix::sys::stat::dev_t;
use sctk::{
    reexports::{
        client::{
            backend::{self, InvalidId},
            protocol::wl_buffer,
            Connection, Dispatch, Proxy, QueueHandle, WEnum,
        },
        protocols::wp::linux_dmabuf::zv1::client::{
            zwp_linux_buffer_params_v1,
            zwp_linux_dmabuf_feedback_v1::{self, TrancheFlags},
            zwp_linux_dmabuf_v1,
        },
    },
    registry::{GlobalProxy, ProvidesRegistryState, RegistryHandler},
};

use crate::backend::{
    allocator::{dmabuf::Dmabuf, Buffer},
    wayland::{AllocateBuffersError, WaylandError},
};

use super::{data::WaylandBackendData, protocol::wl_drm};

#[derive(Debug, thiserror::Error)]
pub enum CreateDmabufError {
    /// A protocol error.
    #[error(transparent)]
    Protocol(#[from] InvalidId),

    /// The dmabuf global is not available.
    #[error("the dmabuf global is not available")]
    MissingDmabufGlobal,
}

#[derive(Debug)]
pub struct DmabufState {
    zwp_linux_dmabuf_v1: GlobalProxy<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    main_device: Option<MainDevice>,
    formats: Vec<DrmFormat>,
    /// Formats supported by the compositor.
    ///
    /// A value of [`None`] indicates the server supports a format we do not understand.
    tranche_format_table: Vec<Option<DrmFormat>>,
    /// The current tranche with data being populated.
    current_tranche: Option<Tranche>,
    tranches: Vec<Tranche>,
    buffers: Vec<BufferEntry>,
}

impl DmabufState {
    pub fn new() -> DmabufState {
        DmabufState {
            zwp_linux_dmabuf_v1: GlobalProxy::NotPresent,
            main_device: None,
            formats: vec![],
            tranche_format_table: vec![],
            current_tranche: None,
            tranches: vec![],
            buffers: vec![],
        }
    }

    pub fn create_buffer<D>(
        &mut self,
        dmabuf: &Dmabuf,
        conn: &Connection,
        qh: &QueueHandle<D>,
    ) -> Result<wl_buffer::WlBuffer, CreateDmabufError>
    where
        D: Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, ()> + 'static,
    {
        // TODO: Double check that wayland-client does not take ownership of the file descriptor.

        let zwp_linux_dmabuf_v1 = self
            .zwp_linux_dmabuf_v1
            .get()
            .map_err(|_| CreateDmabufError::MissingDmabufGlobal)?;
        let params = zwp_linux_dmabuf_v1.create_params(qh, ());

        let mut handles = dmabuf.handles();
        let mut offsets = dmabuf.offsets();
        let mut strides = dmabuf.strides();
        let format = dmabuf.format();

        let modifier: u64 = format.modifier.into();
        // Modifier is in the platform's endianness.
        let modifier_hi = (modifier >> 32) as u32;
        let modifier_lo = modifier as u32;

        for plane_index in 0..dmabuf.num_planes() as u32 {
            params.add(
                handles.next().unwrap(),
                plane_index,
                offsets.next().unwrap(),
                strides.next().unwrap(),
                modifier_hi,
                modifier_lo,
            );
        }

        let size = dmabuf.size();

        let flags = {
            let mut flags = zwp_linux_buffer_params_v1::Flags::empty();

            if dmabuf.y_inverted() {
                flags.insert(zwp_linux_buffer_params_v1::Flags::YInvert);
            }

            // TODO: Interlaced
            // TODO: Bottom first
            flags
        };

        // Create the wl_buffer using the lower level wayland-backend.
        let free = Arc::new(AtomicBool::new(true));
        let buffer = conn.send_request(
            &params,
            zwp_linux_buffer_params_v1::Request::CreateImmed {
                width: size.w,
                height: size.h,
                format: format.code as u32,
                flags: WEnum::Value(flags),
            },
            Some(Arc::new(DmabufBufferObjectData { free: free.clone() })),
        )?;
        let buffer = wl_buffer::WlBuffer::from_id(conn, buffer)?;

        self.buffers.push(BufferEntry {
            free,
            buffer: buffer.clone(),
        });

        Ok(buffer)
    }

    pub fn formats(&self) -> impl Iterator<Item = DrmFormat> {
        // TODO: Do we need to consider other tranches in the format list?
        self.formats.clone().into_iter()
    }

    pub fn get_entry(&self, buffer: &wl_buffer::WlBuffer) -> Option<&BufferEntry> {
        self.buffers.iter().find(|entry| &entry.buffer == buffer)
    }

    pub(crate) fn main_device(&self) -> Option<&MainDevice> {
        self.main_device.as_ref()
    }
}

#[derive(Debug)]
pub struct BufferEntry {
    pub free: Arc<AtomicBool>,
    pub buffer: wl_buffer::WlBuffer,
}

impl From<CreateDmabufError> for WaylandError {
    fn from(err: CreateDmabufError) -> Self {
        match err {
            CreateDmabufError::Protocol(err) => WaylandError::InvalidId(err),
            CreateDmabufError::MissingDmabufGlobal => {
                WaylandError::AllocateBuffers(AllocateBuffersError::Unsupported)
            }
        }
    }
}

impl RegistryHandler<Self> for WaylandBackendData {
    fn ready(state: &mut Self, _conn: &Connection, qh: &QueueHandle<Self>) {
        // Attempt to bind a zwp_linux_dmabuf_v1 at version 4
        match state
            .registry()
            .bind_one::<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, Self, _>(qh, 4..=4, ())
        {
            Ok(dmabuf_global) => {
                state.protocols.dmabuf_state.zwp_linux_dmabuf_v1 = GlobalProxy::Bound(dmabuf_global)
            }

            // version 4 is not available, check if wl_drm is available
            Err(_) => {
                // TODO: https://github.com/Smithay/client-toolkit/issues/283
                if let Ok(_) = state.registry().bind_one::<wl_drm::WlDrm, Self, _>(qh, 1..=1, ()) {
                    // If wl_drm is available, then zwp_linux_dmabuf_v1 is required.
                    if let Ok(zwp_linux_dmabuf_v1) = state
                        .registry()
                        .bind_one::<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, Self, _>(qh, 3..=3, ())
                    {
                        state.protocols.dmabuf_state.zwp_linux_dmabuf_v1 =
                            GlobalProxy::Bound(zwp_linux_dmabuf_v1)
                    }
                }
            }
        }

        if let Ok(dmabuf) = state.protocols.dmabuf_state.zwp_linux_dmabuf_v1.get() {
            if dmabuf.version() >= 4 {
                // Get the default feedback to determine what the main device is.
                let _ = dmabuf.get_default_feedback(qh, ());
            }
        }
    }
}

impl Dispatch<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1, ()> for WaylandBackendData {
    fn event(
        state: &mut Self,
        proxy: &zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
        event: zwp_linux_dmabuf_v1::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // The events on zwp_linux_dmabuf_v1 are not longer emitted in version 4
        if proxy.version() < 4 {
            match event {
                // Ignored
                zwp_linux_dmabuf_v1::Event::Format { .. } => {}

                zwp_linux_dmabuf_v1::Event::Modifier {
                    format,
                    modifier_hi,
                    modifier_lo,
                } => {
                    if let Ok(code) = DrmFourcc::try_from(format) {
                        let modifier = (modifier_hi as u64) << 32 | modifier_lo as u64;

                        let format = DrmFormat {
                            code,
                            modifier: DrmModifier::from(modifier),
                        };

                        state
                            .protocols
                            .dmabuf_state
                            .tranche_format_table
                            .push(Some(format));
                    }
                }

                _ => unreachable!(),
            }
        }
    }
}

impl Dispatch<zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1, ()> for WaylandBackendData {
    fn event(
        _: &mut Self,
        params: &zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1,
        event: zwp_linux_buffer_params_v1::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            zwp_linux_buffer_params_v1::Event::Created { buffer: _ } => {
                // TODO: This will require event_created_child
                todo!("more resilient buffer creation")
            }

            zwp_linux_buffer_params_v1::Event::Failed => {
                // TODO
            }

            _ => unreachable!(),
        }

        // Created and failed both say that the client should destroy the params.
        params.destroy();
    }
}

impl Dispatch<zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1, ()> for WaylandBackendData {
    fn event(
        state: &mut Self,
        feedback: &zwp_linux_dmabuf_feedback_v1::ZwpLinuxDmabufFeedbackV1,
        event: zwp_linux_dmabuf_feedback_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let state = &mut state.protocols.dmabuf_state;

        match event {
            zwp_linux_dmabuf_feedback_v1::Event::FormatTable { fd, size } => {
                let mmap = unsafe {
                    MmapOptions::new()
                        // The protocol mandates we map the fd as copy on write
                        .map_copy_read_only(fd)
                }
                .expect("Cannot mmap format table");

                // SAFETY: the protocol states the table is tightly packed. This ensures the data is aligned.
                let formats = unsafe {
                    slice::from_raw_parts::<dmabuf_format_modifier>(
                        mmap[..].as_ptr() as *const _,
                        // the protocol provides us the size of the table in bytes, so we need to divide by the
                        // size of the struct to get the number of entries since from_raw_parts takes the len.
                        size as usize / size_of::<dmabuf_format_modifier>(),
                    )
                }
                .iter()
                .copied()
                .map(DrmFormat::try_from)
                // tranche_formats assumes no gaps in the format table when it sends indices, so any unknown
                // formats should be represented as an Option
                .map(Result::ok)
                .collect::<Vec<_>>();

                state.tranche_format_table.extend(formats);
            }

            zwp_linux_dmabuf_feedback_v1::Event::MainDevice { device } => match dev_from_array(device) {
                Ok(dev) => {
                    state.main_device = Some(MainDevice::LinuxDmabuf(dev));
                }

                Err(err) => {
                    todo!("invalid dev_t array {}", err)
                }
            },

            zwp_linux_dmabuf_feedback_v1::Event::TrancheTargetDevice { device } => {
                if let Ok(dev) = dev_from_array(device) {
                    // FIXME: dev_t != dev_t is more than definitely wrong, but for now this is fine for
                    // prototyping.
                    if state.main_device == Some(MainDevice::LinuxDmabuf(dev)) {
                        // Set the current_tranche to collect data.
                        state.current_tranche = Some(Tranche {
                            formats: vec![],
                            flags: TrancheFlags::empty(),
                        });
                    }
                }
            }
            zwp_linux_dmabuf_feedback_v1::Event::TrancheFormats { indices } => {
                if state.current_tranche.is_some() {
                    let tranche_formats = indices
                        // Each index is 16-bits in native endianness.
                        .chunks_exact(2)
                        .flat_map(TryInto::<[u8; 2]>::try_into)
                        .map(u16::from_ne_bytes)
                        .filter_map(|index| state.tranche_format_table.get(index as usize))
                        // Filter out indices we couldn't resolve
                        .flatten()
                        .cloned()
                        .collect::<Vec<_>>();

                    let tranche = state.current_tranche.as_mut().unwrap();
                    tranche.formats.extend(tranche_formats);
                }
            }
            zwp_linux_dmabuf_feedback_v1::Event::TrancheFlags { flags } => {
                // Ignore unknown flags.
                let flags = TrancheFlags::from_bits_truncate(flags.into());

                if let Some(tranche) = state.current_tranche.as_mut() {
                    tranche.flags.insert(flags);
                }
            }
            zwp_linux_dmabuf_feedback_v1::Event::TrancheDone => {
                if let Some(tranche) = state.current_tranche.take() {
                    state.tranches.push(tranche);
                }
            }

            zwp_linux_dmabuf_feedback_v1::Event::Done => {
                // Copy the formats in the first tranche
                // TODO: In the future this needs to consider tranches.
                if let Some(tranche) = state.tranches.first() {
                    state.formats.extend(tranche.formats.iter());
                }

                // When the done request is sent, the feedback may not change.
                feedback.destroy();
            }

            _ => unreachable!(),
        }
    }
}

struct DmabufBufferObjectData {
    free: Arc<AtomicBool>,
}

impl backend::ObjectData for DmabufBufferObjectData {
    fn event(
        self: Arc<Self>,
        _: &backend::Backend,
        msg: backend::protocol::Message<backend::ObjectId>,
    ) -> Option<Arc<dyn backend::ObjectData>> {
        debug_assert!(backend::protocol::same_interface(
            msg.sender_id.interface(),
            wl_buffer::WlBuffer::interface()
        ));
        // wl_buffer only has a single event: wl_buffer::release
        debug_assert!(msg.opcode == 0);

        self.free.store(true, Ordering::Relaxed);
        None
    }

    fn destroyed(&self, _: backend::ObjectId) {}
}

#[derive(Debug)]
struct Tranche {
    formats: Vec<DrmFormat>,
    flags: TrancheFlags,
}

#[derive(Debug, PartialEq)]
pub(crate) enum MainDevice {
    LinuxDmabuf(dev_t),

    LegacyWlDrm(String),
}

#[repr(C)]
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy)]
struct dmabuf_format_modifier {
    format: u32,
    _pad: u32,
    modifier: u64,
}

impl TryFrom<dmabuf_format_modifier> for DrmFormat {
    type Error = UnrecognizedFourcc;

    fn try_from(v: dmabuf_format_modifier) -> Result<Self, Self::Error> {
        Ok(DrmFormat {
            code: DrmFourcc::try_from(v.format)?,
            modifier: DrmModifier::from(v.modifier),
        })
    }
}

fn dev_from_array(array: Vec<u8>) -> Result<dev_t, TryFromSliceError> {
    Ok(dev_t::from_ne_bytes(array[..].try_into()?))
}

// Mesa Wayland-DRM handling

impl Dispatch<wl_drm::WlDrm, ()> for WaylandBackendData {
    fn event(
        state: &mut Self,
        _: &wl_drm::WlDrm,
        event: wl_drm::Event,
        _: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_drm::Event::Device { name } => {
                let state = &mut state.protocols.dmabuf_state;

                if state.main_device.is_none() {
                    state.main_device = Some(MainDevice::LegacyWlDrm(name));
                }
            }

            // linux-dmabuf is used for these other fields.
            wl_drm::Event::Format { .. }
            | wl_drm::Event::Authenticated
            | wl_drm::Event::Capabilities { .. } => (),
        }
    }
}
