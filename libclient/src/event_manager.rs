use super::enums::Event;

pub trait EventSink {
    fn on_event(&self, event: &Event);
}

#[cfg(target_arch = "wasm32")]
type BoxedEventSink = Box<dyn EventSink>;

#[cfg(not(target_arch = "wasm32"))]
type BoxedEventSink = Box<dyn EventSink + Send>;

#[derive(Default)]
struct Inner {
    sinks: Vec<BoxedEventSink>,
}

#[derive(Default)]
pub struct EventManager {
    #[cfg(target_arch = "wasm32")]
    inner: std::rc::Rc<std::cell::RefCell<Inner>>,

    #[cfg(not(target_arch = "wasm32"))]
    inner: std::sync::Arc<std::sync::Mutex<Inner>>,
}

impl EventManager {
    pub fn new() -> Self {
        Self {
            inner: Default::default(),
        }
    }

    pub fn add_event_sink(&self, sink: BoxedEventSink) {
        #[cfg(not(target_arch = "wasm32"))]
        let Ok(mut inner) = self.inner.lock() else {
            log::warn!("Failed to lock EventManager::inner");
            return;
        };

        #[cfg(target_arch = "wasm32")]
        let mut inner = self.inner.borrow_mut();

        inner.sinks.push(sink);
    }

    pub fn send_event(&self, event: Event) {
        #[cfg(not(target_arch = "wasm32"))]
        let Ok(inner) = self.inner.lock() else {
            log::warn!("Failed to lock EventManager::inner");
            return;
        };

        #[cfg(target_arch = "wasm32")]
        let inner = self.inner.borrow();

        for sink in inner.sinks.iter() {
            sink.on_event(&event);
        }
    }
}

impl Clone for EventManager {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}
