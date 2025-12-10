use super::InputMethodManagerState;
use crate::utils::{Logical, Point, Rectangle, Size};
use std::cmp::min;
use std::sync::Mutex;
use wayland_protocols_experimental::input_method::v1::server::xx_input_popup_positioner_v1::{
    self, Anchor, ConstraintAdjustment, Gravity, XxInputPopupPositionerV1,
};
use wayland_server::{Dispatch, Resource, WEnum};

/// Not sure what to write here. I just copied the pattern of UserData without analyzing it.
#[derive(Default, Debug)]
pub struct PositionerUserData {
    pub(crate) inner: Mutex<PositionerState>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// The state of a positioner, as set by the client
pub struct PositionerState {
    /// Requested size of the rectangle to position.
    ///
    /// This is treated as the preferred size to aim for, even if it can't always be reached (e.g. due to output too small).
    pub rect_size: Size<u32, Logical>,
    /// Edges defining the anchor point
    pub anchor_edges: Anchor,
    /// Gravity direction for positioning the child surface
    /// relative to its anchor point
    pub gravity: Gravity,
    /// Adjustments to do if previous criteria constrain the
    /// surface
    pub constraint_adjustment: ConstraintAdjustment,
    /// Offset placement relative to the anchor point
    pub offset: Point<i32, Logical>,
    /// When set reactive, the surface is reconstrained if the conditions
    /// used for constraining changed, e.g. the parent window moved.
    ///
    /// If the conditions changed and the popup was reconstrained,
    /// an xdg_popup.configure event is sent with updated geometry,
    /// followed by an xdg_surface.configure event.
    pub reactive: bool,
}

impl Default for PositionerState {
    fn default() -> Self {
        PositionerState {
            anchor_edges: Anchor::None,
            constraint_adjustment: ConstraintAdjustment::empty(),
            gravity: Gravity::None,
            offset: Default::default(),
            rect_size: Default::default(),
            reactive: false,
        }
    }
}

// This mostly but not completely copied from xdg positioner.
// Converted to stateless: PositionerState doesn't store any state.
impl PositionerState {
    pub(crate) fn anchor_has_edge(&self, edge: Anchor) -> bool {
        match edge {
            Anchor::Top => {
                self.anchor_edges == Anchor::Top
                    || self.anchor_edges == Anchor::TopLeft
                    || self.anchor_edges == Anchor::TopRight
            }
            Anchor::Bottom => {
                self.anchor_edges == Anchor::Bottom
                    || self.anchor_edges == Anchor::BottomLeft
                    || self.anchor_edges == Anchor::BottomRight
            }
            Anchor::Left => {
                self.anchor_edges == Anchor::Left
                    || self.anchor_edges == Anchor::TopLeft
                    || self.anchor_edges == Anchor::BottomLeft
            }
            Anchor::Right => {
                self.anchor_edges == Anchor::Right
                    || self.anchor_edges == Anchor::TopRight
                    || self.anchor_edges == Anchor::BottomRight
            }
            _ => unreachable!(),
        }
    }

    /// Get the anchor point for a popup as defined by this positioner.
    ///
    /// Defined by `xdg_positioner.set_anchor_rect` and
    /// `xdg_positioner.set_anchor`.
    pub fn get_anchor_point(&self, anchor_rect: Rectangle<i32, Logical>) -> Point<i32, Logical> {
        let y = anchor_rect.loc.y
            + if self.anchor_has_edge(Anchor::Top) {
                0
            } else if self.anchor_has_edge(Anchor::Bottom) {
                anchor_rect.size.h
            } else {
                anchor_rect.size.h / 2
            };

        let x = anchor_rect.loc.x
            + if self.anchor_has_edge(Anchor::Left) {
                0
            } else if self.anchor_has_edge(Anchor::Right) {
                anchor_rect.size.w
            } else {
                anchor_rect.size.w / 2
            };

        (x, y).into()
    }

    pub(crate) fn gravity_has_edge(&self, edge: Gravity) -> bool {
        match edge {
            Gravity::Top => {
                self.gravity == Gravity::Top
                    || self.gravity == Gravity::TopLeft
                    || self.gravity == Gravity::TopRight
            }
            Gravity::Bottom => {
                self.gravity == Gravity::Bottom
                    || self.gravity == Gravity::BottomLeft
                    || self.gravity == Gravity::BottomRight
            }
            Gravity::Left => {
                self.gravity == Gravity::Left
                    || self.gravity == Gravity::TopLeft
                    || self.gravity == Gravity::BottomLeft
            }
            Gravity::Right => {
                self.gravity == Gravity::Right
                    || self.gravity == Gravity::TopRight
                    || self.gravity == Gravity::BottomRight
            }
            _ => unreachable!(),
        }
    }

    /// Get the geometry without taking surface or display size into account.
    ///
    /// `Rectangle::width` and `Rectangle::height` corresponds to the
    /// size set by `xdg_positioner.set_size`.
    ///
    /// `Rectangle::x` and `Rectangle::y` define the position of the
    /// popup relative to its parent surface's `window_geometry`.
    /// The position is calculated according to the rules defined
    /// in the `xdg_shell` protocol.
    /// The `constraint_adjustment` will not be considered by this
    /// implementation and the position and size should be re-calculated
    /// in the compositor if the compositor implements `constraint_adjustment`
    ///
    /// [`PositionerState::get_unconstrained_geometry`] does take `constraint_adjustment` into account.
    fn get_geometry(&self, anchor_rect: Rectangle<i32, Logical>) -> Rectangle<i32, Logical> {
        // From the `xdg_shell` prococol specification:
        //
        // set_offset:
        //
        //  Specify the surface position offset relative to the position of the
        //  anchor on the anchor rectangle and the anchor on the surface. For
        //  example if the anchor of the anchor rectangle is at (x, y), the surface
        //  has the gravity bottom|right, and the offset is (ox, oy), the calculated
        //  surface position will be (x + ox, y + oy)
        let mut loc = self.offset;
        let size = self.rect_size;

        // Defines the anchor point for the anchor rectangle. The specified anchor
        // is used derive an anchor point that the child surface will be
        // positioned relative to. If a corner anchor is set (e.g. 'top_left' or
        // 'bottom_right'), the anchor point will be at the specified corner;
        // otherwise, the derived anchor point will be centered on the specified
        // edge, or in the center of the anchor rectangle if no edge is specified.
        loc += self.get_anchor_point(anchor_rect);

        // Defines in what direction a surface should be positioned, relative to
        // the anchor point of the parent surface. If a corner gravity is
        // specified (e.g. 'bottom_right' or 'top_left'), then the child surface
        // will be placed towards the specified gravity; otherwise, the child
        // surface will be centered over the anchor point on any axis that had no
        // gravity specified.
        loc.y = if self.gravity_has_edge(Gravity::Top) {
            loc.y.saturating_sub_unsigned(size.h)
        } else if !self.gravity_has_edge(Gravity::Bottom) {
            loc.y.saturating_sub_unsigned(size.h / 2)
        } else {
            loc.y
        };

        loc.x = if self.gravity_has_edge(Gravity::Left) {
            loc.x.saturating_sub_unsigned(size.w)
        } else if !self.gravity_has_edge(Gravity::Right) {
            loc.x.saturating_sub_unsigned(size.w / 2)
        } else {
            loc.x
        };

        let size = (
            0i32.saturating_add_unsigned(self.rect_size.w),
            0i32.saturating_add_unsigned(self.rect_size.h),
        )
            .into();

        Rectangle { loc, size }
    }

    /// Get the geometry for a popup as defined by this positioner, after trying to fit the popup into the
    /// target rectangle.
    ///
    /// `Rectangle::width` and `Rectangle::height` corresponds to the size set by `xdg_positioner.set_size`.
    ///
    /// `Rectangle::x` and `Rectangle::y` define the position of the popup relative to its parent surface's
    /// `window_geometry`. The position is calculated according to the rules defined in the `xdg_shell`
    /// protocol.
    ///
    /// This method does consider `constrain_adjustment` by trying to fit the popup into the provided target
    /// rectangle. The target rectangle is in the same coordinate system as the rectangle returned by this
    /// method. So, it is relative to the parent surface's geometry.
    pub fn get_unconstrained_geometry(
        mut self,
        anchor_rect: Rectangle<i32, Logical>,
        target: Rectangle<i32, Logical>,
    ) -> Rectangle<i32, Logical> {
        // The protocol defines the following order for adjustments: flip, slide, resize. If the flip fails
        // to remove the constraints, it is reverted.
        //
        // The adjustments are applied individually between axes. We can do that reasonably safely, given
        // that both our target and our popup are simple rectangles. The code is grouped per adjustment for
        // easier copy-paste checking, and because flips replace the geometry entirely, while further
        // adjustments change individual fields.
        let mut geo = self.get_geometry(anchor_rect);
        let (mut off_left, mut off_right, mut off_top, mut off_bottom) = compute_offsets(target, geo);

        // Try to flip horizontally.
        if (off_left > 0 || off_right > 0) && self.constraint_adjustment.contains(ConstraintAdjustment::FlipX)
        {
            let mut new = self;
            new.anchor_edges = invert_anchor_x(new.anchor_edges);
            new.gravity = invert_gravity_x(new.gravity);
            let new_geo = new.get_geometry(anchor_rect);
            let (new_off_left, new_off_right, _, _) = compute_offsets(target, new_geo);

            // Apply flip only if it removed the constraint.
            if new_off_left <= 0 && new_off_right <= 0 {
                self = new;
                geo = new_geo;
                off_left = 0;
                off_right = 0;
                // off_top and off_bottom are unchanged since we're using rectangles.
            }
        }

        // Try to flip vertically.
        if (off_top > 0 || off_bottom > 0) && self.constraint_adjustment.contains(ConstraintAdjustment::FlipY)
        {
            let mut new = self;
            new.anchor_edges = invert_anchor_y(new.anchor_edges);
            new.gravity = invert_gravity_y(new.gravity);
            let new_geo = new.get_geometry(anchor_rect);
            let (_, _, new_off_top, new_off_bottom) = compute_offsets(target, new_geo);

            // Apply flip only if it removed the constraint.
            if new_off_top <= 0 && new_off_bottom <= 0 {
                self = new;
                geo = new_geo;
                off_top = 0;
                off_bottom = 0;
                // off_left and off_right are unchanged since we're using rectangles.
            }
        }

        // Try to slide horizontally.
        if (off_left > 0 || off_right > 0)
            && self.constraint_adjustment.contains(ConstraintAdjustment::SlideX)
        {
            // Prefer to show the top-left corner of the popup so that we can easily do a resize
            // adjustment next.
            if off_left > 0 {
                geo.loc.x += off_left;
            } else if off_right > 0 {
                geo.loc.x -= min(off_right, -off_left);
            }

            (_, off_right, _, _) = compute_offsets(target, geo);
            // off_top and off_bottom are the same since we're using rectangles.
        }

        // Try to slide vertically.
        if (off_top > 0 || off_bottom > 0)
            && self.constraint_adjustment.contains(ConstraintAdjustment::SlideY)
        {
            // Prefer to show the top-left corner of the popup so that we can easily do a resize
            // adjustment next.
            if off_top > 0 {
                geo.loc.y += off_top;
            } else if off_bottom > 0 {
                geo.loc.y -= min(off_bottom, -off_top);
            }

            (_, _, _, off_bottom) = compute_offsets(target, geo);
            // off_left and off_right are the same since we're using rectangles.
        }

        // Try to resize horizontally. This makes sense only if the popup is at least partially to the left
        // of the right target edge, which is the same as checking that the offset is smaller than the width.
        if off_right > 0
            && off_right < geo.size.w
            && self.constraint_adjustment.contains(ConstraintAdjustment::ResizeX)
        {
            geo.size.w -= off_right;
        }

        // Try to resize vertically. This makes sense only if the popup is at least partially to the top of
        // the bottom target edge, which is the same as checking that the offset is smaller than the height.
        if off_bottom > 0
            && off_bottom < geo.size.h
            && self.constraint_adjustment.contains(ConstraintAdjustment::ResizeY)
        {
            geo.size.h -= off_bottom;
        }

        geo
    }

    /// Return the popup geometry computed based on the cursor anchor.
    pub fn get_geometry_from_anchor(
        &self,
        cursor: Rectangle<i32, Logical>,
        target: Rectangle<i32, Logical>,
    ) -> Rectangle<i32, Logical> {
        self.get_unconstrained_geometry(cursor, target)
    }
}

impl<D> Dispatch<XxInputPopupPositionerV1, PositionerUserData, D> for InputMethodManagerState
where
/*D: Dispatch<XdgPositioner, XdgPositionerUserData>,
D: XdgShellHandler,
D: 'static,*/
{
    fn request(
        _state: &mut D,
        _client: &wayland_server::Client,
        positioner: &XxInputPopupPositionerV1,
        request: xx_input_popup_positioner_v1::Request,
        data: &PositionerUserData,
        _dhandle: &wayland_server::DisplayHandle,
        _data_init: &mut wayland_server::DataInit<'_, D>,
    ) {
        let mut state = data.inner.lock().unwrap();
        use xx_input_popup_positioner_v1::Request;
        match request {
            Request::SetSize { width, height } => {
                if width < 1 || height < 1 {
                    positioner.post_error(
                        xx_input_popup_positioner_v1::Error::InvalidInput,
                        "Invalid size for positioner.",
                    );
                } else {
                    state.rect_size = (width, height).into();
                }
            }
            Request::SetAnchor { anchor } => {
                if let WEnum::Value(anchor) = anchor {
                    state.anchor_edges = anchor;
                }
            }
            Request::SetGravity { gravity } => {
                if let WEnum::Value(gravity) = gravity {
                    state.gravity = gravity;
                }
            }
            Request::SetConstraintAdjustment {
                constraint_adjustment,
            } => {
                if let WEnum::Value(constraint_adjustment) = constraint_adjustment {
                    state.constraint_adjustment = constraint_adjustment;
                }
            }
            Request::SetOffset { x, y } => {
                state.offset = (x, y).into();
            }
            Request::SetReactive => {
                state.reactive = true;
            }
            Request::Destroy => {
                // handled by destructor
            }
            _ => unreachable!(),
        }
    }
}

fn compute_offsets(target: Rectangle<i32, Logical>, popup: Rectangle<i32, Logical>) -> (i32, i32, i32, i32) {
    let off_left = target.loc.x - popup.loc.x;
    let off_right = (popup.loc.x + popup.size.w) - (target.loc.x + target.size.w);
    let off_top = target.loc.y - popup.loc.y;
    let off_bottom = (popup.loc.y + popup.size.h) - (target.loc.y + target.size.h);
    (off_left, off_right, off_top, off_bottom)
}

fn invert_anchor_x(anchor: Anchor) -> Anchor {
    match anchor {
        Anchor::Left => Anchor::Right,
        Anchor::Right => Anchor::Left,
        Anchor::TopLeft => Anchor::TopRight,
        Anchor::TopRight => Anchor::TopLeft,
        Anchor::BottomLeft => Anchor::BottomRight,
        Anchor::BottomRight => Anchor::BottomLeft,
        x => x,
    }
}

fn invert_anchor_y(anchor: Anchor) -> Anchor {
    match anchor {
        Anchor::Top => Anchor::Bottom,
        Anchor::Bottom => Anchor::Top,
        Anchor::TopLeft => Anchor::BottomLeft,
        Anchor::TopRight => Anchor::BottomRight,
        Anchor::BottomLeft => Anchor::TopLeft,
        Anchor::BottomRight => Anchor::TopRight,
        x => x,
    }
}

fn invert_gravity_x(gravity: Gravity) -> Gravity {
    match gravity {
        Gravity::Left => Gravity::Right,
        Gravity::Right => Gravity::Left,
        Gravity::TopLeft => Gravity::TopRight,
        Gravity::TopRight => Gravity::TopLeft,
        Gravity::BottomLeft => Gravity::BottomRight,
        Gravity::BottomRight => Gravity::BottomLeft,
        x => x,
    }
}

fn invert_gravity_y(gravity: Gravity) -> Gravity {
    match gravity {
        Gravity::Top => Gravity::Bottom,
        Gravity::Bottom => Gravity::Top,
        Gravity::TopLeft => Gravity::BottomLeft,
        Gravity::TopRight => Gravity::BottomRight,
        Gravity::BottomLeft => Gravity::TopLeft,
        Gravity::BottomRight => Gravity::TopRight,
        x => x,
    }
}
