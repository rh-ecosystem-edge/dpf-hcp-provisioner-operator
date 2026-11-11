//! logr-style structured logging (`logr.Logger`) with a pluggable sink.
//!
//! ```ignore
//! log.info("Reconciling", kv!["namespace" => ns, "name" => name]);
//! log.v(1).info("details", kv![]);
//! log.error(&err, "Failed to update status", kv!["phase" => phase]);
//! ```

use std::sync::{Arc, OnceLock, RwLock};

use serde_json::Value;

/// Key/value pairs attached to a log line.
pub type KeyValues = Vec<(String, Value)>;

/// Build a [`KeyValues`] list: `kv!["key" => value, ...]` (values must be `Serialize`).
#[macro_export]
macro_rules! kv {
    () => { ::std::vec::Vec::<(::std::string::String, $crate::__private::serde_json::Value)>::new() };
    ($($k:expr => $v:expr),+ $(,)?) => {
        vec![$( (::std::string::String::from($k), $crate::__private::serde_json::to_value(&$v).unwrap_or($crate::__private::serde_json::Value::Null)) ),+]
    };
}

/// Destination of log records (`logr.LogSink`).
pub trait LogSink: Send + Sync + 'static {
    /// Whether records at the given verbosity (`V(level)`) are emitted.
    fn enabled(&self, level: i32) -> bool;
    /// Emit an info record.
    fn info(&self, level: i32, name: &str, msg: &str, kvs: &[(String, Value)]);
    /// Emit an error record. `err` is `err.Error()` (None for `log.Error(nil, ...)`).
    fn error(&self, err: Option<&str>, name: &str, msg: &str, kvs: &[(String, Value)]);
}

/// Sink discarding everything.
pub struct NullSink;
impl LogSink for NullSink {
    fn enabled(&self, _level: i32) -> bool {
        false
    }
    fn info(&self, _: i32, _: &str, _: &str, _: &[(String, Value)]) {}
    fn error(&self, _: Option<&str>, _: &str, _: &str, _: &[(String, Value)]) {}
}

/// Minimal sink writing human readable lines to stderr (used before a real sink is set and
/// in tests).
pub struct StderrSink {
    pub max_level: i32,
}
impl LogSink for StderrSink {
    fn enabled(&self, level: i32) -> bool {
        level <= self.max_level
    }
    fn info(&self, _level: i32, name: &str, msg: &str, kvs: &[(String, Value)]) {
        eprintln!("INFO\t{name}\t{msg}\t{}", render_kvs(kvs));
    }
    fn error(&self, err: Option<&str>, name: &str, msg: &str, kvs: &[(String, Value)]) {
        let mut kvs = kvs.to_vec();
        if let Some(e) = err {
            kvs.push(("error".into(), Value::String(e.into())));
        }
        eprintln!("ERROR\t{name}\t{msg}\t{}", render_kvs(&kvs));
    }
}

fn render_kvs(kvs: &[(String, Value)]) -> String {
    let mut m = serde_json::Map::new();
    for (k, v) in kvs {
        m.insert(k.clone(), v.clone());
    }
    Value::Object(m).to_string()
}

fn global_sink() -> &'static RwLock<Arc<dyn LogSink>> {
    static SINK: OnceLock<RwLock<Arc<dyn LogSink>>> = OnceLock::new();
    SINK.get_or_init(|| RwLock::new(Arc::new(NullSink)))
}

/// `ctrl.SetLogger`: install the process-wide sink used by [`Logger::root`].
pub fn set_sink(sink: Arc<dyn LogSink>) {
    *global_sink().write().unwrap() = sink;
}

/// logr.Logger
#[derive(Clone)]
pub struct Logger {
    sink: Option<Arc<dyn LogSink>>,
    name: String,
    values: KeyValues,
    level: i32,
}

impl std::fmt::Debug for Logger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Logger").field("name", &self.name).finish()
    }
}

impl Default for Logger {
    fn default() -> Self {
        Self::root()
    }
}

impl Logger {
    /// Logger bound to the global sink (`ctrl.Log`). The sink is resolved at log time, so
    /// loggers created before `set_sink` still pick up the real sink (like the delegating
    /// logger in controller-runtime).
    pub fn root() -> Self {
        Self {
            sink: None,
            name: String::new(),
            values: Vec::new(),
            level: 0,
        }
    }

    /// Logger writing to a specific sink.
    pub fn with_sink(sink: Arc<dyn LogSink>) -> Self {
        Self {
            sink: Some(sink),
            name: String::new(),
            values: Vec::new(),
            level: 0,
        }
    }

    /// Discarding logger (`logr.Discard()`).
    pub fn discard() -> Self {
        Self::with_sink(Arc::new(NullSink))
    }

    fn sink(&self) -> Arc<dyn LogSink> {
        match &self.sink {
            Some(s) => s.clone(),
            None => global_sink().read().unwrap().clone(),
        }
    }

    /// `WithName` — names are joined with `.`
    pub fn with_name(&self, name: &str) -> Self {
        let mut l = self.clone();
        l.name = if l.name.is_empty() {
            name.to_string()
        } else {
            format!("{}.{}", l.name, name)
        };
        l
    }

    /// `WithValues`
    pub fn with_values(&self, kvs: KeyValues) -> Self {
        let mut l = self.clone();
        l.values.extend(kvs);
        l
    }

    /// `V(level)`
    pub fn v(&self, level: i32) -> Self {
        let mut l = self.clone();
        l.level += level;
        l
    }

    /// `Enabled()`
    pub fn enabled(&self) -> bool {
        self.sink().enabled(self.level)
    }

    fn merged(&self, kvs: KeyValues) -> KeyValues {
        if self.values.is_empty() {
            return kvs;
        }
        let mut all = self.values.clone();
        all.extend(kvs);
        all
    }

    /// `Info(msg, keysAndValues...)`
    pub fn info(&self, msg: &str, kvs: KeyValues) {
        let sink = self.sink();
        if sink.enabled(self.level) {
            sink.info(self.level, &self.name, msg, &self.merged(kvs));
        }
    }

    /// `Error(err, msg, keysAndValues...)`
    pub fn error(&self, err: &dyn std::fmt::Display, msg: &str, kvs: KeyValues) {
        let e = err.to_string();
        self.sink().error(Some(&e), &self.name, msg, &self.merged(kvs));
    }

    /// `Error(nil, msg, keysAndValues...)`
    pub fn error_nil(&self, msg: &str, kvs: KeyValues) {
        self.sink().error(None, &self.name, msg, &self.merged(kvs));
    }
}

/// Test sink capturing records for assertions.
#[derive(Default)]
pub struct CaptureSink {
    pub records: std::sync::Mutex<Vec<(String, String, KeyValues)>>,
}
impl LogSink for CaptureSink {
    fn enabled(&self, _level: i32) -> bool {
        true
    }
    fn info(&self, _level: i32, name: &str, msg: &str, kvs: &[(String, Value)]) {
        self.records
            .lock()
            .unwrap()
            .push(("info".into(), format!("{name}: {msg}"), kvs.to_vec()));
    }
    fn error(&self, err: Option<&str>, name: &str, msg: &str, kvs: &[(String, Value)]) {
        let mut kvs = kvs.to_vec();
        if let Some(e) = err {
            kvs.push(("error".into(), Value::String(e.into())));
        }
        self.records
            .lock()
            .unwrap()
            .push(("error".into(), format!("{name}: {msg}"), kvs));
    }
}
