//! The Wayland backend data type.
//!
//! This module contains the [`WaylandBackendData`] type which is provided to all the wayland related dispatch
//! traits.

use std::{
    os::unix::prelude::RawFd,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex, Weak,
    },
};

use sctk::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_keyboard, delegate_output, delegate_pointer, delegate_registry,
    delegate_seat, delegate_xdg_shell, delegate_xdg_window,
    output::{OutputHandler, OutputState},
    reexports::client::{
        protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_surface},
        Connection, QueueHandle,
    },
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Modifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::xdg::{
        window::{Window, WindowConfigure, WindowHandler, XdgWindowState},
        XdgShellHandler, XdgShellState,
    },
};
use slog::debug;

use crate::backend::input::{ButtonState, DeviceCapability, InputEvent, KeyState};

use super::{
    dmabuf::DmabufState,
    window::{self, WindowId},
    WaylandEvent, WaylandKeyboardKeyEvent, WaylandPointerAxisEvent, WaylandPointerButtonEvent,
    WaylandVirtualDevice,
};

#[derive(Debug)]
pub struct WaylandBackendData {
    // Delegate types to implement windowing and presentation capabilities.
    pub(crate) protocols: Protocols,

    // Input state
    //
    // The Wayland backend only supports one seat, but creates a virtual device for each type of device
    // capability.
    pub(crate) wl_seat: Option<wl_seat::WlSeat>,
    pub(crate) wl_pointer: Option<wl_pointer::WlPointer>,
    pub(crate) pointer: Option<WaylandVirtualDevice>,
    pub(crate) wl_keyboard: Option<wl_keyboard::WlKeyboard>,
    pub(crate) keyboard: Option<WaylandVirtualDevice>,

    // Window state
    pub(crate) id_counter: AtomicUsize,
    pub(crate) windows: Vec<Weak<window::Inner>>,
    pub(crate) focus: WindowFocus,
    pub(crate) allocator: Option<Arc<Mutex<gbm::Device<RawFd>>>>,

    pub(crate) recorded: Vec<WaylandEvent>,
    pub(crate) logger: slog::Logger,
}

#[derive(Debug)]
pub(crate) struct Protocols {
    pub(crate) registry_state: RegistryState,
    pub(crate) output_state: OutputState,
    pub(crate) compositor_state: CompositorState,
    pub(crate) seat_state: SeatState,
    pub(crate) xdg_shell_state: XdgShellState,
    pub(crate) xdg_window_state: XdgWindowState,
    pub(crate) dmabuf_state: DmabufState,
}

impl WaylandBackendData {
    pub(crate) fn record_event(&mut self, event: WaylandEvent) {
        self.recorded.push(event);
    }

    pub(crate) fn take_recorded(&mut self) -> Vec<WaylandEvent> {
        self.recorded.drain(..).collect()
    }

    pub(crate) fn next_window_id(&self) -> usize {
        if self.id_counter.load(Ordering::SeqCst) == usize::MAX {
            // Wrapping around and reusing ids is possible here, but the likeihood of a client spawning at
            // minimum 4.2 billion windows (32-bit OS) throughout the lifespan of the application will either
            // take a very long time or the client will run out of protocol ids and be disconnected.
            panic!("No more available window ids")
        }

        self.id_counter.fetch_add(1, Ordering::SeqCst)
    }

    pub(crate) fn window_from_wl_surface(
        &self,
        surface: &wl_surface::WlSurface,
    ) -> Option<Arc<window::Inner>> {
        self.windows
            .iter()
            .filter_map(Weak::upgrade)
            .find(|window| window.sctk.wl_surface() == surface)
    }

    pub(crate) fn window_from_sctk(
        &self,
        sctk: &smithay_client_toolkit::shell::xdg::window::Window,
    ) -> Option<Arc<window::Inner>> {
        self.windows
            .iter()
            .filter_map(Weak::upgrade)
            .find(|window| &window.sctk == sctk)
    }
}

/// Current window focus.
#[derive(Debug)]
pub(crate) struct WindowFocus {
    pub(crate) keyboard: Option<WindowId>,
    pub(crate) pointer: Option<WindowId>,
}

delegate_registry!(WaylandBackendData);
delegate_output!(WaylandBackendData);
delegate_compositor!(WaylandBackendData);
delegate_seat!(WaylandBackendData);
delegate_keyboard!(WaylandBackendData);
delegate_pointer!(WaylandBackendData);
delegate_xdg_shell!(WaylandBackendData);
delegate_xdg_window!(WaylandBackendData);

impl ProvidesRegistryState for WaylandBackendData {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.protocols.registry_state
    }

    registry_handlers! {
        OutputState,
        CompositorState,
        SeatState,
        XdgShellState,
        XdgWindowState,
        // For dmabuf
        Self,
    }
}

impl OutputHandler for WaylandBackendData {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.protocols.output_state
    }

    fn new_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: wl_output::WlOutput) {
        // TODO
    }

    fn update_output(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _output: wl_output::WlOutput) {
        // TODO
    }

    fn output_destroyed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _output: wl_output::WlOutput,
    ) {
        // TODO
    }
}

impl CompositorHandler for WaylandBackendData {
    fn compositor_state(&mut self) -> &mut CompositorState {
        &mut self.protocols.compositor_state
    }

    fn scale_factor_changed(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_factor: i32,
    ) {
        // TODO
    }

    fn frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        if let Some(window) = self.window_from_wl_surface(surface) {
            self.record_event(WaylandEvent::Frame { window_id: window.id })
        }
    }
}

impl SeatHandler for WaylandBackendData {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.protocols.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        if self.wl_seat.is_none() {
            debug!(self.logger, "New seat created");
            self.wl_seat = Some(seat);
        }
    }

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if self.wl_seat.as_ref() == Some(&seat) {
            match capability {
                Capability::Keyboard => {
                    self.wl_keyboard = Some(
                        self.protocols
                            .seat_state
                            .get_keyboard(qh, &seat, None)
                            .expect("Failed to create keyboard"),
                    );
                    debug!(self.logger, "Created keyboard virtual device");
                    self.keyboard = Some(WaylandVirtualDevice {
                        capability: DeviceCapability::Keyboard,
                    });
                    self.record_event(WaylandEvent::Input(InputEvent::DeviceAdded {
                        device: self.keyboard.unwrap(),
                    }));
                }

                Capability::Pointer => {
                    self.wl_pointer = Some(
                        self.protocols
                            .seat_state
                            .get_pointer(qh, &seat)
                            .expect("Failed to create pointer"),
                    );
                    self.pointer = Some(WaylandVirtualDevice {
                        capability: DeviceCapability::Pointer,
                    });
                    self.record_event(WaylandEvent::Input(InputEvent::DeviceAdded {
                        device: self.pointer.unwrap(),
                    }));
                }

                _ => (),
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if self.wl_seat.as_ref() == Some(&seat) {
            match capability {
                Capability::Keyboard => {
                    debug!(self.logger, "Destroyed keyboard virtual device");
                    let device = self.keyboard.take().unwrap();

                    self.focus.keyboard.take();
                    self.wl_keyboard.take().unwrap().release();
                    self.record_event(WaylandEvent::Input(InputEvent::DeviceRemoved { device }));
                }

                Capability::Pointer => {
                    debug!(self.logger, "Destroyed pointer virtual device");
                    let device = self.pointer.take().unwrap();

                    self.focus.pointer.take();
                    self.wl_pointer.take().unwrap().release();
                    self.record_event(WaylandEvent::Input(InputEvent::DeviceRemoved { device }));
                }

                _ => (),
            }
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, seat: wl_seat::WlSeat) {
        if self.wl_seat.as_ref() == Some(&seat) {
            self.wl_seat.take();

            // Focus is no longer valid
            self.focus.pointer.take();
            self.focus.keyboard.take();

            // Destroy the virtual devices for each capability.
            if let Some(device) = self.keyboard.take() {
                debug!(self.logger, "Destroyed keyboard virtual device");
                self.wl_keyboard.take().unwrap().release();
                self.record_event(WaylandEvent::Input(InputEvent::DeviceRemoved { device }));
            }

            if let Some(device) = self.pointer.take() {
                debug!(self.logger, "Destroyed pointer virtual device");
                self.wl_pointer.take().unwrap().release();
                self.record_event(WaylandEvent::Input(InputEvent::DeviceRemoved { device }));
            }
        }
    }
}

impl PointerHandler for WaylandBackendData {
    fn pointer_frame(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            match event.kind {
                PointerEventKind::Enter { .. } => {
                    if let Some(window) = self.window_from_wl_surface(&event.surface) {
                        self.focus.pointer = Some(window.id);
                        // TODO: Emit motion event at the position where the pointer entered the window.
                        // self.record_event(WaylandEvent::Input(InputEvent::PointerMotionAbsolute {
                        //     event: super::WaylandPointerMotionEvent {
                        //         // TODO: What time should this provide?
                        //         time,
                        //         x: event.position.0,
                        //         y: event.position.1,
                        //         window,
                        //         device: self.pointer.unwrap(),
                        //     },
                        // }))
                    }
                }

                PointerEventKind::Leave { .. } => {
                    self.focus.pointer.take();
                }

                PointerEventKind::Motion { time } => {
                    if let Some(window) = self.focus.pointer {
                        self.record_event(WaylandEvent::Input(InputEvent::PointerMotionAbsolute {
                            event: super::WaylandPointerMotionEvent {
                                time,
                                x: event.position.0,
                                y: event.position.1,
                                window,
                                device: self.pointer.unwrap(),
                            },
                        }));
                    }
                }

                PointerEventKind::Press { time, button, .. } => {
                    if let Some(window) = self.focus.pointer {
                        self.record_event(WaylandEvent::Input(InputEvent::PointerButton {
                            event: WaylandPointerButtonEvent {
                                button,
                                time,
                                state: ButtonState::Pressed,
                                window,
                                device: self.pointer.unwrap(),
                            },
                        }));
                    }
                }

                PointerEventKind::Release { time, button, .. } => {
                    if let Some(window) = self.focus.pointer {
                        self.record_event(WaylandEvent::Input(InputEvent::PointerButton {
                            event: WaylandPointerButtonEvent {
                                button,
                                time,
                                state: ButtonState::Released,
                                window,
                                device: self.pointer.unwrap(),
                            },
                        }));
                    }
                }

                PointerEventKind::Axis {
                    time,
                    horizontal,
                    vertical,
                    source,
                } => {
                    if let Some(window) = self.focus.pointer {
                        self.record_event(WaylandEvent::Input(InputEvent::PointerAxis {
                            event: WaylandPointerAxisEvent {
                                device: self.pointer.unwrap(),
                                vertical,
                                horizontal,
                                source,
                                time,
                                window,
                            },
                        }));
                    }
                }
            }
        }
    }
}

impl KeyboardHandler for WaylandBackendData {
    fn enter(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        surface: &wl_surface::WlSurface,
        _serial: u32,
        _raw: &[u32],
        _keysyms: &[u32],
    ) {
        if let Some(window) = self.window_from_wl_surface(surface) {
            self.focus.keyboard = Some(window.id);
        }

        // TODO: Held keys?
    }

    fn leave(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _surface: &wl_surface::WlSurface,
        _serial: u32,
    ) {
        self.focus.keyboard.take();
    }

    fn press_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.record_event(WaylandEvent::Input(InputEvent::Keyboard {
            event: WaylandKeyboardKeyEvent {
                key_code: event.raw_code,
                state: KeyState::Pressed,
                device: self.keyboard.unwrap(),
                // TODO: Count
                count: 0,
                time: event.time,
                window: self.focus.keyboard.unwrap(),
            },
        }));
    }

    fn release_key(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        event: KeyEvent,
    ) {
        self.record_event(WaylandEvent::Input(InputEvent::Keyboard {
            event: WaylandKeyboardKeyEvent {
                key_code: event.raw_code,
                state: KeyState::Released,
                device: self.keyboard.unwrap(),
                // TODO: Count
                count: 0,
                time: event.time,
                window: self.focus.keyboard.unwrap(),
            },
        }));
    }

    fn update_modifiers(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _keyboard: &wl_keyboard::WlKeyboard,
        _serial: u32,
        _modifiers: Modifiers,
    ) {
        // TODO: Modifiers?
    }
}

impl XdgShellHandler for WaylandBackendData {
    fn xdg_shell_state(&mut self) -> &mut XdgShellState {
        &mut self.protocols.xdg_shell_state
    }
}

impl WindowHandler for WaylandBackendData {
    fn xdg_window_state(&mut self) -> &mut XdgWindowState {
        &mut self.protocols.xdg_window_state
    }

    fn request_close(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, window: &Window) {
        if let Some(window) = self.window_from_sctk(window) {
            self.record_event(WaylandEvent::CloseRequested { window_id: window.id });
        }
    }

    fn configure(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        window: &Window,
        configure: WindowConfigure,
        _serial: u32,
    ) {
        let size = if let Some(new_size) = configure.new_size {
            new_size
        } else {
            // FIXME: This is a pretty bad default, so it is temporary.
            (1280, 720)
        }
        .into();

        if let Some(window) = self.window_from_sctk(window) {
            let mut data = window.data.lock().unwrap();

            // Do not send duplicate resize events
            if data.current_size == size {
                return;
            }

            data.current_size = size;
            data.new_size = Some(size);

            let window_id = window.id;
            self.record_event(WaylandEvent::Resized {
                window_id,
                new_size: size,
            });
        }
    }
}

impl AsMut<DmabufState> for WaylandBackendData {
    fn as_mut(&mut self) -> &mut DmabufState {
        &mut self.protocols.dmabuf_state
    }
}
