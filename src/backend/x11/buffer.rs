//! Utilities for importing buffers into X11.
//!
//! Buffers imported into X11 are represented as X pixmaps which are then presented to the window.
//!
//! At the moment only [`Dmabuf`] backed pixmaps are supported.
//!
//! ## Dmabuf pixmaps
//!
//! A [`Dmabuf`] backed pixmap is created using the [`DRI3`](x11rb::protocol::dri3) extension of
//! the X server. One of two code paths is used here. For more modern DRI3 (>= 1.2) implementations
//! multi-plane Dmabufs, may be used to create a pixmap. Otherwise the fallback code path
//! (available in >= 1.0) is used to create the pixmap. Although the Dmabuf may only have one plane.
//!
//! If you do need to modify any of the logic pertaining to the Dmabuf presentation, do ensure you
//! read the `dri3proto.txt` file (link in the non-public comments of the x11 mod.rs).
//!
//! ## Presentation to the window
//!
//! Presentation to the window is handled through the [`Present`](x11rb::protocol::present)
//! extension of the X server. Because we use direct rendering to present to the window, using
//! V-Sync from OpenGL or the equivalents in other rendering APIs will not work. This is where
//! the utility of the present extension is useful. When using the `present_pixmap` function,
//! the X server will notify when the frame has been presented to the window. The notification
//! of presentation usually occurs on a V-blank.
//!
//! If you do need to modify any of the logic pertaining to the using the present extension, do
//! ensure you read the `presentproto.txt` file (link in the non-public comments of the
//! x11 mod.rs).

use std::sync::atomic::Ordering;

use super::{PresentError, Window, X11Error};
use drm_fourcc::DrmFourcc;
use x11rb::{
    connection::Connection,
    protocol::{
        dri3::ConnectionExt as _,
        present::{self, ConnectionExt},
        xproto::PixmapWrapper,
    },
};

use crate::backend::allocator::{dmabuf::Dmabuf, Buffer};

// Shm can be easily supported in the future using, xcb_shm_create_pixmap.

pub trait PixmapWrapperExt<C>
where
    C: Connection,
{
    /// Creates a new Pixmap using the supplied Dmabuf.
    ///
    /// The returned Pixmap is freed when dropped.
    fn with_dmabuf(connection: C, window: &Window, dmabuf: &Dmabuf) -> Result<PixmapWrapper<C>, X11Error>;

    /// Presents the pixmap to the window.
    ///
    /// The wrapper is consumed when this function is called. The return value will contain the
    /// id of the pixmap.
    ///
    /// The pixmap will be automatically dropped when it bubbles up in the X11 event loop after the
    /// X server has finished presentation with the buffer behind the pixmap.
    fn present(self, connection: C, window: &Window) -> Result<u32, X11Error>;
}

impl<C> PixmapWrapperExt<C> for PixmapWrapper<C>
where
    C: Connection,
{
    #[profiling::function]
    fn with_dmabuf(connection: C, window: &Window, dmabuf: &Dmabuf) -> Result<PixmapWrapper<C>, X11Error> {
        if dmabuf.format().code != window.format() {
            return Err(PresentError::IncorrectFormat(window.format()).into());
        }

        let mut fds = Vec::new();

        // XCB closes the file descriptor after sending, so duplicate the file descriptors.
        for handle in dmabuf.handles() {
            let fd = rustix::io::fcntl_dupfd_cloexec(
                handle, 3, // Set to 3 so the fd cannot become stdin, stdout or stderr
            )
            .map_err(|e| PresentError::DupFailed(e.to_string()))?;

            fds.push(fd);
        }

        // We need dri3 >= 1.2 in order to use the enhanced dri3_pixmap_from_buffers function.
        let xid = if window.0.extensions.dri3 >= Some((1, 2)) {
            if dmabuf.num_planes() > 4 {
                return Err(PresentError::TooManyPlanes.into());
            }

            let xid = connection.generate_id()?;
            let mut strides = dmabuf.strides();
            let mut offsets = dmabuf.offsets();

            connection.dri3_pixmap_from_buffers(
                xid,
                window.id(),
                dmabuf.width() as u16,
                dmabuf.height() as u16,
                strides.next().unwrap(), // there must be at least one plane and stride.
                offsets.next().unwrap(),
                // The other planes are optional, so unwrap_or to `NONE` if those planes are not available.
                strides.next().unwrap_or(x11rb::NONE),
                offsets.next().unwrap_or(x11rb::NONE),
                strides.next().unwrap_or(x11rb::NONE),
                offsets.next().unwrap_or(x11rb::NONE),
                strides.next().unwrap_or(x11rb::NONE),
                offsets.next().unwrap_or(x11rb::NONE),
                window.depth(),
                // In the future this could be made nicer.
                match window.format() {
                    DrmFourcc::Argb8888 => 32,
                    DrmFourcc::Xrgb8888 => 24,
                    _ => unreachable!(),
                },
                dmabuf.format().modifier.into(),
                fds,
            )?;

            xid
        } else {
            // Old codepath can only create a pixmap using one plane from a dmabuf.
            if dmabuf.num_planes() != 1 {
                return Err(PresentError::TooManyPlanes.into());
            }

            let xid = connection.generate_id()?;
            let mut strides = dmabuf.strides();
            let stride = strides.next().unwrap();

            connection.dri3_pixmap_from_buffer(
                xid,
                window.id(),
                dmabuf.height() * stride,
                dmabuf.width() as u16,
                dmabuf.height() as u16,
                stride as u16,
                window.depth(),
                // In the future this could be made nicer.
                match window.format() {
                    DrmFourcc::Argb8888 => 32,
                    DrmFourcc::Xrgb8888 => 24,
                    _ => unreachable!(),
                },
                fds.remove(0),
            )?;

            xid
        };

        Ok(PixmapWrapper::for_pixmap(connection, xid))
    }

    #[profiling::function]
    fn present(self, connection: C, window: &Window) -> Result<u32, X11Error> {
        let next_serial = window.0.next_serial.fetch_add(1, Ordering::SeqCst);
        // We want to present as soon as possible, so wait 1ms so the X server will present when next convenient.
        let msc = window.0.last_msc.load(Ordering::SeqCst) + 1;

        // options parameter does not take the enum but a u32.
        const OPTIONS: present::Option = present::Option::NONE;

        connection.present_pixmap(
            window.id(),
            self.pixmap(),
            next_serial,
            x11rb::NONE, // Update the entire window
            x11rb::NONE, // Update the entire window
            0,           // No offsets
            0,
            x11rb::NONE,    // Let the X server pick the most suitable crtc
            x11rb::NONE,    // Do not wait to present
            x11rb::NONE,    // We will wait for the X server to tell us when it is done with the pixmap.
            OPTIONS.into(), // No special presentation options.
            msc,
            0,
            0,
            &[], // We don't need to notify any other windows.
        )?;

        // Pixmaps are reference counted on the X server. Because of reference counting we may
        // drop the wrapper and the X server will free the pixmap when presentation has completed.
        Ok(self.pixmap())
    }
}
