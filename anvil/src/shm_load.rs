use shell::Buffer;
use smithay::wayland::shm::BufferData;
use smithay::wayland_server::protocol::wl_shm::Format;

pub fn load_shm_buffer(data: BufferData, pool: &[u8], log: &::slog::Logger) -> Buffer {
    // ensure consistency, the SHM handler of smithay should ensure this
    debug_assert!(((data.offset + data.stride * data.height) as usize) <= pool.len());

    let mut out = Vec::with_capacity((data.width * data.height * 4) as usize);

    let offset = data.offset as usize;
    let width = data.width as usize;
    let height = data.height as usize;
    let stride = data.stride as usize;

    match data.format {
        Format::Argb8888 => {
            // TODO: this is so slooooow
            for j in 0..height {
                for i in 0..width {
                    // value must be read as native endianness
                    let val: u32 =
                        unsafe { *(&pool[offset + j * stride + i * 4] as *const u8 as *const u32) };
                    out.push(((val & 0x00FF0000) >> 16) as u8); //r
                    out.push(((val & 0x0000FF00) >> 8) as u8); //g
                    out.push(((val & 0x000000FF) >> 0) as u8); //b
                    out.push(((val & 0xFF000000) >> 24) as u8); //a
                }
            }
        }
        Format::Xrgb8888 => {
            // TODO: this is so slooooow
            for j in 0..height {
                for i in 0..width {
                    // value must be read as native endianness
                    let val: u32 =
                        unsafe { *(&pool[offset + j * stride + i * 4] as *const u8 as *const u32) };
                    out.push(((val & 0x00FF0000) >> 16) as u8); //r
                    out.push(((val & 0x0000FF00) >> 8) as u8); //g
                    out.push(((val & 0x000000FF) >> 0) as u8); //b
                    out.push(255); // a
                }
            }
        }
        _ => {
            error!(log, "Unsupported buffer format"; "format" => format!("{:?}", data.format));
            // fill in with black
            for _ in 0..(data.height * data.width) {
                out.extend(&[0, 0, 0, 255])
            }
        }
    }

    Buffer::Shm {
        data: out,
        size: (data.width as u32, data.height as u32),
    }
}
