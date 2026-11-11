//! Port of Go's `flag` package (the subset used by controller-runtime based binaries).
//!
//! Parsing rules, error messages, usage output and exit codes are identical to Go:
//!
//! * `-flag`, `--flag`, `-flag=value`, `--flag=value`; `-flag value` for non-boolean flags.
//! * Boolean flags only take a value through `-flag=value` (`-flag false` sets the flag to
//!   `true` and stops at the positional argument `false`, as in Go).
//! * `--` terminates flag parsing; parsing stops at the first non-flag argument (`-` alone is
//!   a positional argument).
//! * `-h` / `-help` (when not defined) print the usage and return [`FlagError::Help`]
//!   (exit code 0 with [`ErrorHandling::ExitOnError`]).
//! * Any other error prints `<message>\n` followed by the usage to the output (stderr by
//!   default) and returns [`FlagError::Invalid`] (exit code 2 with `ExitOnError`).
//!
//! ```text
//! let mut fs = FlagSet::command_line();
//! fs.string("metrics-bind-address", "0", "The address ...");
//! fs.bool("leader-elect", false, "Enable leader election ...");
//! fs.parse_env();                       // flag.Parse()
//! let addr = fs.get_string("metrics-bind-address");
//! ```

use std::any::Any;
use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::godur;

/// `flag.ErrorHandling`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ErrorHandling {
    /// Return a descriptive error.
    ContinueOnError,
    /// Call `os.Exit(2)` (or `os.Exit(0)` for `-h`/`-help`).
    ExitOnError,
    /// Panic with a descriptive error.
    PanicOnError,
}

/// Error returned by [`FlagSet::parse`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FlagError {
    /// `flag.ErrHelp`: `-help` or `-h` was invoked but no such flag is defined.
    Help,
    /// Any other parse error; the message is exactly what Go prints.
    Invalid(String),
}

impl fmt::Display for FlagError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FlagError::Help => f.write_str("flag: help requested"),
            FlagError::Invalid(m) => f.write_str(m),
        }
    }
}
impl std::error::Error for FlagError {}

/// `flag.Value` (+ the bits of `flag` that inspect the dynamic type of a value).
pub trait Value: Send + Sync + 'static {
    /// `Set(string) error`; the error message is used in
    /// `invalid value %q for flag -%s: %v`.
    fn set(&mut self, s: &str) -> Result<(), String>;
    /// `String() string`
    fn string(&self) -> String;
    /// `boolFlag.IsBoolFlag()`
    fn is_bool_flag(&self) -> bool {
        false
    }
    /// Placeholder name printed by `PrintDefaults` (Go's `UnquoteUsage`): `""` for bool
    /// flags, `"string"`, `"int"`, `"duration"`, and `"value"` for custom values.
    fn type_name(&self) -> &'static str {
        "value"
    }
    /// `String()` of the zero value of the dynamic type (Go's `isZeroValue`).
    fn zero_string(&self) -> String {
        String::new()
    }
    /// Whether `PrintDefaults` quotes the default (`*stringValue` only).
    fn quote_default(&self) -> bool {
        false
    }
    fn as_any(&self) -> &dyn Any;
}

/// `*stringValue`
#[derive(Clone, Debug, Default)]
pub struct StringValue(pub String);
impl Value for StringValue {
    fn set(&mut self, s: &str) -> Result<(), String> {
        self.0 = s.to_string();
        Ok(())
    }
    fn string(&self) -> String {
        self.0.clone()
    }
    fn type_name(&self) -> &'static str {
        "string"
    }
    fn quote_default(&self) -> bool {
        true
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `*boolValue`
#[derive(Clone, Debug, Default)]
pub struct BoolValue(pub bool);
impl Value for BoolValue {
    fn set(&mut self, s: &str) -> Result<(), String> {
        match parse_bool(s) {
            Some(b) => {
                self.0 = b;
                Ok(())
            }
            None => Err("parse error".into()),
        }
    }
    fn string(&self) -> String {
        self.0.to_string()
    }
    fn is_bool_flag(&self) -> bool {
        true
    }
    fn type_name(&self) -> &'static str {
        ""
    }
    fn zero_string(&self) -> String {
        "false".into()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `*intValue` (Go `int` is 64-bit on the supported platforms).
#[derive(Clone, Debug, Default)]
pub struct IntValue(pub i64);
impl Value for IntValue {
    fn set(&mut self, s: &str) -> Result<(), String> {
        self.0 = parse_int(s)?;
        Ok(())
    }
    fn string(&self) -> String {
        self.0.to_string()
    }
    fn type_name(&self) -> &'static str {
        "int"
    }
    fn zero_string(&self) -> String {
        "0".into()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `*durationValue` (nanoseconds, may be negative like Go's `time.Duration`).
#[derive(Clone, Debug, Default)]
pub struct DurationValue(pub i64);
impl Value for DurationValue {
    fn set(&mut self, s: &str) -> Result<(), String> {
        self.0 = godur::parse_nanos(s).map_err(|_| "parse error".to_string())?;
        Ok(())
    }
    fn string(&self) -> String {
        godur::format_nanos(self.0)
    }
    fn type_name(&self) -> &'static str {
        "duration"
    }
    fn zero_string(&self) -> String {
        "0s".into()
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// `strconv.ParseBool`
pub fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

/// `strconv.ParseInt(s, 0, 64)` with the flag package's error mapping
/// (`parse error` / `value out of range`).
pub fn parse_int(s: &str) -> Result<i64, String> {
    const SYNTAX: &str = "parse error";
    const RANGE: &str = "value out of range";
    if s.is_empty() {
        return Err(SYNTAX.into());
    }
    let (neg, body) = match s.as_bytes()[0] {
        b'+' => (false, &s[1..]),
        b'-' => (true, &s[1..]),
        _ => (false, s),
    };
    let un = parse_uint_base0(body).map_err(|e| {
        if e {
            RANGE.to_string()
        } else {
            SYNTAX.to_string()
        }
    })?;
    let cutoff = 1u64 << 63;
    if !neg && un >= cutoff {
        return Err(RANGE.into());
    }
    if neg && un > cutoff {
        return Err(RANGE.into());
    }
    Ok(if neg {
        (un as i128).wrapping_neg() as i64
    } else {
        un as i64
    })
}

/// `strconv.ParseUint(s, 0, 64)`; Err(true) = range error, Err(false) = syntax error.
fn parse_uint_base0(s: &str) -> Result<u64, bool> {
    if s.is_empty() {
        return Err(false);
    }
    let b = s.as_bytes();
    let (base, digits) = if b[0] == b'0' {
        if b.len() >= 3 && (b[1] | 0x20) == b'b' {
            (2u64, &s[2..])
        } else if b.len() >= 3 && (b[1] | 0x20) == b'o' {
            (8, &s[2..])
        } else if b.len() >= 3 && (b[1] | 0x20) == b'x' {
            (16, &s[2..])
        } else {
            (8, &s[1..])
        }
    } else {
        (10, s)
    };
    let mut n: u64 = 0;
    let mut underscores = false;
    for c in digits.bytes() {
        let d = match c {
            b'_' => {
                underscores = true;
                continue;
            }
            b'0'..=b'9' => (c - b'0') as u64,
            b'a'..=b'z' => (c - b'a' + 10) as u64,
            b'A'..=b'Z' => (c - b'A' + 10) as u64,
            _ => return Err(false),
        };
        if d >= base {
            return Err(false);
        }
        match n.checked_mul(base).and_then(|v| v.checked_add(d)) {
            Some(v) => n = v,
            None => return Err(true),
        }
    }
    if underscores && !underscore_ok(s) {
        return Err(false);
    }
    Ok(n)
}

/// strconv.underscoreOK
fn underscore_ok(s: &str) -> bool {
    let b = s.as_bytes();
    // saw tracks the last character (class) we saw:
    // ^ for beginning of number, 0 for a digit or base prefix, _ for an underscore,
    // ! for none of the above.
    let mut saw = b'^';
    let mut i = 0;
    let mut hex = false;
    if !b.is_empty() && (b[0] == b'-' || b[0] == b'+') {
        i += 1;
    }
    if b.len() - i >= 2 && b[i] == b'0' {
        let c = b[i + 1] | 0x20;
        if c == b'b' || c == b'o' || c == b'x' {
            hex = c == b'x';
            i += 2;
            saw = b'0'; // base prefix counts as a digit for "underscore as digit separator"
        }
    }
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() || (hex && (c | 0x20).is_ascii_hexdigit()) {
            saw = b'0';
        } else if c == b'_' {
            if saw != b'0' {
                return false;
            }
            saw = b'_';
        } else {
            if saw == b'_' {
                return false;
            }
            saw = b'!';
        }
        i += 1;
    }
    saw != b'_'
}

/// `flag.Flag`
pub struct Flag {
    pub name: String,
    pub usage: String,
    pub value: Box<dyn Value>,
    /// Default value (as text); for usage message.
    pub def_value: String,
}

impl fmt::Debug for Flag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Flag")
            .field("name", &self.name)
            .field("value", &self.value.string())
            .field("def_value", &self.def_value)
            .finish()
    }
}

/// Shared output writer (`FlagSet.SetOutput`).
pub type Output = Arc<Mutex<dyn Write + Send>>;

type UsageFn = Arc<dyn Fn(&FlagSet) + Send + Sync>;

/// `flag.FlagSet`
pub struct FlagSet {
    name: String,
    parsed: bool,
    actual: BTreeMap<String, ()>,
    formal: BTreeMap<String, Flag>,
    args: Vec<String>,
    error_handling: ErrorHandling,
    output: Option<Output>,
    usage: Option<UsageFn>,
    /// The global `flag.CommandLine` prints `Usage of <os.Args[0]>:` even when the name is
    /// empty.
    command_line: bool,
}

impl fmt::Debug for FlagSet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FlagSet")
            .field("name", &self.name)
            .field("formal", &self.formal)
            .field("args", &self.args)
            .finish()
    }
}

impl FlagSet {
    /// `flag.NewFlagSet(name, errorHandling)`
    pub fn new(name: impl Into<String>, error_handling: ErrorHandling) -> Self {
        Self {
            name: name.into(),
            parsed: false,
            actual: BTreeMap::new(),
            formal: BTreeMap::new(),
            args: Vec::new(),
            error_handling,
            output: None,
            usage: None,
            command_line: false,
        }
    }

    /// The equivalent of `flag.CommandLine`: named after `os.Args[0]`, `ExitOnError`.
    pub fn command_line() -> Self {
        let name = std::env::args().next().unwrap_or_default();
        Self::command_line_named(name)
    }

    /// `flag.CommandLine` with an explicit program name (`os.Args[0]`).
    pub fn command_line_named(program: impl Into<String>) -> Self {
        let mut fs = Self::new(program, ErrorHandling::ExitOnError);
        fs.command_line = true;
        fs
    }

    /// `Name()`
    pub fn name(&self) -> &str {
        &self.name
    }

    /// `ErrorHandling()`
    pub fn error_handling(&self) -> ErrorHandling {
        self.error_handling
    }

    /// `SetOutput(w)`
    pub fn set_output(&mut self, w: Output) {
        self.output = Some(w);
    }

    /// `fs.Usage = f`
    pub fn set_usage(&mut self, f: impl Fn(&FlagSet) + Send + Sync + 'static) {
        self.usage = Some(Arc::new(f));
    }

    fn write_out(&self, s: &str) {
        match &self.output {
            Some(w) => {
                if let Ok(mut w) = w.lock() {
                    let _ = w.write_all(s.as_bytes());
                    let _ = w.flush();
                }
            }
            None => {
                let mut e = std::io::stderr().lock();
                let _ = e.write_all(s.as_bytes());
            }
        }
    }

    /// `Var(value, name, usage)`. Panics with Go's message if the flag is redefined.
    pub fn var(&mut self, name: &str, value: Box<dyn Value>, usage: &str) {
        // Flag must not begin "-" or contain "=".
        if name.starts_with('-') {
            panic!("flag {name:?} begins with -");
        } else if name.contains('=') {
            panic!("flag {name:?} contains =");
        }
        if self.formal.contains_key(name) {
            let msg = if self.name.is_empty() {
                format!("flag redefined: {name}")
            } else {
                format!("{} flag redefined: {name}", self.name)
            };
            self.write_out(&format!("{msg}\n"));
            panic!("{msg}");
        }
        let def_value = value.string();
        self.formal.insert(
            name.to_string(),
            Flag {
                name: name.to_string(),
                usage: usage.to_string(),
                value,
                def_value,
            },
        );
    }

    /// `StringVar` / `String`
    pub fn string(&mut self, name: &str, value: &str, usage: &str) {
        self.var(name, Box::new(StringValue(value.to_string())), usage);
    }
    /// `BoolVar` / `Bool`
    pub fn bool(&mut self, name: &str, value: bool, usage: &str) {
        self.var(name, Box::new(BoolValue(value)), usage);
    }
    /// `IntVar` / `Int`
    pub fn int(&mut self, name: &str, value: i64, usage: &str) {
        self.var(name, Box::new(IntValue(value)), usage);
    }
    /// `DurationVar` / `Duration`
    pub fn duration(&mut self, name: &str, value: Duration, usage: &str) {
        let n = i64::try_from(value.as_nanos()).unwrap_or(i64::MAX);
        self.var(name, Box::new(DurationValue(n)), usage);
    }

    /// `Lookup(name)`
    pub fn lookup(&self, name: &str) -> Option<&Flag> {
        self.formal.get(name)
    }

    /// Typed access to a flag's value (`*T` returned by `flag.String` & co.).
    pub fn value<T: 'static>(&self, name: &str) -> Option<&T> {
        self.formal
            .get(name)
            .and_then(|f| f.value.as_any().downcast_ref::<T>())
    }

    /// Current value of a string flag (empty if not defined).
    pub fn get_string(&self, name: &str) -> String {
        self.value::<StringValue>(name)
            .map(|v| v.0.clone())
            .unwrap_or_default()
    }
    /// Current value of a bool flag (false if not defined).
    pub fn get_bool(&self, name: &str) -> bool {
        self.value::<BoolValue>(name).map(|v| v.0).unwrap_or(false)
    }
    /// Current value of an int flag (0 if not defined).
    pub fn get_int(&self, name: &str) -> i64 {
        self.value::<IntValue>(name).map(|v| v.0).unwrap_or(0)
    }
    /// Current value of a duration flag in nanoseconds (0 if not defined).
    pub fn get_duration_nanos(&self, name: &str) -> i64 {
        self.value::<DurationValue>(name).map(|v| v.0).unwrap_or(0)
    }
    /// Current value of a duration flag (negative durations clamp to zero).
    pub fn get_duration(&self, name: &str) -> Duration {
        Duration::from_nanos(self.get_duration_nanos(name).max(0) as u64)
    }

    /// Whether the flag was set on the command line (or through [`FlagSet::set`]).
    pub fn is_set(&self, name: &str) -> bool {
        self.actual.contains_key(name)
    }

    /// `Set(name, value)`
    pub fn set(&mut self, name: &str, value: &str) -> Result<(), String> {
        let Some(flag) = self.formal.get_mut(name) else {
            return Err(format!("no such flag -{name}"));
        };
        flag.value.set(value)?;
        self.actual.insert(name.to_string(), ());
        Ok(())
    }

    /// `VisitAll(fn)`: all flags in lexicographical order.
    pub fn visit_all(&self, mut f: impl FnMut(&Flag)) {
        for flag in self.formal.values() {
            f(flag);
        }
    }

    /// `Visit(fn)`: only the flags that have been set, in lexicographical order.
    pub fn visit(&self, mut f: impl FnMut(&Flag)) {
        for name in self.actual.keys() {
            if let Some(flag) = self.formal.get(name) {
                f(flag);
            }
        }
    }

    /// `NFlag()`
    pub fn n_flag(&self) -> usize {
        self.actual.len()
    }
    /// `Args()`
    pub fn args(&self) -> &[String] {
        &self.args
    }
    /// `NArg()`
    pub fn n_arg(&self) -> usize {
        self.args.len()
    }
    /// `Arg(i)`
    pub fn arg(&self, i: usize) -> &str {
        self.args.get(i).map(String::as_str).unwrap_or("")
    }
    /// `Parsed()`
    pub fn parsed(&self) -> bool {
        self.parsed
    }

    /// `PrintDefaults()` rendered to a string.
    pub fn defaults_string(&self) -> String {
        let mut out = String::new();
        for flag in self.formal.values() {
            let mut b = format!("  -{}", flag.name);
            let (name, usage) = unquote_usage(flag);
            if !name.is_empty() {
                b.push(' ');
                b.push_str(&name);
            }
            // Boolean flags of one ASCII letter are so common we
            // treat them specially, putting their usage on the same line.
            if b.len() <= 4 {
                b.push('\t');
            } else {
                // Four spaces before the tab triggers good alignment
                // for both 4- and 8-space tab stops.
                b.push_str("\n    \t");
            }
            b.push_str(&usage.replace('\n', "\n    \t"));

            // Print the default value only if it differs to the zero value
            if flag.def_value != flag.value.zero_string() {
                if flag.value.quote_default() {
                    b.push_str(&format!(
                        " (default {})",
                        ctrlrt::error::go_quote(&flag.def_value)
                    ));
                } else {
                    b.push_str(&format!(" (default {})", flag.def_value));
                }
            }
            out.push_str(&b);
            out.push('\n');
        }
        out
    }

    /// `PrintDefaults()`
    pub fn print_defaults(&self) {
        self.write_out(&self.defaults_string());
    }

    /// The default usage text (`Usage of <name>:` + defaults).
    pub fn default_usage_string(&self) -> String {
        let header = if self.name.is_empty() && !self.command_line {
            "Usage:\n".to_string()
        } else {
            format!("Usage of {}:\n", self.name)
        };
        header + &self.defaults_string()
    }

    /// Invoke the usage function (`fs.Usage()` / `defaultUsage`).
    pub fn usage(&self) {
        match &self.usage {
            Some(u) => {
                let u = u.clone();
                u(self)
            }
            None => self.write_out(&self.default_usage_string()),
        }
    }

    fn failf(&self, msg: String) -> FlagError {
        self.write_out(&format!("{msg}\n"));
        self.usage();
        FlagError::Invalid(msg)
    }

    /// parseOne: Ok(true) = a flag was consumed, Ok(false) = done.
    fn parse_one(&mut self) -> Result<bool, FlagError> {
        if self.args.is_empty() {
            return Ok(false);
        }
        let s = self.args[0].clone();
        let sb = s.as_bytes();
        if sb.len() < 2 || sb[0] != b'-' {
            return Ok(false);
        }
        let mut num_minuses = 1;
        if sb[1] == b'-' {
            num_minuses += 1;
            if sb.len() == 2 {
                // "--" terminates the flags
                self.args.remove(0);
                return Ok(false);
            }
        }
        let mut name = &s[num_minuses..];
        if name.is_empty() || name.starts_with('-') || name.starts_with('=') {
            return Err(self.failf(format!("bad flag syntax: {s}")));
        }

        // it's a flag. does it have an argument?
        self.args.remove(0);
        let mut has_value = false;
        let mut value = String::new();
        if let Some(i) = name[1..].find('=') {
            let i = i + 1; // equals cannot be first
            value = name[i + 1..].to_string();
            has_value = true;
            name = &name[..i];
        }
        let name = name.to_string();

        let is_bool = match self.formal.get(&name) {
            None => {
                if name == "help" || name == "h" {
                    // special case for nice help message.
                    self.usage();
                    return Err(FlagError::Help);
                }
                return Err(self.failf(format!("flag provided but not defined: -{name}")));
            }
            Some(f) => f.value.is_bool_flag(),
        };

        if is_bool {
            // special case: doesn't need an arg
            if has_value {
                let r = self.formal.get_mut(&name).map(|f| f.value.set(&value));
                if let Some(Err(e)) = r {
                    return Err(self.failf(format!(
                        "invalid boolean value {} for -{name}: {e}",
                        ctrlrt::error::go_quote(&value)
                    )));
                }
            } else {
                let r = self.formal.get_mut(&name).map(|f| f.value.set("true"));
                if let Some(Err(e)) = r {
                    return Err(self.failf(format!("invalid boolean flag {name}: {e}")));
                }
            }
        } else {
            // It must have a value, which might be the next argument.
            if !has_value && !self.args.is_empty() {
                // value is the next arg
                has_value = true;
                value = self.args.remove(0);
            }
            if !has_value {
                return Err(self.failf(format!("flag needs an argument: -{name}")));
            }
            let r = self.formal.get_mut(&name).map(|f| f.value.set(&value));
            if let Some(Err(e)) = r {
                return Err(self.failf(format!(
                    "invalid value {} for flag -{name}: {e}",
                    ctrlrt::error::go_quote(&value)
                )));
            }
        }
        self.actual.insert(name, ());
        Ok(true)
    }

    /// `Parse(arguments)` (arguments exclude the program name). With `ExitOnError` the
    /// process exits (0 for help, 2 otherwise); with `PanicOnError` it panics.
    pub fn parse<I, S>(&mut self, arguments: I) -> Result<(), FlagError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.parsed = true;
        self.args = arguments.into_iter().map(Into::into).collect();
        loop {
            match self.parse_one() {
                Ok(true) => continue,
                Ok(false) => break,
                Err(err) => match self.error_handling {
                    ErrorHandling::ContinueOnError => return Err(err),
                    ErrorHandling::ExitOnError => {
                        if err == FlagError::Help {
                            std::process::exit(0);
                        }
                        std::process::exit(2);
                    }
                    ErrorHandling::PanicOnError => panic!("{err}"),
                },
            }
        }
        Ok(())
    }

    /// `flag.Parse()`: parse `os.Args[1:]`.
    pub fn parse_env(&mut self) -> Result<(), FlagError> {
        self.parse(std::env::args().skip(1))
    }
}

/// `flag.UnquoteUsage`: extracts a back-quoted name from the usage string and returns it
/// and the un-quoted usage; otherwise the value type name.
pub fn unquote_usage(flag: &Flag) -> (String, String) {
    let usage = &flag.usage;
    if let Some(i) = usage.find('`')
        && let Some(j) = usage[i + 1..].find('`')
    {
        let j = i + 1 + j;
        let name = usage[i + 1..j].to_string();
        let un = format!("{}{}{}", &usage[..i], name, &usage[j + 1..]);
        return (name, un);
    }
    (flag.value.type_name().to_string(), usage.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    pub(crate) struct Buf(pub Vec<u8>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn new_fs() -> (FlagSet, Arc<Mutex<Buf>>) {
        let buf = Arc::new(Mutex::new(Buf::default()));
        let mut fs = FlagSet::new("test", ErrorHandling::ContinueOnError);
        fs.set_output(buf.clone());
        fs.bool("b", false, "bool value");
        fs.bool("bool2", true, "bool2 value");
        fs.int("int", 0, "int value");
        fs.string("string", "0", "string value");
        fs.duration("duration", Duration::from_secs(0), "duration value");
        (fs, buf)
    }

    fn out(buf: &Arc<Mutex<Buf>>) -> String {
        String::from_utf8(buf.lock().unwrap().0.clone()).unwrap()
    }

    // Go: flag_test.go testParse
    #[test]
    fn parse_basic() {
        let (mut fs, _buf) = new_fs();
        let args = [
            "-b",
            "--bool2=false",
            "--int",
            "22",
            "-string=hello",
            "-duration",
            "2m",
            "extra",
            "one-extra-argument",
        ];
        fs.parse(args).unwrap();
        assert!(fs.parsed());
        assert!(fs.get_bool("b"));
        assert!(!fs.get_bool("bool2"));
        assert_eq!(fs.get_int("int"), 22);
        assert_eq!(fs.get_string("string"), "hello");
        assert_eq!(fs.get_duration("duration"), Duration::from_secs(120));
        assert_eq!(fs.args(), ["extra", "one-extra-argument"]);
        assert_eq!(fs.n_flag(), 5);
    }

    #[test]
    fn terminator_and_stop() {
        let (mut fs, _) = new_fs();
        fs.parse(["-b", "--", "-int=3"]).unwrap();
        assert_eq!(fs.args(), ["-int=3"]);
        assert_eq!(fs.get_int("int"), 0);

        let (mut fs, _) = new_fs();
        fs.parse(["-", "-b"]).unwrap();
        assert_eq!(fs.args(), ["-", "-b"]);
        assert!(!fs.get_bool("b"));

        let (mut fs, _) = new_fs();
        // bool flags do not consume the next argument
        fs.parse(["-b", "false", "-int=1"]).unwrap();
        assert!(fs.get_bool("b"));
        assert_eq!(fs.args(), ["false", "-int=1"]);
    }

    #[test]
    fn errors() {
        let cases: &[(&[&str], &str)] = &[
            (&["-x"], "flag provided but not defined: -x"),
            (&["--x=1"], "flag provided but not defined: -x"),
            (&["---b"], "bad flag syntax: ---b"),
            (&["-=b"], "bad flag syntax: -=b"),
            (&["-int"], "flag needs an argument: -int"),
            (
                &["-int=x"],
                "invalid value \"x\" for flag -int: parse error",
            ),
            (
                &["-int=99999999999999999999"],
                "invalid value \"99999999999999999999\" for flag -int: value out of range",
            ),
            (
                &["-b=maybe"],
                "invalid boolean value \"maybe\" for -b: parse error",
            ),
            (
                &["-duration=5"],
                "invalid value \"5\" for flag -duration: parse error",
            ),
        ];
        for (args, msg) in cases {
            let (mut fs, buf) = new_fs();
            let err = fs.parse(args.iter().copied()).unwrap_err();
            assert_eq!(err, FlagError::Invalid(msg.to_string()));
            let o = out(&buf);
            assert!(o.starts_with(&format!("{msg}\nUsage of test:\n")), "{o}");
        }
    }

    #[test]
    fn help() {
        for h in ["-h", "--help", "-help"] {
            let (mut fs, buf) = new_fs();
            assert_eq!(fs.parse([h]).unwrap_err(), FlagError::Help);
            assert!(out(&buf).starts_with("Usage of test:\n"));
        }
        // a defined -help flag is a normal flag
        let (mut fs, _) = new_fs();
        fs.bool("help", false, "");
        fs.parse(["-help"]).unwrap();
        assert!(fs.get_bool("help"));
    }

    // Go: flag_test.go TestPrintDefaults (subset)
    #[test]
    fn print_defaults() {
        let mut fs = FlagSet::new("", ErrorHandling::ContinueOnError);
        fs.bool("A", false, "for bootstrapping, allow 'any' type");
        fs.bool("Alongflagname", false, "disable bounds checking");
        fs.bool("C", true, "a boolean defaulting to true");
        fs.string("D", "", "set relative `path` for local imports");
        fs.string("E", "0", "issue 23543");
        fs.int("I", 0, "a non-zero `number`");
        fs.int("Z", 0, "an int that defaults to zero");
        fs.duration("maxT", Duration::from_secs(0), "set `timeout` for dial");
        fs.duration("d", Duration::from_secs(90), "a duration");
        fs.string("M", "", "a multiline\nhelp\nstring");
        let want = "  -A\tfor bootstrapping, allow 'any' type
  -Alongflagname
    \tdisable bounds checking
  -C\ta boolean defaulting to true (default true)
  -D path
    \tset relative path for local imports
  -E string
    \tissue 23543 (default \"0\")
  -I number
    \ta non-zero number
  -M string
    \ta multiline
    \thelp
    \tstring
  -Z int
    \tan int that defaults to zero
  -d duration
    \ta duration (default 1m30s)
  -maxT timeout
    \tset timeout for dial
";
        assert_eq!(fs.defaults_string(), want);
        assert_eq!(fs.default_usage_string(), format!("Usage:\n{want}"));
    }

    #[test]
    fn parse_int_like_go() {
        assert_eq!(parse_int("0x1f"), Ok(31));
        assert_eq!(parse_int("0o17"), Ok(15));
        assert_eq!(parse_int("017"), Ok(15));
        assert_eq!(parse_int("0b101"), Ok(5));
        assert_eq!(parse_int("-12"), Ok(-12));
        assert_eq!(parse_int("+12"), Ok(12));
        assert_eq!(parse_int("1_000"), Ok(1000));
        assert_eq!(parse_int("1__000"), Err("parse error".into()));
        assert_eq!(
            parse_int("99999999999999999999x"),
            Err("value out of range".into())
        );
        assert_eq!(parse_int("0x_1f"), Ok(31));
        assert_eq!(parse_int("0"), Ok(0));
        assert_eq!(parse_int("08"), Err("parse error".into()));
        assert_eq!(parse_int("0x"), Err("parse error".into()));
        assert_eq!(parse_int("-9223372036854775808"), Ok(i64::MIN));
        assert_eq!(
            parse_int("9223372036854775808"),
            Err("value out of range".into())
        );
        assert_eq!(parse_int(""), Err("parse error".into()));
    }
}
