use std::time::Duration;

use smithay::{
    backend::renderer::{
        element::{surface::WaylandSurfaceRenderElement, AsRenderElements},
        ImportAll, Renderer,
    },
    desktop::{utils::OutputPresentationFeedback, Window, WindowSurfaceType},
    output::Output,
    reexports::{
        wayland_protocols::wp::presentation_time::server::wp_presentation_feedback,
        wayland_server::protocol::wl_surface::WlSurface,
    },
    space_elements,
    utils::{Logical, Point},
    wayland::{compositor::SurfaceData as WlSurfaceData, seat::WaylandFocus},
};
#[cfg(feature = "xwayland")]
use smithay::{
    desktop::utils::{
        send_frames_surface_tree, take_presentation_feedback_surface_tree, under_from_surface_tree,
        with_surfaces_surface_tree,
    },
    xwayland::X11Surface,
};

#[cfg(not(feature = "xwayland"))]
space_elements! {
    #[derive(Debug, Clone, PartialEq)]
    pub WindowElement;
    Wayland=Window,
}

#[cfg(feature = "xwayland")]
space_elements! {
    #[derive(Debug, Clone, PartialEq)]
    pub WindowElement;
    Wayland=Window,
    X11=X11Surface,
}

impl WindowElement {
    pub fn surface_under(
        &self,
        location: Point<f64, Logical>,
        window_type: WindowSurfaceType,
    ) -> Option<(WlSurface, Point<i32, Logical>)> {
        match self {
            WindowElement::Wayland(w) => w.surface_under(location, window_type),
            #[cfg(feature = "xwayland")]
            WindowElement::X11(w) => w
                .wl_surface()
                .and_then(|s| under_from_surface_tree(&s, location, (0, 0), window_type)),
            _ => None,
        }
    }

    pub fn with_surfaces<F>(&self, processor: F)
    where
        F: FnMut(&WlSurface, &WlSurfaceData) + Copy,
    {
        match self {
            WindowElement::Wayland(w) => w.with_surfaces(processor),
            #[cfg(feature = "xwayland")]
            WindowElement::X11(w) => {
                if let Some(surface) = w.wl_surface() {
                    with_surfaces_surface_tree(&surface, processor);
                }
            }
            _ => {}
        }
    }

    pub fn send_frame<T, F>(
        &self,
        output: &Output,
        time: T,
        throttle: Option<Duration>,
        primary_scan_out_output: F,
    ) where
        T: Into<Duration>,
        F: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
    {
        match self {
            WindowElement::Wayland(w) => w.send_frame(output, time, throttle, primary_scan_out_output),
            #[cfg(feature = "xwayland")]
            WindowElement::X11(w) => {
                if let Some(surface) = w.wl_surface() {
                    send_frames_surface_tree(&surface, output, time, throttle, primary_scan_out_output);
                }
            }
            _ => {}
        }
    }

    pub fn take_presentation_feedback<F1, F2>(
        &self,
        output_feedback: &mut OutputPresentationFeedback,
        primary_scan_out_output: F1,
        presentation_feedback_flags: F2,
    ) where
        F1: FnMut(&WlSurface, &WlSurfaceData) -> Option<Output> + Copy,
        F2: FnMut(&WlSurface, &WlSurfaceData) -> wp_presentation_feedback::Kind + Copy,
    {
        match self {
            WindowElement::Wayland(w) => w.take_presentation_feedback(
                output_feedback,
                primary_scan_out_output,
                presentation_feedback_flags,
            ),
            #[cfg(feature = "xwayland")]
            WindowElement::X11(w) => {
                if let Some(surface) = w.wl_surface() {
                    take_presentation_feedback_surface_tree(
                        &surface,
                        output_feedback,
                        primary_scan_out_output,
                        presentation_feedback_flags,
                    );
                }
            }
            _ => {}
        }
    }

    #[cfg(feature = "xwayland")]
    pub fn is_x11(&self) -> bool {
        matches!(self, WindowElement::X11(_))
    }

    pub fn is_wayland(&self) -> bool {
        matches!(self, WindowElement::Wayland(_))
    }

    pub fn wl_surface(&self) -> Option<WlSurface> {
        match self {
            WindowElement::Wayland(w) => w.wl_surface(),
            #[cfg(feature = "xwayland")]
            WindowElement::X11(w) => w.wl_surface(),
            _ => None,
        }
    }
}

impl<R> AsRenderElements<R> for WindowElement
where
    R: Renderer + ImportAll,
    <R as Renderer>::TextureId: 'static,
{
    type RenderElement = WaylandSurfaceRenderElement<R>;

    fn render_elements<C: From<Self::RenderElement>>(
        &self,
        renderer: &mut R,
        location: Point<i32, smithay::utils::Physical>,
        scale: smithay::utils::Scale<f64>,
    ) -> Vec<C> {
        match self {
            WindowElement::Wayland(w) => w.render_elements(renderer, location, scale),
            #[cfg(feature = "xwayland")]
            WindowElement::X11(w) => w.render_elements(renderer, location, scale),
            _ => unreachable!(),
        }
    }
}
