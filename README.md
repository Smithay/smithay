# Smithay

[![Crates.io](https://img.shields.io/crates/v/smithay.svg)](https://crates.io/crates/smithay)
[![docs.rs](https://docs.rs/smithay/badge.svg)](https://docs.rs/smithay)
[![Build Status](https://github.com/Smithay/smithay/workflows/Continuous%20Integration/badge.svg)](https://github.com/Smithay/smithay/actions)
[![Join the chat on matrix at @smithay:matrix.org](matrix_badge.svg)](https://matrix.to/#/#smithay:matrix.org)
[![Join the chat via bridge on gitter at smithay/Lobby ](https://badges.gitter.im/smithay/Lobby.svg)](https://gitter.im/smithay/Lobby?utm_source=badge&utm_medium=badge&utm_campaign=pr-badge&utm_content=badge)

A smithy for rusty wayland compositors

## Goals

Smithay aims to provide building blocks to create wayland compositors in Rust. While not
being a full-blown compositor, it'll provide objects and interfaces implementing common
functionalities that pretty much any compositor will need, in a generic fashion.

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

You can run it with cargo after having cloned this repository:

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
