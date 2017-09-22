mod keyboard;
mod pointer;

pub use self::keyboard::{Error as KbdError, KbdHandle};
pub use self::pointer::PointerHandle;
use wayland_server::{Client, EventLoop, EventLoopHandle, Global, StateToken};
use wayland_server::protocol::{wl_keyboard, wl_pointer, wl_seat};

pub struct Seat {
    log: ::slog::Logger,
    name: String,
    pointer: Option<PointerHandle>,
    keyboard: Option<KbdHandle>,
    known_seats: Vec<wl_seat::WlSeat>,
}

impl Seat {
    pub fn new<L>(evl: &mut EventLoop, name: String, logger: L)
                  -> (StateToken<Seat>, Global<wl_seat::WlSeat, StateToken<Seat>>)
    where
        L: Into<Option<::slog::Logger>>,
    {
        let log = ::slog_or_stdlog(logger);
        let seat = Seat {
            log: log.new(o!("smithay_module" => "seat_handler")),
            name: name,
            pointer: None,
            keyboard: None,
            known_seats: Vec::new(),
        };
        let token = evl.state().insert(seat);
        // TODO: support version 5 (axis)
        let global = evl.register_global(4, seat_global_bind, token.clone());
        (token, global)
    }

    pub fn add_pointer(&mut self) -> PointerHandle {
        let pointer = self::pointer::create_pointer_handler();
        self.pointer = Some(pointer.clone());
        let caps = self.compute_caps();
        for seat in &self.known_seats {
            seat.capabilities(caps);
        }
        pointer
    }

    pub fn add_keyboard(&mut self, model: &str, layout: &str, variant: &str, options: Option<String>,
                        repeat_delay: i32, repeat_rate: i32)
                        -> Result<KbdHandle, KbdError> {
        let keyboard = self::keyboard::create_keyboard_handler(
            "evdev", // we need this one
            model,
            layout,
            variant,
            options,
            repeat_delay,
            repeat_rate,
            self.log.clone(),
        )?;
        self.keyboard = Some(keyboard.clone());
        let caps = self.compute_caps();
        for seat in &self.known_seats {
            seat.capabilities(caps);
        }
        Ok(keyboard)
    }

    fn compute_caps(&self) -> wl_seat::Capability {
        let mut caps = wl_seat::Capability::empty();
        if self.pointer.is_some() {
            caps |= wl_seat::Pointer;
        }
        if self.keyboard.is_some() {
            caps |= wl_seat::Keyboard;
        }
        caps
    }
}

fn seat_global_bind(evlh: &mut EventLoopHandle, token: &mut StateToken<Seat>, _: &Client,
                    seat: wl_seat::WlSeat) {
    evlh.register(&seat, seat_implementation(), token.clone(), None);
    let mut seat_mgr = evlh.state().get_mut(token);
    seat.name(seat_mgr.name.clone());
    seat.capabilities(seat_mgr.compute_caps());
    seat_mgr.known_seats.push(seat);
}

fn seat_implementation() -> wl_seat::Implementation<StateToken<Seat>> {
    wl_seat::Implementation {
        get_pointer: |evlh, token, _, seat, pointer| {
            evlh.register(&pointer, pointer_implementation(), (), None);
            if let Some(ref ptr_handle) = evlh.state().get(token).pointer {
                ptr_handle.new_pointer(pointer);
            }
            // TODO: protocol error ?
        },
        get_keyboard: |evlh, token, _, seat, keyboard| {
            evlh.register(&keyboard, keyboard_implementation(), (), None);
            if let Some(ref kbd_handle) = evlh.state().get(token).keyboard {
                kbd_handle.new_kbd(keyboard);
            }
            // TODO: protocol error ?
        },
        get_touch: |evlh, token, _, seat, touch| {
            // TODO
        },
        release: |_, _, _, _| {},
    }
}

fn pointer_implementation() -> wl_pointer::Implementation<()> {
    wl_pointer::Implementation {
        set_cursor: |_, _, _, _, _, _, _, _| {},
        release: |_, _, _, _| {},
    }
}

fn keyboard_implementation() -> wl_keyboard::Implementation<()> {
    wl_keyboard::Implementation {
        release: |_, _, _, _| {},
    }
}
