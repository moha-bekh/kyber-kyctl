//! A single value cell to share between a producer and a consumer.
//!
//! Only the latest value is kept: if a new value is produced while the previous
//! one has not been consumed, the previous one is discarded.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

pub struct CellBufProducer<T> {
    state: Rc<RefCell<SharedState<T>>>,
}

pub struct CellBufConsumer<T> {
    state: Rc<RefCell<SharedState<T>>>,
}

pub fn cell_buf<T>() -> (CellBufProducer<T>, CellBufConsumer<T>) {
    let shared_state = Rc::new(RefCell::new(SharedState::new()));

    let state = shared_state.clone();
    let producer = CellBufProducer { state };

    let state = shared_state;
    let consumer = CellBufConsumer { state };

    (producer, consumer)
}

struct SharedState<T> {
    data: Option<T>,
    waker: Option<Waker>,
}

impl<T> SharedState<T> {
    fn new() -> Self {
        Self {
            data: None,
            waker: None,
        }
    }
}

impl<T> CellBufProducer<T> {
    pub fn replace(&self, value: T) -> Option<T> {
        let mut state = self.state.borrow_mut();
        let old = state.data.replace(value);
        if let Some(waker) = state.waker.take() {
            if old.is_none() {
                waker.wake();
            }
        }
        old
    }
}

impl<T> CellBufConsumer<T> {
    pub fn take(&self) -> CellBufFuture<T> {
        CellBufFuture {
            state: self.state.clone(),
        }
    }
}

pub struct CellBufFuture<T> {
    state: Rc<RefCell<SharedState<T>>>,
}

impl<T> Future for CellBufFuture<T> {
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.state.borrow_mut();
        if let Some(value) = state.data.take() {
            Poll::Ready(value)
        } else {
            state.waker = Some(cx.waker().clone());
            Poll::Pending
        }
    }
}
