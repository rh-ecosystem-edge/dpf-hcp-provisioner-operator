//! controller-runtime's `pkg/log/zap` (zap + zapr) as a [`ctrlrt::log::LogSink`].
//!
//! ```text
//! let mut opts = ZapOptions { development: true, ..Default::default() };
//! opts.bind_flags(&mut fs);
//! fs.parse_env();
//! opts.apply_flags(&fs);
//! ctrlrt::log::set_sink(Arc::new(ZapSink::new(&opts)));   // ctrl.SetLogger(zap.New(zap.UseFlagOptions(&opts)))
//! ```
//!
//! Output format (byte-compatible with Go where the information is available):
//! * console encoder (development default):
//!   `<time>\t<LEVEL>\t<logger name>\t<message>\t{"k": "v", ...}` (+ `\n<stacktrace>`)
//! * JSON encoder (production default):
//!   `{"level":"info","ts":"...","logger":"...","msg":"...","k":"v","stacktrace":"..."}`
//!
//! Levels follow zapr: `V(n)` logs at zap level `-n` (`DEBUG` for 1, `LEVEL(-2)`...),
//! `Error` logs at `ERROR`.

use std::io::Write;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, FixedOffset, Local, Timelike, Utc};
use serde_json::Value;

use crate::flag::{FlagSet, Value as FlagValue};
use crate::gofmt;

/// zapcore levels.
pub const DEBUG_LEVEL: i8 = -1;
pub const INFO_LEVEL: i8 = 0;
pub const WARN_LEVEL: i8 = 1;
pub const ERROR_LEVEL: i8 = 2;
pub const DPANIC_LEVEL: i8 = 3;
pub const PANIC_LEVEL: i8 = 4;
pub const FATAL_LEVEL: i8 = 5;

/// `zapcore.Level.CapitalString()`
pub fn level_capital(l: i8) -> String {
    match l {
        DEBUG_LEVEL => "DEBUG".into(),
        INFO_LEVEL => "INFO".into(),
        WARN_LEVEL => "WARN".into(),
        ERROR_LEVEL => "ERROR".into(),
        DPANIC_LEVEL => "DPANIC".into(),
        PANIC_LEVEL => "PANIC".into(),
        FATAL_LEVEL => "FATAL".into(),
        _ => format!("LEVEL({l})"),
    }
}

/// `zapcore.Level.String()`
pub fn level_lower(l: i8) -> String {
    match l {
        DEBUG_LEVEL => "debug".into(),
        INFO_LEVEL => "info".into(),
        WARN_LEVEL => "warn".into(),
        ERROR_LEVEL => "error".into(),
        DPANIC_LEVEL => "dpanic".into(),
        PANIC_LEVEL => "panic".into(),
        FATAL_LEVEL => "fatal".into(),
        _ => format!("Level({l})"),
    }
}

/// zapr `toZapLevel`: logr verbosity -> zap level.
pub fn to_zap_level(lvl: i32) -> i8 {
    let lvl = lvl.clamp(0, 127);
    -(lvl as i8)
}

/// `zap-encoder` values.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EncoderKind {
    Json,
    Console,
}

/// `zap-time-encoding` values (`zapcore.*TimeEncoder`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TimeEncoding {
    /// `EpochTimeEncoder`: float seconds.
    Epoch,
    /// `EpochMillisTimeEncoder`: float milliseconds.
    Millis,
    /// `EpochNanosTimeEncoder`: integer nanoseconds.
    Nanos,
    /// `ISO8601TimeEncoder`: `2006-01-02T15:04:05.000Z0700`
    Iso8601,
    /// `RFC3339TimeEncoder`: `2006-01-02T15:04:05Z07:00`
    Rfc3339,
    /// `RFC3339NanoTimeEncoder`: `2006-01-02T15:04:05.999999999Z07:00`
    Rfc3339Nano,
}

/// Destination of log output (`Options.DestWriter`).
pub type Dest = Arc<Mutex<dyn Write + Send>>;

/// Clock used for entry timestamps.
pub type Clock = Arc<dyn Fn() -> DateTime<FixedOffset> + Send + Sync>;

/// `zap.Options`
#[derive(Clone, Default)]
pub struct ZapOptions {
    /// `Development`
    pub development: bool,
    /// `NewEncoder` (None = console in development, JSON otherwise).
    pub encoder: Option<EncoderKind>,
    /// `Level` as zap level (None = Debug in development, Info otherwise).
    pub level: Option<i8>,
    /// `StacktraceLevel` (None = Warn in development, Error otherwise).
    pub stacktrace_level: Option<i8>,
    /// `TimeEncoder` (None = RFC3339).
    pub time_encoding: Option<TimeEncoding>,
    /// `DestWriter` (None = stderr).
    pub dest: Option<Dest>,
    /// `zap.WithClock` (None = local wall clock, like Go's `time.Now()`).
    pub clock: Option<Clock>,
}

impl std::fmt::Debug for ZapOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZapOptions")
            .field("development", &self.development)
            .field("encoder", &self.encoder)
            .field("level", &self.level)
            .field("stacktrace_level", &self.stacktrace_level)
            .field("time_encoding", &self.time_encoding)
            .finish()
    }
}

pub const FLAG_ZAP_DEVEL: &str = "zap-devel";
pub const FLAG_ZAP_ENCODER: &str = "zap-encoder";
pub const FLAG_ZAP_LOG_LEVEL: &str = "zap-log-level";
pub const FLAG_ZAP_STACKTRACE_LEVEL: &str = "zap-stacktrace-level";
pub const FLAG_ZAP_TIME_ENCODING: &str = "zap-time-encoding";

/// `encoderFlag`
#[derive(Default)]
pub struct EncoderFlag {
    pub value: String,
    pub parsed: Option<EncoderKind>,
}
impl FlagValue for EncoderFlag {
    fn set(&mut self, flag_value: &str) -> Result<(), String> {
        let val = flag_value.to_lowercase();
        self.parsed = Some(match val.as_str() {
            "json" => EncoderKind::Json,
            "console" => EncoderKind::Console,
            _ => return Err(format!("invalid encoder value \"{flag_value}\"")),
        });
        self.value = flag_value.to_string();
        Ok(())
    }
    fn string(&self) -> String {
        self.value.clone()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `levelFlag`
#[derive(Default)]
pub struct LevelFlag {
    pub value: String,
    pub parsed: Option<i8>,
}
impl FlagValue for LevelFlag {
    fn set(&mut self, flag_value: &str) -> Result<(), String> {
        let level = match flag_value.to_lowercase().as_str() {
            "debug" => Some(DEBUG_LEVEL),
            "info" => Some(INFO_LEVEL),
            "error" => Some(ERROR_LEVEL),
            "panic" => Some(PANIC_LEVEL),
            _ => None,
        };
        match level {
            Some(l) => self.parsed = Some(l),
            None => {
                let log_level = go_atoi(flag_value)
                    .ok_or_else(|| format!("invalid log level \"{flag_value}\""))?;
                if log_level > 0 {
                    let int_level = -log_level;
                    // zapcore.Level(int8(intLevel)) truncates like Go.
                    self.parsed = Some(int_level as i8);
                } else {
                    return Err(format!("invalid log level \"{flag_value}\""));
                }
            }
        }
        self.value = flag_value.to_string();
        Ok(())
    }
    fn string(&self) -> String {
        self.value.clone()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `stackTraceFlag`
#[derive(Default)]
pub struct StackTraceFlag {
    pub value: String,
    pub parsed: Option<i8>,
}
impl FlagValue for StackTraceFlag {
    fn set(&mut self, flag_value: &str) -> Result<(), String> {
        let level = match flag_value.to_lowercase().as_str() {
            "info" => INFO_LEVEL,
            "error" => ERROR_LEVEL,
            "panic" => PANIC_LEVEL,
            _ => return Err(format!("invalid stacktrace level \"{flag_value}\"")),
        };
        self.parsed = Some(level);
        self.value = flag_value.to_string();
        Ok(())
    }
    fn string(&self) -> String {
        self.value.clone()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `timeEncodingFlag`
#[derive(Default)]
pub struct TimeEncodingFlag {
    pub value: String,
    pub parsed: Option<TimeEncoding>,
}
impl FlagValue for TimeEncodingFlag {
    fn set(&mut self, flag_value: &str) -> Result<(), String> {
        let val = flag_value.to_lowercase();
        self.parsed = Some(match val.as_str() {
            "rfc3339nano" => TimeEncoding::Rfc3339Nano,
            "rfc3339" => TimeEncoding::Rfc3339,
            "iso8601" => TimeEncoding::Iso8601,
            "millis" => TimeEncoding::Millis,
            "nanos" => TimeEncoding::Nanos,
            "epoch" => TimeEncoding::Epoch,
            _ => return Err(format!("invalid time-encoding value \"{flag_value}\"")),
        });
        self.value = flag_value.to_string();
        Ok(())
    }
    fn string(&self) -> String {
        self.value.clone()
    }
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// `strconv.Atoi` (base 10, optional sign, 64-bit).
fn go_atoi(s: &str) -> Option<i64> {
    let (neg, body) = match s.as_bytes().first() {
        Some(b'+') => (false, &s[1..]),
        Some(b'-') => (true, &s[1..]),
        _ => (false, s),
    };
    if body.is_empty() || !body.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut n: i128 = 0;
    for b in body.bytes() {
        n = n * 10 + (b - b'0') as i128;
        if n > (1i128 << 63) {
            return None;
        }
    }
    let n = if neg { -n } else { n };
    i64::try_from(n).ok()
}

impl ZapOptions {
    /// `(*Options).BindFlags(fs)`
    pub fn bind_flags(&self, fs: &mut FlagSet) {
        fs.bool(
            FLAG_ZAP_DEVEL,
            self.development,
            "Development Mode defaults(encoder=consoleEncoder,logLevel=Debug,stackTraceLevel=Warn). \
             Production Mode defaults(encoder=jsonEncoder,logLevel=Info,stackTraceLevel=Error)",
        );
        fs.var(
            FLAG_ZAP_ENCODER,
            Box::<EncoderFlag>::default(),
            "Zap log encoding (one of 'json' or 'console')",
        );
        fs.var(
            FLAG_ZAP_LOG_LEVEL,
            Box::<LevelFlag>::default(),
            "Zap Level to configure the verbosity of logging. Can be one of 'debug', 'info', 'error', 'panic'\
             or any integer value > 0 which corresponds to custom debug levels of increasing verbosity",
        );
        fs.var(
            FLAG_ZAP_STACKTRACE_LEVEL,
            Box::<StackTraceFlag>::default(),
            "Zap Level at and above which stacktraces are captured (one of 'info', 'error', 'panic').",
        );
        fs.var(
            FLAG_ZAP_TIME_ENCODING,
            Box::<TimeEncodingFlag>::default(),
            "Zap time encoding (one of 'epoch', 'millis', 'nano', 'iso8601', 'rfc3339' or 'rfc3339nano'). Defaults to 'epoch'.",
        );
    }

    /// Copy the parsed flag values into the options (Go does this through the flag
    /// callbacks while parsing; only flags present on the command line change anything).
    pub fn apply_flags(&mut self, fs: &FlagSet) {
        if fs.lookup(FLAG_ZAP_DEVEL).is_some() {
            self.development = fs.get_bool(FLAG_ZAP_DEVEL);
        }
        if let Some(v) = fs
            .value::<EncoderFlag>(FLAG_ZAP_ENCODER)
            .and_then(|f| f.parsed)
        {
            self.encoder = Some(v);
        }
        if let Some(v) = fs
            .value::<LevelFlag>(FLAG_ZAP_LOG_LEVEL)
            .and_then(|f| f.parsed)
        {
            self.level = Some(v);
        }
        if let Some(v) = fs
            .value::<StackTraceFlag>(FLAG_ZAP_STACKTRACE_LEVEL)
            .and_then(|f| f.parsed)
        {
            self.stacktrace_level = Some(v);
        }
        if let Some(v) = fs
            .value::<TimeEncodingFlag>(FLAG_ZAP_TIME_ENCODING)
            .and_then(|f| f.parsed)
        {
            self.time_encoding = Some(v);
        }
    }
}

/// zapcore sampler (`NewSamplerWithOptions(core, time.Second, 100, 100)`), used in production
/// mode when the level is above `-2`.
struct Sampler {
    counts: Mutex<Vec<(i64, u64)>>, // (resetAt nanos, count) per [level][hash % 4096]
    tick_nanos: i64,
    first: u64,
    thereafter: u64,
}

const COUNTERS_PER_LEVEL: usize = 4096;
const MIN_LEVEL: i8 = DEBUG_LEVEL;
const MAX_LEVEL: i8 = FATAL_LEVEL;

impl Sampler {
    fn new() -> Self {
        let n = (MAX_LEVEL - MIN_LEVEL + 1) as usize * COUNTERS_PER_LEVEL;
        Self {
            counts: Mutex::new(vec![(0, 0); n]),
            tick_nanos: 1_000_000_000,
            first: 100,
            thereafter: 100,
        }
    }

    fn fnv32a(s: &str) -> u32 {
        let mut h: u32 = 2166136261;
        for b in s.bytes() {
            h ^= b as u32;
            h = h.wrapping_mul(16777619);
        }
        h
    }

    /// Whether the entry is kept.
    fn check(&self, level: i8, msg: &str, now_nanos: i64) -> bool {
        if !(MIN_LEVEL..=MAX_LEVEL).contains(&level) {
            return true;
        }
        let idx = (level - MIN_LEVEL) as usize * COUNTERS_PER_LEVEL
            + (Self::fnv32a(msg) as usize % COUNTERS_PER_LEVEL);
        let Ok(mut counts) = self.counts.lock() else {
            return true;
        };
        let c = &mut counts[idx];
        let n = if now_nanos > c.0 {
            c.0 = now_nanos + self.tick_nanos;
            c.1 = 1;
            1
        } else {
            c.1 += 1;
            c.1
        };
        n <= self.first || (n - self.first).is_multiple_of(self.thereafter)
    }
}

/// zap logger + zapr `logr.LogSink` with controller-runtime's `KubeAwareEncoder`.
pub struct ZapSink {
    encoder: EncoderKind,
    level: i8,
    stacktrace_level: i8,
    time_encoding: TimeEncoding,
    /// `KubeAwareEncoder.Verbose` (= Development).
    verbose: bool,
    dest: Option<Dest>,
    clock: Option<Clock>,
    sampler: Option<Sampler>,
    /// Whether to render stack traces (tests can turn this off).
    capture_stacks: bool,
}

impl std::fmt::Debug for ZapSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZapSink")
            .field("encoder", &self.encoder)
            .field("level", &self.level)
            .field("stacktrace_level", &self.stacktrace_level)
            .field("time_encoding", &self.time_encoding)
            .finish()
    }
}

/// One log entry (`zapcore.Entry` + fields).
pub struct Entry<'a> {
    pub level: i8,
    pub time: DateTime<FixedOffset>,
    pub logger_name: &'a str,
    pub message: &'a str,
    pub stack: String,
}

impl ZapSink {
    /// `zap.New(zap.UseFlagOptions(&opts))` (`NewRaw` + `addDefaults`).
    pub fn new(opts: &ZapOptions) -> Self {
        let (def_enc, def_level, def_stack) = if opts.development {
            (EncoderKind::Console, DEBUG_LEVEL, WARN_LEVEL)
        } else {
            (EncoderKind::Json, INFO_LEVEL, ERROR_LEVEL)
        };
        let level = opts.level.unwrap_or(def_level);
        // Production: disable sampling for increased Debug levels.
        let sampler = if !opts.development && level > -2 {
            Some(Sampler::new())
        } else {
            None
        };
        Self {
            encoder: opts.encoder.unwrap_or(def_enc),
            level,
            stacktrace_level: opts.stacktrace_level.unwrap_or(def_stack),
            time_encoding: opts.time_encoding.unwrap_or(TimeEncoding::Rfc3339),
            verbose: opts.development,
            dest: opts.dest.clone(),
            clock: opts.clock.clone(),
            sampler,
            capture_stacks: true,
        }
    }

    /// Disable capturing of stack traces (the stacktrace section is omitted).
    pub fn without_stacktraces(mut self) -> Self {
        self.capture_stacks = false;
        self
    }

    /// Minimum enabled zap level.
    pub fn level(&self) -> i8 {
        self.level
    }

    fn now(&self) -> DateTime<FixedOffset> {
        match &self.clock {
            Some(c) => c(),
            None => Local::now().fixed_offset(),
        }
    }

    fn level_enabled(&self, l: i8) -> bool {
        l >= self.level
    }

    fn write(&self, level: i8, name: &str, msg: &str, fields: &[(String, Value)]) {
        if !self.level_enabled(level) {
            return;
        }
        let time = self.now();
        if let Some(s) = &self.sampler
            && !s.check(level, msg, time.timestamp_nanos_opt().unwrap_or(0))
        {
            return;
        }
        let stack = if level >= self.stacktrace_level && self.capture_stacks {
            take_stacktrace()
        } else {
            String::new()
        };
        let ent = Entry {
            level,
            time,
            logger_name: name,
            message: msg,
            stack,
        };
        let line = self.encode_entry(&ent, fields);
        match &self.dest {
            Some(d) => {
                if let Ok(mut w) = d.lock() {
                    let _ = w.write_all(line.as_bytes());
                }
            }
            None => {
                let _ = std::io::stderr().lock().write_all(line.as_bytes());
            }
        }
    }

    /// Encode one entry (including the trailing newline).
    pub fn encode_entry(&self, ent: &Entry<'_>, fields: &[(String, Value)]) -> String {
        match self.encoder {
            EncoderKind::Console => self.encode_console(ent, fields),
            EncoderKind::Json => self.encode_json(ent, fields),
        }
    }

    fn encode_console(&self, ent: &Entry<'_>, fields: &[(String, Value)]) -> String {
        let mut line = String::new();
        // time, level, name are elements of a slice encoder printed with fmt.Fprint.
        line.push_str(&self.time_console(&ent.time));
        line.push('\t');
        line.push_str(&level_capital(ent.level));
        if !ent.logger_name.is_empty() {
            line.push('\t');
            line.push_str(ent.logger_name);
        }
        line.push('\t');
        line.push_str(ent.message);
        let ctx = self.fields_json(fields, ", ");
        if !ctx.is_empty() {
            line.push('\t');
            line.push('{');
            line.push_str(&ctx);
            line.push('}');
        }
        if !ent.stack.is_empty() {
            line.push('\n');
            line.push_str(&ent.stack);
        }
        line.push('\n');
        line
    }

    fn encode_json(&self, ent: &Entry<'_>, fields: &[(String, Value)]) -> String {
        let mut out = String::from("{");
        out.push_str("\"level\":\"");
        gofmt::zap_escape_into(&mut out, &level_lower(ent.level));
        out.push('"');
        out.push_str(",\"ts\":");
        out.push_str(&self.time_json(&ent.time));
        if !ent.logger_name.is_empty() {
            out.push_str(",\"logger\":\"");
            gofmt::zap_escape_into(&mut out, ent.logger_name);
            out.push('"');
        }
        out.push_str(",\"msg\":\"");
        gofmt::zap_escape_into(&mut out, ent.message);
        out.push('"');
        let f = self.fields_json(fields, ",");
        if !f.is_empty() {
            out.push(',');
            out.push_str(&f);
        }
        if !ent.stack.is_empty() {
            out.push_str(",\"stacktrace\":\"");
            gofmt::zap_escape_into(&mut out, &ent.stack);
            out.push('"');
        }
        out.push_str("}\n");
        out
    }

    /// Fields as zap's JSON encoder writes them (`"k":v` joined; the console encoder's
    /// JSON encoder is created with spaced=true, giving `"k": v, "k2": v2`).
    fn fields_json(&self, fields: &[(String, Value)], sep: &str) -> String {
        let spaced = sep == ", ";
        let mut out = String::new();
        for (i, (k, v)) in fields.iter().enumerate() {
            if i > 0 {
                out.push_str(sep);
            }
            out.push('"');
            gofmt::zap_escape_into(&mut out, k);
            out.push('"');
            out.push(':');
            if spaced {
                out.push(' ');
            }
            if k == "errorCauses" {
                // Encoded by zap's own encoder (errArray/ObjectMarshaler), not json.Marshal,
                // so nested objects follow the encoder's spacing.
                zap_marshaler_into(&mut out, v, spaced);
            } else {
                self.field_value(&mut out, v, spaced);
            }
        }
        out
    }

    fn field_value(&self, out: &mut String, v: &Value, spaced: bool) {
        match v {
            Value::String(s) => {
                out.push('"');
                gofmt::zap_escape_into(out, s);
                out.push('"');
            }
            Value::Number(n) => out.push_str(&gofmt::number_zap(n)),
            Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
            Value::Null => out.push_str("null"),
            Value::Object(m) if !self.verbose && looks_like_kube_object(m) => {
                // KubeAwareEncoder (non-verbose): apiVersion, kind, namespace, name.
                out.push('{');
                let mut first = true;
                let mut push = |out: &mut String, k: &str, val: &str| {
                    if !first {
                        out.push_str(if spaced { ", " } else { "," });
                    }
                    first = false;
                    out.push('"');
                    out.push_str(k);
                    out.push_str(if spaced { "\": \"" } else { "\":\"" });
                    gofmt::zap_escape_into(out, val);
                    out.push('"');
                };
                let s = |v: Option<&Value>| v.and_then(Value::as_str).unwrap_or("").to_string();
                let api_version = s(m.get("apiVersion"));
                if !api_version.is_empty() {
                    push(out, "apiVersion", &api_version);
                    push(out, "kind", &s(m.get("kind")));
                }
                let md = m.get("metadata");
                let ns = s(md.and_then(|md| md.get("namespace")));
                if !ns.is_empty() {
                    push(out, "namespace", &ns);
                }
                push(out, "name", &s(md.and_then(|md| md.get("name"))));
                out.push('}');
            }
            other => gofmt::json_value_into(out, other),
        }
    }

    fn time_console(&self, t: &DateTime<FixedOffset>) -> String {
        match self.time_encoding {
            TimeEncoding::Epoch => gofmt::format_float_v(epoch_nanos(t) as f64 / 1e9),
            TimeEncoding::Millis => gofmt::format_float_v(epoch_nanos(t) as f64 / 1e6),
            TimeEncoding::Nanos => epoch_nanos(t).to_string(),
            _ => self.time_string(t),
        }
    }

    fn time_json(&self, t: &DateTime<FixedOffset>) -> String {
        match self.time_encoding {
            TimeEncoding::Epoch => gofmt::format_float_f(epoch_nanos(t) as f64 / 1e9),
            TimeEncoding::Millis => gofmt::format_float_f(epoch_nanos(t) as f64 / 1e6),
            TimeEncoding::Nanos => epoch_nanos(t).to_string(),
            _ => {
                let mut s = String::from("\"");
                gofmt::zap_escape_into(&mut s, &self.time_string(t));
                s.push('"');
                s
            }
        }
    }

    fn time_string(&self, t: &DateTime<FixedOffset>) -> String {
        match self.time_encoding {
            TimeEncoding::Iso8601 => format_iso8601(t),
            TimeEncoding::Rfc3339Nano => format_rfc3339_nano(t),
            _ => format_rfc3339(t),
        }
    }
}

fn looks_like_kube_object(m: &serde_json::Map<String, Value>) -> bool {
    m.get("metadata").is_some_and(Value::is_object)
        && (m.contains_key("apiVersion") || m.contains_key("kind") || m.contains_key("spec"))
}

fn epoch_nanos(t: &DateTime<FixedOffset>) -> i64 {
    t.timestamp_nanos_opt().unwrap_or(0)
}

fn zone(t: &DateTime<FixedOffset>, colon: bool) -> String {
    let off = t.offset().local_minus_utc();
    if off == 0 {
        return "Z".into();
    }
    let sign = if off < 0 { '-' } else { '+' };
    let off = off.abs();
    let (h, m) = (off / 3600, (off / 60) % 60);
    if colon {
        format!("{sign}{h:02}:{m:02}")
    } else {
        format!("{sign}{h:02}{m:02}")
    }
}

/// Go `time.RFC3339`.
pub fn format_rfc3339(t: &DateTime<FixedOffset>) -> String {
    format!("{}{}", t.format("%Y-%m-%dT%H:%M:%S"), zone(t, true))
}

/// Go `time.RFC3339Nano` (trailing zeros of the fraction trimmed).
pub fn format_rfc3339_nano(t: &DateTime<FixedOffset>) -> String {
    let ns = t.nanosecond() % 1_000_000_000;
    let mut frac = String::new();
    if ns != 0 {
        frac = format!(".{ns:09}");
        while frac.ends_with('0') {
            frac.pop();
        }
    }
    format!("{}{}{}", t.format("%Y-%m-%dT%H:%M:%S"), frac, zone(t, true))
}

/// zap `ISO8601TimeEncoder`: `2006-01-02T15:04:05.000Z0700`.
pub fn format_iso8601(t: &DateTime<FixedOffset>) -> String {
    let ms = (t.nanosecond() % 1_000_000_000) / 1_000_000;
    format!(
        "{}.{ms:03}{}",
        t.format("%Y-%m-%dT%H:%M:%S"),
        zone(t, false)
    )
}

/// Capture the current stack as zap formats it (`function\n\tfile:line` per frame, frames
/// joined by `\n`), skipping logging internals and runtime/library frames.
pub fn take_stacktrace() -> String {
    let bt = std::backtrace::Backtrace::force_capture().to_string();
    let mut frames: Vec<(String, Option<String>)> = Vec::new();
    for line in bt.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("at ") {
            if let Some(last) = frames.last_mut()
                && last.1.is_none()
            {
                // file:line:col -> file:line
                let loc = match rest.rsplit_once(':') {
                    Some((fl, col)) if col.bytes().all(|b| b.is_ascii_digit()) => fl.to_string(),
                    _ => rest.to_string(),
                };
                last.1 = Some(loc);
            }
        } else if let Some((idx, func)) = t.split_once(": ")
            && idx.bytes().all(|b| b.is_ascii_digit())
        {
            frames.push((strip_hash(func.trim()), None));
        }
    }
    // Drop everything up to (and including) the last logging-internal frame.
    let internal = |f: &str| {
        f.starts_with("ctrlrt::log::")
            || f.starts_with("<ctrlrt::log::")
            || f.starts_with("ctrlrt_cmd::zap::")
            || f.starts_with("<ctrlrt_cmd::zap::")
            || f.starts_with("std::backtrace")
    };
    let start = frames
        .iter()
        .rposition(|(f, _)| internal(f))
        .map(|i| i + 1)
        .unwrap_or(0);
    let skip = |f: &str| {
        const PREFIXES: &[&str] = &[
            "std::",
            "core::",
            "alloc::",
            "tokio::",
            "futures_util::",
            "futures_core::",
            "<std::",
            "<core::",
            "<alloc::",
            "<tokio::",
            "<futures_util::",
            "<F as ",
            "__rust",
            "_start",
            "__libc",
            "<unknown>",
            "main_0",
            "start_thread",
            "clone",
            "__clone",
            "<fn(",
            "test::",
        ];
        PREFIXES.iter().any(|p| f.starts_with(p)) || f.contains(" as core::ops::function::")
    };
    let mut out: Vec<String> = Vec::new();
    for (f, loc) in frames.into_iter().skip(start) {
        if skip(&f) {
            continue;
        }
        match loc {
            Some(l) => out.push(format!("{f}\n\t{l}")),
            None => out.push(f),
        }
    }
    out.join("\n")
}

/// Strip the `::h0123456789abcdef` symbol hash suffix if present.
fn strip_hash(f: &str) -> String {
    if let Some((head, tail)) = f.rsplit_once("::h")
        && tail.len() == 16
        && tail.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return head.to_string();
    }
    f.to_string()
}

impl ctrlrt::log::LogSink for ZapSink {
    fn enabled(&self, level: i32) -> bool {
        self.level_enabled(to_zap_level(level))
    }

    fn info(&self, level: i32, name: &str, msg: &str, kvs: &[(String, Value)]) {
        self.write(to_zap_level(level), name, msg, kvs);
    }

    fn error(&self, err: Option<&str>, name: &str, msg: &str, kvs: &[(String, Value)]) {
        if !self.level_enabled(ERROR_LEVEL) {
            return;
        }
        match err {
            Some(e) => {
                let mut all = Vec::with_capacity(kvs.len() + 1);
                all.extend_from_slice(kvs);
                all.push(("error".to_string(), Value::String(e.to_string())));
                self.write(ERROR_LEVEL, name, msg, &all);
            }
            None => self.write(ERROR_LEVEL, name, msg, kvs),
        }
    }

    /// zap's `Error` field for an error implementing `errorGroup` (`Errors() []error`):
    /// `"error": <msg>, "errorCauses": [{"error": <cause>}, ...]`.
    fn error_with_causes(
        &self,
        err: &str,
        causes: &[String],
        name: &str,
        msg: &str,
        kvs: &[(String, Value)],
    ) {
        if !self.level_enabled(ERROR_LEVEL) {
            return;
        }
        let mut all = Vec::with_capacity(kvs.len() + 2);
        all.extend_from_slice(kvs);
        all.push(("error".to_string(), Value::String(err.to_string())));
        all.push((
            "errorCauses".to_string(),
            Value::Array(
                causes
                    .iter()
                    .map(|c| serde_json::json!({ "error": c }))
                    .collect(),
            ),
        ));
        self.write(ERROR_LEVEL, name, msg, &all);
    }
}

/// Render a value the way zap's encoder renders `ArrayMarshaler`/`ObjectMarshaler` content
/// (console encoder: `", "` and `": "` separators; JSON encoder: none).
fn zap_marshaler_into(out: &mut String, v: &Value, spaced: bool) {
    match v {
        Value::Array(a) => {
            out.push('[');
            for (i, e) in a.iter().enumerate() {
                if i > 0 {
                    out.push_str(if spaced { ", " } else { "," });
                }
                zap_marshaler_into(out, e, spaced);
            }
            out.push(']');
        }
        Value::Object(m) => {
            out.push('{');
            for (i, (k, e)) in m.iter().enumerate() {
                if i > 0 {
                    out.push_str(if spaced { ", " } else { "," });
                }
                out.push('"');
                gofmt::zap_escape_into(out, k);
                out.push_str(if spaced { "\": " } else { "\":" });
                zap_marshaler_into(out, e, spaced);
            }
            out.push('}');
        }
        Value::String(st) => {
            out.push('"');
            gofmt::zap_escape_into(out, st);
            out.push('"');
        }
        other => gofmt::json_value_into(out, other),
    }
}

/// A fixed clock (for tests and golden comparisons).
pub fn fixed_clock(t: DateTime<FixedOffset>) -> Clock {
    Arc::new(move || t)
}

/// Current time in UTC as a fixed-offset time.
pub fn utc_now() -> DateTime<FixedOffset> {
    Utc::now().fixed_offset()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn error_causes_rendering_matches_zap() {
        // Go: zap console encoder, `unable to get kubeconfig` with clientcmd's aggregate error.
        let causes = ["no configuration has been provided".to_string()];
        let mut out = String::new();
        zap_marshaler_into(
            &mut out,
            &Value::Array(
                causes
                    .iter()
                    .map(|c| serde_json::json!({"error": c}))
                    .collect(),
            ),
            true,
        );
        assert_eq!(out, r#"[{"error": "no configuration has been provided"}]"#);
        let mut out = String::new();
        zap_marshaler_into(
            &mut out,
            &Value::Array(
                causes
                    .iter()
                    .map(|c| serde_json::json!({"error": c}))
                    .collect(),
            ),
            false,
        );
        assert_eq!(out, r#"[{"error":"no configuration has been provided"}]"#);
    }

    #[test]
    fn time_formats() {
        let t = FixedOffset::east_opt(0)
            .unwrap()
            .with_ymd_and_hms(2024, 1, 2, 3, 4, 5)
            .unwrap()
            + chrono::Duration::nanoseconds(123_456_700);
        assert_eq!(format_rfc3339(&t), "2024-01-02T03:04:05Z");
        assert_eq!(format_rfc3339_nano(&t), "2024-01-02T03:04:05.1234567Z");
        assert_eq!(format_iso8601(&t), "2024-01-02T03:04:05.123Z");
        let t2 = FixedOffset::east_opt(2 * 3600 + 30 * 60)
            .unwrap()
            .with_ymd_and_hms(2024, 1, 2, 3, 4, 5)
            .unwrap();
        assert_eq!(format_rfc3339(&t2), "2024-01-02T03:04:05+02:30");
        assert_eq!(format_rfc3339_nano(&t2), "2024-01-02T03:04:05+02:30");
        assert_eq!(format_iso8601(&t2), "2024-01-02T03:04:05.000+0230");
    }

    #[test]
    fn level_flag() {
        let mut f = LevelFlag::default();
        assert_eq!(f.set("DEBUG"), Ok(()));
        assert_eq!(f.parsed, Some(-1));
        assert_eq!(f.set("3"), Ok(()));
        assert_eq!(f.parsed, Some(-3));
        assert_eq!(f.set("200"), Ok(()));
        assert_eq!(f.parsed, Some(56));
        assert_eq!(f.set("0"), Err("invalid log level \"0\"".into()));
        assert_eq!(f.set("warn"), Err("invalid log level \"warn\"".into()));
        assert_eq!(f.string(), "200");
    }

    #[test]
    fn stacktrace_is_zap_shaped() {
        let s = take_stacktrace();
        // Every frame is "func" optionally followed by "\n\tfile:line".
        for l in s.lines() {
            assert!(!l.is_empty());
        }
        assert!(!s.contains("ctrlrt_cmd::zap::take_stacktrace"), "{s}");
    }
}
