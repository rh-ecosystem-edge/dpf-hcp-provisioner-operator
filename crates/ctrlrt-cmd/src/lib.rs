//! `ctrlrt-cmd` — the process-level pieces `cmd/main.go` gets from the Go standard library
//! and controller-runtime:
//!
//! * [`flag`]: Go `flag` package compatible command-line parsing.
//! * [`cmdline`]: the flags defined by `cmd/main.go` (+ `--kubeconfig` and the zap flags).
//! * [`zap`]: controller-runtime `pkg/log/zap` compatible [`ctrlrt::log::LogSink`].
//! * [`certwatcher`]: controller-runtime `pkg/certwatcher`.
//! * [`metrics`]: controller-runtime `pkg/metrics/server` + `pkg/metrics/filters`.
//! * [`events`]: client-go `tools/record` event broadcaster/correlator
//!   (`mgr.GetEventRecorderFor(name)`).

pub mod certwatcher;
pub mod events;
pub mod cmdline;
pub mod filters;
pub mod flag;
pub mod godur;
pub mod gofmt;
pub mod klog;
pub mod metrics;
pub mod tlsutil;
pub mod zap;

use std::sync::Arc;

pub use zap::{ZapOptions, ZapSink};

/// `ctrl.SetLogger(zap.New(zap.UseFlagOptions(&opts)))`: build the zap sink and install it
/// as the process-wide [`ctrlrt::log`] sink. Returns the sink.
pub fn setup_log(opts: &ZapOptions) -> Arc<ZapSink> {
    let sink = Arc::new(ZapSink::new(opts));
    ctrlrt::log::set_sink(sink.clone());
    sink
}

/// `ctrl.SetupSignalHandler()`: a token cancelled on the first SIGTERM/SIGINT; a second
/// signal terminates the process with exit code 1. Must be called from within a tokio
/// runtime.
pub fn setup_signal_handler() -> tokio_util::sync::CancellationToken {
    use tokio::signal::unix::{SignalKind, signal};
    let token = tokio_util::sync::CancellationToken::new();
    let t = token.clone();
    tokio::spawn(async move {
        let (Ok(mut term), Ok(mut int)) = (signal(SignalKind::terminate()), signal(SignalKind::interrupt())) else {
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
        std::process::exit(1);
    });
    token
}
