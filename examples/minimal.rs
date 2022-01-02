use std::{cell::RefCell, rc::Rc, sync::Arc};

use smithay::{
    backend::{
        renderer::{
            buffer_dimensions, buffer_type,
            gles2::{Gles2Frame, Gles2Renderer, Gles2Texture},
            BufferType, Frame, ImportAll, Renderer, Transform,
        },
        winit::{self, WinitEvent},
        SwapBuffersError,
    },
    reexports::{calloop::EventLoop, wayland_server::Display},
    utils::{Logical, Physical, Point, Rectangle, Size},
    wayland::{
        compositor::{
            self, is_sync_subsurface, with_surface_tree_upward, BufferAssignment, CompositorDispatch,
            CompositorHandler, CompositorState, Damage, RegionUserData, SubsurfaceCachedState,
            SubsurfaceUserData, SurfaceAttributes, SurfaceUserData, TraversalAction,
        },
        delegate::{DelegateDispatch, DelegateGlobalDispatch},
    },
};
use wayland_server::{
    backend::{ClientData, ClientId, DisconnectReason},
    protocol::{
        wl_buffer::{self, WlBuffer},
        wl_callback::{self, WlCallback},
        wl_compositor::{self, WlCompositor},
        wl_region::{self, WlRegion},
        wl_subcompositor::{self, WlSubcompositor},
        wl_subsurface::{self, WlSubsurface},
        wl_surface::{self, WlSurface},
    },
    socket::ListeningSocket,
    DataInit, Dispatch, DisplayHandle, GlobalDispatch, New,
};

struct InnerApp {
    display: Rc<RefCell<Display<App>>>,
}

impl CompositorHandler for InnerApp {
    fn commit(&mut self, surface: &WlSurface) {
        if !is_sync_subsurface(surface) {
            // Update the buffer of all child surfaces
            with_surface_tree_upward(
                surface,
                (),
                |_, _, _| TraversalAction::DoChildren(()),
                |_, states, _| {
                    states
                        .data_map
                        .insert_if_missing(|| RefCell::new(SurfaceData::default()));
                    let mut data = states
                        .data_map
                        .get::<RefCell<SurfaceData>>()
                        .unwrap()
                        .borrow_mut();
                    data.update_buffer(
                        &mut self.display.borrow().handle(),
                        &mut *states.cached_state.current::<SurfaceAttributes>(),
                    );
                },
                |_, _, _| true,
            );
        }
    }
}

struct App {
    inner: InnerApp,
    compositor_state: CompositorState,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    run_winit()
}

pub fn run_winit() -> Result<(), Box<dyn std::error::Error>> {
    let display: Display<App> = Display::new()?;
    let display = Rc::new(RefCell::new(display));

    let mut state = App {
        inner: InnerApp {
            display: display.clone(),
        },
        compositor_state: CompositorState::new(&mut display.borrow().handle(), None),
    };
    let listener = ListeningSocket::bind("wayland-5").unwrap();
    let mut clients = Vec::new();

    let (mut renderer, mut winit) = winit::init(None)?;

    loop {
        winit.dispatch_new_events(|event| match event {
            WinitEvent::Resized { size, .. } => {}
            WinitEvent::Input(event) => {}
            _ => (),
        })?;

        renderer.render(|renderer, frame| {
            frame.clear([0.0, 0.0, 0.0, 1.0]).unwrap();
            //
        })?;

        match listener.accept()? {
            Some(stream) => {
                println!("Got a client: {:?}", stream);

                let client = display
                    .borrow_mut()
                    .insert_client(stream, Arc::new(ClientState))
                    .unwrap();
                clients.push(client);
            }
            None => {}
        }

        display.borrow_mut().dispatch_clients(&mut state)?;
        display.borrow_mut().flush_clients()?;
    }
}

#[derive(Default)]
pub struct SurfaceData {
    pub buffer: Option<WlBuffer>,
    pub texture: Option<Box<dyn std::any::Any + 'static>>,
    pub geometry: Option<Rectangle<i32, Logical>>,
    pub buffer_dimensions: Option<Size<i32, Physical>>,
    pub buffer_scale: i32,
}

impl SurfaceData {
    fn update_buffer(&mut self, cx: &mut DisplayHandle<App>, attrs: &mut SurfaceAttributes) {
        match attrs.buffer.take() {
            Some(BufferAssignment::NewBuffer { buffer, .. }) => {
                // new contents
                self.buffer_dimensions = buffer_dimensions(&buffer);
                self.buffer_scale = attrs.buffer_scale;
                if let Some(old_buffer) = std::mem::replace(&mut self.buffer, Some(buffer)) {
                    old_buffer.release(cx);
                }
                self.texture = None;
            }
            Some(BufferAssignment::Removed) => {
                // remove the contents
                self.buffer = None;
                self.buffer_dimensions = None;
                self.texture = None;
            }
            None => {}
        }
    }

    /// Send the frame callback if it had been requested
    fn send_frame(cx: &mut DisplayHandle<App>, attrs: &mut SurfaceAttributes, time: u32) {
        for callback in attrs.frame_callbacks.drain(..) {
            callback.done(cx, time);
        }
    }
}

struct BufferTextures {
    buffer: Option<wl_buffer::WlBuffer>,
    texture: Gles2Texture,
}

fn draw_surface_tree(
    cx: &mut DisplayHandle<App>,
    renderer: &mut Gles2Renderer,
    frame: &mut Gles2Frame,
    root: &wl_surface::WlSurface,
    location: Point<i32, Logical>,
    output_scale: f32,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut result = Ok(());

    compositor::with_surface_tree_upward(
        root,
        location,
        |_surface, states, location| {
            let mut location = *location;
            // Pull a new buffer if available
            if let Some(data) = states.data_map.get::<RefCell<SurfaceData>>() {
                let mut data = data.borrow_mut();
                let attributes = states.cached_state.current::<SurfaceAttributes>();
                if data.texture.is_none() {
                    if let Some(buffer) = data.buffer.take() {
                        let damage = attributes
                            .damage
                            .iter()
                            .map(|dmg| match dmg {
                                Damage::Buffer(rect) => *rect,
                                // TODO also apply transformations
                                Damage::Surface(rect) => rect.to_buffer(attributes.buffer_scale),
                            })
                            .collect::<Vec<_>>();

                        match renderer.import_buffer(&buffer, Some(states), &damage) {
                            Some(Ok(m)) => {
                                let texture_buffer = if let Some(BufferType::Shm) = buffer_type(&buffer) {
                                    buffer.release(cx);
                                    None
                                } else {
                                    Some(buffer)
                                };
                                data.texture = Some(Box::new(BufferTextures {
                                    buffer: texture_buffer,
                                    texture: m,
                                }))
                            }
                            Some(Err(err)) => {
                                buffer.release(cx);
                            }
                            None => {
                                buffer.release(cx);
                            }
                        }
                    }
                }
                // Now, should we be drawn ?
                if data.texture.is_some() {
                    // if yes, also process the children
                    if states.role == Some("subsurface") {
                        let current = states.cached_state.current::<SubsurfaceCachedState>();
                        location += current.location;
                    }
                    TraversalAction::DoChildren(location)
                } else {
                    // we are not displayed, so our children are neither
                    TraversalAction::SkipChildren
                }
            } else {
                // we are not displayed, so our children are neither
                TraversalAction::SkipChildren
            }
        },
        |_surface, states, location| {
            let mut location = *location;
            if let Some(data) = states.data_map.get::<RefCell<SurfaceData>>() {
                let mut data = data.borrow_mut();
                let buffer_scale = data.buffer_scale;
                if let Some(texture) = data
                    .texture
                    .as_mut()
                    .and_then(|x| x.downcast_mut::<BufferTextures>())
                {
                    // we need to re-extract the subsurface offset, as the previous closure
                    // only passes it to our children
                    if states.role == Some("subsurface") {
                        let current = states.cached_state.current::<SubsurfaceCachedState>();
                        location += current.location;
                    }
                    if let Err(err) = frame.render_texture_at(
                        &texture.texture,
                        location.to_f64().to_physical(output_scale as f64).to_i32_round(),
                        buffer_scale,
                        output_scale as f64,
                        Transform::Normal, /* TODO */
                        1.0,
                    ) {
                        result = Err(err.into());
                    }
                }
            }
        },
        |_, _, _| true,
    );

    result
}

struct ClientState;
impl ClientData<App> for ClientState {
    fn initialized(&self, client_id: ClientId) {
        println!("initialized");
    }

    fn disconnected(&self, client_id: ClientId, reason: DisconnectReason) {
        println!("disconnected");
    }
}

impl GlobalDispatch<WlCompositor> for App {
    type GlobalData = ();

    fn bind(
        &mut self,
        handle: &mut wayland_server::DisplayHandle<'_, Self>,
        client: &wayland_server::Client,
        resource: New<WlCompositor>,
        global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, Self>,
    ) {
        DelegateGlobalDispatch::<WlCompositor, _>::bind(
            &mut CompositorDispatch(&mut self.compositor_state, &mut self.inner),
            handle,
            client,
            resource,
            global_data,
            data_init,
        );
    }
}

impl Dispatch<WlCompositor> for App {
    type UserData = ();

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlCompositor,
        request: wl_compositor::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<WlCompositor, _>::request(
            &mut CompositorDispatch(&mut self.compositor_state, &mut self.inner),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

impl Dispatch<WlSurface> for App {
    type UserData = SurfaceUserData;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlSurface,
        request: wl_surface::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<WlSurface, _>::request(
            &mut CompositorDispatch(&mut self.compositor_state, &mut self.inner),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

impl Dispatch<WlRegion> for App {
    type UserData = RegionUserData;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlRegion,
        request: wl_region::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<WlRegion, _>::request(
            &mut CompositorDispatch(&mut self.compositor_state, &mut self.inner),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

impl Dispatch<WlCallback> for App {
    type UserData = ();

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlCallback,
        request: wl_callback::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
    }
}

impl GlobalDispatch<WlSubcompositor> for App {
    type GlobalData = ();

    fn bind(
        &mut self,
        handle: &mut wayland_server::DisplayHandle<'_, Self>,
        client: &wayland_server::Client,
        resource: New<WlSubcompositor>,
        global_data: &Self::GlobalData,
        data_init: &mut DataInit<'_, Self>,
    ) {
        DelegateGlobalDispatch::<WlSubcompositor, _>::bind(
            &mut CompositorDispatch(&mut self.compositor_state, &mut self.inner),
            handle,
            client,
            resource,
            global_data,
            data_init,
        );
    }
}

impl Dispatch<WlSubcompositor> for App {
    type UserData = ();

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlSubcompositor,
        request: wl_subcompositor::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<WlSubcompositor, _>::request(
            &mut CompositorDispatch(&mut self.compositor_state, &mut self.inner),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}

impl Dispatch<WlSubsurface> for App {
    type UserData = SubsurfaceUserData;

    fn request(
        &mut self,
        client: &wayland_server::Client,
        resource: &WlSubsurface,
        request: wl_subsurface::Request,
        data: &Self::UserData,
        cx: &mut DisplayHandle<'_, Self>,
        data_init: &mut wayland_server::DataInit<'_, Self>,
    ) {
        DelegateDispatch::<WlSubsurface, _>::request(
            &mut CompositorDispatch(&mut self.compositor_state, &mut self.inner),
            client,
            resource,
            request,
            data,
            cx,
            data_init,
        );
    }
}
