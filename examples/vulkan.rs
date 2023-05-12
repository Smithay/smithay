use drm_fourcc::{DrmFourcc, DrmModifier};
use smithay::backend::{
    allocator::{
        dmabuf::AsDmabuf,
        vulkan::{ImageUsageFlags, VulkanAllocator},
        Allocator, Buffer,
    },
    vulkan::{version::Version, Instance, PhysicalDevice},
};

fn main() {
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

    let physical_device = PhysicalDevice::enumerate(&instance)
        .unwrap()
        .next()
        .expect("No physical devices");

    // The allocator should create buffers that are suitable as render targets.
    let mut allocator = VulkanAllocator::new(&physical_device, ImageUsageFlags::COLOR_ATTACHMENT).unwrap();

    let image = allocator
        .create_buffer(100, 200, DrmFourcc::Argb8888, &[DrmModifier::Linear])
        .expect("create");

    assert_eq!(image.width(), 100);
    assert_eq!(image.height(), 200);

    let image_dmabuf = image.export().expect("Export dmabuf");

    drop(image);

    let _image2 = allocator
        .create_buffer(200, 200, DrmFourcc::Argb8888, &[DrmModifier::Linear])
        .expect("create");

    drop(allocator);
    drop(image_dmabuf);
}
