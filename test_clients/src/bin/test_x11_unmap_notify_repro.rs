//! X11 client for testing `UnmapNotify`
//!
//! Examples of how to run this:
//! ```sh
//! test_x11_unmap_notify_repro "unmap, map"
//! test_x11_unmap_notify_repro "unmap_notify"
//! test_x11_unmap_notify_repro "withdraw, map, withdraw"
//! test_x11_unmap_notify_repro "withdraw, map, withdraw, map"
//! ```

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use tracing::info;
use x11rb::{
    connection::Connection,
    protocol::{Event, xproto::*},
    wrapper::ConnectionExt as _,
};

// I got this from https://gitlab.freedesktop.org/xorg/lib/libx11/-/blob/4c8ed231178d218915f9749f593b3faeac7d27b6/src/Withdraw.c#L65
fn x_withdraw_window(conn: &impl Connection, root: Window, win: Window) -> Result<()> {
    conn.unmap_window(win)?;
    send_unmap_notify(conn, root, win)?;
    Ok(())
}

fn send_unmap_notify(conn: &impl Connection, root: Window, win: Window) -> Result<()> {
    // Based on a table I fond at https://www.x.org/releases/current/doc/xorg-docs/icccm/icccm.html#Changing_Window_State
    conn.send_event(
        false,
        root,
        EventMask::SUBSTRUCTURE_REDIRECT | EventMask::SUBSTRUCTURE_NOTIFY,
        UnmapNotifyEvent {
            response_type: UNMAP_NOTIFY_EVENT,
            sequence: 0,
            event: root,
            window: win,
            from_configure: false,
        },
    )?;
    Ok(())
}

fn main() -> Result<()> {
    test_clients::init_logging();

    let (conn, screen_num) = x11rb::connect(None)?;
    let root = conn.setup().roots[screen_num].root;

    let win = create_window(&conn, root)?;

    let commands = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "withdraw,map,withdraw".to_owned());

    for cmd in commands.split(',') {
        match Command::parse(cmd)? {
            Command::Withdraw => {
                x_withdraw_window(&conn, root, win)?;
            }
            Command::Map => {
                conn.map_window(win)?;
            }
            Command::Unmap => {
                conn.unmap_window(win)?;
            }
            Command::UnmapNotify => {
                send_unmap_notify(&conn, root, win)?;
            }
        }
    }

    conn.flush()?;

    info!("waiting for events until killed");

    loop {
        conn.wait_for_event()?;
    }
}

#[derive(Debug, Clone, Copy)]
enum Command {
    Withdraw,
    Map,
    Unmap,
    UnmapNotify,
}

impl Command {
    fn parse(v: &str) -> Result<Self> {
        Ok(match v.trim() {
            "withdraw" => Self::Withdraw,
            "map" => Self::Map,
            "unmap" => Self::Unmap,
            "unmap_notify" => Self::UnmapNotify,
            cmd => bail!("unknown cmd {cmd:?}"),
        })
    }
}

fn create_window(conn: &impl Connection, root: Window) -> Result<u32> {
    let win = conn.generate_id()?;
    conn.create_window(
        x11rb::COPY_DEPTH_FROM_PARENT,
        win,
        root,
        100,
        100,
        300,
        200,
        0,
        WindowClass::INPUT_OUTPUT,
        0,
        &CreateWindowAux::new()
            .background_pixel(0x00cc3344)
            .event_mask(EventMask::STRUCTURE_NOTIFY),
    )?;
    conn.change_property8(
        PropMode::REPLACE,
        win,
        AtomEnum::WM_NAME,
        AtomEnum::STRING,
        "test_x11_stale_unmap_repro".as_bytes(),
    )?;
    conn.map_window(win)?;
    conn.flush()?;

    let mut parent = root;
    let mut mapped = false;
    let start = Instant::now();

    // Wait until the WM has reparented and mapped
    while parent == root || !mapped {
        if start.elapsed() > Duration::from_secs(5) {
            bail!("window was never managed (no ReparentNotify + MapNotify)");
        }
        match conn.poll_for_event()? {
            Some(Event::ReparentNotify(e)) if e.window == win => parent = e.parent,
            Some(Event::MapNotify(e)) if e.window == win => mapped = true,
            Some(Event::UnmapNotify(e)) if e.window == win => mapped = false,
            Some(_) => {}
            None => std::thread::sleep(Duration::from_millis(5)),
        }
    }

    info!("window {win:#x} managed in frame {parent:#x}");

    // Quick hack to give the compositor time to finish mapping the window
    std::thread::sleep(Duration::from_millis(500));

    Ok(win)
}
