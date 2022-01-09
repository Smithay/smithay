use std::{cmp::Ordering, convert::TryFrom};

use wayland_protocols::wlr::unstable::layer_shell::v1::server::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};
use wayland_server::WEnum;

/// Available layers for surfaces
///
/// These values indicate which layers a surface can be rendered in.
/// They are ordered by z depth, bottom-most first.
/// Traditional shell surfaces will typically be rendered between the bottom and top layers.
/// Fullscreen shell surfaces are typically rendered at the top layer.
/// Multiple surfaces can share a single layer, and ordering within a single layer is undefined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layer {
    /// The lowest layer, used usualy for wallpapers
    Background,
    /// The layer bellow the windows and above the wallpaper
    Bottom,
    /// The layer above the windows and bellow overlay
    Top,
    /// The top layer above all other layers
    Overlay,
}

impl TryFrom<WEnum<zwlr_layer_shell_v1::Layer>> for Layer {
    type Error = (zwlr_layer_shell_v1::Error, String);

    fn try_from(layer: WEnum<zwlr_layer_shell_v1::Layer>) -> Result<Self, Self::Error> {
        use zwlr_layer_shell_v1::Layer;

        match layer {
            WEnum::Value(Layer::Background) => Ok(Self::Background),
            WEnum::Value(Layer::Bottom) => Ok(Self::Bottom),
            WEnum::Value(Layer::Top) => Ok(Self::Top),
            WEnum::Value(Layer::Overlay) => Ok(Self::Overlay),
            layer => Err((
                zwlr_layer_shell_v1::Error::InvalidLayer,
                format!("invalid layer: {:?}", layer),
            )),
        }
    }
}

impl Default for Layer {
    fn default() -> Self {
        Self::Background
    }
}

/// Types of keyboard interaction possible for a layer shell surface
///
/// The rationale for this is twofold:
/// - some applications are not interested in keyboard events
///   and not allowing them to be focused can improve the desktop experience
/// - some applications will want to take exclusive keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyboardInteractivity {
    /// This value indicates that this surface is not interested in keyboard events
    /// and the compositor should never assign it the keyboard focus.
    ///
    /// This is the default value, set for newly created layer shell surfaces.
    ///
    /// This is useful for e.g. desktop widgets that display information
    /// or only have interaction with non-keyboard input devices.
    None,
    /// Request exclusive keyboard focus if this surface is above the shell surface layer.
    ///
    /// For the top and overlay layers, the seat will always give exclusive keyboard focus
    /// to the top-most layer which has keyboard interactivity set to exclusive.
    /// If this layer contains multiple surfaces with keyboard interactivity set to exclusive,
    /// the compositor determines the one receiving keyboard events in an implementation- defined manner.
    /// In this case, no guarantee is made when this surface will receive keyboard focus (if ever).
    ///
    /// For the bottom and background layers, the compositor is allowed to use normal focus semantics.
    ///
    /// This setting is mainly intended for applications that need to
    /// ensure they receive all keyboard events, such as a lock screen or a password prompt.
    Exclusive,
    /// This requests the compositor to allow this surface
    /// to be focused and unfocused by the user in an implementation-defined manner.
    /// The user should be able to unfocus this surface even regardless of the layer it is on.
    ///
    /// Typically, the compositor will want to use its normal mechanism
    /// to manage keyboard focus between layer shell surfaces
    /// with this setting and regular toplevels on the desktop layer (e.g. click to focus).
    /// Nevertheless, it is possible for a compositor to require a special interaction
    /// to focus or unfocus layer shell surfaces
    ///
    /// This setting is mainly intended for desktop shell components that allow keyboard interaction.
    /// Using this option can allow implementing a desktop shell that can be fully usable without the mouse.
    OnDemand,
}

impl Default for KeyboardInteractivity {
    fn default() -> Self {
        Self::None
    }
}

impl TryFrom<WEnum<zwlr_layer_surface_v1::KeyboardInteractivity>> for KeyboardInteractivity {
    type Error = (zwlr_layer_surface_v1::Error, String);

    fn try_from(ki: WEnum<zwlr_layer_surface_v1::KeyboardInteractivity>) -> Result<Self, Self::Error> {
        use zwlr_layer_surface_v1::KeyboardInteractivity;

        match ki {
            WEnum::Value(KeyboardInteractivity::None) => Ok(Self::None),
            WEnum::Value(KeyboardInteractivity::Exclusive) => Ok(Self::Exclusive),
            WEnum::Value(KeyboardInteractivity::OnDemand) => Ok(Self::OnDemand),
            ki => Err((
                zwlr_layer_surface_v1::Error::InvalidKeyboardInteractivity,
                format!("wrong keyboard interactivity value: {:?}", ki),
            )),
        }
    }
}

bitflags::bitflags! {
    /// Anchor bitflags, describing how the layers surface should be positioned and sized
    pub struct Anchor: u32 {
        /// The top edge of the anchor rectangle
        const TOP = 1;
        /// The bottom edge of the anchor rectangle
        const BOTTOM = 2;
        /// The left edge of the anchor rectangle
        const LEFT = 4;
        /// The right edge of the anchor rectangle
        const RIGHT = 8;
    }
}

impl Anchor {
    /// Check if anchored horizontally
    ///
    /// If it is anchored to `left` and `right` anchor at the same time
    /// it returns `true`
    pub fn anchored_horizontally(&self) -> bool {
        self.contains(Self::LEFT) && self.contains(Self::RIGHT)
    }
    /// Check if anchored vertically
    ///
    /// If it is anchored to `top` and `bottom` anchor at the same time
    /// it returns `true`
    pub fn anchored_vertically(&self) -> bool {
        self.contains(Self::TOP) && self.contains(Self::BOTTOM)
    }
}

impl Default for Anchor {
    fn default() -> Self {
        Self::empty()
    }
}

impl TryFrom<WEnum<zwlr_layer_surface_v1::Anchor>> for Anchor {
    type Error = (zwlr_layer_surface_v1::Error, String);

    fn try_from(anchor: WEnum<zwlr_layer_surface_v1::Anchor>) -> Result<Self, Self::Error> {
        let a = if let WEnum::Value(anchor) = anchor {
            Anchor::from_bits(anchor.bits())
        } else {
            None
        };

        a.ok_or((
            zwlr_layer_surface_v1::Error::InvalidAnchor,
            format!("invalid anchor {:?}", anchor),
        ))
    }
}

/// Exclusive zone descriptor
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExclusiveZone {
    /// Requests that the compositor avoids occluding an area with other surfaces.
    ///
    /// A exclusive zone value is the distance from the edge in surface-local coordinates to consider exclusive.
    ///
    /// A exclusive value is only meaningful if the surface is
    /// anchored to one edge or an edge and both perpendicular edges.
    ///
    /// If the surface is:
    /// - not anchored
    /// - anchored to only two perpendicular edges (a corner),
    /// - anchored to only two parallel edges or anchored to all edges,
    ///
    /// The exclusive value should be treated the same as [`ExclusiveZone::Neutral`].
    Exclusive(u32),
    /// If set to Neutral,
    /// the surface indicates that it would like to be moved to avoid occluding surfaces with a exclusive zone.
    Neutral,
    /// If set to DontCare,
    /// the surface indicates that it would not like to be moved to accommodate for other surfaces,
    /// and the compositor should extend it all the way to the edges it is anchored to.
    DontCare,
}

impl Default for ExclusiveZone {
    fn default() -> Self {
        Self::Neutral
    }
}

impl From<i32> for ExclusiveZone {
    fn from(v: i32) -> Self {
        match v.cmp(&0) {
            Ordering::Greater => Self::Exclusive(v as u32),
            Ordering::Equal => Self::Neutral,
            Ordering::Less => Self::DontCare,
        }
    }
}

impl From<ExclusiveZone> for i32 {
    fn from(z: ExclusiveZone) -> i32 {
        match z {
            ExclusiveZone::Exclusive(v) => v as i32,
            ExclusiveZone::Neutral => 0,
            ExclusiveZone::DontCare => -1,
        }
    }
}

/// Describes distance from the anchor point of the output, in surface-local coordinates.
///
/// If surface did not anchor curtain edge, margin for that edge should be ignored.
///
/// The exclusive zone should includes the margins.
#[derive(Debug, Default, Clone, Copy)]
pub struct Margins {
    /// Distance from [`Anchor::TOP`]
    pub top: i32,
    /// Distance from [`Anchor::TOP`]
    pub right: i32,
    /// Distance from [`Anchor::BOTTOM`]
    pub bottom: i32,
    /// Distance from [`Anchor::LEFT`]
    pub left: i32,
}
