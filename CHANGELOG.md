# Smithay Changelog

## Unreleased

### Breaking Changes

#### Clients & Protocols

- Remove `xdg-shell-unstable-v6` backwards compatibility
- `XdgPositionerState` moved to `XdgPopupState` and added to `XdgRequest::NewPopup`
- `PopupSurface::send_configure` now checks the protocol version and returns an `Result`
- `KeyboardHandle::input` filter closure now receives a `KeysymHandle` instead of a `Keysym` and returns a `FilterResult`.
- `PointerButtonEvent::button` now returns an `Option<MouseButton>`.
- `MouseButton` is now non-exhaustive.
- Remove `Other` and add `Forward` and `Back` variants to `MouseButton`. Use the new `PointerButtonEvent::button_code` in place of `Other`.

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

#### Backends

- New `x11` backend to run the compositor as an X11 client. Enabled through the `backend_x11` feature.
- `x11rb` event source integration used in anvil's XWayland implementation is now part of smithay at `utils::x11rb`. Enabled through the `x11rb_event_source` feature. 
- `KeyState`, `MouseButton`, `ButtonState` and `Axis` in `backend::input` now derive `Hash`.
- New `DrmNode` type in drm backend. This is primarily for use a backend which needs to run as client inside another session.
- The button code for a `PointerButtonEvent` may now be obtained using `PointerButtonEvent::button_code`. 
- `Renderer` now allows texture filtering methods to be set.

#### Utils

- `Rectangle` can now also be converted from f64 to i32 variants
- `Rectangle::contains_rect` can be used to check if a rectangle is contained within another
- `Coordinate` is now part of the public api, so it can be used for coordinate agnositic functions outside of the utils module or even out-of-tree

### Bugfixes

#### Clients & Protocols

- `Multicache::has()` now correctly does what is expected of it
- `xdg_shell` had an issue where it was possible that configured state gets overwritten before it was acked/committed.
- `wl_keyboard` rewind the `keymap` file before passing it to the client

#### Backends

- EGLBufferReader now checks if buffers are alive before using them.
- LibSeat no longer panics on seat disable event.
- X11 backend will report an error when trying to present a dmabuf fails.

### Anvil

- Anvil now implements the x11 backend in smithay. Run by passing `--x11` into the arguments when launching.
- Passing `ANVIL_MUTEX_LOG` in environment variables now uses the slower `Mutex` logging drain.

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
