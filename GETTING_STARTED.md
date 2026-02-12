# Getting started

So you want to write a new wayland compositor with smithay, but don't know where to start?

## Wayland itself

Wayland is just a set of XML files describing the protocol by which a compositor and client communicate.
To work on a compositor, you need to establish some understanding of the state mandated by the protocol,
most importantly the lifetime of objects defined by it.

We recommend the official [wayland-docs](https://wayland.freedesktop.org/docs/html/) to get started, in particular [Chapter 4](https://wayland.freedesktop.org/docs/html/ch04.html),
which talks about the protocol.

Additionally it can help to familiarize yourself with the viewpoint of clients. The [wayland-book](https://wayland-book.com/) is a good resource here.

There is also an (unfurtonately stalled - at the time of writing this document -) effort to write a [smithay book](https://smithay.github.io/book/) to
cover similar topics, but focused on [wayland-rs](https://github.com/Smithay/wayland-rs) (the underlying rust-based wayland implementation) rather than
[libwayland](https://gitlab.freedesktop.org/wayland/wayland/). At least the client side of this is a worthwhile read as well.

## Rust

smithay makes heavy use of Rust's features, which is likely why you selected the framework in the first place.
As such we assume you have *some* experience with the language already.

If not the [awesome-rust resource section](https://github.com/rust-unofficial/awesome-rust?tab=readme-ov-file#resources) has you covered for various learning resources.

## Smithay

Unfortunately the smithay book doesn't cover the server-side (yet).

Which leaves you with the following resources to understand how smithay works:

- A large portion of smithay is build around [wayland-rs](https://github.com/smithay/wayland-rs) Dispatch machinery. It is thus highly recommend to read and understand
the documentation of the [wayland-server](https://smithay.github.io/wayland-rs/wayland_server/) crate.
- smithay itself has [comprehensive documentation](https://smithay.github.io/smithay/) including for it's [various](https://smithay.github.io/smithay/smithay/backend/index.html) [modules](https://smithay.github.io/smithay/smithay/wayland/index.html)!
- [smallvil](https://github.com/Smithay/smithay/tree/master/smallvil) is the smallest (somewhat) usable compositor using smithay and intended as a learning resource and potential starting point.
- [anvil](https://github.com/Smithay/smithay/tree/master/anvil) is smithay's testing ground and thus a much more complete compositor to study. It's code strives to fill a gap between real-world examples and easy-to-understand/maintain code paths, however it is lacking in overall quality.
- Many real world compositors you smithay, please refer to our [README](https://github.com/Smithay/smithay/blob/master/README.md#other-compositors-that-use-smithay) for a list of targets to study.

### Side-note: Which version of smithay should I use?

Smithay itself does not follow a regular release schedule, as such many compositors choose to depend on a given git commit and frequent updates. However we strive to do better in this department and try to publish semi-frequently to crates.io.

However the framework is still evolving a lot, thus we cannot really commit to any sort of ABI stability. Thus you currently need to rely on git-based updates to not only get new features, but also bugfixes. As a result released smithay versions don't really have a benefit other than coming from crates.io.

If you are still unsure, check our [Changelog](https://github.com/Smithay/smithay/blob/master/CHANGELOG.md) and see how many unreleased features
are currently waiting for a new smithay release. The longer the list, the more we recommend to start off with the current git commit to make updating
to the next released version as easy as possible.

## Further help

If you still feel stuck trying to work your way through these resources or have questions not covered here, please feel free to reach out in our chatroom.
You can find us on matrix: [#smithay:matrix.org](https://matrix.to/#/#smithay:matrix.org). If you don't want to use matrix, this room is also bridged to libera.chat IRC on #smithay.
