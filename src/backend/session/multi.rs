use std::{
    collections::HashMap,
    os::unix::io::RawFd,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};

use super::{SessionNotifier, SessionObserver};

static ID_COUNTER: AtomicUsize = AtomicUsize::new(0);

/// Ids of registered `SessionObserver`s of the `DirectSessionNotifier`
#[derive(Debug, PartialEq, Eq, Clone, Copy, Hash)]
pub struct Id(usize);

struct MultiObserver {
    observer: Arc<Mutex<HashMap<Id, Box<SessionObserver>>>>,
}

impl SessionObserver for MultiObserver {
    fn pause(&mut self, device: Option<(u32, u32)>) {
        let mut lock = self.observer.lock().unwrap();
        for mut observer in lock.values_mut() {
            observer.pause(device)
        }
    }
    fn activate(&mut self, device: Option<(u32, u32, Option<RawFd>)>) {
        let mut lock = self.observer.lock().unwrap();
        for mut observer in lock.values_mut() {
            observer.activate(device)
        }
    }
}

struct MultiNotifier {
    observer: Arc<Mutex<HashMap<Id, Box<SessionObserver>>>>,
}

impl SessionNotifier for MultiNotifier {
    type Id = Id;

    fn register<S: SessionObserver + 'static>(&mut self, signal: S) -> Self::Id {
        let id = Id(ID_COUNTER.fetch_add(1, Ordering::SeqCst));
        self.observer.lock().unwrap().insert(id, Box::new(signal));
        id
    }

    fn unregister(&mut self, signal: Self::Id) {
        self.observer.lock().unwrap().remove(&signal);
    }
}

/// Create a pair of a linked [`SessionObserver`](../trait.SessionObserver.html) and a
/// [`SessionNotifier`](../trait.SessionNotifier.html).
///
/// Observers added to the returned notifier are notified,
/// when the returned observer is notified.
pub fn notify_multiplexer() -> (impl SessionObserver, impl SessionNotifier<Id = Id>) {
    let observer = Arc::new(Mutex::new(HashMap::new()));

    (
        MultiObserver {
            observer: observer.clone(),
        },
        MultiNotifier { observer },
    )
}
