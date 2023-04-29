pub struct FakeDevice;
impl std::os::unix::prelude::AsFd for FakeDevice {
    fn as_fd(&self) -> std::os::unix::prelude::BorrowedFd<'_> {
        unimplemented!()
    }
}
impl drm::Device for FakeDevice {}
impl drm::control::Device for FakeDevice {}
