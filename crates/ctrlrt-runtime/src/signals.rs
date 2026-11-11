//! `ctrl.SetupSignalHandler()` and a logging-only default event recorder.

use std::sync::atomic::{AtomicBool, Ordering};

use k8s_openapi::api::core::v1::ObjectReference;
use tokio_util::sync::CancellationToken;

static ONLY_ONE_SIGNAL_HANDLER: AtomicBool = AtomicBool::new(false);

/// `signals.SetupSignalHandler()`: returns a token cancelled on SIGTERM or SIGINT. If a second
/// signal is caught, the program is terminated with exit code 1.
///
/// Must be called from within a Tokio runtime. Panics when called twice (like Go, which closes
/// an already closed channel).
pub fn setup_signal_handler() -> CancellationToken {
    if ONLY_ONE_SIGNAL_HANDLER.swap(true, Ordering::SeqCst) {
        panic!("close of closed channel");
    }
    let token = CancellationToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        use tokio::signal::unix::{SignalKind, signal};
        let (Ok(mut term), Ok(mut int)) = (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) else {
            return;
        };
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
        t.cancel();
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
        std::process::exit(1); // second signal. Exit directly.
    });
    token
}

/// Default event recorder used when no real recorder factory is injected: mirrors the log line
/// controller-runtime's recorder provider writes for every event
/// (`logger.WithName("events").V(1).Info(message, "type", ..., "object", ..., "reason", ...)`).
pub struct LoggingRecorder {
    log: ctrlrt::Logger,
}

impl LoggingRecorder {
    pub fn new(log: ctrlrt::Logger) -> Self {
        Self { log }
    }
}

impl ctrlrt::EventRecorder for LoggingRecorder {
    fn event_ref(&self, object: &ObjectReference, event_type: &str, reason: &str, message: &str) {
        let obj = serde_json::to_value(object).unwrap_or(serde_json::Value::Null);
        self.log.v(1).info(
            message,
            ctrlrt::kv!["type" => event_type, "object" => obj, "reason" => reason],
        );
    }
}
