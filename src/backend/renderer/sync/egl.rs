use std::{os::unix::io::OwnedFd, time::Duration};

use crate::backend::{egl::fence::EGLFence, renderer::sync::Fence};

impl Fence for EGLFence {
    fn wait(&self) {
        self.client_wait(None, false);
    }

    fn is_exportable(&self) -> bool {
        self.is_native()
    }

    fn export(&self) -> Option<OwnedFd> {
        self.export().ok()
    }

    fn is_signaled(&self) -> bool {
        self.client_wait(Some(Duration::ZERO), false)
    }
}
