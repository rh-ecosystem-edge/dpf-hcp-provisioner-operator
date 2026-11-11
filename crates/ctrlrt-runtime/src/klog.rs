//! Minimal `k8s.io/klog/v2` text-format writer.
//!
//! client-go components (leader election, reflectors) log through klog's global logger, which
//! the Go operator never redirects: those lines are written to stderr in klog's text format
//! (`I1111 10:00:00.000000   12345 leaderelection.go:258] "msg" key="value"`), independently
//! of the zap logger used by controller-runtime. This module reproduces that output.

use std::io::Write;
use std::sync::atomic::{AtomicI32, Ordering};

use chrono::{Datelike, Local, Timelike};
use serde_json::Value;

/// klog `-v` level (default 0).
static VERBOSITY: AtomicI32 = AtomicI32::new(0);

/// Set klog's verbosity (`-v`).
pub fn set_verbosity(v: i32) {
    VERBOSITY.store(v, Ordering::Relaxed);
}

/// Whether `V(level)` is enabled.
pub fn v_enabled(level: i32) -> bool {
    level <= VERBOSITY.load(Ordering::Relaxed)
}

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
        now.nanosecond() / 1000 % 1_000_000,
        std::process::id(),
        file_line
    )
}

fn format_value(v: &Value) -> String {
    match v {
        Value::String(s) => ctrlrt::error::go_quote(s),
        Value::Null => "<nil>".into(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        other => other.to_string(),
    }
}

fn render(msg: &str, err: Option<&str>, kvs: &[(&str, Value)]) -> String {
    let mut line = ctrlrt::error::go_quote(msg);
    if let Some(e) = err {
        line.push_str(" err=");
        line.push_str(&ctrlrt::error::go_quote(e));
    }
    for (k, v) in kvs {
        line.push(' ');
        line.push_str(k);
        line.push('=');
        line.push_str(&format_value(v));
    }
    line
}

fn write_line(s: String) {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(s.as_bytes());
    let _ = stderr.write_all(b"\n");
}

/// `logger.Info(msg, kvs...)` (structured, `InfoS`).
pub fn info(file_line: &str, msg: &str, kvs: &[(&str, Value)]) {
    write_line(format!("{}{}", header('I', file_line), render(msg, None, kvs)));
}

/// `logger.V(level).Info(msg, kvs...)`.
pub fn info_v(level: i32, file_line: &str, msg: &str, kvs: &[(&str, Value)]) {
    if v_enabled(level) {
        info(file_line, msg, kvs);
    }
}

/// `logger.Error(err, msg, kvs...)` (structured, `ErrorS`).
pub fn error(file_line: &str, err: &dyn std::fmt::Display, msg: &str, kvs: &[(&str, Value)]) {
    let e = err.to_string();
    write_line(format!("{}{}", header('E', file_line), render(msg, Some(&e), kvs)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_structured_text() {
        assert_eq!(
            render(
                "Attempting to acquire leader lease...",
                None,
                &[("lock", Value::String("ns/id".into()))]
            ),
            r#""Attempting to acquire leader lease..." lock="ns/id""#
        );
        assert_eq!(
            render("Failed", Some("boom"), &[("n", Value::from(3))]),
            r#""Failed" err="boom" n=3"#
        );
        let h = header('I', "leaderelection.go:258");
        assert!(h.starts_with('I'));
        assert!(h.ends_with(" leaderelection.go:258] "));
        assert_eq!(h.len(), "I1111 10:00:00.000000 ".len() + 8 + "leaderelection.go:258] ".len());
    }
}
