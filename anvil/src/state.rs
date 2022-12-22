use std::{
    os::unix::io::{AsRawFd, OwnedFd},
    sync::{atomic::AtomicBool, Arc, Mutex},
    time::Duration,
};

use smithay::{
    backend::renderer::element::{default_primary_scanout_output_compare, RenderElementStates},
    delegate_compositor, delegate_data_device, delegate_fractional_scale, delegate_input_method_manager,
    delegate_keyboard_shortcuts_inhibit, delegate_layer_shell, delegate_output, delegate_presentation,
    delegate_primary_selection, delegate_seat, delegate_shm, delegate_tablet_manager,
    delegate_text_input_manager, delegate_viewporter, delegate_virtual_keyboard_manager,
    delegate_xdg_activation, delegate_xdg_decoration, delegate_xdg_shell,
    desktop::{
        utils::{
            surface_presentation_feedback_flags_from_states, surface_primary_scanout_output,
            update_surface_primary_scanout_output, OutputPresentationFeedback,
        },
        PopupManager, Space, Window,
    },
    input::{keyboard::XkbConfig, pointer::CursorImageStatus, Seat, SeatHandler, SeatState},
    output::Output,
    reexports::{
        calloop::{generic::Generic, Interest, LoopHandle, Mode, PostAction},
        wayland_protocols::xdg::decoration::{
            self as xdg_decoration, zv1::server::zxdg_toplevel_decoration_v1::Mode as DecorationMode,
        },
        wayland_server::{
            backend::{ClientData, ClientId, DisconnectReason},
            protocol::{wl_data_source::WlDataSource, wl_surface::WlSurface},
            Display, DisplayHandle, Resource,
        },
    },
    utils::{Clock, Logical, Monotonic, Point},
    wayland::{
        compositor::{get_parent, with_states, CompositorState},
        data_device::{
            set_data_device_focus, ClientDndGrabHandler, DataDeviceHandler, DataDeviceState,
            ServerDndGrabHandler,
        },
        fractional_scale::{with_fractional_scale, FractionScaleHandler, FractionalScaleManagerState},
        input_method::{InputMethodManagerState, InputMethodSeat},
        keyboard_shortcuts_inhibit::{
            KeyboardShortcutsInhibitHandler, KeyboardShortcutsInhibitState, KeyboardShortcutsInhibitor,
        },
        output::OutputManagerState,
        presentation::PresentationState,
        primary_selection::{set_primary_focus, PrimarySelectionHandler, PrimarySelectionState},
        seat::WaylandFocus,
        shell::{
            wlr_layer::WlrLayerShellState,
            xdg::{
                decoration::{XdgDecorationHandler, XdgDecorationState},
                ToplevelSurface, XdgShellState,
            },
        },
        shm::{ShmHandler, ShmState},
        socket::ListeningSocketSource,
        tablet_manager::TabletSeatTrait,
        text_input::TextInputManagerState,
        viewporter::ViewporterState,
        virtual_keyboard::VirtualKeyboardManagerState,
        xdg_activation::{
            XdgActivationHandler, XdgActivationState, XdgActivationToken, XdgActivationTokenData,
        },
    },
};

use crate::focus::FocusTarget;
#[cfg(feature = "xwayland")]
use crate::xwayland::X11State;
#[cfg(feature = "xwayland")]
use smithay::xwayland::{XWayland, XWaylandEvent};

pub struct CalloopData<BackendData: 'static> {
    pub state: AnvilState<BackendData>,
    pub display: Display<AnvilState<BackendData>>,
}

#[derive(Debug, Default)]
pub struct ClientState;
impl ClientData for ClientState {
    /// Notification that a client was initialized
    fn initialized(&self, _client_id: ClientId) {}
    /// Notification that a client is disconnected
    fn disconnected(&self, _client_id: ClientId, _reason: DisconnectReason) {}
}

#[derive(Debug)]
pub struct AnvilState<BackendData: 'static> {
    pub backend_data: BackendData,
    pub socket_name: Option<String>,
    pub display_handle: DisplayHandle,
    pub running: Arc<AtomicBool>,
    pub handle: LoopHandle<'static, CalloopData<BackendData>>,

    // desktop
    pub space: Space<Window>,
    pub popups: PopupManager,

    // smithay state
    pub compositor_state: CompositorState,
    pub data_device_state: DataDeviceState,
    pub layer_shell_state: WlrLayerShellState,
    pub output_manager_state: OutputManagerState,
    pub primary_selection_state: PrimarySelectionState,
    pub seat_state: SeatState<AnvilState<BackendData>>,
    pub keyboard_shortcuts_inhibit_state: KeyboardShortcutsInhibitState,
    pub shm_state: ShmState,
    pub viewporter_state: ViewporterState,
    pub xdg_activation_state: XdgActivationState,
    pub xdg_decoration_state: XdgDecorationState,
    pub xdg_shell_state: XdgShellState,
    pub presentation_state: PresentationState,
    pub fractional_scale_manager_state: FractionalScaleManagerState,

    pub dnd_icon: Option<WlSurface>,
    pub log: slog::Logger,

    // input-related fields
    pub suppressed_keys: Vec<u32>,
    pub pointer_location: Point<f64, Logical>,
    pub cursor_status: Arc<Mutex<CursorImageStatus>>,
    pub seat_name: String,
    pub seat: Seat<AnvilState<BackendData>>,
    pub clock: Clock<Monotonic>,

    #[cfg(feature = "xwayland")]
    pub xwayland: XWayland,
    #[cfg(feature = "xwayland")]
    pub x11_state: Option<X11State>,

    #[cfg(feature = "debug")]
    pub renderdoc: Option<renderdoc::RenderDoc<renderdoc::V141>>,

    pub show_window_preview: bool,
}

delegate_compositor!(@<BackendData: Backend + 'static> AnvilState<BackendData>);

impl<BackendData> DataDeviceHandler for AnvilState<BackendData> {
    fn data_device_state(&self) -> &DataDeviceState {
        &self.data_device_state
    }
    fn send_selection(&mut self, _mime_type: String, _fd: OwnedFd) {
        unreachable!("Anvil doesn't do server-side selections");
    }
}
impl<BackendData> ClientDndGrabHandler for AnvilState<BackendData> {
    fn started(&mut self, _source: Option<WlDataSource>, icon: Option<WlSurface>, _seat: Seat<Self>) {
        self.dnd_icon = icon;
    }
    fn dropped(&mut self, _seat: Seat<Self>) {
        self.dnd_icon = None;
    }
}
impl<BackendData> ServerDndGrabHandler for AnvilState<BackendData> {
    fn send(&mut self, _mime_type: String, _fd: OwnedFd) {
        unreachable!("Anvil doesn't do server-side grabs");
    }
}
delegate_data_device!(@<BackendData: 'static> AnvilState<BackendData>);

delegate_output!(@<BackendData: 'static> AnvilState<BackendData>);

impl<BackendData> PrimarySelectionHandler for AnvilState<BackendData> {
    fn primary_selection_state(&self) -> &PrimarySelectionState {
        &self.primary_selection_state
    }
}
delegate_primary_selection!(@<BackendData: 'static> AnvilState<BackendData>);

impl<BackendData> ShmHandler for AnvilState<BackendData> {
    fn shm_state(&self) -> &ShmState {
        &self.shm_state
    }
}
delegate_shm!(@<BackendData: 'static> AnvilState<BackendData>);

impl<BackendData> SeatHandler for AnvilState<BackendData> {
    type KeyboardFocus = FocusTarget;
    type PointerFocus = FocusTarget;

    fn seat_state(&mut self) -> &mut SeatState<AnvilState<BackendData>> {
        &mut self.seat_state
    }

    fn focus_changed(&mut self, seat: &Seat<Self>, target: Option<&FocusTarget>) {
        let dh = &self.display_handle;

        let focus = target
            .and_then(WaylandFocus::wl_surface)
            .and_then(|s| dh.get_client(s.id()).ok());
        set_data_device_focus(dh, seat, focus.clone());
        set_primary_focus(dh, seat, focus);
    }
    fn cursor_image(&mut self, _seat: &Seat<Self>, image: CursorImageStatus) {
        *self.cursor_status.lock().unwrap() = image;
    }
}
delegate_seat!(@<BackendData: 'static> AnvilState<BackendData>);

delegate_tablet_manager!(@<BackendData: 'static> AnvilState<BackendData>);

delegate_text_input_manager!(@<BackendData: 'static> AnvilState<BackendData>);

delegate_input_method_manager!(@<BackendData: 'static> AnvilState<BackendData>);

impl<BackendData> KeyboardShortcutsInhibitHandler for AnvilState<BackendData> {
    fn keyboard_shortcuts_inhibit_state(&mut self) -> &mut KeyboardShortcutsInhibitState {
        &mut self.keyboard_shortcuts_inhibit_state
    }

    fn new_inhibitor(&mut self, inhibitor: KeyboardShortcutsInhibitor) {
        // Just grant the wish for everyone
        inhibitor.activate();
    }
}

delegate_keyboard_shortcuts_inhibit!(@<BackendData: 'static> AnvilState<BackendData>);

delegate_virtual_keyboard_manager!(@<BackendData: 'static> AnvilState<BackendData>);

delegate_viewporter!(@<BackendData: 'static> AnvilState<BackendData>);

impl<BackendData> XdgActivationHandler for AnvilState<BackendData> {
    fn activation_state(&mut self) -> &mut XdgActivationState {
        &mut self.xdg_activation_state
    }

    fn request_activation(
        &mut self,
        token: XdgActivationToken,
        token_data: XdgActivationTokenData,
        surface: WlSurface,
    ) {
        if token_data.timestamp.elapsed().as_secs() < 10 {
            // Just grant the wish
            let w = self
                .space
                .elements()
                .find(|window| window.toplevel().wl_surface() == &surface)
                .cloned();
            if let Some(window) = w {
                self.space.raise_element(&window, true);
            }
        } else {
            // Discard the request
            self.xdg_activation_state.remove_request(&token);
        }
    }

    fn destroy_activation(
        &mut self,
        _token: XdgActivationToken,
        _token_data: XdgActivationTokenData,
        _surface: WlSurface,
    ) {
        // The request is cancelled
    }
}
delegate_xdg_activation!(@<BackendData: 'static> AnvilState<BackendData>);

impl<BackendData> XdgDecorationHandler for AnvilState<BackendData> {
    fn new_decoration(&mut self, toplevel: ToplevelSurface) {
        use xdg_decoration::zv1::server::zxdg_toplevel_decoration_v1::Mode;
        toplevel.with_pending_state(|state| {
            state.decoration_mode = Some(Mode::ClientSide);
        });
        toplevel.send_configure();
    }
    fn request_mode(&mut self, _toplevel: ToplevelSurface, _mode: DecorationMode) {}
    fn unset_mode(&mut self, _toplevel: ToplevelSurface) {}
}
delegate_xdg_decoration!(@<BackendData: Backend + 'static> AnvilState<BackendData>);

delegate_xdg_shell!(@<BackendData: Backend + 'static> AnvilState<BackendData>);
delegate_layer_shell!(@<BackendData: 'static> AnvilState<BackendData>);
delegate_presentation!(@<BackendData: 'static> AnvilState<BackendData>);

impl<BackendData> FractionScaleHandler for AnvilState<BackendData> {
    fn new_fractional_scale(
        &mut self,
        surface: smithay::reexports::wayland_server::protocol::wl_surface::WlSurface,
    ) {
        // Here we can set the initial fractional scale
        //
        // First we look if the surface already has a primary scan-out output, if not
        // we test if the surface is a subsurface and try to use the primary scan-out output
        // of the root surface. If the root also has no primary scan-out output we just try
        // to use the first output of the toplevel.
        // If the surface is the root we also try to use the first output of the toplevel.
        //
        // If all the above tests do not lead to a output we just use the first output
        // of the space (which in case of anvil will also be the output a toplevel will
        // initially be placed on)
        let mut root = surface.clone();
        while let Some(parent) = get_parent(&root) {
            root = parent;
        }

        with_states(&surface, |states| {
            let primary_scanout_output = surface_primary_scanout_output(&surface, states)
                .or_else(|| {
                    if root != surface {
                        with_states(&root, |states| {
                            surface_primary_scanout_output(&root, states).or_else(|| {
                                self.window_for_surface(&root).and_then(|window| {
                                    self.space.outputs_for_element(&window).first().cloned()
                                })
                            })
                        })
                    } else {
                        self.window_for_surface(&root)
                            .and_then(|window| self.space.outputs_for_element(&window).first().cloned())
                    }
                })
                .or_else(|| self.space.outputs().next().cloned());
            if let Some(output) = primary_scanout_output {
                with_fractional_scale(states, |fractional_scale| {
                    fractional_scale.set_preferred_scale(output.current_scale().fractional_scale());
                });
            }
        });
    }
}
delegate_fractional_scale!(@<BackendData: 'static> AnvilState<BackendData>);

impl<BackendData: Backend + 'static> AnvilState<BackendData> {
    pub fn init(
        display: &mut Display<AnvilState<BackendData>>,
        handle: LoopHandle<'static, CalloopData<BackendData>>,
        backend_data: BackendData,
        log: slog::Logger,
        listen_on_socket: bool,
    ) -> AnvilState<BackendData> {
        let clock = Clock::new().expect("failed to initialize clock");

        // init wayland clients
        let socket_name = if listen_on_socket {
            let source = ListeningSocketSource::new_auto(log.clone()).unwrap();
            let socket_name = source.socket_name().to_string_lossy().into_owned();
            handle
                .insert_source(source, |client_stream, _, data| {
                    if let Err(err) = data
                        .display
                        .handle()
                        .insert_client(client_stream, Arc::new(ClientState))
                    {
                        slog::warn!(data.state.log, "Error adding wayland client: {}", err);
                    };
                })
                .expect("Failed to init wayland socket source");
            info!(log, "Listening on wayland socket"; "name" => socket_name.clone());
            ::std::env::set_var("WAYLAND_DISPLAY", &socket_name);
            Some(socket_name)
        } else {
            None
        };
        handle
            .insert_source(
                Generic::new(
                    display.backend().poll_fd().as_raw_fd(),
                    Interest::READ,
                    Mode::Level,
                ),
                |_, _, data| {
                    data.display.dispatch_clients(&mut data.state).unwrap();
                    Ok(PostAction::Continue)
                },
            )
            .expect("Failed to init wayland server source");

        // init globals
        let dh = display.handle();
        let compositor_state = CompositorState::new::<Self, _>(&dh, log.clone());
        let data_device_state = DataDeviceState::new::<Self, _>(&dh, log.clone());
        let layer_shell_state = WlrLayerShellState::new::<Self, _>(&dh, log.clone());
        let output_manager_state = OutputManagerState::new_with_xdg_output::<Self>(&dh);
        let primary_selection_state = PrimarySelectionState::new::<Self, _>(&dh, log.clone());
        let mut seat_state = SeatState::new();
        let shm_state = ShmState::new::<Self, _>(&dh, vec![], log.clone());
        let viewporter_state = ViewporterState::new::<Self, _>(&dh, log.clone());
        let xdg_activation_state = XdgActivationState::new::<Self, _>(&dh, log.clone());
        let xdg_decoration_state = XdgDecorationState::new::<Self, _>(&dh, log.clone());
        let xdg_shell_state = XdgShellState::new::<Self, _>(&dh, log.clone());
        let presentation_state = PresentationState::new::<Self>(&dh, clock.id() as u32);
        let fractional_scale_manager_state = FractionalScaleManagerState::new::<Self, _>(&dh, log.clone());
        TextInputManagerState::new::<Self>(&dh);
        InputMethodManagerState::new::<Self>(&dh);
        VirtualKeyboardManagerState::new::<Self, _>(&dh, |_client| true);

        // init input
        let seat_name = backend_data.seat_name();
        let mut seat = seat_state.new_wl_seat(&dh, seat_name.clone(), log.clone());

        let cursor_status = Arc::new(Mutex::new(CursorImageStatus::Default));
        seat.add_pointer();
        seat.add_keyboard(XkbConfig::default(), 200, 25)
            .expect("Failed to initialize the keyboard");

        let cursor_status2 = cursor_status.clone();
        seat.tablet_seat().on_cursor_surface(move |_tool, new_status| {
            // TODO: tablet tools should have their own cursors
            *cursor_status2.lock().unwrap() = new_status;
        });

        seat.add_input_method(XkbConfig::default(), 200, 25);

        let keyboard_shortcuts_inhibit_state = KeyboardShortcutsInhibitState::new::<Self>(&display.handle());

        #[cfg(feature = "xwayland")]
        let xwayland = {
            let (xwayland, channel) = XWayland::new(log.clone(), &display.handle());
            let ret = handle.insert_source(channel, |event, _, data| match event {
                XWaylandEvent::Ready {
                    connection, client, ..
                } => data.state.xwayland_ready(connection, client),
                XWaylandEvent::Exited => data.state.xwayland_exited(),
            });
            if let Err(e) = ret {
                error!(
                    log,
                    "Failed to insert the XWaylandSource into the event loop: {}", e
                );
            }
            xwayland
        };

        AnvilState {
            backend_data,
            display_handle: display.handle(),
            socket_name,
            running: Arc::new(AtomicBool::new(true)),
            handle,
            space: Space::new(log.clone()),
            popups: PopupManager::new(log.clone()),
            compositor_state,
            data_device_state,
            layer_shell_state,
            output_manager_state,
            primary_selection_state,
            seat_state,
            keyboard_shortcuts_inhibit_state,
            shm_state,
            viewporter_state,
            xdg_activation_state,
            xdg_decoration_state,
            xdg_shell_state,
            presentation_state,
            fractional_scale_manager_state,
            dnd_icon: None,
            log,
            suppressed_keys: Vec::new(),
            pointer_location: (0.0, 0.0).into(),
            cursor_status,
            seat_name,
            seat,
            clock,
            #[cfg(feature = "xwayland")]
            xwayland,
            #[cfg(feature = "xwayland")]
            x11_state: None,
            #[cfg(feature = "debug")]
            renderdoc: renderdoc::RenderDoc::new().ok(),
            show_window_preview: false,
        }
    }
}

pub fn post_repaint(
    output: &Output,
    render_element_states: &RenderElementStates,
    space: &Space<Window>,
    time: impl Into<Duration>,
) {
    let time = time.into();
    let throttle = Some(Duration::from_secs(1));

    space.elements().for_each(|window| {
        window.with_surfaces(|surface, states| {
            let primary_scanout_output = update_surface_primary_scanout_output(
                surface,
                output,
                states,
                render_element_states,
                default_primary_scanout_output_compare,
            );

            if let Some(output) = primary_scanout_output {
                with_fractional_scale(states, |fraction_scale| {
                    fraction_scale.set_preferred_scale(output.current_scale().fractional_scale());
                });
            }
        });

        if space.outputs_for_element(window).contains(output) {
            window.send_frame(output, time, throttle, surface_primary_scanout_output);
        }
    });
    let map = smithay::desktop::layer_map_for_output(output);
    for layer_surface in map.layers() {
        layer_surface.with_surfaces(|surface, states| {
            let primary_scanout_output = update_surface_primary_scanout_output(
                surface,
                output,
                states,
                render_element_states,
                default_primary_scanout_output_compare,
            );

            if let Some(output) = primary_scanout_output {
                with_fractional_scale(states, |fraction_scale| {
                    fraction_scale.set_preferred_scale(output.current_scale().fractional_scale());
                });
            }
        });

        layer_surface.send_frame(output, time, throttle, surface_primary_scanout_output);
    }
}

pub fn take_presentation_feedback(
    output: &Output,
    space: &Space<Window>,
    render_element_states: &RenderElementStates,
) -> OutputPresentationFeedback {
    let mut output_presentation_feedback = OutputPresentationFeedback::new(output);

    space.elements().for_each(|window| {
        if space.outputs_for_element(window).contains(output) {
            window.take_presentation_feedback(
                &mut output_presentation_feedback,
                surface_primary_scanout_output,
                |surface, _| surface_presentation_feedback_flags_from_states(surface, render_element_states),
            );
        }
    });
    let map = smithay::desktop::layer_map_for_output(output);
    for layer_surface in map.layers() {
        layer_surface.take_presentation_feedback(
            &mut output_presentation_feedback,
            surface_primary_scanout_output,
            |surface, _| surface_presentation_feedback_flags_from_states(surface, render_element_states),
        );
    }

    output_presentation_feedback
}

pub trait Backend {
    fn seat_name(&self) -> String;
    fn reset_buffers(&mut self, output: &Output);
    fn early_import(&mut self, surface: &WlSurface);
}
