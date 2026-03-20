use std::os::unix::io::OwnedFd;

use super::{Fence, Interrupted};
use crate::backend::drm::sync::DrmSyncPoint;

impl Fence for DrmSyncPoint {
    fn is_signaled(&self) -> bool {
        self.timeline
            .query_signalled_point()
            .ok()
            .is_some_and(|point| point >= self.point)
    }

    fn wait(&self) -> Result<(), Interrupted> {
        self.wait(i64::MAX).map_err(|_| Interrupted)
    }

    fn is_exportable(&self) -> bool {
        true
    }

    fn export(&self) -> Option<OwnedFd> {
        self.export_sync_file().ok()
    }
}
