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
    fn location(&self, scale: Scale<f64>) -> Point<i32, Physical>;
    /// Get the geometry relative to the output
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical>;
    /// Get the damage since the provided commit relative to the element
    fn damage_since(&self, scale: Scale<f64>, commit: Option<usize>) -> Vec<Rectangle<i32, Physical>>;
    /// Get the opaque regions of the element relative to the element
    fn opaque_regions(&self, scale: Scale<f64>) -> Vec<Rectangle<i32, Physical>>;
    /// Get the underlying storage of this element, may be used to optimize rendering (eg. drm planes)
    fn underlying_storage(&self, renderer: &R) -> Option<UnderlyingStorage<'_, R>>;
    /// Draw this element
    fn draw(
        &self,
        renderer: &mut R,
        frame: &mut <R as Renderer>::Frame,
        scale: Scale<f64>,
        damage: &[Rectangle<i32, Physical>],
        log: &slog::Logger,
    );
}


#[macro_export]
#[doc(hidden)]
macro_rules! render_elements_internal {
    (@enum $vis:vis $name:ident; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
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
    (@enum $vis:vis $name:ident<$renderer:ident>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
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
    (@enum $lt:lifetime $vis:vis $name:ident<$renderer:ident>; $($(#[$meta:meta])* $body:ident=$field:ty$( as <$other_renderer:ty>)?),* $(,)?) => {
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
        )
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
                    $(
                        #[$meta]
                    )*
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
            scale: impl Into<$crate::utils::Scale<f64>>,
            location: $crate::utils::Point<f64, $crate::utils::Physical>,
            damage: &[$crate::utils::Rectangle<i32, $crate::utils::Physical>],
            log: &slog::Logger,
        )
        {
            match self {
                $(
                    $(
                        #[$meta]
                    )*
                    Self::$body(x) => $crate::render_elements_internal!(@call $renderer $(as $other_renderer)?; draw; x, renderer, frame, scale, location, damage, log)
                ),*,
                Self::_GenericCatcher(_) => unreachable!(),
            }
        }
    };
    (@impl $name:ident<$renderer:ident>; $($tail:tt)*) => {
        impl<$renderer> $crate::backend::renderer::output::element::RenderElement<$renderer> for $name<$renderer>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll + 'static,
            <$renderer as Renderer>::TextureId: 'static,
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };
    (@impl $lt:lifetime $name:ident<$renderer:ident>; $($tail:tt)*) => {
        impl<$lt, $renderer> $crate::backend::renderer::output::element::RenderElement<$renderer> for $name<$lt, $renderer>
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll + 'static,
            <$renderer as Renderer>::TextureId: 'static,
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };
    (@impl $name:ident; $renderer:ident; $($tail:tt)*) => {
        impl<$renderer> $crate::backend::renderer::output::element::RenderElement<$renderer> for $name
        where
            $renderer: $crate::backend::renderer::Renderer + $crate::backend::renderer::ImportAll + 'static,
            <$renderer as Renderer>::TextureId: 'static,
        {
            $crate::render_elements_internal!(@body $renderer; $($tail)*);
            $crate::render_elements_internal!(@draw <$renderer>; $($tail)*);
        }
    };
    (@impl $name:ident<=$renderer:ty>; $($tail:tt)*) => {
        impl $crate::backend::renderer::output::element::RenderElement<$renderer> for $name
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
    (@from $lt:lifetime $name:ident<$renderer:ident>; $($(#[$meta:meta])* $body:ident=$field:ty $(as <$other_renderer:ty>)?),* $(,)?) => {
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
}

#[macro_export]
macro_rules! render_elements {
    ($vis:vis $name:ident<=$renderer:ty>; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $vis $name; $($tail)*);
        $crate::render_elements_internal!(@impl $name<=$renderer>; $($tail)*);
        $crate::render_elements_internal!(@from $name; $($tail)*);

    };
    ($vis:vis $name:ident<=$lt:lifetime, $renderer:ty>; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $lt $vis $name; $($tail)*);
        $crate::render_elements_internal!(@impl $lt $name<=$renderer>; $($tail)*);
        $crate::render_elements_internal!(@from $lt $name; $($tail)*);

    };
    ($vis:vis $name:ident<$renderer:ident>; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $vis $name<$renderer>; $($tail)*);
        $crate::render_elements_internal!(@impl $name<$renderer>; $($tail)*);
        $crate::render_elements_internal!(@from $name<$renderer>; $($tail)*);
    };
    ($vis:vis $name:ident<$lt:lifetime, $renderer:ident>; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $lt $vis $name<$renderer>; $($tail)*);
        $crate::render_elements_internal!(@impl $lt $name<$renderer>; $($tail)*);
        $crate::render_elements_internal!(@from $lt $name<$renderer>; $($tail)*);
    };
    ($vis:vis $name:ident; $($tail:tt)*) => {
        $crate::render_elements_internal!(@enum $vis $name; $($tail)*);
        $crate::render_elements_internal!(@impl $name; R; $($tail)*);
        $crate::render_elements_internal!(@from $name; $($tail)*);
    };
}

pub use render_elements;