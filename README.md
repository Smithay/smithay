# Smithay

[![Crates.io](http://meritbadge.herokuapp.com/smithay)](https://crates.io/crates/smithay)
[![docs.rs](https://docs.rs/smithay/badge.svg)](https://docs.rs/smithay)
[![Build Status](https://travis-ci.org/Smithay/smithay.svg?branch=master)](https://travis-ci.org/Smithay/smithay)
[![Join the chat on matrix at @smithay:matrix.org](matrix_badge.svg)](https://matrix.to/#/#smithay:matrix.org)
[![Join the chat via bridge at https://gitter.im/smithay/Lobby](https://badges.gitter.im/smithay/Lobby.svg)](https://gitter.im/smithay/Lobby?utm_source=badge&utm_medium=badge&utm_campaign=pr-badge&utm_content=badge)

A smithy for rusty wayland compositors

**Warning:** This is a very new project, still in the process of shaping itself. I cannot
recommend to use it *unless* you want to help driving it forward. ;-)

## Goals

Smithay aims to provide building blocks to create wayland compositors in Rust. While not
being a full-blown compositor, it'll provide objects and interfaces implementing common
functionnalities that pretty much any compositor will need, in a generic fashion.

Also:

- **Safety:** Smithay will target to be safe to use, because Rust.
- **Modularity:** Smithay is not a framework, and will not be constraining. If there is a
  part you don't want to use, you should not be forced to use it.
- **High-level:** You should be able to not have to worry about gory low-level stuff (but 
  Smithay won't stop you if you really want to dive into it).

## Current status

Nothing is done yet, I'm starting to figure out the design.

## Why?

I'm doing this because I find it interesting. Also, I'd love to see a pure-rust¹ wayland
compositor.

*(¹: Almost, as some very low-level bits will necessarily still be C. But let's keep them minimal, shall we?)*
