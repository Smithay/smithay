use smithay::{
    backend::libei::{EiInput, EiInputEvent},
    input::keyboard::XkbConfig,
    reexports::{
        calloop,
        reis::{calloop::EisListenerSource, eis},
    },
};

use crate::{state::AnvilState, udev::UdevData};

pub fn listen_eis(handle: &calloop::LoopHandle<'static, AnvilState<UdevData>>) {
    let listener = match eis::Listener::bind_auto() {
        Ok(listener) => listener,
        Err(err) => {
            tracing::error!("Failed to bind EI listener socket: {}", err);
            return;
        }
    };

    std::env::set_var("LIBEI_SOCKET", listener.path());

    let listener_source = EisListenerSource::new(listener);
    let handle_clone = handle.clone();
    handle
        .insert_source(listener_source, move |context, _, _| {
            let source = EiInput::new(context);
            handle_clone
                .insert_source(source, |event, connection, data| match event {
                    EiInputEvent::Connected => {
                        let seat = connection.add_seat("default");
                        let _ = seat.add_keyboard("virtual keyboard", XkbConfig::default());
                        seat.add_pointer("virtual pointer");
                        seat.add_pointer_absolute("virtual absolute pointer");
                        seat.add_touch("virtual touch");
                    }
                    EiInputEvent::Disconnected => {}
                    EiInputEvent::Event(event) => {
                        let dh = data.display_handle.clone();
                        data.process_input_event(&dh, event);
                    }
                })
                .unwrap();
            Ok(calloop::PostAction::Continue)
        })
        .unwrap();
}
