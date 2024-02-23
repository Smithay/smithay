use std::hash::Hash;

#[cfg(feature = "wayland_frontend")]
use crate::{
    backend::renderer::{element::surface::WaylandSurfaceRenderElement, ImportAll},
    desktop::LayerSurface,
    wayland::shell::wlr_layer::Layer,
};
use crate::{
    backend::renderer::{
        element::{AsRenderElements, Wrap},
        Renderer, Texture,
    },
    output::Output,
    utils::{IsAlive, Logical, Physical, Point, Rectangle, Scale},
};

#[cfg(feature = "wayland_frontend")]
mod wayland;
#[cfg(feature = "wayland_frontend")]
pub use self::wayland::SurfaceTree;

/// Indicates default values for some zindexs inside smithay
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum RenderZindex {
    /// WlrLayer::Background default zindex
    Background = 10,
    /// WlrLayer::Bottom default zindex
    Bottom = 20,
    /// Default zindex for Windows
    Shell = 30,
    /// WlrLayer::Top default zindex
    Top = 40,
    /// Default Layer for RenderElements
    Overlay = 60,
}

impl From<RenderZindex> for u8 {
    fn from(idx: RenderZindex) -> u8 {
        idx as u8
    }
}

impl From<RenderZindex> for Option<u8> {
    fn from(idx: RenderZindex) -> Option<u8> {
        Some(idx as u8)
    }
}

/// Element mappable onto a [`Space`](super::Space)
pub trait SpaceElement: IsAlive {
    /// Returns the geometry of this element.
    ///
    /// Defaults to be equal to it's bounding box.
    fn geometry(&self) -> Rectangle<i32, Logical> {
        self.bbox()
    }
    /// Returns the bounding box of this element
    fn bbox(&self) -> Rectangle<i32, Logical>;
    /// Returns whenever a given point inside this element will be able to receive input
    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool;
    /// Gets the z-index of this element
    fn z_index(&self) -> u8 {
        RenderZindex::Overlay as u8
    }

    /// Set the rendered state to activated, if applicable to this element
    fn set_activate(&self, activated: bool);
    /// The element is displayed on a given output.
    ///
    /// Maybe called for an already entered output,
    /// if the overlap changes
    fn output_enter(&self, output: &Output, overlap: Rectangle<i32, Logical>);
    /// The element left a given output
    fn output_leave(&self, output: &Output);
    /// Periodically called to update internal state, if necessary
    fn refresh(&self) {}
}

impl<T: SpaceElement> SpaceElement for &T {
    fn geometry(&self) -> Rectangle<i32, Logical> {
        SpaceElement::geometry(*self)
    }
    fn bbox(&self) -> Rectangle<i32, Logical> {
        SpaceElement::bbox(*self)
    }
    fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool {
        SpaceElement::is_in_input_region(*self, point)
    }
    fn z_index(&self) -> u8 {
        SpaceElement::z_index(*self)
    }

    fn set_activate(&self, activated: bool) {
        SpaceElement::set_activate(*self, activated)
    }
    fn output_enter(&self, output: &Output, overlap: Rectangle<i32, Logical>) {
        SpaceElement::output_enter(*self, output, overlap)
    }
    fn output_leave(&self, output: &Output) {
        SpaceElement::output_leave(*self, output)
    }
    fn refresh(&self) {
        SpaceElement::refresh(*self)
    }
}

#[derive(Debug)]
pub(super) enum SpaceElements<'a, E> {
    #[cfg(feature = "wayland_frontend")]
    Layer {
        surface: LayerSurface,
        output_location: Point<i32, Logical>,
    },
    Element(&'a InnerElement<E>),
}

impl<'a, E> SpaceElements<'a, E>
where
    E: SpaceElement,
{
    pub(super) fn z_index(&self) -> u8 {
        match self {
            #[cfg(feature = "wayland_frontend")]
            SpaceElements::Layer { surface, .. } => {
                let layer = match surface.layer() {
                    Layer::Background => RenderZindex::Background,
                    Layer::Bottom => RenderZindex::Bottom,
                    Layer::Top => RenderZindex::Top,
                    Layer::Overlay => RenderZindex::Overlay,
                };
                layer as u8
            }
            SpaceElements::Element(inner) => inner.element.z_index(),
        }
    }

    pub(super) fn bbox(&self) -> Rectangle<i32, Logical> {
        match self {
            #[cfg(feature = "wayland_frontend")]
            SpaceElements::Layer {
                surface,
                output_location,
            } => {
                let mut bbox = surface.bbox();
                bbox.loc += *output_location;
                bbox
            }
            SpaceElements::Element(inner) => inner.bbox(),
        }
    }

    pub(super) fn render_location(&self) -> Point<i32, Logical> {
        match self {
            #[cfg(feature = "wayland_frontend")]
            SpaceElements::Layer { .. } => self.bbox().loc,
            SpaceElements::Element(inner) => inner.render_location(),
        }
    }
}

impl<
        'a,
        #[cfg(feature = "wayland_frontend")] R: Renderer + ImportAll,
        #[cfg(not(feature = "wayland_frontend"))] R: Renderer,
        E: AsRenderElements<R>,
    > AsRenderElements<R> for SpaceElements<'a, E>
where
    <R as Renderer>::TextureId: Texture + 'static,
    <E as AsRenderElements<R>>::RenderElement: 'a,
    SpaceRenderElements<R, <E as AsRenderElements<R>>::RenderElement>:
        From<Wrap<<E as AsRenderElements<R>>::RenderElement>>,
{
    type RenderElement = SpaceRenderElements<R, <E as AsRenderElements<R>>::RenderElement>;

    #[profiling::function]
    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        renderer: &mut R,
        location: Point<i32, Physical>,
        scale: Scale<f64>,
        alpha: f32,
    ) -> Vec<C> {
        match &self {
            #[cfg(feature = "wayland_frontend")]
            SpaceElements::Layer { surface, .. } => AsRenderElements::<R>::render_elements::<
                WaylandSurfaceRenderElement<R>,
            >(surface, renderer, location, scale, alpha)
            .into_iter()
            .map(SpaceRenderElements::Surface)
            .map(C::from)
            .collect(),
            SpaceElements::Element(element) => element
                .element
                .render_elements::<Wrap<<E as AsRenderElements<R>>::RenderElement>>(
                    renderer, location, scale, alpha,
                )
                .into_iter()
                .map(SpaceRenderElements::Element)
                .map(C::from)
                .collect(),
        }
    }
}

#[macro_export]
#[doc(hidden)]
macro_rules! space_elements_internal {
    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, $custom:ident>; $($(#[$meta:meta])* $body:ident=$field:ty),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$lt, $custom> {
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
    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$custom:ident>; $($(#[$meta:meta])* $body:ident=$field:ty),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$custom> {
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
    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime>; $($(#[$meta:meta])* $body:ident=$field:ty),* $(,)?) => {
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
    (@enum $(#[$attr:meta])* $vis:vis $name:ident; $($(#[$meta:meta])* $body:ident=$field:ty),* $(,)?) => {
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
    (@call $name:ident; $($x:ident),*) => {
        $crate::desktop::space::SpaceElement::$name($($x),*)
    };
    (@alive $($(#[$meta:meta])* $body:ident=$field:ty),* $(,)?) => {
        fn alive(&self) -> bool {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::utils::IsAlive::alive(x)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
    };
    (@body $($(#[$meta:meta])* $body:ident=$field:ty),* $(,)?) => {
        fn geometry(&self) -> $crate::utils::Rectangle<i32, $crate::utils::Logical> {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::space_elements_internal!(@call geometry; x)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
        fn bbox(&self) -> $crate::utils::Rectangle<i32, $crate::utils::Logical> {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::space_elements_internal!(@call bbox; x)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
        fn is_in_input_region(&self, point: &$crate::utils::Point<f64, $crate::utils::Logical>) -> bool {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::space_elements_internal!(@call is_in_input_region; x, point)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn z_index(&self) -> u8 {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::space_elements_internal!(@call z_index; x)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn set_activate(&self, activated: bool) {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::space_elements_internal!(@call set_activate; x, activated)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
        fn output_enter(&self, output: &$crate::output::Output, overlap: $crate::utils::Rectangle<i32, $crate::utils::Logical>) {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::space_elements_internal!(@call output_enter; x, output, overlap)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
        fn output_leave(&self, output: &$crate::output::Output) {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::space_elements_internal!(@call output_leave; x, output)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
        fn refresh(&self) {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::space_elements_internal!(@call refresh; x)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
    };
    (@impl $name:ident<$lt:lifetime>; $($tail:tt)*) => {
        impl<$lt> $crate::desktop::space::SpaceElement for $name<$lt>
        {
            $crate::space_elements_internal!(@body $($tail)*);
        }
        impl<$lt> $crate::utils::IsAlive for $name<$lt>
        {
            $crate::space_elements_internal!(@alive $($tail)*);
        }
    };
    (@impl $name:ident<$lt:lifetime, $custom:ident>; $($tail:tt)*) => {
        impl<$lt, $custom> $crate::desktop::space::SpaceElement for $name<$lt, $custom>
        where
            $custom: $crate::desktop::space::SpaceElement,
        {
            $crate::space_elements_internal!(@body $($tail)*);
        }
        impl<$lt, $custom> $crate::utils::IsAlive for $name<$lt, $custom>
        where
            $custom: $crate::utils::IsAlive,
        {
            $crate::space_elements_internal!(@alive $($tail)*);
        }
    };
    (@impl $name:ident<$custom:ident>; $($tail:tt)*) => {
        impl<$custom> $crate::desktop::space::SpaceElement for $name<$custom>
        where
            $custom: $crate::desktop::space::SpaceElement,
        {
            $crate::space_elements_internal!(@body $($tail)*);
        }
        impl<$custom> $crate::utils::IsAlive for $name<$custom>
        where
            $custom: $crate::utils::IsAlive,
        {
            $crate::space_elements_internal!(@alive $($tail)*);
        }
    };
    (@impl $name:ident; $($tail:tt)*) => {
        impl $crate::desktop::space::SpaceElement for $name
        {
            $crate::space_elements_internal!(@body $($tail)*);
        }
        impl $crate::utils::IsAlive for $name
        {
            $crate::space_elements_internal!(@alive $($tail)*);
        }
    };
}

/// Aggregate multiple types implementing [`SpaceElement`] into a single enum type to be used
/// with a [`Space`].
///
/// # Example
///
/// ```
/// use smithay::desktop::space::space_elements;
/// # use smithay::{desktop::space::SpaceElement, output::Output, utils::{Point, Rectangle, Logical, IsAlive}};
/// # struct Window;
/// # impl SpaceElement for Window {
/// #     fn bbox(&self) -> Rectangle<i32, Logical> { unimplemented!() }
/// #     fn is_in_input_region(&self, point: &Point<f64, Logical>) -> bool { unimplemented!() }
/// #     fn set_activate(&self, activated: bool) {}
/// #     fn output_enter(&self, output: &Output, overlap: Rectangle<i32, Logical>) {}
/// #     fn output_leave(&self, output: &Output) {}
/// # }
/// # impl IsAlive for Window {
/// #     fn alive(&self) -> bool { unimplemented!() }
/// # }
///
/// space_elements! {
///     /// Name of the type
///     pub MySpaceElements<'a, C>; // can be generic if necessary
///     Window=Window, // variant name = type
///     Custom=&'a C, // also supports references or usage of generic types
/// }
/// ```
#[macro_export]
macro_rules! space_elements {
    ($(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, $custom:ident>; $($tail:tt)*) => {
        $crate::space_elements_internal!(@enum $(#[$attr])* $vis $name<$lt, $custom>; $($tail)*);
        $crate::space_elements_internal!(@impl $name<$lt, $custom>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<$custom:ident>; $($tail:tt)*) => {
        $crate::space_elements_internal!(@enum $(#[$attr])* $vis $name<$custom>; $($tail)*);
        $crate::space_elements_internal!(@impl $name<$custom>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime>; $($tail:tt)*) => {
        $crate::space_elements_internal!(@enum $(#[$attr])* $vis $name<$lt>; $($tail)*);
        $crate::space_elements_internal!(@impl $name<$lt>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident; $($tail:tt)*) => {
        $crate::space_elements_internal!(@enum $(#[$attr])* $vis $name; $($tail)*);
        $crate::space_elements_internal!(@impl $name; $($tail)*);
    };
}

pub use space_elements;

use super::{InnerElement, SpaceRenderElements};

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    use crate::{
        desktop::space::SpaceElement,
        output::Output,
        utils::{IsAlive, Logical, Point, Rectangle},
    };

    pub struct TestElement;
    impl SpaceElement for TestElement {
        fn bbox(&self) -> Rectangle<i32, Logical> {
            unimplemented!()
        }
        fn is_in_input_region(&self, _point: &Point<f64, Logical>) -> bool {
            unimplemented!()
        }
        fn set_activate(&self, _activated: bool) {}
        fn output_enter(&self, _output: &Output, _overlap: Rectangle<i32, Logical>) {}
        fn output_leave(&self, _output: &Output) {}
    }
    impl IsAlive for TestElement {
        fn alive(&self) -> bool {
            unimplemented!()
        }
    }

    space_elements! {
        /// Some test space elements
        pub TestSpaceElements;
        /// A complete surface tree
        Window=TestElement,
    }

    space_elements! {
        /// Some test space elements
        pub TestSpaceElements2<'a>;
        /// A complete surface tree
        Window=&'a TestElement,
    }

    space_elements! {
        /// Some test space elements
        pub TestSpaceElements5<'a, C>;
        /// A complete surface tree
        Window=TestElement,
        Custom=&'a C,
    }

    space_elements! {
        /// Some test space elements
        pub TestSpaceElements7<C>;
        /// A complete surface tree
        Window=TestElement,
        Custom=C,
    }
}
