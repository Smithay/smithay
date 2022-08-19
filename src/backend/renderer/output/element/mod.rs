//! TODO: Docs

use crate::{
    backend::renderer::Renderer,
    utils::{Physical, Point, Rectangle, Scale},
};

mod id;

pub use id::Id;
use wayland_server::protocol::wl_buffer;

pub mod surface;
pub mod texture;

/// The underlying storage for a element
#[derive(Debug)]
pub enum UnderlyingStorage<'a, R: Renderer> {
    /// A wayland buffer
    Wayland(wl_buffer::WlBuffer),
    /// A texture
    External(&'a R::TextureId),
}

/// A single render element
pub trait RenderElement<R: Renderer> {
    /// Get the unique id of this element
    fn id(&self) -> &Id;
    /// Get the current commit position of this element
    fn current_commit(&self) -> usize;
    /// Get the location relative to the output
    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.geometry(scale).loc
    }
    /// Get the geometry relative to the output
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical>;
    /// Get the damage since the provided commit relative to the element
    fn damage_since(&self, scale: Scale<f64>, commit: Option<usize>) -> Vec<Rectangle<i32, Physical>> {
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
        scale: Scale<f64>,
        damage: &[Rectangle<i32, Physical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error>;
}

impl<R, E> RenderElement<R> for &E
where
    R: Renderer,
    E: RenderElement<R>,
{
    fn id(&self) -> &Id {
        (*self).id()
    }

    fn current_commit(&self) -> usize {
        (*self).current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        (*self).location(scale)
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        (*self).geometry(scale)
    }

    fn damage_since(&self, scale: Scale<f64>, commit: Option<usize>) -> Vec<Rectangle<i32, Physical>> {
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
        scale: Scale<f64>,
        damage: &[Rectangle<i32, Physical>],
        log: &slog::Logger,
    ) -> Result<(), R::Error> {
        (*self).draw(renderer, frame, scale, damage, log)
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
    (@enum $(#[$attr:meta])* $vis:vis $name:ident $custom:ident; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
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
    (@enum $(#[$attr:meta])* $vis:vis $name:ident $lt:lifetime $custom:ident; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
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

    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$renderer:ident>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$renderer>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
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
    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$renderer:ident, $custom:ident>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$renderer, $custom>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
            $custom: $crate::backend::renderer::output::element::RenderElement<$renderer>,
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
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
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
    (@enum $(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, $renderer:ident, $custom:ident>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
        $(#[$attr])*
        $vis enum $name<$lt, $renderer, $custom>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
            <$renderer as $crate::backend::renderer::Renderer>::TextureId: 'static,
            $custom: $crate::backend::renderer::output::element::RenderElement<$renderer>,
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
        $crate::backend::renderer::output::element::RenderElement::<$renderer>::$name($($x),*)
    };
    (@call $renderer:ty as $other:ty; draw; $x:ident, $renderer_ref:ident, $frame:ident, $($tail:ident),*) => {
        $crate::backend::renderer::output::element::RenderElement::<$other>::draw($x, $renderer_ref.as_mut(), $frame.as_mut(), $($tail),*).map_err(Into::into)
    };
    (@call $renderer:ty as $other:ty; $name:ident; $($x:ident),*) => {
        $crate::backend::renderer::output::element::RenderElement::<$other>::$name($($x),*)
    };
    (@body $renderer:ty; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
        fn id(&self) -> &$crate::backend::renderer::output::element::Id {
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

        fn underlying_storage(&self, renderer: &$renderer) -> Option<$crate::backend::renderer::output::element::UnderlyingStorage<'_, $renderer>>
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

        fn current_commit(&self) -> usize {
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

        fn damage_since(&self, scale: $crate::utils::Scale<f64>, commit: Option<usize>) -> Vec<$crate::utils::Rectangle<i32, $crate::utils::Physical>> {
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
                    Self::$body(x) => $crate::render_elements_internal!(@call $renderer $(as $other_renderer)?; draw; x, renderer, frame, scale, damage, log)
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
                    Self::$body(x) => $crate::render_elements_internal!(@call $renderer $(as $other_renderer)?; draw; x, renderer, frame, scale, damage, log)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
    };
    // Generic renderer
    (@impl $name:ident<$renderer:ident>; $($tail:tt)*) => {
        impl<$renderer> $crate::backend::renderer::output::element::RenderElement<$renderer> for $name<$renderer>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
            <$renderer as Renderer>::TextureId: 'static,
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };
    (@impl $name:ident<$lt:lifetime, $renderer:ident>; $($tail:tt)*) => {
        impl<$lt, $renderer> $crate::backend::renderer::output::element::RenderElement<$renderer> for $name<$lt, $renderer>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
            <$renderer as Renderer>::TextureId: 'static,
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };
    (@impl $name:ident<$renderer:ident, $custom:ident>; $($tail:tt)*) => {
        impl<$renderer, $custom> $crate::backend::renderer::output::element::RenderElement<$renderer> for $name<$renderer, $custom>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
            <$renderer as Renderer>::TextureId: 'static,
            $custom: $crate::backend::renderer::output::element::RenderElement<$renderer>,
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };
    (@impl $name:ident<$lt:lifetime, $renderer:ident, $custom:ident>; $($tail:tt)*) => {
        impl<$lt, $renderer, $custom> $crate::backend::renderer::output::element::RenderElement<$renderer> for $name<$lt, $renderer, $custom>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
            <$renderer as Renderer>::TextureId: 'static,
            $custom: $crate::backend::renderer::output::element::RenderElement<$renderer>,
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };
    (@impl $name:ident; $renderer:ident; $($tail:tt)*) => {
        impl<$renderer> $crate::backend::renderer::output::element::RenderElement<$renderer> for $name
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
            <$renderer as Renderer>::TextureId: 'static,
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };

    // Specific renderer
    (@impl $name:ident<=$renderer:ty>; $($tail:tt)*) => {
        impl $crate::backend::renderer::output::element::RenderElement<$renderer> for $name
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw $renderer; $($tail)*);
        }
    };
    (@impl $name:ident<=$renderer:ty, $custom:ident>; $($tail:tt)*) => {
        impl<$custom> $crate::backend::renderer::output::element::RenderElement<$renderer> for $name<$custom>
        where $custom: $crate::backend::renderer::output::element::RenderElement<$renderer>
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw $renderer; $($tail)*);
        }
    };

    (@impl $name:ident<=$renderer:ty, $lt:lifetime>; $($tail:tt)*) => {
        impl<$lt> $crate::backend::renderer::output::element::RenderElement<$renderer> for $name<$lt>
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw $renderer; $($tail)*);
        }
    };

    (@impl $name:ident<=$renderer:ty, $lt:lifetime, $custom:ident>; $($tail:tt)*) => {
        impl<$lt, $custom> $crate::backend::renderer::output::element::RenderElement<$renderer> for $name<$lt, $custom>
        where $custom: $crate::backend::renderer::output::element::RenderElement<$renderer>
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
                $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
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
                $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
                $custom: $crate::backend::renderer::output::element::RenderElement<$renderer>,
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
                $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
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
                $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll,
                $custom: $crate::backend::renderer::output::element::RenderElement<$renderer>,
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

/// TODO: Docs
#[macro_export]
macro_rules! render_elements {
    ($(#[$attr:meta])* $vis:vis $name:ident<=$lt:lifetime, $renderer:ty, $custom:ident>; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name $lt $custom; $($tail)*);
        $crate::render_elements_internal!(@impl $name<=$renderer, $lt, $custom>; $($tail)*);
        $crate::render_elements_internal!(@from $name<$lt, $custom>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<=$lt:lifetime, $renderer:ty>; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name $lt; $($tail)*);
        $crate::render_elements_internal!(@impl $name<=$renderer, $lt>; $($tail)*);
        $crate::render_elements_internal!(@from $name<$lt>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<=$renderer:ty, $custom:ident>; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name $custom; $($tail)*);
        $crate::render_elements_internal!(@impl $name<=$renderer, $custom>; $($tail)*);
        $crate::render_elements_internal!(@from $name<$custom>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<=$renderer:ty>; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name; $($tail)*);
        $crate::render_elements_internal!(@impl $name<=$renderer>; $($tail)*);
        $crate::render_elements_internal!(@from $name; $($tail)*);
    };



    ($(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, $renderer:ident, $custom:ident>; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name<$lt, $renderer, $custom>; $($tail)*);
        $crate::render_elements_internal!(@impl $name<$lt, $renderer, $custom>; $($tail)*);
        $crate::render_elements_internal!(@from $name<$lt, $renderer, $custom>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<$lt:lifetime, $renderer:ident>; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name<$lt, $renderer>; $($tail)*);
        $crate::render_elements_internal!(@impl $name<$lt, $renderer>; $($tail)*);
        $crate::render_elements_internal!(@from $name<$lt, $renderer>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<$renderer:ident, $custom:ident>; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name<$renderer, $custom>; $($tail)*);
        $crate::render_elements_internal!(@impl $name<$renderer, $custom>; $($tail)*);
        $crate::render_elements_internal!(@from $name<$renderer, $custom>; $($tail)*);
    };
    ($(#[$attr:meta])* $vis:vis $name:ident<$renderer:ident>; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $(#[$attr])* $vis $name<$renderer>; $($tail)*);
        $crate::render_elements_internal!(@impl $name<$renderer>; $($tail)*);
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

    fn current_commit(&self) -> usize {
        self.0.current_commit()
    }

    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical> {
        self.0.location(scale)
    }

    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        self.0.geometry(scale)
    }

    fn draw(
        &self,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: Scale<f64>,
        damage: &[Rectangle<i32, Physical>],
        log: &slog::Logger,
    ) -> Result<(), <R as Renderer>::Error> {
        self.0.draw(renderer, frame, scale, damage, log)
    }

    fn damage_since(&self, scale: Scale<f64>, commit: Option<usize>) -> Vec<Rectangle<i32, Physical>> {
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
mod tests {
    use std::marker::PhantomData;

    use crate::{
        backend::renderer::{gles2::Gles2Renderer, Renderer},
        utils::{Physical, Point, Rectangle, Scale},
    };

    use super::{Id, RenderElement, Wrap};

    render_elements! {
        Test<='a, Gles2Renderer>;
        Surface=TestRenderElement<'a, Gles2Renderer>
    }

    render_elements! {
        Test2<=Gles2Renderer>;
        Surface=TestRenderElement2<Gles2Renderer>
    }

    render_elements! {
        Test3<='a, Gles2Renderer, C>;
        Surface=TestRenderElement<'a, Gles2Renderer>,
        Custom=&'a C,
    }

    render_elements! {
        Test4<=Gles2Renderer, C>;
        Surface=TestRenderElement2<Gles2Renderer>,
        Custom=C
    }

    render_elements! {
        TestG<'a, R>;
        Surface=TestRenderElement<'a, R>
    }

    render_elements! {
        TestG2<R>;
        Surface=TestRenderElement2<R>
    }

    render_elements! {
        TestG3<'a, R, C>;
        Surface=TestRenderElement<'a, R>,
        Custom=&'a C,
    }

    render_elements! {
        TestG4<R, C>;
        Surface=TestRenderElement2<R>,
        Custom=Wrap<C>
    }

    render_elements! {
        TestG5;
        What=Empty,
    }

    render_elements! {
        TestG6<'a, R, C>;
        Surface=TestRenderElement<'a, R>,
        Custom=&'a C,
        Custom2=Wrap<C>,
    }

    impl<R> RenderElement<R> for Empty
    where
        R: Renderer,
    {
        fn id(&self) -> &Id {
            todo!()
        }

        fn current_commit(&self) -> usize {
            todo!()
        }

        fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
            todo!()
        }

        fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
            todo!()
        }

        fn draw(
            &self,
            _renderer: &mut R,
            _frame: &mut <R as Renderer>::Frame,
            _scale: Scale<f64>,
            _damage: &[Rectangle<i32, Physical>],
            _log: &slog::Logger,
        ) -> Result<(), <R as Renderer>::Error> {
            todo!()
        }
    }

    struct Empty;

    struct TestRenderElement2<R> {
        _phantom: PhantomData<R>,
    }

    impl<R> RenderElement<R> for TestRenderElement2<R>
    where
        R: Renderer,
    {
        fn id(&self) -> &Id {
            todo!()
        }

        fn current_commit(&self) -> usize {
            todo!()
        }

        fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
            todo!()
        }

        fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
            todo!()
        }

        fn draw(
            &self,
            _renderer: &mut R,
            _frame: &mut <R as Renderer>::Frame,
            _scale: Scale<f64>,
            _damage: &[Rectangle<i32, Physical>],
            _log: &slog::Logger,
        ) -> Result<(), <R as Renderer>::Error> {
            todo!()
        }
    }

    struct TestRenderElement<'a, R> {
        _test: &'a usize,
        _phantom: PhantomData<R>,
    }

    impl<'a, R> RenderElement<R> for TestRenderElement<'a, R>
    where
        R: Renderer,
    {
        fn id(&self) -> &Id {
            todo!()
        }

        fn current_commit(&self) -> usize {
            todo!()
        }

        fn location(&self, _scale: Scale<f64>) -> Point<i32, Physical> {
            todo!()
        }

        fn geometry(&self, _scale: Scale<f64>) -> Rectangle<i32, Physical> {
            todo!()
        }

        fn draw(
            &self,
            _renderer: &mut R,
            _frame: &mut <R as Renderer>::Frame,
            _scale: Scale<f64>,
            _damage: &[Rectangle<i32, Physical>],
            _log: &slog::Logger,
        ) -> Result<(), <R as Renderer>::Error> {
            todo!()
        }
    }
}
