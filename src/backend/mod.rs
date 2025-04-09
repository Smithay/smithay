//! Backend (rendering/input) helpers
//!
//! This module provides helpers for interaction with the operating system.
//!
//! ## Module structure
//!
//! The module is largely structured around three main aspects of interaction with the OS:
//! session management, input handling, and graphics.
//!
//! ### Session management
//!
//! Session management relates to mechanisms allowing the compositor to access the resources
//! it needs to function. It contains interaction with the login manager if any ((e)logind or
//! seatd), as well as releasing those resources when TTY-switching. It is handled by the
//! [`session`] module, gated by the `backend_session` cargo feature. You will generally need
//! it to run your compositor directly on a TTY.
//!
//! This module is tightly coupled with the [`udev`] module (gated by the `backend_udev` cargo
//! feature), which allows the discovery of usable graphics and input devices on the system, using
//! the udev system daemon.
//!
//! ### Input handling
//!
//! Input handling consists in discovering the various available input devices, and receiving
//! all inputs events from it. Smithay is build to support different possible sources for
//! that input data, with a generic API provided by the traits and types defined in the
//! [`input`] module. An input provider following this API based on `libinput` is given in the
//! [`libinput`] module, gated by the `backend_libinput` cargo feature. The winit backend
//! (see below) also provides an input provider.
//!
//! ### Graphics
//!
//! Combining content from the clients and displaying it on the screen is the central role of
//! a wayland compositor, and also one of its most complex tasks; several backend modules are
//! dedicated to this task.
//!
//! Smithay provides a rendering infrastructure built around graphics buffers: you retrieve buffers
//! for your client, you composite them into a new buffer holding the contents of your desktop,
//! that you will then submit to the hardware for display. The backbone of this infrastructure is
//! structured around two modules:
//!
//! - [`allocator`] contains generic traits representing the capability to
//!   allocate and convert graphical buffers, as well as an implementation of this
//!   capability using GBM (see its module-level docs for details).
//! - [`renderer`] provides traits representing the capability of graphics
//!   rendering using those buffers, as well as an implementation of this
//!   capability using GLes2 (see its module-level docs for details).
//!
//! Alongside this backbone capability, Smithay also provides the [`drm`] module, which handles
//! direct interaction with the graphical physical devices to setup the display pipeline and
//! submit rendered buffers to the monitors for display. This module is gated by the
//! `backend_drm` cargo feature.
//!
//! The [`egl`] module provides the logic to setup an OpenGL context. It is used by the Gles2
//! renderer (which is based on OpenGL), and also provides the capability for clients to use
//! the `wl_drm`-based hardware-acceleration provided by Mesa, a precursor to the
//! [`linux_dmabuf`](crate::wayland::dmabuf) Wayland protocol extension. Note that, at the
//! moment, even clients using dma-buf still require that the `wl_drm` infrastructure is
//! initialized to have hardware-acceleration.
//!
//! ## X11 backend
//!
//! Alongside this infrastructure, Smithay also provides an alternative backend based on
//! [x11rb](https://crates.io/crates/x11rb), which makes it possible to run your compositor as
//! an X11 client. This is generally quite helpful for development and debugging.
//!
//! The X11 backend does not concern itself with what renderer is in use, allowing presentation to
//! the window assuming you can provide it with a [`Dmabuf`](crate::backend::allocator::dmabuf::Dmabuf).
//! The X11 backend is also an input provider, and is accessible in the [`x11`] module, gated by
//! the `backend_x11` cargo feature.
//!
//! ## Winit backend
//!
//! Alongside this infrastructure, Smithay also provides an alternative backend based on
//! [winit](https://crates.io/crates/winit), which makes it possible to run your compositor as
//! a Wayland or X11 client. You are encouraged to use the X11 backend where possible since winit
//! does not integrate into calloop too well. This backend is generally quite helpful for
//! development and debugging. That backend is both a renderer and an input provider, and is
//! accessible in the [`winit`] module, gated by the `backend_winit` cargo feature.
//!

pub mod allocator;
pub mod input;
pub mod renderer;

#[cfg(feature = "backend_drm")]
pub mod drm;
#[cfg(feature = "backend_egl")]
pub mod egl;
#[cfg(feature = "backend_libinput")]
pub mod libinput;
#[cfg(feature = "backend_session")]
pub mod session;
#[cfg(feature = "backend_udev")]
pub mod udev;

#[cfg(feature = "backend_vulkan")]
pub mod vulkan;

#[cfg(feature = "backend_winit")]
pub mod winit;

#[cfg(feature = "backend_x11")]
pub mod x11;

/// Error that can happen when swapping buffers.
#[derive(Debug, thiserror::Error)]
pub enum SwapBuffersError {
    /// The buffers have already been swapped.
    ///
    /// This error can be returned when `swap_buffers` has been called multiple times
    /// without any modification in between.
    #[error("Buffers are already swapped, swap_buffers was called too many times")]
    AlreadySwapped,
    /// The corresponding context has been lost and needs to be recreated.
    ///
    /// All the objects associated to it (textures, buffers, programs, etc.)
    /// need to be recreated from scratch. Underlying resources like native surfaces
    /// might also need to be recreated.
    ///
    /// Operations will have no effect. Functions that read textures, buffers, etc.
    /// will return uninitialized data instead.
    #[error("The context has been lost, it needs to be recreated: {0}")]
    ContextLost(Box<dyn std::error::Error + Send + Sync>),
    /// A temporary condition caused to rendering to fail.
    ///
    /// Depending on the underlying error this *might* require fixing internal state of the rendering backend,
    /// but failures mapped to `TemporaryFailure` are always recoverable without re-creating the entire stack,
    /// as is represented by `ContextLost`.
    ///
    /// Proceed after investigating the source to reschedule another full rendering step or just this page_flip at a later time.
    /// If the root cause cannot be discovered and subsequent renderings also fail, it is advised to fallback to
    /// recreation.
    #[error("A temporary condition caused the page flip to fail: {0}")]
    TemporaryFailure(Box<dyn std::error::Error + Send + Sync>),
}
