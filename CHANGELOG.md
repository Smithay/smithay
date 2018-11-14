# Smithay Changelog

## Unreleased

### General

- **[Breaking]** Upgrade to wayland-rs 0.21

### Backends

- **[Breaking]** WinitBackend: Upgrade to winit 0.18

### Clients & Protocol

- Basic XWayland support

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
