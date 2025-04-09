# Smithay Changelog

## Unreleased

## 0.6.0

### Breaking Changes

`RenderContext::draw` callback now accepts a mutable reference
```diff
-fn smithay::backend::renderer::element::texture::RenderContext::draw(&mut self, f: impl FnOnce(&T))
+fn smithay::backend::renderer::element::texture::RenderContext::draw(&mut self, f: impl FnOnce(&mut T))
```

Framebuffer now requires `Texture` implementation
```rs
type smithay::backend::renderer::RendererSuper::Framebuffer: smithay::backend::renderer::Texture
```

`Output::client_outputs` no longer returns a Vec
```diff
-fn smithay::output::Output::client_outputs(&self, client: &Client) -> Vec<WlOutput>;
+fn smithay::output::Output::client_outputs(&self, client: &Client) -> impl Iterator<Item = WlOutput>;
```
DamageBag/DamageSnapshot damage getters got renamed
```diff
-fn smithay::backend::renderer::utils::DamageBag::damage(&self) -> impl Iterator<Item = impl Iterator<Item = &Rectangle>>
+fn smithay::backend::renderer::utils::DamageBag::raw(&self) -> impl Iterator<Item = impl Iterator<Item = &Rectangle>>
-fn smithay::backend::renderer::utils::DamageSnapshot::damage(&self) -> impl Iterator<Item = impl Iterator<Item = &Rectangle>>
+fn smithay::backend::renderer::utils::DamageSnapshot::raw(&self) -> impl Iterator<Item = impl Iterator<Item = &Rectangle>>
```
RendererSurfaceState::damage now returns a DamageSnapshot
```diff
-fn smithay::backend::renderer::utils::RendererSurfaceState::damage(&self) -> impl core::iter::traits::iterator::Iterator<Item = impl core::iter::traits::iterator::Iterator<Item = &smithay::utils::Rectangle<i32, smithay::utils::Buffer>>>
+fn smithay::backend::renderer::utils::RendererSurfaceState::damage(&self) -> smithay::backend::renderer::utils::DamageSnapshot<i32, smithay::utils::Buffer>
```
Client scale can now be fractional
```diff
-fn smithay::wayland::compositor::CompositorClientState::client_scale(&self) -> u32
+fn smithay::wayland::compositor::CompositorClientState::client_scale(&self) -> f64
-fn smithay::wayland::compositor::CompositorClientState::set_client_scale(&self, new_scale: u32)
+fn smithay::wayland::compositor::CompositorClientState::set_client_scale(&self, new_scale: f64)
```

Raw `renderer_id` got replaced with new `smithay::backend::renderer::ContextId` newtype 
```diff
-fn smithay::backend::renderer::gles::GlesFrame::id(&self) -> usize;
+fn smithay::backend::renderer::gles::GlesFrame::context_id(&self) -> ContextId;
-fn smithay::backend::renderer::gles::GlesRenderer::id(&self) -> usize;
+fn smithay::backend::renderer::gles::GlesRenderer::context_id(&self) -> ContextId;
-fn smithay::backend::renderer::glow::GlowFrame::id(&self) -> usize;
+fn smithay::backend::renderer::glow::GlowFrame::context_id(&self) -> ContextId;
-fn smithay::backend::renderer::glow::GlowRenderer::id(&self) -> usize;
+fn smithay::backend::renderer::glow::GlowRenderer::context_id(&self) -> ContextId;
-fn smithay::backend::renderer::multigpu::MultiFrame::id(&self) -> usize;
+fn smithay::backend::renderer::multigpu::MultiFrame::context_id(&self) -> ContextId;
-fn smithay::backend::renderer::multigpu::MultiRenderer::id(&self) -> usize;
+fn smithay::backend::renderer::multigpu::MultiRenderer::context_id(&self) -> ContextId;
-fn smithay::backend::renderer::pixman::PixmanFrame::id(&self) -> usize;
+fn smithay::backend::renderer::pixman::PixmanFrame::context_id(&self) -> ContextId;
-fn smithay::backend::renderer::pixman::PixmanRenderer::id(&self) -> usize;
+fn smithay::backend::renderer::pixman::PixmanRenderer::context_id(&self) -> ContextId;
-fn smithay::backend::renderer::test::DummyFrame::id(&self) -> usize;
+fn smithay::backend::renderer::test::DummyFrame::context_id(&self) -> ContextId;
-fn smithay::backend::renderer::test::DummyRenderer::id(&self) -> usize;
+fn smithay::backend::renderer::test::DummyRenderer::context_id(&self) -> ContextId;
-fn smithay::backend::renderer::Frame::id(&self) -> usize;
+fn smithay::backend::renderer::Frame::context_id(&self) -> ContextId;
-fn smithay::backend::renderer::Renderer::id(&self) -> usize;
+fn smithay::backend::renderer::Renderer::context_id(&self) -> ContextId;
-fn smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(id: Id, renderer_id: usize, ...) -> Self;
+fn smithay::backend::renderer::element::texture::TextureRenderElement::from_static_texture(id: Id, context_id: ContextId, ...) -> Self;
-fn smithay::backend::renderer::element::texture::TextureRenderElement::from_texture_with_damage(id: Id, renderer_id: usize, ...) -> Self;
+fn smithay::backend::renderer::element::texture::TextureRenderElement::from_texture_with_damage(id: Id, context_id: ContextId, ...) -> Self;
-fn smithay::backend::renderer::utils::RendererSurfaceState::texture<R>(&self, id: usize) -> Option<&TextureId>;
+fn smithay::backend::renderer::utils::RendererSurfaceState::texture<R>(&self, id: &ContextId) -> Option<&TextureId>;
```

`CursorShapeDeviceUserData` now has an additional generic argument
```diff
-struct smithay::wayland::cursor_shape::CursorShapeDeviceUserData;
+struct smithay::wayland::cursor_shape::CursorShapeDeviceUserData<D: SeatHandler>;
```

The explicit frame buffers got introduced, but for the sake of my sanity those changes are not described here, you can look at: https://github.com/Smithay/smithay/commit/df08c6f29eb6ebfa2fce6fc374590483bcbaf21a

### API Additions

It is now possible to check if the OpenGL context is shared with another.
```rs
fn smithay::backend::egl::context::EGLContext::is_shared();
```
It is now possible to check that there are no other references to the underlying GL texture.
```rs
fn smithay::backend::renderer::gles::GlesTexture::is_unique_reference();
```

There is a new gles capability for support of fencing and exporting to EGL
```rs
smithay::backend::renderer::gles::Capability::ExportFence;
```

There is a new BlitFrame trait for frames that support blitting contents from/to the current framebuffer to/from another.
```rs
trait smithay::backend::renderer::BlitFrame;
impl BlitFrame for smithay::backend::renderer::gles::GlesFrame;
impl BlitFrame for smithay::backend::renderer::glow::GlowFrame;
impl BlitFrame for smithay::backend::renderer::multigpu::MultiFrame;
```

It is now possible to iterate over all known tokens and their associated data
```rs
fn smithay::wayland::xdg_activation::XdgActivationState::tokens() -> impl Iterator<Item = (&XdgActivationToken, &XdgActivationTokenData)>;
```

There are new errors for missing DRM crtc/connector/plane mapping
```rs
smithay::backend::drm::DrmError::{UnknownConnector, UnknownCrtc, UnknownPlane};
```

Texture has a few new implementations
```rs
impl Texture for smithay::backend::renderer::gles::GlesTarget;
impl Texture for smithay::backend::renderer::multigpu:MultiFramebuffer;
impl Texture for smithay::backend::renderer::pixman::PixmanTarget;
impl Texture for smithay::backend::renderer::test::DummyFramebuffer
```

It is now possible to access WlKeyboard/WlPointer instances
```rs
fn smithay::input::keyboard::KeyboardHandle::client_keyboards(&self, client: &Client) -> impl Iterator<Item = WlKeyboard>;
fn smithay::input::pointer::PointerHandle::client_pointers(&self, client: &:Client) -> impl Iterator<Item = WlPointer>;
```

New APIs for X11 randr output management 
```rs
enum smithay::xwayland::xwm::PrimaryOutputError { OutputUnknown, X11Error(x11rb::errors::ReplyError) };

impl From<x11rb::errors::ConnectionError> for smithay::xwayland::xwm::PrimaryOutputError;
fn smithay::xwayland::xwm::PrimaryOutputError::from(value: x11rb::errors::ConnectionError) -> Self;
fn smithay::xwayland::xwm::X11Wm::get_randr_primary_output(&self) -> Result<Option<String>, x11rb::errors::ReplyError>;
fn smithay::xwayland::xwm::X11Wm::set_randr_primary_output(&mut self, output: Option<&smithay::output::Output>) -> Result<(), smithay::xwayland::xwm::PrimaryOutputError>;
fn smithay::xwayland::xwm::XwmHandler::randr_primary_output_change(&mut self, xwm: smithay::xwayland::xwm::XwmId, output_name: Option<String>);
```

It is now possible to get the DrmNode of the device the buffer was allocated on
```rs
fn smithay::backend::allocator::gbm::GbmBuffer::device_node(&self) -> Option<drm::node::DrmNode>;
```

It is now possible to create a `GbmBuffer` from an existing `BufferObject` explicitly defining the device node
```rs
fn smithay::backend::allocator::gbm::GbmBuffer::from_bo_with_node(bo: gbm::buffer_object::BufferObject<()>, implicit: bool, drm_node: core::option::Option<drm::node::DrmNode>) -> Self;
```

It is now possible to access the `Allocator` of this output manager
```rs
fn smithay::backend::drm::output::DrmOutputManager::allocator(&self) -> &Allocator;
```

Is is now possible to check if EGLDevice is backed by actual device node or is it a software device.
```rs
fn smithay::backend::egl::EGLDevice::is_software(&self) -> bool
```

This adds a way to query next deadline of a commit timing barrier.
Allows a compositor to schedule re-evaluating commit timers without
busy looping.
```rs
fn smithay::wayland::commit_timing::CommitTimerBarrierState::next_deadline(&self) -> Option<smithay::wayland::commit_timing::Timestamp>;
```

Support for casting `Timestamp` back to `Time`
This might be useful to compare the next deadline with a monotonic time
from the presentation clock
```rs
impl From<smithay::wayland::commit_timing::Timestamp> for smithay::utils::Time;
```

Support for creating a weak reference to a `Seat`
```rs
fn smithay::input::Seat::downgrade(&self) -> smithay::input::WeakSeat;

fn smithay::input::WeakSeat::is_alive(&self) -> bool;
pub fn smithay::input::WeakSeat::upgrade(&self) -> Option<smithay::input::Seat>;
```

## 0.5.0

### API Changes
Items either removed or deprecated from the public API
```rs
/// Use Clone instead
impl Copy for smithay::utils::HookId;
/// Use `from_extremities` instead
fn smithay::utils::Rectangle::from_extemities(topleft, bottomright) -> Self;
```

Items added to the public API
```rs
/// Replaces deprecated `from_extemities`
fn smithay::utils::Rectangle::from_extremities(topleft, bottomright) -> Self;
/// Access the active text-input instance for the currently focused surface.
fn smithay::wayland::text_input::TextInputHandle::with_active_text_input(&self, f);
/// Just a new protocol
mod smithay::wayland::selection::ext_data_control;
```

Items changed in the public API
```diff
# create_external_token now accepts `XdgActivationTokenData` instead of `String`
-fn smithay::wayland::xdg_activation::XdgActivationState::create_external_token(&mut self, app_id: impl Into<Option<String>>);
+fn smithay::wayland::xdg_activation::XdgActivationState::create_external_token(&mut self, data: impl Into<Option<XdgActivationTokenData>>);
```

### New protocols
* Introduce ext data control protocol by @PolyMeilex in https://github.com/Smithay/smithay/pull/1577
* Update idle notify to version 2 by @PolyMeilex in https://github.com/Smithay/smithay/pull/1618

### TextInput improvements
* Revert "Fix repeated key input issue in Chrome with multiple windows" by @Drakulix in https://github.com/Smithay/smithay/pull/1647
* text-input: fix active instance tracking by @kchibisov in https://github.com/Smithay/smithay/pull/1648
* text-input: properly handle double buffered state by @kchibisov in https://github.com/Smithay/smithay/pull/1649

### Miscellaneous
* clock: Fix current monotonic time in millis u32 overflow panic by @YaLTeR in https://github.com/Smithay/smithay/pull/1645
* xwm: Update override-redirect flag on map request by @Ottatop in https://github.com/Smithay/smithay/pull/1656
* utils: Rework `HookId` recycle logic by @Paraworker in https://github.com/Smithay/smithay/pull/1657
* rename Rectangle::from_extemities to Rectangle::from_extremeties by @m4rch3n1ng in https://github.com/Smithay/smithay/pull/1646
* xdg_activation: Allow passing all data in XdgActivationState::create_external_token by @bbb651 in https://github.com/Smithay/smithay/pull/1658

## 0.4.0

### Breaking Changes

**`wayland-server` was updated to 0.30:**
- Most of the wayland frontend API is changed to follow the new request dispatching mechanism built around the `Dispatch` trait from `wayland-server`
- Modules that provide handlers for Wayland globals now provide `DelegateDispatch` implementations, as well as macros to simplify the dispatching from your main state

#### Clients & Protocols

- Remove `xdg-shell-unstable-v6` backwards compatibility
- `XdgPositionerState` moved to `XdgPopupState` and added to `XdgRequest::NewPopup`
- `PopupSurface::send_configure` now checks the protocol version and returns an `Result`
- `KeyboardHandle::input` filter closure now receives a `KeysymHandle` instead of a `Keysym` and returns a `FilterResult`.
- `PointerButtonEvent::button` now returns an `Option<MouseButton>`.
- `MouseButton` is now non-exhaustive.
- Remove `Other` and add `Forward` and `Back` variants to `MouseButton`. Use the new `PointerButtonEvent::button_code` in place of `Other`.
- `GrabStartData` has been renamed to `PointerGrabStartData`
- The `slot` method on touch events no longer returns an `Option` and multi-touch capability is thus opaque to the compositor
- `wayland::output::Output` now is created separately from it's `Global` as reflected by [`Output::new`] and the new [`Output::create_global] method.
- `PointerHandle` no longer sends an implicit motion event when a grab is set, `time` has been replaced by an explicit `focus` parameter in [`PointerHandle::set_grab`]
- `ToplevelSurface::send_configure`/`PopupSurface::send_configure`/`LayerSurface::send_configure` now always send a configure event regardless of changes and return
  the serial of the configure event. `send_pending_configure` can be used to only send a configure event on pending changes.

#### Backends

- Rename `WinitInputBacked` to `WinitEventLoop`.
- Rename `WinitInputError` to `WinitError`;
- `WinitInputBackend` no longer implements `InputBackend`. Input events are now received from the `WinitEvent::Input` variant.
- All winit backend internal event types now use `WinitInput` as the backend type.
- `WinitEventLoop::dispatch_new_events` is now used to receive some `WinitEvent`s.
- Added `TabletToolType::Unknown` as an option for tablet events
- `render_texture` was removed from `Frame`, use `render_texture_at` or `render_texture_from_to` instead or use `Gles2Renderer::render_texture` as a direct replacement.
- Remove `InputBackend::dispatch_new_events`, turning `InputBackend` into a definition of backend event types. Future input backends should be a `calloop::EventSource`.
- Remove `InputBackend::EventError` associated type as it is unneeded since `dispatch_new_events` was removed.
- `Swapchain` does not have a generic Userdata-parameter anymore, but utilizes `UserDataMap` instead
- `GbmBufferedSurface::next_buffer` now additionally returns the age of the buffer
- `Present` was merged into the `X11Surface`
- `X11Surface::buffer` now additionally returns the age of the buffer
- `X11Surface` now has an explicit `submit` function
- `X11Surface` is now multi-window capable.
- `Renderer::clear` now expects a second argument to optionally only clear parts of the buffer/surface
- `Transform::transform_size` now takes a `Size` instead of two `u32`
- `Gles2Renderer` now automatically flips the `render` result to account for OpenGLs coordinate system
- `Frame::clear`, `Frame::render_texture_at` and `Frame::render_texture_from_to` now have an additional damage argument
- `EGLNativeSurface` implementations overriding `swap_buffers` now receive and additional `damage` attribute to be used with `eglSwapBuffersWithDamageEXT` if desired
- `EGLSurface::swap_buffers` now accepts an optional `damage` parameter
- `WinitGraphicsBackend` does no longer provide a `render`-method and exposes its `Renderer` directly instead including new functions `bind` and `submit` to handle swapping buffers.
- `ImportShm` was renamed to `ImportMem`
- `ImportMem` and `ImportDma` were split and do now have accompanying traits `ImportMemWl` and `ImportDmaWl` to import wayland buffers.
- Added `EGLSurface::get_size`
- `EGLDisplay::get_extensions` was renamed to `extensions` and now returns a `&[String]`.
- Added gesture input events, which are supported with the libinput backend.

### Additions

#### Clients & Protocols

- `xdg_activation_v1` support
- `wlr-layer-shell-unstable-v1` support
- Added public api constants for the roles of `wl_shell_surface`, `zxdg_toplevel` and `xdg_toplevel`. See the
  `shell::legacy` and `shell::xdg` modules for these constants.
- Whether a surface is toplevel equivalent can be determined with the new function `shell::is_toplevel_equivalent`.
- Setting the parent of a toplevel surface is now possible with the `xdg::ToplevelSurface::set_parent` function.
- Add support for the zxdg-foreign-v2 protocol.
- Support for `xdg_wm_base` protocol version 3
- Added the option to initialize the dmabuf global with a client filter
- `wayland::output::Output` now has user data attached to it and more functions to query its properties
- Added a `KeyboardGrab` similar to the existing `PointerGrab`
- `wayland::output::Output` now has a `current_scale` method to quickly retrieve its set scale.
- `wayland::shell::wlr_layer::KeyboardInteractivity` now implements `PartialEq` and `Eq`.
- Added `TouchHandle` for Wayland client touch support (see `Seat::get_touch`)
- `wayland::output::Scale` was introduced to handle fractional scale values better
- Support for `wl_output` global version 4
- Support for `wl_seat` global version 7
- Support for `wl_compositor` global version 5
- Support for the `wp_viewporter` protocol
- Support for the `zwp_input_method_v2` protocol
- Support for the `zwp_text_input_v3` protocol

#### Backends

- New `x11` backend to run the compositor as an X11 client. Enabled through the `backend_x11` feature.
- `x11rb` event source integration used in anvil's XWayland implementation is now part of smithay at `utils::x11rb`. Enabled through the `x11rb_event_source` feature.
- `KeyState`, `MouseButton`, `ButtonState` and `Axis` in `backend::input` now derive `Hash`.
- New `DrmNode` type in drm backend. This is primarily for use a backend which needs to run as client inside another session.
- The button code for a `PointerButtonEvent` may now be obtained using `PointerButtonEvent::button_code`.
- `Renderer` now allows texture filtering methods to be set.
- `backend::renderer` has a new `utils`-module that can take care of client buffer management for you.
- `EGLSurface::buffer_age` can be used to query the surface buffer age.
- `GbmBufferedSurface::reset_buffers` can now be used to reset underlying buffers.
- Added new `Offscreen` trait to create offscreen surfaces for `Renderer`s
- Added functions to `ImportMem` to upload bitmaps from memory
- Added `ExportDma` trait to export framebuffers and textures into dmabufs
- Added `ExportMem` trait to copy framebuffers and textures into memory
- Added `multigpu`-module to the renderer, which makes handling multi-gpu setups easier!
- Added `backend::renderer::utils::import_surface_tree` to be able to import buffers before rendering
- Added `EGLContext::display` to allow getting the underlying display of some context.
- Make `EGLContext::dmabuf_render_formats` and `EGLContext::dmabuf_texture_formats` also accessible from `EGLDisplay`.

#### Desktop

- New `desktop` module to handle window placement, tracks popups, layer surface and various rendering helpers including automatic damage-tracking! (+so much more)

#### Utils

- `Rectangle` can now also be converted from f64 to i32 variants
- `Rectangle::contains_rect` can be used to check if a rectangle is contained within another
- `Coordinate` is now part of the public api, so it can be used for coordinate agnositic functions outside of the utils module or even out-of-tree

### Bugfixes

#### Clients & Protocols

- `Multicache::has()` now correctly does what is expected of it
- `xdg_shell` had an issue where it was possible that configured state gets overwritten before it was acked/committed.
- `wl_keyboard` rewind the `keymap` file before passing it to the client
- `wl_shm` properly validates parameters when creating a `wl_buffer`.
- `ServerDnDGrab` and `DnDGrab` now correctly send data device `leave` event on button release
- Client are now allowed to reassign the same role to a surface
- `xdg_output` now applies the output transforms to the reported logical size

#### Backends

- EGLBufferReader now checks if buffers are alive before using them.
- LibSeat no longer panics on seat disable event.
- X11 backend will report an error when trying to present a dmabuf fails.

### Anvil

- Anvil now implements the x11 backend in smithay. Run by passing `--x11` into the arguments when launching.
- Passing `ANVIL_MUTEX_LOG` in environment variables now uses the slower `Mutex` logging drain.
- Only toplevel surfaces now get implicit keyboard focus
- Fix popup drawing for fullscreen windows

## version 0.3.0 (2021-07-25)

Large parts of Smithay were changed with numerous API changes. It is thus recommended to
approach version 0.3 as if it was a new crate altogether compared to 0.2.

The most notable changes are:

- Deep refactor of the graphics backends around a workflows centered on allocating graphics buffers,
  and a Gles2-based renderer abstraction is provided.
- Support for DRM atomic modesetting as well as client-provided DMABUF
- Most backends are now `calloop` event sources generating events. The recommended organization for
  your smithay-based compositor is thus to centralize most of your logic on a global state struct,
  and delegate event handling to it via the shared data mechanism of `calloop`. Most of the callbacks
  you provide to Smithay are given mutable access to this shared data.
- The `wayland::compositor` handling logic now automatically handles state tracking and delayed commit
  for wayland surfaces.

Many thanks to the new contributors to Smithay, who contributed the following:

- Support for [`libseat`](https://sr.ht/~kennylevinsen/seatd/) as a session backend, by
  @PolyMeilex
- Support for graphics tablets via the `tablet` protocol extension, by @PolyMeilex
- Support for running Smithay on `aarch64` architectures, by @cmeissl
- A rework of the `xdg-shell` handlers to better fit the protocol logic and correctly track configure
  events, by @cmeissl
- Basic Xwayland support, by @psychon

## version 0.2.0 (2019-01-03)

### General

- **[Breaking]** Upgrade to wayland-rs 0.21
- **[Breaking]** Moving the public dependencies to a `reexports` module
- Migrate the codebase to Rust 2018

### Backends

- **[Breaking]** WinitBackend: Upgrade to winit 0.18
- **[Breaking]** Global refactor of the DRM & Session backends
- **[Breaking]** Restructuration of the backends around the `calloop` event-loop

### Clients & Protocol

- Basic XWayland support
- Data device & Drag'n'Drop support
- Custom client pointers support

## version 0.1.0 (2017-10-01)

### Protocol handling

- Low-level handling routines for several wayland globals:
  - `wayland::shm` handles `wl_shm`
  - `wayland::compositor` handles `wl_compositor` and `wl_subcompositor`
  - `wayland::shell` handles `wl_shell` and `xdg_shell`
  - `wayland::seat` handles `wl_seat`
  - `wayland::output` handles `wl_output`

### Backend

- Winit backend (EGL context & input)
- DRM backend
- libinput backend
- glium integration
