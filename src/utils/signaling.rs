//! A general purpose signaling mechanism
//!
//! This mechanism allows inter-module communication, by letting your modules
//! register callbacks to listen for events generated by other modules. This
//! signaling mechanism is synchronous and non-threadsafe. If you need
//! ascynchronous threadsafe communication, instead consider relying on channels.
//!
//! The whole mechanism is built on the [`Signaler`] type.
//! It serves both as a message sending facility and a way to register new callbacks
//! for these messages. It can be cloned and passed around between your modules with
//! `Rc`-like semantics.
//!
//! When sending a new signal with [`Signaler::signal`], the provided value `E` will
//! be made accessible as a reference `&E` to all registered callback.
//!
//! Sending a signal or registering a new callback from within a callback is supported.
//! These will however take effect after the current signal is completely delivered.
//! Ordering of sent signals and callback registration is preserved.

use std::{
    any::Any,
    cell::RefCell,
    collections::VecDeque,
    fmt,
    rc::{Rc, Weak},
};

/// A signaler, main type for signaling
#[derive(Debug)]
pub struct Signaler<S> {
    inner: Rc<SignalInner<S>>,
}

// Manual clone impl because of type parameters
impl<S> Clone for Signaler<S> {
    fn clone(&self) -> Signaler<S> {
        Signaler {
            inner: self.inner.clone(),
        }
    }
}

impl<S> Signaler<S> {
    /// Create a new signaler for given signal type
    pub fn new() -> Signaler<S> {
        Signaler {
            inner: Rc::new(SignalInner::new()),
        }
    }

    /// Register a new callback to this signaler
    ///
    /// This method returns a `SignalToken`, which you must keep as long
    /// as you need your callback to remain in place. Dropping it will
    /// disable and free your callback. If you don't plan to ever disable
    /// your callback, see [`SignalToken::leak()`](./struct.SignalToken.html).
    ///
    /// If you register a callback from within a callback of the same Signaler,
    /// the new callback will only be inserted *after* the current signal is
    /// completely delivered, and thus will not receive it.
    #[must_use]
    pub fn register<F: FnMut(&S) + 'static>(&self, f: F) -> SignalToken {
        let rc = Rc::new(RefCell::new(f));
        let weak = Rc::downgrade(&rc) as Weak<RefCell<dyn FnMut(&S)>>;
        self.inner.insert(weak);
        SignalToken { signal: rc }
    }

    /// Signal the callbacks
    ///
    /// All registered callbacks will be invoked with a reference to the value
    /// you provide here, after which that value will be dropped.
    ///
    /// If this method is invoked from within a callback of the same Signaler,
    /// its signalling will be delayed until the current signal is completely
    /// delivered and this method will return immediately.
    pub fn signal(&self, signal: S) {
        self.inner.send(signal);
    }
}

impl<S> Default for Signaler<S> {
    fn default() -> Self {
        Self::new()
    }
}

/// A token associated with a callback registered to a Signaler
///
/// Dropping it will disable and drop the callback it is associated to.
/// If you don't plan to ever disable the callback, you can use the `leak`
/// method to safely get rid of this value.
#[derive(Debug)]
pub struct SignalToken {
    signal: Rc<dyn Any>,
}

impl SignalToken {
    /// Destroy the token without disabling the associated callback
    pub fn leak(self) {
        // leak the Rc, so that it is never deallocated
        let _ = Rc::into_raw(self.signal);
    }
}

type WeakCallback<S> = Weak<RefCell<dyn FnMut(&S)>>;

struct SignalInner<S> {
    callbacks: RefCell<Vec<WeakCallback<S>>>,
    pending_callbacks: RefCell<Vec<WeakCallback<S>>>,
    pending_events: RefCell<VecDeque<S>>,
}

// WeakCallback does not implement debug, so we have to impl Debug manually
impl<S: fmt::Debug> fmt::Debug for SignalInner<S> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SignalInner")
            .field("callbacks::len()", &self.callbacks.borrow().len())
            .field("pending_callbacks::len()", &self.pending_callbacks.borrow().len())
            .field("pending_events", &self.pending_events)
            .finish()
    }
}

impl<S> SignalInner<S> {
    fn new() -> SignalInner<S> {
        SignalInner {
            callbacks: RefCell::new(Vec::new()),
            pending_callbacks: RefCell::new(Vec::new()),
            pending_events: RefCell::new(VecDeque::new()),
        }
    }

    fn insert(&self, weak: WeakCallback<S>) {
        // attempt to insert the new callback
        if let Ok(mut guard) = self.callbacks.try_borrow_mut() {
            // success, insert it
            guard.push(weak);
        } else {
            // The callback list is already borrowed, this means that this insertion is
            // done from within a callback.
            // In that case, insert the callback into the pending list, `send`
            // will insert it in the callback list when it is finished dispatching
            // the current event.
            self.pending_callbacks.borrow_mut().push(weak);
        }
    }

    fn send(&self, event: S) {
        // insert the new event into the pending list
        self.pending_events.borrow_mut().push_back(event);
        // now try to dispatch the events from the pending list
        // new events might be added by other callbacks in the process
        // so we try to completely drain it before returning
        //
        // If we cannot get the guard, that means an other dispatching is
        // already in progress. It'll empty the pending list, so there is
        // nothing more we need to do.
        if let Ok(mut guard) = self.callbacks.try_borrow_mut() {
            // We cannot just use `while let` because this would keep the
            // borrow of self.pending_events alive during the whole loop, rather
            // than just the evaluation of the condition. :/
            loop {
                let next_event = self.pending_events.borrow_mut().pop_front();
                if let Some(event) = next_event {
                    // Send the message, cleaning up defunct callbacks in the process
                    guard.retain(|weak| {
                        if let Some(cb) = Weak::upgrade(weak) {
                            (&mut *cb.borrow_mut())(&event);
                            true
                        } else {
                            false
                        }
                    });
                    // integrate any pending callbacks resulting from the dispatching
                    // of this event
                    guard.extend(self.pending_callbacks.borrow_mut().drain(..));
                } else {
                    break;
                }
            }
        }
    }
}

/// Trait representing the capability of an object to listen for some signals
///
/// It is provided so that the signaling system can play nicely into generic
/// constructs.
pub trait Linkable<S> {
    /// Make this object listen for signals from given signaler
    fn link(&mut self, signaler: Signaler<S>);
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{cell::Cell, rc::Rc};

    #[test]
    fn basic_signal() {
        let signaler = Signaler::<u32>::new();

        let signaled = Rc::new(Cell::new(false));
        let signaled2 = signaled.clone();

        let _token = signaler.register(move |_| signaled2.set(true));

        signaler.signal(0);

        assert!(signaled.get());
    }

    #[test]
    fn remove_callback() {
        let signaler = Signaler::<u32>::new();

        let token = signaler.register(|&i| assert_eq!(i, 42));

        signaler.signal(42);

        ::std::mem::drop(token);

        signaler.signal(41);

        let _token = signaler.register(|&i| assert_eq!(i, 39));

        signaler.signal(39);
    }

    #[test]
    fn delayed_signal() {
        let signaler = Signaler::<u32>::new();

        let mut signaled = false;
        let sign2 = signaler.clone();
        let _token = signaler.register(move |&i| {
            if !signaled {
                sign2.signal(42);
                signaled = true;
            } else {
                assert_eq!(i, 42);
            }
        });

        signaler.signal(0);
    }

    #[test]
    fn delayed_register() {
        let signaler = Signaler::<bool>::new();

        let signaled = Rc::new(Cell::new(0u32));
        let signaled2 = signaled.clone();
        let sign2 = signaler.clone();

        let _token1 = signaler.register(move |&original| {
            signaled2.set(signaled2.get() + 1);
            if original {
                let signaled3 = signaled2.clone();
                sign2.register(move |_| signaled3.set(signaled3.get() + 1)).leak();
                sign2.signal(false);
            }
        });

        signaler.signal(true);

        // Two rounds of signals, the first triggers 1 callback, the second triggers 2
        assert_eq!(signaled.get(), 3);
    }
}
