# Smithay

[![Crates.io](https://img.shields.io/crates/v/smithay.svg)](https://crates.io/crates/smithay)
[![docs.rs](https://docs.rs/smithay/badge.svg)](https://docs.rs/smithay)
[![Build Status](https://github.com/Smithay/smithay/workflows/Continuous%20Integration/badge.svg)](https://github.com/Smithay/smithay/actions)
[![Join the chat on matrix at #smithay:matrix.org](https://img.shields.io/badge/%5Bm%5D-%23smithay%3Amatrix.org-blue.svg)](https://matrix.to/#/#smithay:matrix.org)
![Join the chat via bridge on #smithay on libera.chat](https://img.shields.io/badge/IRC-%23Smithay-blue.svg)

A smithy for rusty wayland compositors

## Goals

Smithay aims to provide building blocks to create wayland compositors in Rust. While not
being a full-blown compositor, it'll provide objects and interfaces implementing common
functionalities that pretty much any compositor will need, in a generic fashion.

It supports the [core Wayland protocols](https://gitlab.freedesktop.org/wayland/wayland), the official [protocol extensions](https://gitlab.freedesktop.org/wayland/wayland-protocols), and _some_ external extensions, such as those made by and for [wlroots](https://gitlab.freedesktop.org/wlroots/wlr-protocols) and [KDE](https://invent.kde.org/libraries/plasma-wayland-protocols)

<!-- https://github.com/Smithay/smithay/pull/779#discussion_r993640470 https://github.com/Smithay/smithay/issues/778 -->

Also:

- **Documented:** Smithay strives to maintain a clear and detailed documentation of its API and its
  functionalities. Compiled documentations are available on [docs.rs](https://docs.rs/smithay) for released
  versions, and [here](https://smithay.github.io/smithay) for the master branch.
- **Safety:** Smithay will target to be safe to use, because Rust.
- **Modularity:** Smithay is not a framework, and will not be constraining. If there is a
  part you don't want to use, you should not be forced to use it.
- **High-level:** You should be able to not have to worry about gory low-level stuff (but
  Smithay won't stop you if you really want to dive into it).

## Anvil

Like others, Smithay as a compositor library has its own sample compositor: anvil.

You can run it with cargo after having cloned this repository and installed the required dependencies:

### Feature: Default

- `libudev`
- `libinput`
- `libseat`
- `libgbm`
- `libsystemd`
- `xwayland`

### Feature: UDEV

- `libudev`
- `libinput`
- `libseat`
- `libgbm`
- `libsystemd`

### Feature: xwayland

- `xwayland`

```bash
# (for Debian/Ubuntu)
sudo apt install libdbus-glib-1-dev libudev-dev libsystemd-dev libxkbcommon-dev libinput-dev libgbm-dev

# (for Arch Linux/Arch-Based Distro)
sudo pacman -S seatd mesa xorg-xwayland
```

Then, run:

```
cd anvil;

cargo run -- --{backend}
```

The currently available backends are:

- `--x11`: start anvil as an X11 client. This allows you to run the compositor inside an X11 session or any compositor supporting XWayland. Should be preferred over the winit backend where possible.
- `--winit`: start anvil as a [Winit](https://github.com/tomaka/winit) application. This allows you to run it
  inside of an other X11 or Wayland session.
- `--tty-udev`: start anvil in a tty with udev support. This is the "traditional" launch of a Wayland
  compositor. Note that this requires you to start anvil as root if your system does not have logind
  available.

## Contact us

If you have questions or want to discuss the project with us, our main chatroom is on Matrix: [`#smithay:matrix.org`](https://matrix.to/#/#smithay:matrix.org). You can also join it via an IRC bridge, on `#smithay` on [libera.chat](https://libera.chat/).
