//! Common base for elements that can be drawn by a [`Renderer`]
//!
//! A [`RenderElement`] defines what should be [`draw`](RenderElement::draw)n where.
//! Additionally it provides the foundation for effective damage tracked rendering
//! by allowing to query for damage between two [`CommitCounter`]s.
//!
//! For specialized renderers it can optionally provide access to the [`UnderlyingStorage`]
//! of the element.
//!
//! Out of the box smithay provides the following elements
//! - [`memory`](crate::backend::renderer::element::memory) - Memory based render element
//! - [`texture`](crate::backend::renderer::element::texture) - Texture based render element
//! - [`surface`](crate::backend::renderer::element::surface) - Wayland surface render element
//!
//! The [`render_elements!`] macro provides an easy way to aggregate multiple different [RenderElement]s
//! into a single enum.
//!
//! See the [`damage`](crate::backend::renderer::damage) module for more information on
//! damage tracking.

use std::{collections::HashMap, sync::Arc};

#[cfg(feature = "wayland_frontend")]
use wayland_server::{backend::ObjectId, protocol::wl_buffer, Resource};

use crate::utils::{Buffer as BufferCoords, Physical, Point, Rectangle, Scale, Size, Transform};

use super::{utils::CommitCounter, Renderer};

pub mod memory;
#[cfg(feature = "wayland_frontend")]
pub mod surface;
pub mod texture;

crate::utils::ids::id_gen!(next_external_id, EXTERNAL_ID, EXTERNAL_IDS);

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
/// A unique id for a [`RenderElement`]
pub struct Id(InnerId);

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
enum InnerId {
    #[cfg(feature = "wayland_frontend")]
    WaylandResource(ObjectId),
    External(Arc<ExternalId>),
}

#[derive(Debug, PartialEq, Eq, Hash, Clone)]
struct ExternalId(usize);

impl ExternalId {
    fn new() -> Self {
        ExternalId(next_external_id())
    }
}

impl Drop for ExternalId {
    fn drop(&mut self) {
        EXTERNAL_IDS.lock().unwrap().remove(&self.0);
    }
}

impl Id {
    /// Create an id from a [`Resource`]
    ///
    /// Note: Calling this function for the same [`Resource`]
    /// multiple times will return the same id.
    #[cfg(feature = "wayland_frontend")]
    pub fn from_wayland_resource<R: Resource>(resource: &R) -> Self {
        Id(InnerId::WaylandResource(resource.id()))
    }

    /// Create a new unique id
    ///
    /// Note: The id will be re-used once all instances of this [`Id`]
    /// are dropped.
    pub fn new() -> Self {
        Id(InnerId::External(Arc::new(ExternalId::new())))
    }
}

#[cfg(feature = "wayland_frontend")]
impl<R: Resource> From<&R> for Id {
    fn from(resource: &R) -> Self {
        Id::from_wayland_resource(resource)
    }
}

/// The underlying storage for a element
#[derive(Debug)]
pub enum UnderlyingStorage<'a, R: Renderer> {
    /// A wayland buffer
    #[cfg(feature = "wayland_frontend")]
    Wayland(wl_buffer::WlBuffer),
    /// A texture
    External(&'a R::TextureId),
}

/// Defines the presentation state of an element after rendering
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderElementPresentationState {
    /// The element was rendered
    Rendered,
    /// The element was directly scanned out
    ScannedOut,
    /// The element was skipped
    Skipped,
}

/// Defines the element render state after rendering
#[derive(Debug, Clone, Copy)]
pub struct RenderElementState {
    /// Holds the visible portion of the element on the output
    ///
    /// Note: If the presentation_state is [`RenderElementPresentationState::Skipped`] this will be zero.
    pub visible_portion: Size<i32, Physical>,
    /// Holds the presentation state of the element on the output
    pub presentation_state: RenderElementPresentationState,
}

impl RenderElementState {
    pub(crate) fn skipped() -> Self {
        RenderElementState {
            visible_portion: Size::default(),
            presentation_state: RenderElementPresentationState::Skipped,
        }
    }

    pub(crate) fn rendered(visible_portion: Size<i32, Physical>) -> Self {
        RenderElementState {
            visible_portion,
            presentation_state: RenderElementPresentationState::Rendered,
        }
    }
}

/// Holds the states for a set of [`RenderElement`]s
#[derive(Debug, Clone)]
pub struct RenderElementStates {
    /// Holds the render states of the elements
    pub states: HashMap<Id, RenderElementState>,
}

impl RenderElementStates {
    /// Return the [`RenderElementState`] for the specified [`Id`]
    ///
    /// Return `None` if the element is not included in the states
    pub fn element_render_state(&self, id: impl Into<Id>) -> Option<RenderElementState> {
        self.states.get(&id.into()).copied()
    }

    /// Returns whether the element with the specified id was presented
    ///
    /// Returns `false` if the element with the id was not found or skipped
    pub fn element_was_presented(&self, id: impl Into<Id>) -> bool {
        self.element_render_state(id)
            .map(|state| state.presentation_state != RenderElementPresentationState::Skipped)
            .unwrap_or(false)
    }
}

/// A single render element
pub trait RenderElement<R: Renderer> {
    /// Get the unique id of this element
    fn id(&self) -> &Id;
    /// Get the current commit position of this element
    fn current_commit(&self) -> CommitCounter;
    /// Get the location relative to the output
    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.geometry(scale).loc
    }
    /// Get the src of the underlying buffer
    fn src(&self) -> Rectangle<f64, BufferCoords>;
    /// Get the transform of the underlying buffer
    fn transform(&self) -> Transform {
        Transform::Normal
    }
    /// Get the geometry relative to the output
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical>;
    /// Get the damage since the provided commit relative to the element
    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> Vec<Rectangle<i32, Physical>> {
        if commit != Some(self.current_commit()) {
            vec![Rectangle::from_loc_and_size((0, 0), self.geometry(scale).size)]
        } else {
            vec![]
        }
    }
    /// Get the opaque regions of the element relative to the element
    fn opaque_regions(&self, _scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        vec![]
    }
    /// Get the underlying storage of this element, may be used to optimize rendering (eg. drm planes)
    fn underlying_storage(&self, _renderer: &R) -> Option<UnderlyingStorage<'_, R>> {
        None
    }
    /// Draw this element
    fn draw(
        &self,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        damage: &[Rectangle<i32, Physical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error>;
}

/// Types that can be converted into [`RenderElement`]s
pub trait AsRenderElements<R>
where
    R: Renderer,
{
    /// Type of the render element
    type RenderElement: RenderElement<R>;
    /// Returns render elements for a given position and scale
    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
    ) -> Vec<C>;
}

impl<R, E> RenderElement<R> for &E
where
    R: Renderer,
    E: RenderElement<R>,
{
    fn id(&self) -> &Id {
        (*self).id()
    }

    fn current_commit(&self) -> CommitCounter {
        (*self).current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        (*self).location(scale)
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        (*self).src()
    }

    fn transform(&self) -> Transform {
        (*self).transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        (*self).geometry(scale)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> Vec<Rectangle<i32, Physical>> {
        (*self).damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        (*self).opaque_regions(scale)
    }

    fn underlying_storage(&self, renderer: &R) -> Option<UnderlyingStorage<'_, R>> {
        (*self).underlying_storage(renderer)
    }

    fn draw(
        &self,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        damage: &[Rectangle<i32, Physical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error> {
        (*self).draw(renderer, frame, location, scale, damage, log)
    }
}

#[macro_export]
#[doc(hidden)]
macro_rules! render_elements_internal {
    (@enum $(#[$attr:meta])* $vis:vis $name:ident; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name {
            $(
                $(
                    #[$meta]
                )*
                $body($field)
            ),*,
            #[doc(hidden)]
            _GenericCatcher(std::convert::Infallible),
        }
    };
    (@enum $(#[$attr:meta])* $vis:vis $name:ident $($custom:ident)+; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$($custom),+> {
            $(
                $(
                    #[$meta]
                )*
                $body($field)
            ),*,
            #[doc(hidden)]
            _GenericCatcher(std::convert::Infallible),
        }
    };
    (@enum $(#[$attr:meta])* $vis:vis $name:ident $lt:lifetime; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$lt> {
            $(
                $(
                    #[$meta]
                )*
                $body($field)
            ),*,
            #[doc(hidden)]
            _GenericCatcher(std::convert::Infallible),
        }
    };
    (@enum $(#[$attr:meta])* $vis:vis $name:ident $lt:lifetime $($custom:ident)+; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$lt, $($custom),+> {
            $(
                $(
                    #[$meta]
                )*
                $body($field)
            ),*,
            #[doc(hidden)]
            _GenericCatcher(std::convert::Infallible),
        }
    };

    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$renderer:ident>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$renderer>
        where
            $renderer: $crate::backend::renderer::Renderer,
        {
            $(
                $(
                    #[$meta]
                )*
                $body($field)
            ),*,
            #[doc(hidden)]
            _GenericCatcher((std::marker::PhantomData<$renderer>, std::convert::Infallible)),
        }
    };
    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$renderer:ident, $($custom:ident),+>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$renderer, $($custom),+>
        where
            $renderer: $crate::backend::renderer::Renderer,
            $(
                $custom: $crate::backend::renderer::element::RenderElement<$renderer>,
            )+
        {
            $(
                $(
                    #[$meta]
                )*
                $body($field)
            ),*,
            #[doc(hidden)]
            _GenericCatcher((std::marker::PhantomData<$renderer>, std::convert::Infallible)),
        }
    };
    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, $renderer:ident>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$lt, $renderer>
        where
            $renderer: $crate::backend::renderer::Renderer,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: 'static,
        {
            $(
                $(
                    #[$meta]
                )*
                $body($field)
            ),*,
            #[doc(hidden)]
            _GenericCatcher((std::marker::PhantomData<$renderer>, std::convert::Infallible)),
        }
    };
    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, $renderer:ident, $($custom:ident),+>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$lt, $renderer, $($custom),+>
        where
            $renderer: $crate::backend::renderer::Renderer,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: 'static,
            $(
                $custom: $crate::backend::renderer::element::RenderElement<$renderer>,
            )+
        {
            $(
                $(
                    #[$meta]
                )*
                $body($field)
            ),*,
            #[doc(hidden)]
            _GenericCatcher((std::marker::PhantomData<$renderer>, std::convert::Infallible)),
        }
    };
    (@call $renderer:ty; $name:ident; $($x:ident),*) => {
        $crate::backend::renderer::element::RenderElement::<$renderer>::$name($($x),*)
    };
    (@call $renderer:ty as $other:ty; draw; $x:ident, $renderer_ref:ident, $frame:ident, $($tail:ident),*) => {
        $crate::backend::renderer::element::RenderElement::<$other>::draw($x, $renderer_ref.as_mut(), $frame.as_mut(), $($tail),*).map_err(Into::into)
    };
    (@call $renderer:ty as $other:ty; $name:ident; $($x:ident),*) => {
        $crate::backend::renderer::element::RenderElement::<$other>::$name($($x),*)
    };
    (@body $renderer:ty; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        fn id(&self) -> &$crate::backend::renderer::element::Id {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::render_elements_internal!(@call $renderer $(as $other_renderer)?; id; x)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn location(&self, scale: $crate::utils::Scale<f64>) -> $crate::utils::Point<i32, $crate::utils::Physical> {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::render_elements_internal!(@call $renderer $(as $other_renderer)?; location; x, scale)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn src(&self) -> $crate::utils::Rectangle<f64, $crate::utils::Buffer> {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::render_elements_internal!(@call $renderer $(as $other_renderer)?; src; x)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn transform(&self) -> $crate::utils::Transform {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::render_elements_internal!(@call $renderer $(as $other_renderer)?; transform; x)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn geometry(&self, scale: $crate::utils::Scale<f64>) -> $crate::utils::Rectangle<i32, $crate::utils::Physical> {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::render_elements_internal!(@call $renderer $(as $other_renderer)?; geometry; x, scale)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn underlying_storage(&self, renderer: &$renderer) -> Option<$crate::backend::renderer::element::UnderlyingStorage<'_, $renderer>>
        {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::render_elements_internal!(@call $renderer $(as $other_renderer)?; underlying_storage; x, renderer)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn current_commit(&self) -> $crate::backend::renderer::utils::CommitCounter {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::render_elements_internal!(@call $renderer $(as $other_renderer)?; current_commit; x)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn damage_since(&self, scale: $crate::utils::Scale<f64>, commit: Option<$crate::backend::renderer::utils::CommitCounter>) -> Vec<$crate::utils::Rectangle<i32, $crate::utils::Physical>> {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::render_elements_internal!(@call $renderer $(as $other_renderer)?; damage_since; x, scale, commit)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn opaque_regions(&self, scale: $crate::utils::Scale<f64>) -> Vec<$crate::utils::Rectangle<i32, $crate::utils::Physical>> {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::render_elements_internal!(@call $renderer $(as $other_renderer)?; opaque_regions; x, scale)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
    };
    (@draw <$renderer:ty>; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        fn draw(
            &self,
            renderer: &mut $renderer,
            frame: &mut <$renderer as $crate::backend::renderer::Renderer>::Frame,
            location: $crate::utils::Point<i32, $crate::utils::Physical>,
            scale: $crate::utils::Scale<f64>,
            damage: &[$crate::utils::Rectangle<i32, $crate::utils::Physical>],
            log: &slog::Logger,
        ) -> Result<(), <$renderer as $crate::backend::renderer::Renderer>::Error>
        where
        $(
            $(
                $renderer: std::convert::AsMut<$other_renderer>,
                <$renderer as $crate::backend::renderer::Renderer>::Frame: std::convert::AsMut<<$other_renderer as $crate::backend::renderer::Renderer>::Frame>,
                <$other_renderer as $crate::backend::renderer::Renderer>::Error: Into<<$renderer as $crate::backend::renderer::Renderer>::Error>,
            )*
        )*
        {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::render_elements_internal!(@call $renderer $(as $other_renderer)?; draw; x, renderer, frame, location, scale, damage, log)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
    };
    (@draw $renderer:ty; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        fn draw(
            &self,
            renderer: &mut $renderer,
            frame: &mut <$renderer as $crate::backend::renderer::Renderer>::Frame,
            location: $crate::utils::Point<i32, $crate::utils::Physical>,
            scale: $crate::utils::Scale<f64>,
            damage: &[$crate::utils::Rectangle<i32, $crate::utils::Physical>],
            log: &slog::Logger,
        ) -> Result<(), <$renderer as $crate::backend::renderer::Renderer>::Error>
        {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::render_elements_internal!(@call $renderer $(as $other_renderer)?; draw; x, renderer, frame, location, scale, damage, log)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
    };
    // Generic renderer
    (@impl $name:ident<$renderer:ident> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        impl<$renderer> $crate::backend::renderer::element::RenderElement<$renderer> for $name<$renderer>
        where
            $renderer: $crate::backend::renderer::Renderer,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: 'static,
            $($($target: $bound $(+ $additional_bound)*),+)?
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };
    (@impl $name:ident<$lt:lifetime, $renderer:ident> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        impl<$lt, $renderer> $crate::backend::renderer::element::RenderElement<$renderer> for $name<$lt, $renderer>
        where
            $renderer: $crate::backend::renderer::Renderer,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: 'static,
            $($($target: $bound $(+ $additional_bound)*),+)?
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };
    (@impl $name:ident<$renderer:ident, $($custom:ident),+> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        impl<$renderer, $($custom),+> $crate::backend::renderer::element::RenderElement<$renderer> for $name<$renderer, $($custom),+>
        where
            $renderer: $crate::backend::renderer::Renderer,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: 'static,
            $(
                $custom: $crate::backend::renderer::element::RenderElement<$renderer>,
            )+
            $($($target: $bound $(+ $additional_bound)*),+)?
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };
    (@impl $name:ident<$lt:lifetime, $renderer:ident, $($custom:ident),+> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        impl<$lt, $renderer, $($custom),+> $crate::backend::renderer::element::RenderElement<$renderer> for $name<$lt, $renderer, $($custom),+>
        where
            $renderer: $crate::backend::renderer::Renderer,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: 'static,
            $(
                $custom: $crate::backend::renderer::element::RenderElement<$renderer>,
            )+
            $($($target: $bound $(+ $additional_bound)*),+)?
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };
    (@impl $name:ident; $renderer:ident; $($tail:tt)*) => {
        impl<$renderer> $crate::backend::renderer::element::RenderElement<$renderer> for $name
        where
            $renderer: $crate::backend::renderer::Renderer,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: 'static,
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };

    // Specific renderer
    (@impl $name:ident<=$renderer:ty>; $($tail:tt)*) => {
        impl $crate::backend::renderer::element::RenderElement<$renderer> for $name
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw $renderer; $($tail)*);
        }
    };
    (@impl $name:ident<=$renderer:ty, $($custom:ident),+> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        impl<$($custom),+> $crate::backend::renderer::element::RenderElement<$renderer> for $name<$($custom),+>
        where
        $(
            $custom: $crate::backend::renderer::element::RenderElement<$renderer>,
        )+
        $($($target: $bound $(+ $additional_bound)*),+)?
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw $renderer; $($tail)*);
        }
    };

    (@impl $name:ident<=$renderer:ty, $lt:lifetime>; $($tail:tt)*) => {
        impl<$lt> $crate::backend::renderer::element::RenderElement<$renderer> for $name<$lt>
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw $renderer; $($tail)*);
        }
    };

    (@impl $name:ident<=$renderer:ty, $lt:lifetime, $($custom:ident),+> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        impl<$lt, $($custom),+> $crate::backend::renderer::element::RenderElement<$renderer> for $name<$lt, $($custom),+>
        where
        $(
            $custom: $crate::backend::renderer::element::RenderElement<$renderer>,
        )+
        $($($target: $bound $(+ $additional_bound)*),+)?
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw $renderer; $($tail)*);
        }
    };


    (@from $name:ident<$renderer:ident>; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        $(
            $(
                #[$meta]
            )*
            impl<$renderer> From<$field> for $name<$renderer>
            where
                $renderer: $crate::backend::renderer::Renderer,
                $(
                    $($renderer: std::convert::AsMut<$other_renderer>,)?
                )*
            {
                fn from(field: $field) -> $name<$renderer> {
                    $name::$body(field)
                }
            }
        )*
    };
    (@from $name:ident<$renderer:ident, $custom:ident>; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        $(
            $(
                #[$meta]
            )*
            impl<$renderer, $custom> From<$field> for $name<$renderer, $custom>
            where
                $renderer: $crate::backend::renderer::Renderer,
                $custom: $crate::backend::renderer::element::RenderElement<$renderer>,
                $(
                    $($renderer: std::convert::AsMut<$other_renderer>,)?
                )*
            {
                fn from(field: $field) -> $name<$renderer, $custom> {
                    $name::$body(field)
                }
            }
        )*
    };
    (@from $name:ident<$lt:lifetime, $renderer:ident>; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        $(
            $(
                #[$meta]
            )*
            impl<$lt, $renderer> From<$field> for $name<$lt, $renderer>
            where
                $renderer: $crate::backend::renderer::Renderer,
                $(
                    $($renderer: std::convert::AsMut<$other_renderer>,)?
                )*
            {
                fn from(field: $field) -> $name<$lt, $renderer> {
                    $name::$body(field)
                }
            }
        )*
    };
    (@from $name:ident<$lt:lifetime, $renderer:ident, $custom:ident>; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        $(
            $(
                #[$meta]
            )*
            impl<$lt, $renderer, $custom> From<$field> for $name<$lt, $renderer, $custom>
            where
                $renderer: $crate::backend::renderer::Renderer,
                $custom: $crate::backend::renderer::element::RenderElement<$renderer>,
                $(
                    $($renderer: std::convert::AsMut<$other_renderer>,)?
                )*
            {
                fn from(field: $field) -> $name<$lt, $renderer, $custom> {
                    $name::$body(field)
                }
            }
        )*
    };
    (@from $name:ident; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        $(
            $(
                #[$meta]
            )*
            impl From<$field> for $name {
                fn from(field: $field) -> $name {
                    $name::$body(field)
                }
            }
        )*
    };
    (@from $name:ident<$lt:lifetime>; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        $(
            $(
                #[$meta]
            )*
            impl<$lt> From<$field> for $name<$lt> {
                fn from(field: $field) -> $name<$lt> {
                    $name::$body(field)
                }
            }
        )*
    };
}

/// Aggregate multiple types implementing [`RenderElement`] into a single enum type
///
/// ```
/// # use smithay::{
/// #     backend::renderer::{
/// #         element::{Id, RenderElement},
/// #         utils::CommitCounter,
/// #         Renderer,
/// #     },
/// #     utils::{Buffer, Point, Physical, Rectangle, Scale, Transform},
/// # };
/// #
/// # struct MyRenderElement1;
/// # struct MyRenderElement2;
/// #
/// # impl<R: Renderer> RenderElement<R> for MyRenderElement1 {
/// #     fn id(&self) -> &Id {
/// #         unimplemented!()
/// #     }
/// #
/// #     fn current_commit(&self) -> CommitCounter {
/// #         unimplemented!()
/// #     }
/// #
/// #     fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
/// #         unimplemented!()
/// #     }
/// #
/// #     fn src(&self) -> Rectangle<f64, Buffer> {
/// #         unimplemented!()
/// #     }
/// #
/// #     fn draw(
/// #         &self,
/// #         _renderer: &mut R,
/// #         _frame: &mut <R as Renderer>::Frame,
/// #         _location: Point<i32, Physical>,
/// #         _scale: Scale<f64>,
/// #         _damage: &[Rectangle<i32, Physical>],
/// #         _log: &slog::Logger,
/// #     ) -> Result<(), <R as Renderer>::Error> {
/// #         unimplemented!()
/// #     }
/// # }
/// #
/// # impl<R: Renderer> RenderElement<R> for MyRenderElement2 {
/// #     fn id(&self) -> &Id {
/// #         unimplemented!()
/// #     }
/// #
/// #     fn current_commit(&self) -> CommitCounter {
/// #         unimplemented!()
/// #     }
/// #
/// #     fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
/// #         unimplemented!()
/// #     }
/// #
/// #     fn src(&self) -> Rectangle<f64, Buffer> {
/// #         unimplemented!()
/// #     }
/// #
/// #     fn draw(
/// #         &self,
/// #         _renderer: &mut R,
/// #         _frame: &mut <R as Renderer>::Frame,
/// #         _location: Point<i32, Physical>,
/// #         _scale: Scale<f64>,
/// #         _damage: &[Rectangle<i32, Physical>],
/// #         _log: &slog::Logger,
/// #     ) -> Result<(), <R as Renderer>::Error> {
/// #         unimplemented!()
/// #     }
/// # }
/// use smithay::backend::renderer::element::render_elements;
///
/// render_elements! {
///     MyRenderElements;
///     First=MyRenderElement1,
///     Second=MyRenderElement2,
/// }
/// ```
///
/// If the [`RenderElement`] has special requirements on the [`Renderer`] you can
/// express them with a syntax similar to HRTBs.
///
/// For example the [`MemoryRenderBufferRenderElement`](crate::backend::renderer::element::memory::MemoryRenderBufferRenderElement) requires
/// the [`Renderer`] to implement the [`ImportMem`](crate::backend::renderer::ImportMem) trait.
///
/// ```
/// use smithay::backend::renderer::{
///     element::{memory::MemoryRenderBufferRenderElement, render_elements},
///     ImportMem,
/// };
///
/// render_elements! {
///     MyRenderElements<R> where R: ImportMem;
///     Memory=MemoryRenderBufferRenderElement,
/// }
/// ```
///
/// In case you want to use a reference or an element with an explicit lifetime the macro
/// additionally takes a lifetime on the defined enum.
///
/// ```
/// use smithay::backend::renderer::{
///     element::{memory::MemoryRenderBufferRenderElement, render_elements},
///     ImportMem,
/// };
///
/// render_elements! {
///     MyRenderElements<'a, R> where R: ImportMem;
///     Memory=&'a MemoryRenderBufferRenderElement,
/// }
/// ```
///
/// Additionally the macro can be used to define generic enums
///
/// ```
/// use smithay::backend::renderer::{
///     element::{memory::MemoryRenderBufferRenderElement, render_elements},
///     ImportMem,
/// };
///
/// render_elements! {
///     MyRenderElements<'a, R, A, B> where R: ImportMem;
///     Memory=&'a MemoryRenderBufferRenderElement,
///     Owned=A,
///     Borrowed=&'a B,
/// }
/// ```
///
/// If your elements require a specific [`Renderer`] instead of being
/// generic over it you can specify the type like in the following example.
///
/// ```
/// # use smithay::{
/// #     backend::renderer::{Frame, Renderer, Texture, TextureFilter},
/// #     utils::{Buffer, Physical, Rectangle, Size, Transform},
/// # };
/// #
/// # #[derive(Clone)]
/// # struct MyRendererTextureId;
/// #
/// # impl Texture for MyRendererTextureId {
/// #     fn width(&self) -> u32 {
/// #         unimplemented!()
/// #     }
/// #     fn height(&self) -> u32 {
/// #         unimplemented!()
/// #     }
/// # }
/// #
/// # struct MyRendererFrame;
/// #
/// # impl Frame for MyRendererFrame {
/// #     type Error = std::convert::Infallible;
/// #     type TextureId = MyRendererTextureId;
/// #
/// #     fn clear(&mut self, _: [f32; 4], _: &[Rectangle<i32, Physical>]) -> Result<(), Self::Error> {
/// #         unimplemented!()
/// #     }
/// #     fn render_texture_from_to(
/// #         &mut self,
/// #         _: &Self::TextureId,
/// #         _: Rectangle<f64, Buffer>,
/// #         _: Rectangle<i32, Physical>,
/// #         _: &[Rectangle<i32, Physical>],
/// #         _: Transform,
/// #         _: f32,
/// #     ) -> Result<(), Self::Error> {
/// #         unimplemented!()
/// #     }
/// #     fn transformation(&self) -> Transform {
/// #         unimplemented!()
/// #     }
/// # }
/// #
/// # struct MyRenderer;
/// #
/// # impl Renderer for MyRenderer {
/// #     type Error = std::convert::Infallible;
/// #     type TextureId = MyRendererTextureId;
/// #     type Frame = MyRendererFrame;
/// #
/// #     fn id(&self) -> usize {
/// #         unimplemented!()
/// #     }
/// #     fn downscale_filter(&mut self, _: TextureFilter) -> Result<(), Self::Error> {
/// #         unimplemented!()
/// #     }
/// #     fn upscale_filter(&mut self, _: TextureFilter) -> Result<(), Self::Error> {
/// #         unimplemented!()
/// #     }
/// #     fn render<F, R>(&mut self, _: Size<i32, Physical>, _: Transform, _: F) -> Result<R, Self::Error>
/// #     where
/// #         F: FnOnce(&mut Self, &mut Self::Frame) -> R,
/// #     {
/// #         unimplemented!()
/// #     }
/// # }
/// use smithay::backend::renderer::element::{render_elements, texture::TextureRenderElement};
///
/// render_elements! {
///     MyRenderElements<=MyRenderer>;
///     Texture=TextureRenderElement<MyRendererTextureId>,
/// }
/// ```
#[macro_export]
macro_rules! render_elements {
    ($(#[$attr:meta])* $vis:vis $name:ident<=$lt:lifetime, $renderer:ty, $custom:ident> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name $lt $custom; $($tail)*);
        $crate::render_elements_internal!(@impl $name<=$renderer, $lt, $custom> $(where $($target: $bound $(+ $additional_bound)*),+)?; $($tail)*);
        $crate::render_elements_internal!(@from $name<$lt, $custom>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<=$lt:lifetime, $renderer:ty, $($custom:ident),+> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name $lt $($custom)+; $($tail)*);
        $crate::render_elements_internal!(@impl $name<=$renderer, $lt, $($custom)+> $(where $($target: $bound $(+ $additional_bound)*),+)?; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<=$lt:lifetime, $renderer:ty>; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name $lt; $($tail)*);
        $crate::render_elements_internal!(@impl $name<=$renderer, $lt>; $($tail)*);
        $crate::render_elements_internal!(@from $name<$lt>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<=$renderer:ty, $custom:ident> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name $custom; $($tail)*);
        $crate::render_elements_internal!(@impl $name<=$renderer, $custom> $(where $($target: $bound $(+ $additional_bound)*),+)?; $($tail)*);
        $crate::render_elements_internal!(@from $name<$custom>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<=$renderer:ty, $($custom:ident),+> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name $custom1 $custom2; $($tail)*);
        $crate::render_elements_internal!(@impl $name<=$renderer, $($custom),+> $(where $($target: $bound $(+ $additional_bound)*),+)?; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<=$renderer:ty>; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name; $($tail)*);
        $crate::render_elements_internal!(@impl $name<=$renderer>; $($tail)*);
        $crate::render_elements_internal!(@from $name; $($tail)*);
    };

    ($(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, $renderer:ident, $custom:ident> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name<$lt, $renderer, $custom>; $($tail)*);
        $crate::render_elements_internal!(@impl $name<$lt, $renderer, $custom> $(where $($target: $bound $(+ $additional_bound)*),+)?; $($tail)*);
        $crate::render_elements_internal!(@from $name<$lt, $renderer, $custom>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, $renderer:ident, $($custom:ident),+> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name<$lt, $renderer, $($custom),+>; $($tail)*);
        $crate::render_elements_internal!(@impl $name<$lt, $renderer, $($custom),+> $(where $($target: $bound $(+ $additional_bound)*),+)?; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, $renderer:ident> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name<$lt, $renderer>; $($tail)*);
        $crate::render_elements_internal!(@impl $name<$lt, $renderer> $(where $($target: $bound $(+ $additional_bound)*),+)?; $($tail)*);
        $crate::render_elements_internal!(@from $name<$lt, $renderer>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<$renderer:ident, $custom:ident> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name<$renderer, $custom>; $($tail)*);
        $crate::render_elements_internal!(@impl $name<$renderer, $custom> $(where $($target: $bound $(+ $additional_bound)*),+)?; $($tail)*);
        $crate::render_elements_internal!(@from $name<$renderer, $custom>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<$renderer:ident, $($custom:ident),+> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name<$renderer, $($custom),+>; $($tail)*);
        $crate::render_elements_internal!(@impl $name<$renderer, $($custom),+> $(where $($target: $bound $(+ $additional_bound)*),+)?; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<$renderer:ident> $(where $($target:ty: $bound:tt $(+ $additional_bound:tt)*),+)?; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name<$renderer>; $($tail)*);
        $crate::render_elements_internal!(@impl $name<$renderer> $(where $($target: $bound $(+ $additional_bound)*),+)?; $($tail)*);
        $crate::render_elements_internal!(@from $name<$renderer>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name; $($tail)*);
        $crate::render_elements_internal!(@impl $name; R; $($tail)*);
        $crate::render_elements_internal!(@from $name; $($tail)*);
    };
}

pub use render_elements;

/// New-type wrapper for wrapping owned elements
/// in render_elements!
#[derive(Debug)]
pub struct Wrap<C>(C);

impl<C> From<C> for Wrap<C> {
    fn from(from: C) -> Self {
        Self(from)
    }
}

impl<R, C> RenderElement<R> for Wrap<C>
where
    R: Renderer,
    C: RenderElement<R>,
{
    fn id(&self) -> &Id {
        self.0.id()
    }

    fn current_commit(&self) -> CommitCounter {
        self.0.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.0.location(scale)
    }

    fn src(&self) -> Rectangle<f64, BufferCoords> {
        self.0.src()
    }

    fn transform(&self) -> Transform {
        self.0.transform()
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.0.geometry(scale)
    }

    fn draw(
        &self,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        damage: &[Rectangle<i32, Physical>],
        log: &slog::Logger,
    ) -> Result<(), <R as Renderer>::Error> {
        self.0.draw(renderer, frame, location, scale, damage, log)
    }

    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> Vec<Rectangle<i32, Physical>> {
        self.0.damage_since(scale, commit)
    }

    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>> {
        self.0.opaque_regions(scale)
    }

    fn underlying_storage(&self, renderer: &R) -> Option<UnderlyingStorage<'_, R>> {
        self.0.underlying_storage(renderer)
    }
}

#[cfg(all(test, feature = "renderer_gl"))]
#[allow(dead_code)]
mod tests;
