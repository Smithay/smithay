use std::{fs, time::Duration};

use ash::vk::FormatFeatureFlags;
use drm_fourcc::{DrmFourcc, DrmModifier};
use rustix::fs::{Mode, OFlags};
use smithay::{
    backend::{
        allocator::{dmabuf::AsDmabuf, format::FormatSet, vulkan::VulkanAllocator, Allocator, Buffer},
        drm::DrmDeviceFd,
        egl::{EGLContext, EGLDevice, EGLDisplay},
        renderer::{
            gles::{GlesError, GlesRenderer},
            sync::Interrupted,
            vulkan::VulkanRenderer,
            Bind, Color32F, ExportMem, Frame, ImportMem, Renderer,
        },
        vulkan::{
            format::{get_vk_format, known_formats},
            image::{ImageUsageFlags, VulkanImage},
            version::Version,
            Instance, PhysicalDevice,
        },
    },
    utils::{DeviceFd, Point, Rectangle, Size, Transform},
};

fn main() {
    let path = "./test.png";
    if let Ok(env_filter) = tracing_subscriber::EnvFilter::try_from_default_env() {
        tracing_subscriber::fmt().with_env_filter(env_filter).init();
    } else {
        tracing_subscriber::fmt().init();
    }

    println!(
        "Available instance extensions: {:?}",
        Instance::enumerate_extensions().unwrap().collect::<Vec<_>>()
    );
    println!();

    let instance = Instance::new(Version::VERSION_1_3, None).unwrap();

    for (idx, phy) in PhysicalDevice::enumerate(&instance).unwrap().enumerate() {
        println!(
            "Device #{}: {} v{}, {:?}",
            idx,
            phy.name(),
            phy.api_version(),
            phy.driver()
        );
    }

    for phys in PhysicalDevice::enumerate(&instance).unwrap() {
        println!("{}", phys.name());
        for fourcc in known_formats() {
            let format = get_vk_format(*fourcc).unwrap();
            let mut props = Default::default();
            unsafe { phys.get_format_properties(format, &mut props) };
            if props
                .format_properties
                .optimal_tiling_features
                .contains(FormatFeatureFlags::STORAGE_IMAGE)
            {
                println!("{}", fourcc,);
            }
        }
        println!("");
    }

    let devices = PhysicalDevice::enumerate(&instance).unwrap();
    let physical_device = devices.skip(0).next().expect("No physical devices");

    let render_node = physical_device
        .render_node()
        .expect("render node")
        .expect("render node");
    let drm = DrmDeviceFd::new(DeviceFd::from(
        rustix::fs::open(
            render_node.dev_path().expect("render node path"),
            OFlags::RDWR | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .expect("open render node"),
    ));
    let mut renderer = VulkanRenderer::new(&physical_device, Some(drm.clone())).expect("renderer");

    // The allocator should create buffers that are suitable as render targets.
    let mut allocator = VulkanAllocator::from_renderer(
        &renderer,
        ImageUsageFlags::STORAGE | ImageUsageFlags::HOST_TRANSFER_EXT,
    );
    let formats = Bind::<VulkanImage>::supported_formats(&renderer)
        .unwrap()
        .into_iter()
        .filter(|format| {
            format.code == DrmFourcc::Abgr8888
                || format.code == DrmFourcc::Argb8888
                || format.code == DrmFourcc::Rgba8888
                || format.code == DrmFourcc::Bgra8888
        })
        .collect::<FormatSet>();
    let selected_format = dbg!(formats.iter().next().expect("No format found").code);
    let modifiers = formats
        .into_iter()
        .filter(|fmt| fmt.code == selected_format)
        .map(|fmt| fmt.modifier)
        .collect::<Vec<_>>();

    let size = Size::new(512_i32, 512_i32);
    let mut image = allocator
        .create_buffer(
            size.w as u32,
            size.h as u32,
            selected_format,
            &[DrmModifier::Linear],
        ) //&modifiers)
        .expect("create");

    let png = image::open("./examples/resources/cursor.png").unwrap();
    let png_buf = png.as_rgba8().unwrap();
    let tex = renderer
        .import_memory(&png_buf, DrmFourcc::Abgr8888, Size::new(256, 256), false)
        .expect("import tex");
    let mut fb = renderer.bind(&mut image).expect("Bind failed");

    let mut frame = renderer.render(&mut fb, size, Transform::Normal).expect("frame");
    frame
        .clear(Color32F::new(1., 0., 0., 1.), &[Rectangle::from_size(size)])
        .expect("clear red");
    frame
        .clear(
            Color32F::new(1., 1., 0., 1.),
            &[Rectangle::new(Point::new(50, 50), Size::new(100, 100))],
        )
        .expect("clear second");
    frame
        .clear(
            Color32F::new(0.5, 0.5, 0.0, 0.2),
            &[Rectangle::new(Point::new(120, 120), Size::new(280, 280))],
        )
        .expect("draw third");
    frame
        .render_texture_from_to(
            &tex,
            Rectangle::new(Point::new(0., 0.), Size::new(256., 256.)),
            Rectangle::new(Point::new(128, 128), Size::new(256, 256)),
            &[Rectangle::new(Point::new(0, 0), Size::new(256, 256))],
            &[],
            Transform::Normal,
            1.0,
        )
        .expect("Failed to render cursor tex");
    frame
        .draw_solid(
            Rectangle::new(Point::new(100, 300), Size::new(200, 100)),
            &[Rectangle::new(Point::new(0, 0), Size::new(200, 100))],
            Color32F::new(0.5, 0., 0.5, 0.2),
        )
        .expect("draw third");

    let sync = frame.finish().unwrap();

    loop {
        match sync.wait() {
            Err(Interrupted) => {}
            x => break x,
        }
    }
    .expect("failed to wait");
    let mapping = renderer
        .copy_framebuffer(
            &fb,
            Rectangle::from_size(size.to_logical(1).to_buffer(1, Transform::Normal)),
            selected_format,
        )
        .expect("map");
    let copy = renderer.map_texture(&mapping).expect("read");
    let _ = fs::remove_file(path);
    image::save_buffer(path, copy, size.w as u32, size.h as u32, image::ColorType::Rgba8).expect("save");

    renderer.cleanup().expect("cleanup");
    drop(mapping);
    drop((fb, tex));
    drop(image);
    drop((allocator, renderer));
}
