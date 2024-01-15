use std::{ffi::CStr, fmt, fs::File, os::unix::io::OwnedFd, path::PathBuf};

use clap::{Args, Parser, Subcommand, ValueEnum};
use smithay::{
    backend::{
        allocator::{
            dmabuf::{AnyError, Dmabuf, DmabufAllocator},
            dumb::DumbAllocator,
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
            vulkan::{ImageUsageFlags, VulkanAllocator},
            Allocator, Fourcc, Modifier,
        },
        drm::{DrmDeviceFd, DrmNode},
        egl::{EGLContext, EGLDevice, EGLDisplay},
        renderer::{
            gles::{GlesRenderbuffer, GlesRenderer},
            Bind, ExportMem, Frame, ImportDma, Offscreen, Renderer,
        },
        vulkan::{version::Version, Instance, PhysicalDevice},
    },
    utils::{DeviceFd, Rectangle, Transform},
};
use tracing::info;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    subcommand: Subcommands,
}

#[derive(Subcommand, Debug)]
enum Subcommands {
    /// Intersect formats to find usable combinations
    #[command(arg_required_else_help = true)]
    Formats {
        /// Devices that should be able to render a format
        #[arg(short, long)]
        render: Vec<String>,
        /// Devices that should be able to sample from a format
        #[arg(short, long)]
        sample: Vec<String>,
    },
    /// Test exporting and importing buffers with various apis and devices
    Test(TestArgs),
}

#[derive(Args, Debug)]
struct TestArgs {
    /// Allocator used for creating the buffer
    #[arg(short, long)]
    allocator: AllocatorType,
    /// Usage flags for allocating the image, when using the vulkan api
    #[arg(short, long, value_parser = usage_flags_from_string, required_if_eq("allocator", "vulkan"))]
    usage_flags: Option<ImageUsageFlags>,

    /// Exporting device node
    #[arg(short, long, required_if_eq_any([("allocator", "dumb"), ("allocator", "gbm"), ("allocator", "vulkan")]))]
    export: Option<String>,
    /// Importing device node (may be equal to export)
    #[arg(short, long)]
    import: String,
    /// Renderer used to draw into buffer before exporting
    #[arg(short = 'r', long)]
    export_renderer: Option<RendererType>,
    #[arg(short = 't', long)]
    import_renderer: Option<RendererType>,
    #[arg(short, long, requires("export_renderer"), requires("import_renderer"))]
    dump: Option<String>,

    #[arg(long, default_value_t = 256)]
    width: u32,
    #[arg(long, default_value_t = 256)]
    height: u32,
    #[arg(short, long, default_value = "Abgr8888", value_parser = fourcc_from_string)]
    fourcc: Fourcc,
    #[arg(short, long, default_value = "Linear", value_parser = modifier_from_string)]
    modifier: Modifier,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum AllocatorType {
    #[value(name = "dumb")]
    DumbBuffer,
    #[value(name = "gbm")]
    Gbm,
    #[value(name = "vulkan")]
    Vulkan,
}

#[derive(ValueEnum, Debug, Clone, Copy)]
pub enum RendererType {
    #[value(name = "gles")]
    Gles,
}

impl fmt::Display for RendererType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Gles")
    }
}

fn main() {
    let args = Cli::parse();

    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    match args.subcommand {
        Subcommands::Formats { render, sample } => format_test(render, sample),
        Subcommands::Test(args) => buffer_test(args),
    }
}

fn format_test(render: Vec<String>, sample: Vec<String>) {
    for format in render
        .iter()
        .map(|path| {
            let path = PathBuf::from(&path);
            let device = EGLDevice::enumerate()
                .expect("Failed to enumerate EGL devices")
                .find(|device| {
                    device.drm_device_path().ok().as_ref() == Some(&path)
                        || device.render_device_path().ok().as_ref() == Some(&path)
                })
                .expect("Unable to find egl device");
            let display = unsafe { EGLDisplay::new(device).expect("Failed to create EGL display") };
            display.dmabuf_render_formats().clone()
        })
        .chain(sample.iter().map(|path| {
            let path = PathBuf::from(&path);
            let device = EGLDevice::enumerate()
                .expect("Failed to enumerate EGL devices")
                .find(|device| {
                    device.drm_device_path().ok().as_ref() == Some(&path)
                        || device.render_device_path().ok().as_ref() == Some(&path)
                })
                .expect("Unable to find egl device");
            let display = unsafe { EGLDisplay::new(device).expect("Failed to create EGL display") };
            display.dmabuf_texture_formats().clone()
        }))
        .fold(None, |set, formats| match set {
            None => Some(formats),
            Some(set) => Some(set.intersection(&formats).cloned().collect()),
        })
        .unwrap_or_default()
    {
        println!(
            "Format: Fourcc {} / Modifier {}",
            format.code,
            Into::<u64>::into(format.modifier)
        )
    }
}

fn open_device(path: &str) -> DrmDeviceFd {
    let file = File::options()
        .read(true)
        .write(true)
        .open(path)
        .expect("Failed to open device node");
    DrmDeviceFd::new(DeviceFd::from(Into::<OwnedFd>::into(file)))
}

fn buffer_test(args: TestArgs) {
    // 1. create allocator
    let path = args
        .export
        .expect("Dumb buffer allocator requires an export device");
    info!("Export device: {}", path);
    let mut allocator = match args.allocator {
        AllocatorType::DumbBuffer => {
            let fd = open_device(&path);
            let dumb_allocator = DumbAllocator::new(fd);
            Box::new(DmabufAllocator(dumb_allocator)) as Box<dyn Allocator<Buffer = Dmabuf, Error = AnyError>>
        }
        AllocatorType::Gbm => {
            let fd = open_device(&path);
            let gbm = GbmDevice::new(fd).expect("Failed to init gbm device");
            let gbm_allocator = GbmAllocator::new(gbm, GbmBufferFlags::RENDERING);
            Box::new(DmabufAllocator(gbm_allocator)) as Box<_>
        }
        AllocatorType::Vulkan => {
            let node = DrmNode::from_path(&path).expect("Failed to find drm node");
            let instance =
                Instance::new(Version::VERSION_1_2, None).expect("Unable to create vulkan instance");
            let physical_device = PhysicalDevice::enumerate(&instance)
                .expect("Failed to enumerate physical devices")
                .filter(|phd| {
                    phd.has_device_extension(unsafe {
                        CStr::from_bytes_with_nul_unchecked(b"VK_EXT_physical_device_drm\0")
                    })
                })
                .find(|phd| {
                    phd.primary_node().unwrap() == Some(node) || phd.render_node().unwrap() == Some(node)
                })
                .expect("Unable to find physical device");
            Box::new(DmabufAllocator(
                VulkanAllocator::new(&physical_device, args.usage_flags.unwrap())
                    .expect("Failed to create vulkan allocator"),
            )) as Box<_>
        }
    };

    // 2. alloc buffer
    let buffer = allocator
        .create_buffer(args.width, args.height, args.fourcc, &[args.modifier])
        .expect("Failed to allocate buffer");

    // 3. render into buffer on src device
    match args.export_renderer {
        None => {}
        Some(RendererType::Gles) => {
            let path = PathBuf::from(&path);
            let device = EGLDevice::enumerate()
                .expect("Failed to enumerate EGL devices")
                .find(|device| {
                    device.drm_device_path().ok().as_ref() == Some(&path)
                        || device.render_device_path().ok().as_ref() == Some(&path)
                })
                .expect("Unable to find egl device");
            let display = unsafe { EGLDisplay::new(device).expect("Failed to create EGL display") };

            let context = EGLContext::new(&display).expect("Failed to create EGL context");
            let mut renderer = unsafe { GlesRenderer::new(context).expect("Failed to init GL ES renderer") };

            render_into(
                &mut renderer,
                buffer.clone(),
                args.width as i32,
                args.height as i32,
            );
        }
    }

    // 4. import / render buffer on dst device
    let path = PathBuf::from(args.import);
    info!("Import device: {}", path.display());
    match args.import_renderer {
        None => {
            let device = EGLDevice::enumerate()
                .expect("Failed to enumerate EGL devices")
                .find(|device| {
                    device.drm_device_path().ok().as_ref() == Some(&path)
                        || device.render_device_path().ok().as_ref() == Some(&path)
                })
                .expect("Unable to find egl device");
            let display = unsafe { EGLDisplay::new(device).expect("Failed to create EGL display") };

            display
                .create_image_from_dmabuf(&buffer)
                .expect("Failed to import dmabuf");
        }
        Some(RendererType::Gles) => {
            let device = EGLDevice::enumerate()
                .expect("Failed to enumerate EGL devices")
                .find(|device| {
                    device.drm_device_path().ok().as_ref() == Some(&path)
                        || device.render_device_path().ok().as_ref() == Some(&path)
                })
                .expect("Unable to find egl device");
            let display = unsafe { EGLDisplay::new(device).expect("Failed to create EGL display") };

            let context = EGLContext::new(&display).expect("Failed to create EGL context");
            let mut renderer = unsafe { GlesRenderer::new(context).expect("Failed to init GL ES renderer") };

            render_from::<_, GlesRenderbuffer>(
                &mut renderer,
                buffer,
                args.width as i32,
                args.height as i32,
                args.dump,
            );
        }
    }
}

fn render_into<R, T>(renderer: &mut R, buffer: T, w: i32, h: i32)
where
    R: Renderer + Bind<T>,
{
    // Bind it as a framebuffer
    renderer.bind(buffer).expect("Failed to bind dmabuf");

    let mut frame = renderer
        .render((w, h).into(), Transform::Normal)
        .expect("Failed to create render frame");
    frame
        .clear(
            [1.0, 0.0, 0.0, 1.0],
            &[Rectangle::from_loc_and_size((0, 0), (w / 2, h / 2))],
        )
        .expect("Render error");
    frame
        .clear(
            [0.0, 1.0, 0.0, 1.0],
            &[Rectangle::from_loc_and_size((w / 2, 0), (w / 2, h / 2))],
        )
        .expect("Render error");
    frame
        .clear(
            [0.0, 0.0, 1.0, 1.0],
            &[Rectangle::from_loc_and_size((0, h / 2), (w / 2, h / 2))],
        )
        .expect("Render error");
    frame
        .clear(
            [1.0, 1.0, 0.0, 1.0],
            &[Rectangle::from_loc_and_size((w / 2, h / 2), (w / 2, h / 2))],
        )
        .expect("Render error");
    frame.finish().expect("Failed to finish render frame").wait();
}

fn render_from<R, T>(renderer: &mut R, buffer: Dmabuf, w: i32, h: i32, dump: Option<String>)
where
    R: Renderer + ImportDma + ExportMem + Offscreen<T>,
{
    let texture = renderer
        .import_dmabuf(&buffer, None)
        .expect("Failed to import dmabuf");
    let offscreen = Offscreen::<T>::create_buffer(renderer, Fourcc::Abgr8888, (w, h).into())
        .expect("Failed to create offscreen buffer");
    renderer.bind(offscreen).expect("Failed to bind offscreen buffer");
    let mut frame = renderer
        .render((w, h).into(), Transform::Normal)
        .expect("Failed to create render frame");
    frame
        .render_texture_at(
            &texture,
            (0, 0).into(),
            1,
            1.,
            Transform::Normal,
            &[Rectangle::from_loc_and_size((0, 0), (w, h))],
            1.0,
        )
        .expect("Failed to sample dmabuf");
    frame.finish().expect("Failed to finish render frame").wait();

    if let Some(path) = dump {
        let mapping = renderer
            .copy_framebuffer(Rectangle::from_loc_and_size((0, 0), (w, h)), Fourcc::Abgr8888)
            .expect("Failed to map framebuffer");
        let copy = renderer.map_texture(&mapping).expect("Failed to read mapping");
        image::save_buffer(path, copy, w as u32, h as u32, image::ColorType::Rgba8)
            .expect("Failed to save image");
    }
}

fn fourcc_from_string(name: &str) -> Result<Fourcc, &'static str> {
    Ok(match name {
        "Abgr1555" => Fourcc::Abgr1555,
        "Abgr16161616f" => Fourcc::Abgr16161616f,
        "Abgr2101010" => Fourcc::Abgr2101010,
        "Abgr4444" => Fourcc::Abgr4444,
        "Abgr8888" => Fourcc::Abgr8888,
        "Argb1555" => Fourcc::Argb1555,
        "Argb16161616f" => Fourcc::Argb16161616f,
        "Argb2101010" => Fourcc::Argb2101010,
        "Argb4444" => Fourcc::Argb4444,
        "Argb8888" => Fourcc::Argb8888,
        "Axbxgxrx106106106106" => Fourcc::Axbxgxrx106106106106,
        "Ayuv" => Fourcc::Ayuv,
        "Bgr233" => Fourcc::Bgr233,
        "Bgr565" => Fourcc::Bgr565,
        "Bgr565_a8" => Fourcc::Bgr565_a8,
        "Bgr888" => Fourcc::Bgr888,
        "Bgr888_a8" => Fourcc::Bgr888_a8,
        "Bgra1010102" => Fourcc::Bgra1010102,
        "Bgra4444" => Fourcc::Bgra4444,
        "Bgra5551" => Fourcc::Bgra5551,
        "Bgra8888" => Fourcc::Bgra8888,
        "Bgrx1010102" => Fourcc::Bgrx1010102,
        "Bgrx4444" => Fourcc::Bgrx4444,
        "Bgrx5551" => Fourcc::Bgrx5551,
        "Bgrx8888" => Fourcc::Bgrx8888,
        "Bgrx8888_a8" => Fourcc::Bgrx8888_a8,
        "Big_endian" => Fourcc::Big_endian,
        "C8" => Fourcc::C8,
        "Gr1616" => Fourcc::Gr1616,
        "Gr88" => Fourcc::Gr88,
        "Nv12" => Fourcc::Nv12,
        "Nv15" => Fourcc::Nv15,
        "Nv16" => Fourcc::Nv16,
        "Nv21" => Fourcc::Nv21,
        "Nv24" => Fourcc::Nv24,
        "Nv42" => Fourcc::Nv42,
        "Nv61" => Fourcc::Nv61,
        "P010" => Fourcc::P010,
        "P012" => Fourcc::P012,
        "P016" => Fourcc::P016,
        "P210" => Fourcc::P210,
        "Q401" => Fourcc::Q401,
        "Q410" => Fourcc::Q410,
        "R16" => Fourcc::R16,
        "R8" => Fourcc::R8,
        "Rg1616" => Fourcc::Rg1616,
        "Rg88" => Fourcc::Rg88,
        "Rgb332" => Fourcc::Rgb332,
        "Rgb565" => Fourcc::Rgb565,
        "Rgb565_a8" => Fourcc::Rgb565_a8,
        "Rgb888" => Fourcc::Rgb888,
        "Rgb888_a8" => Fourcc::Rgb888_a8,
        "Rgba1010102" => Fourcc::Rgba1010102,
        "Rgba4444" => Fourcc::Rgba4444,
        "Rgba5551" => Fourcc::Rgba5551,
        "Rgba8888" => Fourcc::Rgba8888,
        "Rgbx1010102" => Fourcc::Rgbx1010102,
        "Rgbx4444" => Fourcc::Rgbx4444,
        "Rgbx5551" => Fourcc::Rgbx5551,
        "Rgbx8888" => Fourcc::Rgbx8888,
        "Rgbx8888_a8" => Fourcc::Rgbx8888_a8,
        "Uyvy" => Fourcc::Uyvy,
        "Vuy101010" => Fourcc::Vuy101010,
        "Vuy888" => Fourcc::Vuy888,
        "Vyuy" => Fourcc::Vyuy,
        "X0l0" => Fourcc::X0l0,
        "X0l2" => Fourcc::X0l2,
        "Xbgr1555" => Fourcc::Xbgr1555,
        "Xbgr16161616f" => Fourcc::Xbgr16161616f,
        "Xbgr2101010" => Fourcc::Xbgr2101010,
        "Xbgr4444" => Fourcc::Xbgr4444,
        "Xbgr8888" => Fourcc::Xbgr8888,
        "Xbgr8888_a8" => Fourcc::Xbgr8888_a8,
        "Xrgb1555" => Fourcc::Xrgb1555,
        "Xrgb16161616f" => Fourcc::Xrgb16161616f,
        "Xrgb2101010" => Fourcc::Xrgb2101010,
        "Xrgb4444" => Fourcc::Xrgb4444,
        "Xrgb8888" => Fourcc::Xrgb8888,
        "Xrgb8888_a8" => Fourcc::Xrgb8888_a8,
        "Xvyu12_16161616" => Fourcc::Xvyu12_16161616,
        "Xvyu16161616" => Fourcc::Xvyu16161616,
        "Xvyu2101010" => Fourcc::Xvyu2101010,
        "Xyuv8888" => Fourcc::Xyuv8888,
        "Y0l0" => Fourcc::Y0l0,
        "Y0l2" => Fourcc::Y0l2,
        "Y210" => Fourcc::Y210,
        "Y212" => Fourcc::Y212,
        "Y216" => Fourcc::Y216,
        "Y410" => Fourcc::Y410,
        "Y412" => Fourcc::Y412,
        "Y416" => Fourcc::Y416,
        "Yuv410" => Fourcc::Yuv410,
        "Yuv411" => Fourcc::Yuv411,
        "Yuv420" => Fourcc::Yuv420,
        "Yuv420_10bit" => Fourcc::Yuv420_10bit,
        "Yuv420_8bit" => Fourcc::Yuv420_8bit,
        "Yuv422" => Fourcc::Yuv422,
        "Yuv444" => Fourcc::Yuv444,
        "Yuyv" => Fourcc::Yuyv,
        "Yvu410" => Fourcc::Yvu410,
        "Yvu411" => Fourcc::Yvu411,
        "Yvu420" => Fourcc::Yvu420,
        "Yvu422" => Fourcc::Yvu422,
        "Yvu444" => Fourcc::Yvu444,
        "Yvyu" => Fourcc::Yvyu,
        _ => {
            return Err("Unknown pixel format");
        }
    })
}

fn modifier_from_string(modifier: &str) -> Result<Modifier, std::num::ParseIntError> {
    Ok(match modifier {
        "Invalid" => Modifier::Invalid,
        "Linear" => Modifier::Linear,
        x => Modifier::from(str::parse::<u64>(x)?),
    })
}

fn usage_flags_from_string(flags: &str) -> Result<ImageUsageFlags, String> {
    let flags = flags
        .split('+')
        .map(|f| {
            Ok(match f.to_lowercase().trim() {
                "all" => ImageUsageFlags::all(),
                "empty" | "none" => ImageUsageFlags::empty(),
                "color_attachment" => ImageUsageFlags::COLOR_ATTACHMENT,
                "sampled" => ImageUsageFlags::SAMPLED,
                "transfer_src" => ImageUsageFlags::TRANSFER_SRC,
                "transfer_dst" => ImageUsageFlags::TRANSFER_DST,
                x => {
                    return Err(format!("Unknown Usage Flag: {}", x));
                }
            })
        })
        .collect::<Result<Vec<ImageUsageFlags>, String>>()?;
    Ok(flags
        .into_iter()
        .fold(ImageUsageFlags::empty(), |akk, val| akk | val))
}
