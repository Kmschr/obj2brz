use std::fmt;
use std::sync::{Arc, Mutex};

type Sink = Arc<dyn Fn(&str) + Send + Sync>;

/// Collects progress messages during a conversion. Front-ends decide how to
/// surface them: the GUI polls [`Logger::get_messages`], while the CLI can
/// attach a [`Logger::with_sink`] callback to stream each line as it arrives.
#[derive(Clone, Default)]
pub struct Logger {
    messages: Arc<Mutex<Vec<String>>>,
    sink: Option<Sink>,
}

impl Logger {
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a logger that also forwards every message to `sink` immediately.
    pub fn with_sink(sink: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self {
            messages: Arc::new(Mutex::new(Vec::new())),
            sink: Some(Arc::new(sink)),
        }
    }

    pub fn log(&self, message: String) {
        if let Some(sink) = &self.sink {
            sink(&message);
        }
        if let Ok(mut messages) = self.messages.lock() {
            messages.push(message);
        }
    }

    pub fn clear(&self) {
        if let Ok(mut messages) = self.messages.lock() {
            messages.clear();
        }
    }

    pub fn get_messages(&self) -> Vec<String> {
        self.messages.lock().ok()
            .map(|m| m.clone())
            .unwrap_or_default()
    }
}

impl fmt::Debug for Logger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Logger")
            .field("messages", &self.messages)
            .field("sink", &self.sink.as_ref().map(|_| "..."))
            .finish()
    }
}
