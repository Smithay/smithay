use std::{rc::Rc, cell::RefCell};

#[derive(Debug)]
struct SignalerInner<E> {
    closures: RefCell<Vec<Box<dyn FnMut(&mut E)>>>
}

impl<E> SignalerInner<E> {
    fn new() -> SignalerInner<E> {
        SignalerInner {
            closures: RefCell::new(Vec::new())
        }
    }
}

#[derive(Debug)]
pub struct Signaler<E> {
    inner: Rc<SignalerInner<E>>
}

impl<E> Clone for Signaler<E> {
    fn clone(&self) -> Signaler<E> {
        Signaler {
            inner: self.inner.clone()
        }
    }
}

impl<E> Signaler<E> {
    pub fn new() -> Signaler<E> {
        Signaler {
            inner: Rc::new(SignalerInner::new())
        }
    }

    fn register_closure<F: FnMut(&mut E) + 'static>(&self, f: F) {
    }

    fn send_event(&self, event: &mut E) {
    }
}
