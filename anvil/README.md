# Anvil

A compositor used as a testing ground for new smithay features.
For a simple example compositor consider reading [smallvil](https://github.com/Smithay/smithay/tree/master/smallvil)

## Dependencies

You'll need to install the following dependencies (note, that those package
names may vary depending on your OS and linux distribution):

- `libwayland`
- `libxkbcommon`

#### These are needed for the "Udev/DRM backend"

- `libudev`
- `libinput`
- `libgbm`
- [`libseat`](https://git.sr.ht/~kennylevinsen/seatd)

If you want to enable X11 support (to run X11 applications within anvil),
then you'll need to install the following packages as well:
    - `xwayland`

## Build and run

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

### Supported Environment Variables

| Variable                      | Example         | Backends  |
|-------------------------------|-----------------|-----------|
| ANVIL_DRM_DEVICE              | /dev/dri/card0  | tty-udev  |
| ANVIL_DISABLE_10BIT           | any             | tty-udev  |
| ANVIL_DISABLE_DIRECT_SCANOUT  | any             | tty-udev  |
| ANVIL_NO_VULKAN               | 1,true,yes,y    | x11       |
| SMITHAY_USE_LEGACY            | 1,true,yes,y    | tty-udev  |
| SMITHAY_VK_VERSION            | 1.3             |           |
