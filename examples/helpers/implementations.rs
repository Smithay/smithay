use rand;
use smithay::compositor::{CompositorToken, SurfaceUserImplementation};
use smithay::shell::{PopupConfigure, ShellSurfaceRole, ShellSurfaceUserImplementation, ToplevelConfigure};
use smithay::shm::with_buffer_contents;

define_roles!(Roles => [ ShellSurface, ShellSurfaceRole ] );

#[derive(Default)]
pub struct SurfaceData {
    pub buffer: Option<(Vec<u8>, (u32, u32))>,
    pub location: Option<(i32, i32)>,
}

pub fn surface_implementation() -> SurfaceUserImplementation<SurfaceData, Roles, ()> {
    SurfaceUserImplementation {
        commit: |_, _, surface, token| {
            // we retrieve the contents of the associated buffer and copy it
            token.with_surface_data(surface, |attributes| {
                match attributes.buffer.take() {
                    Some(Some((buffer, (_x, _y)))) => {
                        // we ignore hotspot coordinates in this simple example
                        with_buffer_contents(&buffer, |slice, data| {
                            let offset = data.offset as usize;
                            let stride = data.stride as usize;
                            let width = data.width as usize;
                            let height = data.height as usize;
                            let mut new_vec = Vec::with_capacity(width * height * 4);
                            for i in 0..height {
                                new_vec
                                    .extend(&slice[(offset + i * stride)..(offset + i * stride + width * 4)]);
                            }
                            attributes.user_data.buffer =
                                Some((new_vec, (data.width as u32, data.height as u32)));
                        }).unwrap();
                        buffer.release();
                    }
                    Some(None) => {
                        // erase the contents
                        attributes.user_data.buffer = None;
                    }
                    None => {}
                }
            });
        },
        frame: |_, _, _, callback, _| {
            callback.done(0);
        },
    }
}

pub fn shell_implementation(
    )
    -> ShellSurfaceUserImplementation<SurfaceData, Roles, (), CompositorToken<SurfaceData, Roles, ()>, ()>
{
    ShellSurfaceUserImplementation {
        new_client: |_, _, _| {},
        client_pong: |_, _, _| {},
        new_toplevel: |_, token, toplevel| {
            let wl_surface = toplevel.get_surface().unwrap();
            token.with_surface_data(wl_surface, |data| {
                // place the window at a random location in the [0;300]x[0;300] square
                use rand::distributions::{IndependentSample, Range};
                let range = Range::new(0, 300);
                let mut rng = rand::thread_rng();
                let x = range.ind_sample(&mut rng);
                let y = range.ind_sample(&mut rng);
                data.user_data.location = Some((x, y))
            });
            ToplevelConfigure {
                size: None,
                states: vec![],
                serial: 42,
            }
        },
        new_popup: |_, _, _| {
            PopupConfigure {
                size: (10, 10),
                position: (10, 10),
                serial: 42,
            }
        },
        move_: |_, _, _, _, _| {},
        resize: |_, _, _, _, _, _| {},
        grab: |_, _, _, _, _| {},
        change_display_state: |_, _, _, _, _, _, _| {
            ToplevelConfigure {
                size: None,
                states: vec![],
                serial: 42,
            }
        },
        show_window_menu: |_, _, _, _, _, _, _| {},
    }
}
