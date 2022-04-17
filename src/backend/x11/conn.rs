use slog::{info, o, Logger};
use std::{
    io::IoSlice,
    sync::{Arc, Weak},
};
use x11rb::{
    connection::{
        BufWithFds, Connection, DiscardMode, EventAndSeqNumber, RawEventAndSeqNumber, ReplyOrError,
        RequestConnection, RequestKind, SequenceNumber,
    },
    cookie::{Cookie, CookieWithFds, VoidCookie},
    protocol::{xproto::Setup, Event},
    utils::RawFdContainer,
    x11_utils::{ExtensionInformation, TryParse, TryParseFd, X11Error as X11rbError},
    xcb_ffi::{ConnectionError, ParseError, ReplyError, ReplyOrIdError, XCBConnection},
};

use super::{extension::Extensions, X11Error};

/// Represents an active connection to the X.
///
/// Can be used to create a [`X11Backend`] or an [`smithay::backend::egl::EGLDisplay`].
#[derive(Debug, Clone)]
pub struct X11Connection {
    pub(crate) display: Arc<*mut xlib_sys::xlib::Display>,
    pub(crate) xcb: Arc<XCBConnection>,
    pub(super) screen: usize,
    pub(super) logger: Logger,
}

unsafe impl Send for X11Connection {}
unsafe impl Sync for X11Connection {}

impl X11Connection {
    /// Initializes the X11 connection to the X server.
    pub fn new<L>(logger: L) -> Result<X11Connection, X11Error>
    where
        L: Into<Option<slog::Logger>>,
    {
        let logger = crate::slog_or_fallback(logger).new(o!("smithay_module" => "backend_x11"));

        info!(logger, "Connecting to the X server");

        let (connection, display, screen) = unsafe {
            xlib_sys::xlib::XInitThreads();
            let display = xlib_sys::xlib::XOpenDisplay(std::ptr::null());
            if display.is_null() {
                return Err(X11Error::ConnectionFailed(
                    x11rb::errors::ConnectError::UnknownError,
                ));
            }
            xlib_sys::xlib_xcb::XSetEventQueueOwner(
                display,
                xlib_sys::xlib_xcb::XEventQueueOwner::XCBOwnsEventQueue,
            );
            let screen = xlib_sys::xlib::XDefaultScreen(display);
            let xcb_raw = xlib_sys::xlib_xcb::XGetXCBConnection(display);
            (
                XCBConnection::from_raw_xcb_connection(xcb_raw, false)?,
                display,
                screen as usize,
            )
        };
        let display = Arc::new(display);
        let connection = Arc::new(connection);
        info!(logger, "Connected to screen {}", screen);

        let _ = Extensions::check_extensions(&*connection, &logger)?;

        Ok(X11Connection {
            display,
            xcb: connection,
            screen,
            logger,
        })
    }

    /// Creates a weak reference for this X11Connection.
    pub fn weak(&self) -> WeakX11Connection {
        WeakX11Connection {
            display: Arc::downgrade(&self.display),
            xcb: Arc::downgrade(&self.xcb),
            screen: self.screen,
            logger: self.logger.clone(),
        }
    }
}

impl RequestConnection for X11Connection {
    type Buf = <XCBConnection as RequestConnection>::Buf;

    fn send_request_with_reply<R>(
        &self,
        bufs: &[IoSlice<'_>],
        fds: Vec<RawFdContainer>,
    ) -> Result<Cookie<'_, Self, R>, ConnectionError>
    where
        R: TryParse,
    {
        let sequence = self
            .xcb
            .send_request_with_reply::<R>(bufs, fds)?
            .sequence_number();
        Ok(Cookie::new(self, sequence))
    }
    fn send_request_with_reply_with_fds<R>(
        &self,
        bufs: &[IoSlice<'_>],
        fds: Vec<RawFdContainer>,
    ) -> Result<CookieWithFds<'_, Self, R>, ConnectionError>
    where
        R: TryParseFd,
    {
        let sequence = self
            .xcb
            .send_request_with_reply_with_fds::<R>(bufs, fds)?
            .sequence_number();
        Ok(CookieWithFds::new(self, sequence))
    }
    fn send_request_without_reply(
        &self,
        bufs: &[IoSlice<'_>],
        fds: Vec<RawFdContainer>,
    ) -> Result<VoidCookie<'_, Self>, ConnectionError> {
        let sequence = self.xcb.send_request_without_reply(bufs, fds)?.sequence_number();
        Ok(VoidCookie::new(self, sequence))
    }
    fn discard_reply(&self, sequence: SequenceNumber, kind: RequestKind, mode: DiscardMode) {
        self.xcb.discard_reply(sequence, kind, mode)
    }
    fn prefetch_extension_information(&self, extension_name: &'static str) -> Result<(), ConnectionError> {
        self.xcb.prefetch_extension_information(extension_name)
    }
    fn extension_information(
        &self,
        extension_name: &'static str,
    ) -> Result<Option<ExtensionInformation>, ConnectionError> {
        self.xcb.extension_information(extension_name)
    }
    fn wait_for_reply_or_raw_error(
        &self,
        sequence: SequenceNumber,
    ) -> Result<ReplyOrError<Self::Buf>, ConnectionError> {
        self.xcb.wait_for_reply_or_raw_error(sequence)
    }
    fn wait_for_reply(&self, sequence: SequenceNumber) -> Result<Option<Self::Buf>, ConnectionError> {
        self.xcb.wait_for_reply(sequence)
    }
    fn wait_for_reply_with_fds_raw(
        &self,
        sequence: SequenceNumber,
    ) -> Result<ReplyOrError<BufWithFds<Self::Buf>, Self::Buf>, ConnectionError> {
        self.xcb.wait_for_reply_with_fds_raw(sequence)
    }
    fn check_for_raw_error(&self, sequence: SequenceNumber) -> Result<Option<Self::Buf>, ConnectionError> {
        self.xcb.check_for_raw_error(sequence)
    }
    fn prefetch_maximum_request_bytes(&self) {
        self.xcb.prefetch_maximum_request_bytes()
    }
    fn maximum_request_bytes(&self) -> usize {
        self.xcb.maximum_request_bytes()
    }
    fn parse_error(&self, error: &[u8]) -> Result<X11rbError, ParseError> {
        self.xcb.parse_error(error)
    }
    fn parse_event(&self, event: &[u8]) -> Result<Event, ParseError> {
        self.xcb.parse_event(event)
    }

    fn wait_for_reply_or_error(&self, sequence: SequenceNumber) -> Result<Self::Buf, ReplyError> {
        self.xcb.wait_for_reply_or_error(sequence)
    }
    fn wait_for_reply_with_fds(&self, sequence: SequenceNumber) -> Result<BufWithFds<Self::Buf>, ReplyError> {
        self.xcb.wait_for_reply_with_fds(sequence)
    }
    fn check_for_error(&self, sequence: SequenceNumber) -> Result<(), ReplyError> {
        self.xcb.check_for_error(sequence)
    }
}

impl Connection for X11Connection {
    fn wait_for_raw_event_with_sequence(&self) -> Result<RawEventAndSeqNumber<Self::Buf>, ConnectionError> {
        self.xcb.wait_for_raw_event_with_sequence()
    }
    fn poll_for_raw_event_with_sequence(
        &self,
    ) -> Result<Option<RawEventAndSeqNumber<Self::Buf>>, ConnectionError> {
        self.xcb.poll_for_raw_event_with_sequence()
    }
    fn flush(&self) -> Result<(), ConnectionError> {
        self.xcb.flush()
    }
    fn setup(&self) -> &Setup {
        self.xcb.setup()
    }
    fn generate_id(&self) -> Result<u32, ReplyOrIdError> {
        self.xcb.generate_id()
    }

    fn wait_for_event(&self) -> Result<Event, ConnectionError> {
        self.xcb.wait_for_event()
    }
    fn wait_for_raw_event(&self) -> Result<Self::Buf, ConnectionError> {
        self.xcb.wait_for_raw_event()
    }
    fn wait_for_event_with_sequence(&self) -> Result<EventAndSeqNumber, ConnectionError> {
        self.xcb.wait_for_event_with_sequence()
    }
    fn poll_for_event(&self) -> Result<Option<Event>, ConnectionError> {
        self.xcb.poll_for_event()
    }
    fn poll_for_raw_event(&self) -> Result<Option<Self::Buf>, ConnectionError> {
        self.xcb.poll_for_raw_event()
    }
    fn poll_for_event_with_sequence(&self) -> Result<Option<EventAndSeqNumber>, ConnectionError> {
        self.xcb.poll_for_event_with_sequence()
    }
}

impl Drop for X11Connection {
    fn drop(&mut self) {
        unsafe {
            xlib_sys::xlib::XCloseDisplay(*self.display);
        }
    }
}

/// Weak reference to a X11Connection
#[derive(Debug)]
pub struct WeakX11Connection {
    pub(crate) display: Weak<*mut xlib_sys::xlib::Display>,
    pub(crate) xcb: Weak<XCBConnection>,
    screen: usize,
    logger: Logger,
}

unsafe impl Send for WeakX11Connection {}
unsafe impl Sync for WeakX11Connection {}

impl WeakX11Connection {
    /// Try to upgrade this weak reference to the underlying [`X11Connection`], if still connected.
    pub fn upgrade(&self) -> Option<X11Connection> {
        if let (Some(display), Some(xcb)) = (self.display.upgrade(), self.xcb.upgrade()) {
            Some(X11Connection {
                display,
                xcb,
                screen: self.screen,
                logger: self.logger.clone(),
            })
        } else {
            None
        }
    }
}
