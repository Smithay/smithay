use std::borrow::Cow;

use smithay::{reexports::wayland_server::protocol::wl_shm::Format, wayland::shm::BufferData};

use glium::texture::{ClientFormat, RawImage2d};

pub fn load_shm_buffer(data: BufferData, pool: &[u8]) -> Result<(RawImage2d<'_, u8>, usize), Format> {
    let offset = data.offset as usize;
    let width = data.width as usize;
    let height = data.height as usize;
    let stride = data.stride as usize;

    // number of bytes per pixel
    // TODO: compute from data.format
    let pixelsize = 4;

    // ensure consistency, the SHM handler of smithay should ensure this
    assert!(offset + (height - 1) * stride + width * pixelsize <= pool.len());

    let slice: Cow<'_, [u8]> = if stride == width * pixelsize {
        // the buffer is cleanly continuous, use as-is
        Cow::Borrowed(&pool[offset..(offset + height * width * pixelsize)])
    } else {
        // the buffer is discontinuous or lines overlap
        // we need to make a copy as unfortunately Glium does not
        // expose the OpenGL APIs we would need to load this buffer :/
        let mut data = Vec::with_capacity(height * width * pixelsize);
        for i in 0..height {
            data.extend(&pool[(offset + i * stride)..(offset + i * stride + width * pixelsize)]);
        }
        Cow::Owned(data)
    };

    // sharders format need to be reversed to account for endianness
    let (client_format, fragment) = load_format(data.format)?;
    Ok((
        RawImage2d {
            data: slice,
            width: width as u32,
            height: height as u32,
            format: client_format,
        },
        fragment,
    ))
}

pub fn load_format(format: Format) -> Result<(ClientFormat, usize), Format> {
    Ok(match format {
        Format::Argb8888 => (ClientFormat::U8U8U8U8, crate::shaders::BUFFER_BGRA),
        Format::Xrgb8888 => (ClientFormat::U8U8U8U8, crate::shaders::BUFFER_BGRX),
        Format::Rgba8888 => (ClientFormat::U8U8U8U8, crate::shaders::BUFFER_ABGR),
        Format::Rgbx8888 => (ClientFormat::U8U8U8U8, crate::shaders::BUFFER_XBGR),
        _ => return Err(format),
    })
}
