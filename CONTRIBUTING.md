# Contributing

Smithay is open to contributions from anyone. Here are a few tips to get started if you want to participate.

## Coordination

Most discussion about features and their implementations takes place on github.
If you have questions, suggestions, ideas, you can open an issue to discuss it, or add your message in an already existing issue
if it fits its scope.

If you want a more realtime discussion I (@vberger) have a Matrix room dedicated to Smithay and
my other wayland crates: [#smithay:matrix.org](https://matrix.to/#/#smithay:matrix.org). If you don't want to
use matrix, this room is also bridged to libera.chat IRC on #smithay.

## Scope

Smithay attempts to be as generic and un-opinionated as possible. As such, if you have an idea of a feature that would be usefull
for your compositor project and would like it to be integrated in Smithay, please consider whether it is in its scope:

- If this is a very generic feature that probably many different projects would find useful, it can be integrated in Smithay
- If it is a rather specific feature, but can be framed as a special case of a more general feature, this general feature is
  likely worth adding to Smithay
- If this feature is really specific to your use-case, it is out of scope for Smithay

## Structure

Smithay aims to be a modular hierarchical library:

- Functionalities should be split into independent modules as much as possible
- There can be dependencies in functionalities
- Even if most people would directly use a high-level functionality, the lower level abstractions it is built on should
  still be exposed independently if possible

The goal is for Smithay to be a "use what you want" library, and features that are not used should have no impact on the
application built with Smithay.
