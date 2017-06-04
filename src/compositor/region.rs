use super::{Rectangle, RectangleKind, RegionAttributes};

use std::sync::Mutex;
use wayland_server::Resource;

use wayland_server::protocol::wl_region;

#[derive(Default)]
pub struct RegionData {
    attributes: RegionAttributes,
}

impl RegionData {
    /// Initialize the user_data of a region, must be called right when the surface is created
    pub unsafe fn init(region: &wl_region::WlRegion) {
        region.set_user_data(Box::into_raw(Box::new(Mutex::new(RegionData::default()))) as *mut _)
    }

    /// Cleans the user_data of that surface, must be called when it is destroyed
    pub unsafe fn cleanup(region: &wl_region::WlRegion) {
        let ptr = region.get_user_data();
        region.set_user_data(::std::ptr::null_mut());
        let _my_data_mutex: Box<Mutex<RegionData>> = Box::from_raw(ptr as *mut _);
    }

    unsafe fn get_data(region: &wl_region::WlRegion) -> &Mutex<RegionData> {
        let ptr = region.get_user_data();
        &*(ptr as *mut _)
    }

    pub unsafe fn get_attributes(region: &wl_region::WlRegion) -> RegionAttributes {
        let data_mutex = Self::get_data(region);
        let data_guard = data_mutex.lock().unwrap();
        data_guard.attributes.clone()
    }

    pub unsafe fn add_rectangle(region: &wl_region::WlRegion, kind: RectangleKind, rect: Rectangle) {
        let data_mutex = Self::get_data(region);
        let mut data_guard = data_mutex.lock().unwrap();
        data_guard.attributes.rects.push((kind, rect));
    }
}
