use ash::vk::{
    self, AccessFlags, BufferImageCopy2, BufferUsageFlags, CopyBufferToImageInfo2, DependencyFlags, Extent3D,
    ExternalMemoryHandleTypeFlags, Fence, ImageAspectFlags, ImageLayout, ImageMemoryBarrier,
    ImageSubresourceLayers, ImageSubresourceRange, ImageUsageFlags, Offset3D, PipelineStageFlags, SubmitInfo,
    TimelineSemaphoreSubmitInfo, QUEUE_FAMILY_IGNORED,
};
use smallvec::SmallVec;
use tracing::debug;

use crate::{
    backend::{
        allocator::{format::get_bpp, Buffer as _},
        renderer::vulkan::{buffer::Buffer, Error, VulkanRenderer},
        vulkan::{
            format::get_drm_format,
            image::{Error as ImageError, VulkanImage},
        },
    },
    utils::{Buffer as BufferCoords, Point, Rectangle, Size},
};

impl VulkanRenderer {
    fn update_from_buffer(
        &mut self,
        img: &VulkanImage,
        buf: &Buffer,
        stride: usize,
        bpp: usize,
        regions: &[Rectangle<i32, BufferCoords>],
    ) -> Result<u64, Error> {
        let size = img.size();
        let cmd = self.cmd_pool.create_and_begin_buffer()?;
        unsafe {
            self.device.vk().cmd_pipeline_barrier(
                cmd,
                PipelineStageFlags::TRANSFER,
                PipelineStageFlags::TRANSFER,
                DependencyFlags::empty(),
                &[],
                &[],
                &[ImageMemoryBarrier::default()
                    .image(*img.vk())
                    .old_layout(ImageLayout::UNDEFINED)
                    .new_layout(ImageLayout::GENERAL)
                    .src_queue_family_index(QUEUE_FAMILY_IGNORED)
                    .dst_queue_family_index(QUEUE_FAMILY_IGNORED)
                    .src_access_mask(AccessFlags::empty())
                    .dst_access_mask(AccessFlags::empty())
                    .subresource_range(
                        ImageSubresourceRange::default()
                            .aspect_mask(ImageAspectFlags::COLOR)
                            .layer_count(1)
                            .level_count(1),
                    )],
            );
            let regions = regions
                .iter()
                .map(|rect| {
                    BufferImageCopy2::default()
                        .buffer_offset(0)
                        .buffer_row_length((stride / bpp) as u32)
                        .buffer_image_height(size.h as u32)
                        .image_subresource(
                            ImageSubresourceLayers::default()
                                .aspect_mask(ImageAspectFlags::COLOR)
                                .base_array_layer(0)
                                .layer_count(1)
                                .mip_level(0),
                        )
                        .image_offset(Offset3D::default().x(rect.loc.x).y(rect.loc.y))
                        .image_extent(
                            Extent3D::default()
                                .width(rect.size.w as u32)
                                .height(rect.size.h as u32)
                                .depth(1),
                        )
                })
                .collect::<SmallVec<[BufferImageCopy2<'static>; 4]>>();
            let info = CopyBufferToImageInfo2::default()
                .src_buffer(*buf.vk())
                .dst_image(*img.vk())
                .dst_image_layout(ImageLayout::GENERAL)
                .regions(&regions);
            self.device.vk().cmd_copy_buffer_to_image2(cmd, &info);
            self.device
                .vk()
                .end_command_buffer(cmd)
                .map_err(Error::CommandBufferError)?;
        }

        self.seq_no += 1;
        let next_seq_no = [self.seq_no];
        let mut timeline_info = TimelineSemaphoreSubmitInfo::default().signal_semaphore_values(&next_seq_no);

        unsafe {
            self.device
                .vk()
                .queue_submit(
                    *self.device.queue(),
                    &[SubmitInfo::default()
                        .command_buffers(&[cmd])
                        .signal_semaphores(&[self.timeline.vk])
                        .push_next(&mut timeline_info)],
                    Fence::null(),
                )
                .map_err(Error::SubmitError)?;
        }

        self.cmd_pool.store_pending_buffer(cmd, next_seq_no[0], None);
        unsafe { self.device.vk().device_wait_idle() }; // BIG TODO

        Ok(next_seq_no[0])
    }

    pub(super) fn upload_from_slice(
        &mut self,
        data: &[u8],
        size: Size<i32, BufferCoords>,
        stride: usize,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
    ) -> Result<VulkanImage, Error> {
        let bpp = get_drm_format(format)
            .and_then(get_bpp)
            .ok_or(ImageError::UnsupportedFormat)?
            / 8;
        let buffer = match Buffer::from_ptr(
            &self.device,
            unsafe { std::mem::transmute(data.as_ptr()) },
            data.len(),
            ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT,
            BufferUsageFlags::TRANSFER_SRC,
        ) {
            Ok(buf) => buf,
            Err(err) => {
                debug!("EXT_External_memory_host error: {:?}", err);
                let buffer = Buffer::new(
                    &self.device,
                    data.len(),
                    None,
                    BufferUsageFlags::TRANSFER_SRC,
                    true,
                )?;
                let dst = buffer.mapped_ptr().unwrap();
                unsafe {
                    std::ptr::copy_nonoverlapping(data.as_ptr(), dst.as_ptr() as *mut _, data.len());
                }
                buffer
            }
        };

        let image = VulkanImage::new(
            &self.device,
            size.w as u32,
            size.h as u32,
            format,
            usage | ImageUsageFlags::TRANSFER_DST,
            false,
        )?;

        self.update_from_buffer(
            &image,
            &buffer,
            stride,
            bpp,
            &[Rectangle::new(Point::new(0, 0), size)],
        )?;

        Ok(image)
    }

    pub(super) fn update_from_slice(
        &mut self,
        data: &[u8],
        stride: usize,
        image: &VulkanImage,
        regions: &[Rectangle<i32, BufferCoords>],
    ) -> Result<(), Error> {
        let format = image.format();
        let bpp = get_drm_format(format)
            .and_then(get_bpp)
            .ok_or(ImageError::UnsupportedFormat)?
            / 8;

        let buffer = match Buffer::from_ptr(
            &self.device,
            unsafe { std::mem::transmute(data.as_ptr()) },
            data.len(),
            ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT,
            BufferUsageFlags::TRANSFER_SRC,
        ) {
            Ok(buf) => buf,
            Err(err) => {
                debug!("EXT_External_memory_host error: {:?}", err);
                let buffer = Buffer::new(
                    &self.device,
                    data.len(),
                    None,
                    BufferUsageFlags::TRANSFER_SRC,
                    true,
                )?;
                let dst = buffer.mapped_ptr().unwrap();
                for region in regions {
                    let offset = (stride * region.loc.y as usize + region.loc.x as usize * bpp) as isize;
                    let len = stride * region.size.h as usize + region.size.w as usize * bpp;
                    unsafe {
                        std::ptr::copy_nonoverlapping(
                            data.as_ptr().offset(offset),
                            dst.as_ptr() as *mut _,
                            len,
                        );
                    }
                }
                buffer
            }
        };

        self.update_from_buffer(image, &buffer, stride, bpp, regions)?;

        Ok(())
    }

    // TODO: Safety, ptr must live until the sequence is done
    pub(super) fn upload_from_shm(
        &mut self,
        data: *const u8,
        len: usize,
        size: Size<i32, BufferCoords>,
        stride: usize,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
    ) -> Result<VulkanImage, Error> {
        let bpp = get_drm_format(format)
            .and_then(get_bpp)
            .ok_or(ImageError::UnsupportedFormat)?
            / 8;
        let buffer = match Buffer::from_ptr(
            &self.device,
            unsafe { std::mem::transmute(data) },
            len,
            ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT,
            BufferUsageFlags::TRANSFER_SRC,
        ) {
            Ok(buf) => buf,
            Err(err) => {
                debug!("EXT_External_memory_host error: {:?}", err);
                let buffer = Buffer::new(&self.device, len, None, BufferUsageFlags::TRANSFER_SRC, true)?;
                let dst = buffer.mapped_ptr().unwrap();
                unsafe {
                    std::ptr::copy_nonoverlapping(data, dst.as_ptr() as *mut _, len);
                }
                buffer
            }
        };

        let image = VulkanImage::new(
            &self.device,
            size.w as u32,
            size.h as u32,
            format,
            usage | ImageUsageFlags::TRANSFER_DST,
            false,
        )?;

        self.update_from_buffer(
            &image,
            &buffer,
            stride,
            bpp,
            &[Rectangle::new(Point::new(0, 0), size)],
        )?;

        Ok(image)
    }

    pub(super) fn update_from_shm(
        &mut self,
        data: *const u8,
        len: usize,
        stride: usize,
        image: &VulkanImage,
        regions: &[Rectangle<i32, BufferCoords>],
    ) -> Result<(), Error> {
        let format = image.format();
        let bpp = get_drm_format(format)
            .and_then(get_bpp)
            .ok_or(ImageError::UnsupportedFormat)?
            / 8;

        let buffer = match Buffer::from_ptr(
            &self.device,
            unsafe { std::mem::transmute(data) },
            len,
            ExternalMemoryHandleTypeFlags::HOST_ALLOCATION_EXT,
            BufferUsageFlags::TRANSFER_SRC,
        ) {
            Ok(buf) => buf,
            Err(err) => {
                debug!("EXT_External_memory_host error: {:?}", err);
                let buffer = Buffer::new(&self.device, len, None, BufferUsageFlags::TRANSFER_SRC, true)?;
                let dst = buffer.mapped_ptr().unwrap();
                for region in regions {
                    let offset = (stride * region.loc.y as usize + region.loc.x as usize * bpp) as isize;
                    let len = stride * region.size.h as usize - region.loc.x as usize * bpp;
                    unsafe {
                        std::ptr::copy_nonoverlapping(data.offset(offset), dst.as_ptr() as *mut _, len);
                    }
                }
                buffer
            }
        };

        self.update_from_buffer(image, &buffer, stride, bpp, regions)?;

        Ok(())
    }
}
