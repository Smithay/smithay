use ::backend::session::{AsErrno, Session, SessionNotifier, SessionObserver};
use nix::fcntl::OFlag;
use nix::sys::stat::{stat, fstat, major, minor};
use std::cell::RefCell;
use std::io::Result as IoResult;
use std::os::unix::io::RawFd;
use std::rc::{Rc, Weak};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use systemd::login;
use dbus::{BusType, Connection, ConnectionItems, ConnectionItem, Message, BusName, Interface, Member, Path as DbusPath, OwnedFd, MessageItem, Watch, WatchEvent};
use wayland_server::EventLoopHandle;
use wayland_server::sources::{FdEventSource, FdEventSourceImpl, FdInterest};

struct LogindSessionImpl {
    conn: RefCell<Connection>,
    session_path: DbusPath<'static>,
    active: AtomicBool,
    signals: RefCell<Vec<Option<Box<SessionObserver>>>>,
    seat: String,
    logger: ::slog::Logger,
}

#[derive(Clone)]
pub struct LogindSession {
    internal: Weak<LogindSessionImpl>,
    seat: String,
}

pub struct LogindSessionNotifier {
    internal: Rc<LogindSessionImpl>
}

impl LogindSession {
    pub fn new<L>(logger: L) -> Result<(LogindSession, LogindSessionNotifier)>
    where
        L: Into<Option<::slog::Logger>>
    {
        let logger = ::slog_or_stdlog(logger)
            .new(o!("smithay_module" => "backend_session", "session_type" => "logind"));

        let session_id = login::get_session(None).chain_err(|| ErrorKind::FailedToGetSession)?;
        let seat = login::get_seat(session_id.clone()).chain_err(|| ErrorKind::FailedToGetSeat)?;
        /*let vt = if seat == "seat0" {
            Some(login::get_vt(session_id.clone()).chain_err(|| ErrorKind::FailedToGetVT)?)
        } else {
            None
        };*/
        let vt = login::get_vt(session_id.clone()).ok();

        let conn = Connection::get_private(BusType::System).chain_err(|| ErrorKind::FailedDbusConnection)?;

        let session_path = LogindSessionImpl::blocking_call(
            &conn,
            "org.freedesktop.login1",
            "/org/freedesktop/login1",
            "org.freedesktop.login1.Manager",
            "GetSession",
            Some(vec![session_id.clone().into()])
        )?.get1::<DbusPath<'static>>()
        .chain_err(|| ErrorKind::UnexpectedMethodReturn)?;

        let match1 = String::from("type='signal',\
            sender='org.freedesktop.login1',\
            interface='org.freedesktop.login1.Manager',\
            member='SessionRemoved',\
            path='/org/freedesktop/login1'");
        conn.add_match(&match1).chain_err(|| ErrorKind::DbusMatchFailed(match1))?;
        let match2 = format!("type='signal',\
            sender='org.freedesktop.login1',\
            interface='org.freedesktop.login1.Session',\
            member='PauseDevice',\
            path='{}'", &session_path);
        conn.add_match(&match2).chain_err(|| ErrorKind::DbusMatchFailed(match2))?;
        let match3 = format!("type='signal',\
            sender='org.freedesktop.login1',\
            interface='org.freedesktop.login1.Session',\
            member='ResumeDevice',\
            path='{}'", &session_path);
        conn.add_match(&match3).chain_err(|| ErrorKind::DbusMatchFailed(match3))?;
        let match4 = format!("type='signal',\
            sender='org.freedesktop.login1',\
            interface='org.freedesktop.DBus.Properties',\
            member='PropertiesChanged',\
            path='{}'", &session_path);
        conn.add_match(&match4).chain_err(|| ErrorKind::DbusMatchFailed(match4))?;

        LogindSessionImpl::blocking_call(
            &conn,
            "org.freedesktop.login1",
            session_path.clone(),
            "org.freedesktop.login1.Session",
            "Activate",
            None,
        )?;

        LogindSessionImpl::blocking_call(
            &conn,
            "org.freedesktop.login1",
            session_path.clone(),
            "org.freedesktop.login1.Session",
            "TakeControl",
            Some(vec![false.into()]),
        )?;

        let signals = RefCell::new(Vec::new());
        let conn = RefCell::new(conn);

        let internal = Rc::new(LogindSessionImpl {
            conn,
            session_path,
            active: AtomicBool::new(true),
            signals,
            seat: seat.clone(),
            logger: logger.new(o!("id" => session_id, "seat" => seat.clone(), "vt" => format!("{:?}", &vt))),
        });

        Ok((
            LogindSession {
                internal: Rc::downgrade(&internal),
                seat,
            },
            LogindSessionNotifier {
                internal,
            }
        ))
    }
}

impl LogindSessionNotifier {
    pub fn session(&self) -> LogindSession {
        LogindSession {
            internal: Rc::downgrade(&self.internal),
            seat: self.internal.seat.clone(),
        }
    }
}

impl LogindSessionImpl {
    fn blocking_call<'d, 'p, 'i, 'm, D, P, I, M>
    (
        conn: &Connection,
        destination: D,
        path: P,
        interface: I,
        method: M,
        arguments: Option<Vec<MessageItem>>
    ) -> Result<Message>
    where
        D: Into<BusName<'d>>,
        P: Into<DbusPath<'p>>,
        I: Into<Interface<'i>>,
        M: Into<Member<'m>>,
    {
        let destination = destination.into().into_static();
        let path = path.into().into_static();
        let interface = interface.into().into_static();
        let method = method.into().into_static();

        let mut message = Message::method_call(&destination, &path, &interface, &method);

        if let Some(arguments) = arguments {
            message.append_items(&arguments)
        };

        let mut message = conn.send_with_reply_and_block(message, 1000)
        .chain_err(|| ErrorKind::FailedToSendDbusCall(
            destination.clone(),
            path.clone(),
            interface.clone(),
            method.clone()
        ))?;

        match message.as_result() {
            Ok(_) => Ok(message),
            Err(err) => Err(Error::with_chain(err, ErrorKind::DbusCallFailed(
                destination.clone(),
                path.clone(),
                interface.clone(),
                method.clone()
            )))
        }
    }

    fn handle_signals(&self, evlh: &mut EventLoopHandle, signals: ConnectionItems) -> Result<()> {
        for item in signals {
            let message = if let ConnectionItem::Signal(ref s) = item {
                s
            } else {
                continue
            };
            if &*message.interface().unwrap() == "org.freedesktop.login1.Manager"
                && &*message.member().unwrap() == "SessionRemoved"
            {
                error!(self.logger, "Session got closed by logind");
                //Ok... now what?
                //This session will never live again, but the user maybe has other sessions open
                //So lets just put it to sleep.. forever
                for signal in &mut *self.signals.borrow_mut() {
                    if let &mut Some(ref mut signal) = signal {
                        signal.pause(&mut evlh.state().as_proxy(), None);
                    }
                }
                self.active.store(false, Ordering::SeqCst);
                warn!(self.logger, "Session is now considered inactive");
            } else if &*message.interface().unwrap() == "org.freedesktop.login1.Session" {
                if &*message.member().unwrap() == "PauseDevice" {
                    let (major, minor, pause_type) = message.get3::<u32, u32, String>();
                    let major = major.chain_err(|| ErrorKind::UnexpectedMethodReturn)?;
                    let minor = minor.chain_err(|| ErrorKind::UnexpectedMethodReturn)?;
                    let pause_type = pause_type.chain_err(|| ErrorKind::UnexpectedMethodReturn)?;
                    debug!(self.logger, "Request of type \"{}\" to close device ({},{})", pause_type, major, minor);
                    for signal in &mut *self.signals.borrow_mut() {
                        if let &mut Some(ref mut signal) = signal {
                            signal.pause(&mut evlh.state().as_proxy(), Some((major, minor)));
                        }
                    }
                    // the other possible types are "force" or "gone" (unplugged),
                    // both expect no acknoledgement (note even this is not *really* necessary,
                    // logind would just timeout and send a "force" event. There is no way to
                    // keep the device.)
                    if &*pause_type == "pause" {
                        LogindSessionImpl::blocking_call(
                            &*self.conn.borrow(),
                            "org.freedesktop.login1",
                            self.session_path.clone(),
                            "org.freedesktop.login1.Session",
                            "PauseDeviceComplete",
                            Some(vec![major.into(), minor.into()])
                        )?;
                    }
                } else if &*message.member().unwrap() == "ResumeDevice" {
                    let (major, minor, fd) = message.get3::<u32, u32, OwnedFd>();
                    let major = major.chain_err(|| ErrorKind::UnexpectedMethodReturn)?;
                    let minor = minor.chain_err(|| ErrorKind::UnexpectedMethodReturn)?;
                    let fd    = fd.chain_err(|| ErrorKind::UnexpectedMethodReturn)?.into_fd();
                    debug!(self.logger, "Reactivating device ({},{})", major, minor);
                    for signal in &mut *self.signals.borrow_mut() {
                        if let &mut Some(ref mut signal) = signal {
                            signal.activate(&mut evlh.state().as_proxy(), Some((major, minor, Some(fd))));
                        }
                    }
                }
            } else if &*message.interface().unwrap() == "org.freedesktop.DBus.Properties"
                && &*message.member().unwrap() == "PropertiesChanged"
            {
                use dbus::arg::{Array, Dict, Iter, Variant, Get};

                let (_, changed, _) = message.get3::<String, Dict<String, Variant<Iter>, Iter>, Array<String, Iter>>();
                let mut changed = changed.chain_err(|| ErrorKind::UnexpectedMethodReturn)?;
                if let Some((_, mut value)) = changed.find(|&(ref key, _)| &*key == "Active") {
                    if let Some(active) = Get::get(&mut value.0) {
                        self.active.store(active, Ordering::SeqCst);
                    }
                }
            }
        }
        Ok(())
    }
}

impl Session for LogindSession {
    type Error = Error;

    fn open(&mut self, path: &Path, _flags: OFlag) -> Result<RawFd> {
        if let Some(session) = self.internal.upgrade() {
            let stat = stat(path).chain_err(|| ErrorKind::FailedToStatDevice)?;
            // TODO handle paused
            let (fd, _paused) = LogindSessionImpl::blocking_call(
                &*session.conn.borrow(),
                "org.freedesktop.login1",
		        session.session_path.clone(),
                "org.freedesktop.login1.Session",
                "TakeDevice",
                Some(vec![
                    (major(stat.st_rdev) as u32).into(),
                    (minor(stat.st_rdev) as u32).into(),
                ])
            )?.get2::<OwnedFd, bool>();
            let fd = fd.chain_err(|| ErrorKind::UnexpectedMethodReturn)?.into_fd();
            Ok(fd)
        } else {
            bail!(ErrorKind::SessionLost)
        }
    }

    fn close(&mut self, fd: RawFd) -> Result<()> {
        if let Some(session) = self.internal.upgrade() {
            let stat = fstat(fd).chain_err(|| ErrorKind::FailedToStatDevice)?;
            LogindSessionImpl::blocking_call(
                &*session.conn.borrow(),
                "org.freedesktop.login1",
		        session.session_path.clone(),
                "org.freedesktop.login1.Session",
                "ReleaseDevice",
                Some(vec![
                    (major(stat.st_rdev) as u32).into(),
                    (minor(stat.st_rdev) as u32).into(),
                ])
            ).map(|_| ())
        } else {
            bail!(ErrorKind::SessionLost)
        }
    }

    fn is_active(&self) -> bool {
        if let Some(internal) = self.internal.upgrade() {
            internal.active.load(Ordering::SeqCst)
        } else {
            false
        }
    }

    fn seat(&self) -> String {
        self.seat.clone()
    }

    fn change_vt(&mut self, vt_num: i32) -> Result<()> {
        if let Some(session) = self.internal.upgrade() {
            LogindSessionImpl::blocking_call(
                &*session.conn.borrow_mut(),
                "org.freedesktop.login1",
    	        "/org/freedesktop/login1/seat/self",
                "org.freedesktop.login1.Seat",
                "SwitchTo",
                Some(vec![
                    (vt_num as u32).into(),
                ])
            ).map(|_| ())
        } else {
            bail!(ErrorKind::SessionLost)
        }
    }
}

/// Ids of registered `SessionObserver`s of the `LogindSessionNotifier`
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub struct Id(usize);

impl SessionNotifier for LogindSessionNotifier {
    type Id = Id;

    fn register<S: SessionObserver + 'static>(&mut self, signal: S) -> Id {
        self.internal.signals.borrow_mut().push(Some(Box::new(signal)));
        Id(self.internal.signals.borrow().len() - 1)
    }
    fn unregister(&mut self, signal: Id) {
        self.internal.signals.borrow_mut()[signal.0] = None;
    }

    fn is_active(&self) -> bool {
        self.internal.active.load(Ordering::SeqCst)
    }

    fn seat(&self) -> &str {
        &self.internal.seat
    }
}

pub struct BoundLogindSession {
    notifier: LogindSessionNotifier,
    watches: Vec<Watch>,
    sources: Vec<FdEventSource<Rc<LogindSessionImpl>>>,
}

pub fn logind_session_bind(
    notifier: LogindSessionNotifier, evlh: &mut EventLoopHandle
) -> IoResult<BoundLogindSession>
{
    let watches = notifier.internal.conn.borrow().watch_fds();
    let sources = watches.clone().into_iter().map(|watch| {
        let mut interest = FdInterest::empty();
        interest.set(FdInterest::READ, watch.readable());
        interest.set(FdInterest::WRITE, watch.writable());
        evlh.add_fd_event_source(
            watch.fd(),
            fd_event_source_implementation(),
            notifier.internal.clone(),
            interest
        )
    }).collect::<IoResult<Vec<FdEventSource<Rc<LogindSessionImpl>>>>>()?;

    Ok(BoundLogindSession {
        notifier,
        watches,
        sources,
    })
}

impl BoundLogindSession {
    pub fn close(self) -> LogindSessionNotifier {
        for source in self.sources {
            source.remove();
        }
        self.notifier
    }
}

impl Drop for LogindSessionNotifier {
    fn drop(&mut self) {
        info!(self.internal.logger, "Closing logind session");
        let _ = LogindSessionImpl::blocking_call(
            &*self.internal.conn.borrow(),
            "org.freedesktop.login1",
            self.internal.session_path.clone(),
            "org.freedesktop.login1.Session",
            "ReleaseControl",
            None,
        );
    }
}

fn fd_event_source_implementation() -> FdEventSourceImpl<Rc<LogindSessionImpl>> {
    FdEventSourceImpl {
        ready: |evlh, session, fd, interest| {
            let conn = session.conn.borrow();
            let items = conn.watch_handle(fd, match interest {
                x if x.contains(FdInterest::READ) && x.contains(FdInterest::WRITE) =>
                    WatchEvent::Readable as u32 | WatchEvent::Writable as u32,
                x if x.contains(FdInterest::READ) => WatchEvent::Readable as u32,
                x if x.contains(FdInterest::WRITE) => WatchEvent::Writable as u32,
                _ => return,
            });
            if let Err(err) = session.handle_signals(evlh, items) {
                error!(session.logger, "Error handling dbus signals: {}", err);
            }
        },
        error: |evlh, session, fd, error| {
            warn!(session.logger, "Error on dbus connection: {:?}", error);
            // handle the remaining messages, they might contain the SessionRemoved event
            // in case the server did close the connection.
            let conn = session.conn.borrow();
            let items = conn.watch_handle(fd, WatchEvent::Error as u32);
            if let Err(err) = session.handle_signals(evlh, items) {
                error!(session.logger, "Error handling dbus signals: {}", err);
            }
        },
    }
}

error_chain! {
    errors {
        #[doc = "Failed to connect to dbus system socket"]
        FailedDbusConnection {
            description("Failed to connect to dbus system socket"),
        }

        #[doc = "Failed to get session from logind"]
        FailedToGetSession {
            description("Failed to get session from logind")
        }

        #[doc = "Failed to get seat from logind"]
        FailedToGetSeat {
            description("Failed to get seat from logind")
        }

        #[doc = "Failed to get vt from logind"]
        FailedToGetVT {
            description("Failed to get vt from logind")
        }

        #[doc = "Failed to call dbus method"]
        FailedToSendDbusCall(bus: BusName<'static>, path: DbusPath<'static>, interface: Interface<'static>, member: Member<'static>) {
            description("Failed to call dbus method")
            display("Failed to call dbus method for service: {:?}, path: {:?}, interface: {:?}, member: {:?}", bus, path, interface, member),
        }

        #[doc = "Dbus method call failed"]
        DbusCallFailed(bus: BusName<'static>, path: DbusPath<'static>, interface: Interface<'static>, member: Member<'static>) {
            description("Dbus method call failed")
            display("Dbus message call failed for service: {:?}, path: {:?}, interface: {:?}, member: {:?}", bus, path, interface, member),
        }

        #[doc = "Dbus method return had unexpected format"]
        UnexpectedMethodReturn {
            description("Dbus method return returned unexpected format")
        }

        #[doc = "Failed to setup dbus match rule"]
        DbusMatchFailed(rule: String) {
            description("Failed to setup dbus match rule"),
            display("Failed to setup dbus match rule {}", rule),
        }

        #[doc = "Failed to stat device"]
        FailedToStatDevice {
            description("Failed to stat device")
        }

        #[doc = "Session is already closed"]
        SessionLost {
            description("Session is already closed")
        }
    }
}

impl AsErrno for Error {
    fn as_errno(&self) -> Option<i32> {
        None
    }
}
