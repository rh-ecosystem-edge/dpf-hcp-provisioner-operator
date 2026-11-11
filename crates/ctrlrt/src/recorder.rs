//! `record.EventRecorder` analogue.

use std::collections::VecDeque;
use std::sync::Mutex;

use k8s_openapi::api::core::v1::ObjectReference;

use crate::object::Object;

pub const EVENT_TYPE_NORMAL: &str = "Normal";
pub const EVENT_TYPE_WARNING: &str = "Warning";

/// Object-safe event recorder (`record.EventRecorder`).
pub trait EventRecorder: Send + Sync + 'static {
    /// `recorder.Event(obj, eventtype, reason, message)` for an already-resolved reference.
    fn event_ref(&self, object: &ObjectReference, event_type: &str, reason: &str, message: &str);
}

/// `reference.GetReference(scheme, obj)`
pub fn object_reference<K: Object>(obj: &K) -> ObjectReference {
    let info = K::resource_info();
    ObjectReference {
        api_version: Some(info.api_version()),
        kind: Some(info.kind),
        name: Some(obj.name().to_string()).filter(|s| !s.is_empty()),
        namespace: Some(obj.namespace().to_string()).filter(|s| !s.is_empty()),
        uid: Some(obj.uid().to_string()).filter(|s| !s.is_empty()),
        resource_version: Some(obj.resource_version().to_string()).filter(|s| !s.is_empty()),
        field_path: None,
    }
}

/// Typed convenience methods (`Event`, `Eventf`).
pub trait RecorderExt {
    fn event<K: Object>(&self, obj: &K, event_type: &str, reason: &str, message: &str);
}

impl<R: EventRecorder + ?Sized> RecorderExt for R {
    fn event<K: Object>(&self, obj: &K, event_type: &str, reason: &str, message: &str) {
        self.event_ref(&object_reference(obj), event_type, reason, message);
    }
}

/// Shared recorder handle.
pub type Recorder = std::sync::Arc<dyn EventRecorder>;

/// `record.FakeRecorder`: records `"<type> <reason> <message>"` strings.
#[derive(Default)]
pub struct FakeRecorder {
    events: Mutex<VecDeque<String>>,
    /// When `false` (Go: `Events == nil`), events are dropped.
    enabled: bool,
}

impl FakeRecorder {
    /// `record.NewFakeRecorder(bufferSize)` (buffer is unbounded here).
    pub fn new(_buffer_size: usize) -> Self {
        Self {
            events: Mutex::new(VecDeque::new()),
            enabled: true,
        }
    }

    /// A recorder that drops all events (`&record.FakeRecorder{}`).
    pub fn discarding() -> Self {
        Self {
            events: Mutex::new(VecDeque::new()),
            enabled: false,
        }
    }

    /// Non-blocking receive (`select { case e := <-r.Events: ... default: }`).
    pub fn try_recv(&self) -> Option<String> {
        self.events.lock().unwrap().pop_front()
    }

    /// Drain all recorded events.
    pub fn drain(&self) -> Vec<String> {
        self.events.lock().unwrap().drain(..).collect()
    }

    /// Snapshot without consuming.
    pub fn events(&self) -> Vec<String> {
        self.events.lock().unwrap().iter().cloned().collect()
    }

    /// `len(r.Events)`
    pub fn len(&self) -> usize {
        self.events.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl EventRecorder for FakeRecorder {
    fn event_ref(&self, _object: &ObjectReference, event_type: &str, reason: &str, message: &str) {
        if self.enabled {
            self.events
                .lock()
                .unwrap()
                .push_back(format!("{event_type} {reason} {message}"));
        }
    }
}

impl EventRecorder for std::sync::Arc<FakeRecorder> {
    fn event_ref(&self, object: &ObjectReference, event_type: &str, reason: &str, message: &str) {
        (**self).event_ref(object, event_type, reason, message)
    }
}
