use crate::{
    backend::renderer::{element::surface::render_elements_from_surface_tree, ImportAll, Renderer},
    desktop::{space::*, utils::bbox_from_surface_tree},
    output::Output,
    utils::{Logical, Physical, Point, Rectangle, Scale},
};
use std::hash::Hash;

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

/// Trait for a space element
pub trait SpaceElement<R, E>
where
    R: Renderer + ImportAll,
    E: RenderElement<R>,
{
    /// Gets the location of this element on the specified space
    fn location(&self, space_id: usize) -> Point<i32, Logical>;
    /// Gets the geometry of this element on the specified space
    fn geometry(&self, space_id: usize) -> Rectangle<i32, Logical>;
    /// Gets the z-index of this element on the specified space
    fn z_index(&self, _space_id: usize) -> u8 {
        RenderZindex::Overlay as u8
    }
    /// Gets render elements of this space element
    fn render_elements(&self, location: Point<i32, Physical>, scale: Scale<f64>) -> Vec<E>;
}

space_elements! {
    pub(crate) SpaceElements<'a, _, C>[
        WaylandSurfaceRenderElement,
        TextureRenderElement<<R as Renderer>::TextureId>,
    ];
    Layer=&'a LayerSurface,
    Window=&'a Window,
    Custom=&'a C,
}

impl<T, R, E> SpaceElement<R, E> for &T
where
    T: SpaceElement<R, E>,
    E: RenderElement<R>,
    R: Renderer + ImportAll,
{
    fn location(&self, space_id: usize) -> Point<i32, Logical> {
        (*self).location(space_id)
    }

    fn geometry(&self, space_id: usize) -> Rectangle<i32, Logical> {
        (*self).geometry(space_id)
    }

    fn z_index(&self, space_id: usize) -> u8 {
        (*self).z_index(space_id)
    }

    fn render_elements(&self, location: Point<i32, Physical>, scale: Scale<f64>) -> Vec<E> {
        (*self).render_elements(location, scale)
    }
}
/// A custom surface tree
#[derive(Debug)]
pub struct SurfaceTree {
    location: Point<i32, Logical>,
    surface: WlSurface,
}

impl SurfaceTree {
    /// Create a surface tree from a surface
    pub fn from_surface(surface: &WlSurface, location: impl Into<Point<i32, Logical>>) -> Self {
        SurfaceTree {
            location: location.into(),
            surface: surface.clone(),
        }
    }
}

impl<R, E> SpaceElement<R, E> for SurfaceTree
where
    R: Renderer + ImportAll,
    E: RenderElement<R> + From<WaylandSurfaceRenderElement>,
{
    fn location(&self, _space_id: usize) -> Point<i32, Logical> {
        self.location
    }

    fn geometry(&self, _space_id: usize) -> Rectangle<i32, Logical> {
        bbox_from_surface_tree(&self.surface, self.location)
    }

    fn render_elements(&self, location: Point<i32, Physical>, scale: Scale<f64>) -> Vec<E> {
        render_elements_from_surface_tree(&self.surface, location, scale)
    }
}

#[macro_export]
#[doc(hidden)]
macro_rules! space_elements_internal {
    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, _, $custom:ident>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
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
    (@enum $(#[$attr:meta])* $vis:vis $name:ident<_, $custom:ident>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
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
    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, $renderer:ident, $custom:ident>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$lt, $renderer, $custom>
        where $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll, {
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
    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, $renderer:ident>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$lt, $renderer>
        where $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll, {
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
    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
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
    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$renderer:ident>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$renderer>
        where $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll, {
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
    (@call $renderer:ident $render_element:ident; $name:ident; $($x:ident),*) => {
        $crate::desktop::space::SpaceElement::<$renderer, $render_element>::$name($($x),*)
    };
    (@body $renderer:ident $render_element:ident; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        fn location(&self, space_id: usize) -> $crate::utils::Point<i32, $crate::utils::Logical> {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::space_elements_internal!(@call $renderer $render_element; location; x, space_id)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn geometry(&self, space_id: usize) -> $crate::utils::Rectangle<i32, $crate::utils::Logical> {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::space_elements_internal!(@call $renderer $render_element; geometry; x, space_id)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn z_index(&self, space_id: usize) -> u8 {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::space_elements_internal!(@call $renderer $render_element; z_index; x, space_id)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }

        fn render_elements(&self, location: $crate::utils::Point<i32, $crate::utils::Physical>, scale: $crate::utils::Scale<f64>) -> Vec<$render_element> {
            match self {
                $(
                    #[allow(unused_doc_comments)]
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::space_elements_internal!(@call $renderer $render_element; render_elements; x, location, scale)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
    };
    (@impl $name:ident<$lt:lifetime, $renderer:ident, $custom:ident>; $render_element:ident; $($what:ty$(,)?)+; $($tail:tt)*) => {
        impl<$lt, $renderer, $render_element, $custom> $crate::desktop::space::SpaceElement<$renderer, $render_element> for $name<$lt, $renderer, $custom>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: Clone + 'static,
            $custom: $crate::desktop::space::SpaceElement<$renderer, $render_element>,
            $render_element: $crate::backend::renderer::element::RenderElement<$renderer> $(+ From<$what>)*,
        {
            $crate::space_elements_internal!(@body $renderer $render_element; $($tail)*);
        }
    };
    (@impl $name:ident<$lt:lifetime, $renderer:ident>; $render_element:ident; $($what:ty$(,)?)+; $($tail:tt)*) => {
        impl<$lt, $renderer, $render_element> $crate::desktop::space::SpaceElement<$renderer, $render_element> for $name<$lt, $renderer>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: Clone + 'static,
            $render_element: $crate::backend::renderer::element::RenderElement<$renderer> $(+ From<$what>)*,
        {
            $crate::space_elements_internal!(@body $renderer $render_element; $($tail)*);
        }
    };
    (@impl $name:ident<$lt:lifetime>; $renderer:ident $render_element:ident; $($what:ty$(,)?)+; $($tail:tt)*) => {
        impl<$lt, $renderer, $render_element> $crate::desktop::space::SpaceElement<$renderer, $render_element> for $name<$lt>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: Clone + 'static,
            $render_element: $crate::backend::renderer::element::RenderElement<$renderer> $(+ From<$what>)*,
        {
            $crate::space_elements_internal!(@body $renderer $render_element; $($tail)*);
        }
    };
    (@impl $name:ident<$renderer:ident>; $render_element:ident; $($what:ty$(,)?)+; $($tail:tt)*) => {
        impl<$renderer, $render_element> $crate::desktop::space::SpaceElement<$renderer, $render_element> for $name<$renderer>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: Clone + 'static,
            $render_element: $crate::backend::renderer::element::RenderElement<$renderer> $(+ From<$what>)*,
        {
            $crate::space_elements_internal!(@body $renderer $render_element; $($tail)*);
        }
    };
    (@impl $name:ident<$lt:lifetime, $custom:ident>; $renderer:ident $render_element:ident; $($what:ty$(,)?)+;$($tail:tt)*) => {
        impl<$lt, $renderer, $render_element, $custom> $crate::desktop::space::SpaceElement<$renderer, $render_element> for $name<$lt, $custom>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: Clone + 'static,
            $custom: $crate::desktop::space::SpaceElement<$renderer, $render_element>,
            $render_element: $crate::backend::renderer::element::RenderElement<$renderer> $(+ From<$what>)*,
        {
            $crate::space_elements_internal!(@body $renderer $render_element; $($tail)*);
        }
    };
    (@impl $name:ident<$custom:ident>; $renderer:ident $render_element:ident; $($what:ty$(,)?)+;$($tail:tt)*) => {
        impl<$renderer, $render_element, $custom> $crate::desktop::space::SpaceElement<$renderer, $render_element> for $name<$custom>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: Clone + 'static,
            $custom: $crate::desktop::space::SpaceElement<$renderer, $render_element>,
            $render_element: $crate::backend::renderer::element::RenderElement<$renderer> $(+ From<$what>)*,
        {
            $crate::space_elements_internal!(@body $renderer $render_element; $($tail)*);
        }
    };
    (@impl $name:ident; $renderer:ident $render_element:ident; $($what:ty$(,)?)+;$($tail:tt)*) => {
        impl<$renderer, $render_element> $crate::desktop::space::SpaceElement<$renderer, $render_element> for $name
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: Clone + 'static,
            $render_element: $crate::backend::renderer::element::RenderElement<$renderer> $(+ From<$what>)*,
        {
            $crate::space_elements_internal!(@body $renderer $render_element; $($tail)*);
        }
    };
}

/// TODO: Docs
#[macro_export]
macro_rules! space_elements {
    ($(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, _, $custom:ident>[$($what:ty$(,)?)+]; $($tail:tt)*) => {
        $crate::space_elements_internal!(@enum $(#[$attr])* $vis $name<$lt, _, $custom>; $($tail)*);
        $crate::space_elements_internal!(@impl $name<$lt, $custom>; R E; $($what)*; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<_, $custom:ident>[$($what:ty$(,)?)+]; $($tail:tt)*) => {
        $crate::space_elements_internal!(@enum $(#[$attr])* $vis $name<_, $custom>; $($tail)*);
        $crate::space_elements_internal!(@impl $name<$custom>; R E; $($what)*; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, $renderer:ident, $custom:ident>[$($what:ty$(,)?)+]; $($tail:tt)*) => {
        $crate::space_elements_internal!(@enum $(#[$attr])* $vis $name<$lt, $renderer, $custom>; $($tail)*);
        $crate::space_elements_internal!(@impl $name<$lt, $renderer, $custom>; E; $($what)*; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, $renderer:ident>[$($what:ty$(,)?)+]; $($tail:tt)*) => {
        $crate::space_elements_internal!(@enum $(#[$attr])* $vis $name<$lt, $renderer>; $($tail)*);
        $crate::space_elements_internal!(@impl $name<$lt, $renderer>; E; $($what)*; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime>[$($what:ty$(,)?)+]; $($tail:tt)*) => {
        $crate::space_elements_internal!(@enum $(#[$attr])* $vis $name<$lt>; $($tail)*);
        $crate::space_elements_internal!(@impl $name<$lt>; R E; $($what)*; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<$renderer:ident>[$($what:ty$(,)?)+]; $($tail:tt)*) => {
        $crate::space_elements_internal!(@enum $(#[$attr])* $vis $name<$renderer>; $($tail)*);
        $crate::space_elements_internal!(@impl $name<$renderer>; E; $($what)*; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident[$($what:ty$(,)?)+]; $($tail:tt)*) => {
        $crate::space_elements_internal!(@enum $(#[$attr])* $vis $name; $($tail)*);
        $crate::space_elements_internal!(@impl $name; R E; $($what)*; $($tail)*);
    };
}

pub(self) use space_elements;

#[cfg(test)]
#[allow(dead_code)]
mod tests {
    use crate::{
        backend::renderer::{
            element::{surface::WaylandSurfaceRenderElement, texture::TextureRenderElement, RenderElement},
            ImportAll, Renderer, Texture,
        },
        desktop::{LayerSurface, Window},
        utils::{Logical, Physical, Point, Rectangle, Scale},
    };

    use super::{SpaceElement, SurfaceTree};

    space_elements! {
        /// Some test space elements
        pub TestSpaceElements[
            WaylandSurfaceRenderElement,
            TextureRenderElement<<R as Renderer>::TextureId>,
        ];
        /// A complete surface tree
        SurfaceTree=SurfaceTree,
    }

    space_elements! {
        /// Some test space elements
        pub TestSpaceElements2<'a>[
            WaylandSurfaceRenderElement,
            TextureRenderElement<<R as Renderer>::TextureId>,
        ];
        /// A complete surface tree
        Window=&'a Window,
        /// A layer surface
        LayerSurface=&'a LayerSurface,
    }

    space_elements! {
        /// Some test space elements
        pub TestSpaceElements3<R>[
            WaylandSurfaceRenderElement,
            TextureRenderElement<<R as Renderer>::TextureId>,
        ];
        /// A complete surface tree
        SurfaceTree=SurfaceTree,
        Something=SomeSpaceElement<<R as Renderer>::TextureId>,
    }

    space_elements! {
        /// Some test space elements
        pub TestSpaceElements4<'a, R>[
            WaylandSurfaceRenderElement,
            TextureRenderElement<<R as Renderer>::TextureId>,
        ];
        /// A complete surface tree
        SurfaceTree=SurfaceTree,
        Something=&'a SomeSpaceElement<<R as Renderer>::TextureId>,
    }

    space_elements! {
        /// Some test space elements
        pub TestSpaceElements5<'a, R, C>[
            WaylandSurfaceRenderElement,
            TextureRenderElement<<R as Renderer>::TextureId>,
        ];
        /// A complete surface tree
        SurfaceTree=SurfaceTree,
        Something=&'a SomeSpaceElement<<R as Renderer>::TextureId>,
        Custom=&'a C,
    }

    space_elements! {
        /// Some test space elements
        pub TestSpaceElements6<'a, _, C>[
            WaylandSurfaceRenderElement,
            TextureRenderElement<<R as Renderer>::TextureId>,
        ];
        /// A complete surface tree
        SurfaceTree=SurfaceTree,
        Custom=&'a C,
    }

    space_elements! {
        /// Some test space elements
        pub TestSpaceElements7<_, C>[
            WaylandSurfaceRenderElement,
            TextureRenderElement<<R as Renderer>::TextureId>,
        ];
        /// A complete surface tree
        SurfaceTree=SurfaceTree,
        Custom=C,
    }

    pub struct SomeSpaceElement<T: Texture> {
        _texture: T,
    }

    impl<T, R, E> SpaceElement<R, E> for SomeSpaceElement<T>
    where
        T: Texture,
        R: Renderer<TextureId = T> + ImportAll,
        E: RenderElement<R>,
    {
        fn location(&self, _space_id: usize) -> Point<i32, Logical> {
            todo!()
        }

        fn geometry(&self, _space_id: usize) -> Rectangle<i32, Logical> {
            todo!()
        }

        fn render_elements(&self, _location: Point<i32, Physical>, _scale: Scale<f64>) -> Vec<E> {
            todo!()
        }
    }
}
