# Smithay Changelog

## Unreleased

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
