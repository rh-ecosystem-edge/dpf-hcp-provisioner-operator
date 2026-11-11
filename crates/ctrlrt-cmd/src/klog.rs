//! Minimal klog (text format) output used by the client-go / apiserver code paths that log
//! through klog rather than the logr logger (the operator never redirects klog, so these
//! lines go to stderr in klog's own format):
//!
//! `E1111 13:05:06.123456   12345 webhook.go:154] Failed to make webhook authenticator request: ...`
//! `E1111 13:05:06.123456   12345 event.go:359] "Server rejected event (will not retry!)" err="..." event="..."`

use std::io::Write;

use chrono::{Datelike, Local, Timelike};

fn header(severity: char, file_line: &str) -> String {
    let now = Local::now();
    format!(
        "{}{:02}{:02} {:02}:{:02}:{:02}.{:06} {:>7} {}] ",
        severity,
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
        now.nanosecond() % 1_000_000_000 / 1000,
        std::process::id(),
        file_line
    )
}

/// klog quoting of a string value (`strconv.Quote` for strings with special characters).
fn quote(s: &str) -> String {
    ctrlrt::error::go_quote(s)
}

/// `klog.Errorf(format, ...)` (already formatted message).
pub fn errorf(file_line: &str, msg: &str) {
    let mut line = header('E', file_line);
    line.push_str(msg);
    if !line.ends_with('\n') {
        line.push('\n');
    }
    let _ = std::io::stderr().lock().write_all(line.as_bytes());
}

/// Structured `logger.Error(err, msg, kv...)` / `logger.Info(msg, kv...)` in klog text
/// format. String values are quoted.
pub fn structured(
    severity: char,
    file_line: &str,
    msg: &str,
    err: Option<&str>,
    kvs: &[(&str, String)],
) {
    let mut line = header(severity, file_line);
    line.push_str(&quote(msg));
    if let Some(e) = err {
        line.push_str(" err=");
        line.push_str(&quote(e));
    }
    for (k, v) in kvs {
        line.push(' ');
        line.push_str(k);
        line.push('=');
        line.push_str(&quote(v));
    }
    line.push('\n');
    let _ = std::io::stderr().lock().write_all(line.as_bytes());
}
