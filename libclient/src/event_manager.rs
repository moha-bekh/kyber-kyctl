use super::enums::Event;

pub trait EventSink {
    fn on_event(&self, event: &Event);
}

#[derive(Default)]
pub struct EventManager {
    sinks: Vec<Box<dyn EventSink + Send>>,
}

impl EventManager {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_event_sink(&mut self, sink: Box<dyn EventSink + Send>) {
        self.sinks.push(sink);
    }

    pub fn send_event(&mut self, event: Event) {
        for sink in &self.sinks {
            sink.on_event(&event);
        }
    }
}
