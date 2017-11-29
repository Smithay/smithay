use dbus::{BusType, Connection as DbusConnection};
use systemd::login as logind;

pub struct LogindSession {
    dbus: DbusConnection,
}

impl Session for LogindSession {

}

impl LogindSession {
    pub fn new() -> Result<LogindSession> {
        let session = logind::get_session(None)?;
        let vt = logind::get_vt(&session)?;
        let seat = logind::get_seat(&session)?;

        let dbus = DbusConnection::get_private(BusType::System)?;
        
    }
}

error_chain! {
    errors {

    }
}
