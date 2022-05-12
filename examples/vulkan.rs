use std::sync::Mutex;

use slog::{o, Drain};
use smithay::backend::vulkan::{Instance, PhysicalDevice};

fn main() {
    let logger = slog::Logger::root(Mutex::new(slog_term::term_full().fuse()).fuse(), o!());

    println!("Version: {}", Instance::max_instance_version().unwrap());
    println!(
        "Available instance extensions: {:?}",
        Instance::enumerate_extensions().unwrap().collect::<Vec<_>>()
    );
    println!();

    let instance = Instance::new(Version::VERSION_1_3, None, logger).unwrap();

    for (idx, phy) in PhysicalDevice::enumerate(&instance).unwrap().enumerate() {
        println!(
            "Device #{}: {} v{}, {:?}",
            idx,
            phy.name(),
            phy.api_version(),
            phy.driver()
        );
    }
}
