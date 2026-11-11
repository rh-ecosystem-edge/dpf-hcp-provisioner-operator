//! A minimal port of the parts of Ginkgo v2 the e2e suite relies on.
//!
//! The Go suite is a single `go test` test (`TestE2E`) that hands control to Ginkgo. The Rust
//! suite mirrors that: one libtest test drives a [`Runner`] which executes the spec tree with
//! Ginkgo's semantics for `Ordered` containers (vendored ginkgo `internal/group.go`):
//!
//! * specs of an ordered container run in declaration order;
//! * `BeforeAll` runs once, before the first spec that needs it; `AfterAll` runs once, after the
//!   last (non-filtered) spec, or as soon as a spec fails (all later specs are then skipped with
//!   "Spec skipped because an earlier spec in an ordered container failed");
//! * `Skip()` inside `BeforeAll` skips every remaining spec of the container ("Spec skipped
//!   because Skip() was called in BeforeAll"), `AfterAll` still runs;
//! * `Skip()` inside an `It`/`BeforeEach` only skips that spec;
//! * `BeforeEach` run outer-to-inner, `AfterEach`/`AfterAll` inner-to-outer and only down to the
//!   nesting level of the node that stopped the spec;
//! * panics in a node are recovered and reported as a failure (`[PANICKED]`);
//! * `BeforeSuite`/`AfterSuite` run once around all specs, and only when at least one spec will
//!   run; a failing `BeforeSuite` skips every spec;
//! * a suite timeout (`-ginkgo.timeout`) interrupts the running node, runs the cleanup nodes and
//!   skips the remaining specs.
//!
//! Spec selection mirrors `-ginkgo.label-filter` (env `E2E_LABEL_FILTER`) and `-ginkgo.focus`
//! (env `E2E_FOCUS`); the suite timeout mirrors `-ginkgo.timeout` (env `E2E_SUITE_TIMEOUT`).

use std::collections::HashMap;
use std::fmt::Debug;
use std::future::Future;
use std::io::Write;
use std::panic::AssertUnwindSafe;
use std::time::Duration;

use futures::FutureExt;
use tokio::time::Instant;

/// Env var selecting specs by label (`-ginkgo.label-filter`), e.g. `!ocp-required`.
pub const LABEL_FILTER_ENV: &str = "E2E_LABEL_FILTER";
/// Env var selecting specs by a regular expression over the full spec text (`-ginkgo.focus`).
pub const FOCUS_ENV: &str = "E2E_FOCUS";
/// Env var with the suite timeout as a Go duration (`-ginkgo.timeout`), default `1h`.
pub const SUITE_TIMEOUT_ENV: &str = "E2E_SUITE_TIMEOUT";
/// Ginkgo's default `--timeout`.
pub const DEFAULT_SUITE_TIMEOUT: Duration = Duration::from_secs(3600);
/// Ginkgo's default `--grace-period` for nodes that run after an interrupt.
pub const GRACE_PERIOD: Duration = Duration::from_secs(30);

// ---------------------------------------------------------------------------------------------
// GinkgoWriter / By
// ---------------------------------------------------------------------------------------------

/// `fmt.Fprintf(GinkgoWriter, ...)`: Ginkgo streams GinkgoWriter output in verbose serial runs.
/// Written straight to the process stderr so it is visible even when libtest captures output.
pub fn write(s: &str) {
    // Unit tests of this crate: keep the output captured by libtest.
    if cfg!(test) {
        eprint!("{s}");
        return;
    }
    let mut e = std::io::stderr().lock();
    let _ = e.write_all(s.as_bytes());
    let _ = e.flush();
}

/// `fmt.Fprintf(GinkgoWriter, format, args...)`
#[macro_export]
macro_rules! gw {
    ($($arg:tt)*) => {
        $crate::ginkgo::write(&format!($($arg)*))
    };
}

fn timestamp() -> String {
    chrono::Local::now()
        .format("%m/%d/%y %H:%M:%S%.3f")
        .to_string()
}

/// `By(text)`: report a step of the current node.
pub fn by(text: &str) {
    write(&format!("STEP: {text} @ {}\n", timestamp()));
}

// ---------------------------------------------------------------------------------------------
// Node outcomes
// ---------------------------------------------------------------------------------------------

/// How a node stopped early: `Fail(...)` (also failed `Expect`s) or `Skip(...)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Interrupt {
    Fail(String),
    Skip(String),
}

impl std::fmt::Display for Interrupt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Interrupt::Fail(m) => write!(f, "[FAILED] {m}"),
            Interrupt::Skip(m) => write!(f, "[SKIPPED] {m}"),
        }
    }
}

/// Result of a node body or of a helper that may `Fail`/`Expect`.
pub type NodeResult<T = ()> = Result<T, Interrupt>;

/// `Fail(message)`
pub fn fail<T>(message: impl Into<String>) -> NodeResult<T> {
    Err(Interrupt::Fail(message.into()))
}

/// `Skip(message)`
pub fn skip<T>(message: impl Into<String>) -> NodeResult<T> {
    Err(Interrupt::Skip(message.into()))
}

/// A Go runtime panic (e.g. nil pointer dereference) inside a node: Ginkgo recovers it and
/// fails the spec as `[PANICKED]`. Helpers return this instead of panicking.
pub fn go_panic(message: impl std::fmt::Display) -> Interrupt {
    Interrupt::Fail(format!("[PANICKED] Test Panicked\n{message}"))
}

/// Go's message for dereferencing a nil pointer.
pub const NIL_POINTER_DEREFERENCE: &str =
    "runtime error: invalid memory address or nil pointer dereference";

// ---------------------------------------------------------------------------------------------
// Spec tree
// ---------------------------------------------------------------------------------------------

/// Setup node types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NodeKind {
    BeforeAll,
    AfterAll,
    BeforeEach,
    AfterEach,
    It,
}

impl NodeKind {
    fn label(self) -> &'static str {
        match self {
            NodeKind::BeforeAll => "BeforeAll",
            NodeKind::AfterAll => "AfterAll",
            NodeKind::BeforeEach => "BeforeEach",
            NodeKind::AfterEach => "AfterEach",
            NodeKind::It => "It",
        }
    }
}

/// A container (`Describe`/`Context`) with its setup nodes and children.
#[derive(Clone, Debug)]
pub struct Tree<N> {
    pub text: String,
    pub labels: Vec<String>,
    pub ordered: bool,
    pub nodes: Vec<(NodeKind, N)>,
    pub children: Vec<Child<N>>,
}

/// A container child: a nested container or an `It`.
#[derive(Clone, Debug)]
pub enum Child<N> {
    Container(Tree<N>),
    It {
        text: String,
        labels: Vec<String>,
        node: N,
    },
}

impl<N> Tree<N> {
    /// `Describe(text, ...)` / `Context(text, ...)`
    pub fn describe(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            labels: Vec::new(),
            ordered: false,
            nodes: Vec::new(),
            children: Vec::new(),
        }
    }
    /// `Ordered` decorator
    pub fn ordered(mut self) -> Self {
        self.ordered = true;
        self
    }
    /// `Label(...)` decorator
    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.labels.push(label.into());
        self
    }
    pub fn before_all(mut self, n: N) -> Self {
        self.nodes.push((NodeKind::BeforeAll, n));
        self
    }
    pub fn after_all(mut self, n: N) -> Self {
        self.nodes.push((NodeKind::AfterAll, n));
        self
    }
    pub fn before_each(mut self, n: N) -> Self {
        self.nodes.push((NodeKind::BeforeEach, n));
        self
    }
    pub fn after_each(mut self, n: N) -> Self {
        self.nodes.push((NodeKind::AfterEach, n));
        self
    }
    /// `It(text, ...)`
    pub fn it(mut self, text: impl Into<String>, n: N) -> Self {
        self.children.push(Child::It {
            text: text.into(),
            labels: Vec::new(),
            node: n,
        });
        self
    }
    /// A nested `Context`.
    pub fn context(mut self, t: Tree<N>) -> Self {
        self.children.push(Child::Container(t));
        self
    }
}

/// What a node can see of the spec it runs in (`CurrentSpecReport()`).
#[derive(Clone, Debug, Default)]
pub struct SpecReport {
    pub full_text: String,
    pub labels: Vec<String>,
    failed: bool,
}

impl SpecReport {
    /// `CurrentSpecReport().Failed()`
    pub fn failed(&self) -> bool {
        self.failed
    }
}

/// An ordered top-level container: provides its tree and runs its nodes.
#[allow(async_fn_in_trait)]
pub trait Container {
    type Node: Copy + Debug + PartialEq;
    fn tree(&self) -> Tree<Self::Node>;
    async fn run(&mut self, node: Self::Node, spec: &SpecReport) -> NodeResult;
}

/// Identity of a run-once (`BeforeAll`/`AfterAll`) node: (container id, node index).
type PairId = (usize, usize);

#[derive(Clone, Debug)]
struct SpecNode<N> {
    kind: NodeKind,
    node: N,
    level: usize,
    /// BeforeAll/AfterAll: the container they belong to (run-once pair).
    pair: Option<PairId>,
}

#[derive(Clone, Debug)]
struct FlatSpec<N> {
    full_text: String,
    labels: Vec<String>,
    containers: Vec<usize>,
    before: Vec<SpecNode<N>>,
    it: SpecNode<N>,
    after: Vec<SpecNode<N>>,
    filtered: bool,
}

fn flatten<N: Copy>(tree: &Tree<N>) -> Vec<FlatSpec<N>> {
    struct Frame<'a, N> {
        tree: &'a Tree<N>,
        id: usize,
    }
    fn walk<'a, N: Copy>(
        t: &'a Tree<N>,
        stack: &mut Vec<Frame<'a, N>>,
        next_id: &mut usize,
        out: &mut Vec<FlatSpec<N>>,
    ) {
        let id = *next_id;
        *next_id += 1;
        stack.push(Frame { tree: t, id });
        for child in &t.children {
            match child {
                Child::Container(c) => walk(c, stack, next_id, out),
                Child::It { text, labels, node } => {
                    let mut texts: Vec<String> =
                        stack.iter().map(|f| f.tree.text.clone()).collect();
                    texts.push(text.clone());
                    let mut all_labels: Vec<String> =
                        stack.iter().flat_map(|f| f.tree.labels.clone()).collect();
                    all_labels.extend(labels.iter().cloned());
                    let mut before = Vec::new();
                    let mut after_each = Vec::new();
                    let mut after_all = Vec::new();
                    for (depth, f) in stack.iter().enumerate() {
                        let level = depth + 1;
                        for (idx, (kind, n)) in f.tree.nodes.iter().enumerate() {
                            let pair = matches!(kind, NodeKind::BeforeAll | NodeKind::AfterAll)
                                .then_some((f.id, idx));
                            let sn = SpecNode {
                                kind: *kind,
                                node: *n,
                                level,
                                pair,
                            };
                            match kind {
                                NodeKind::BeforeAll | NodeKind::BeforeEach => before.push(sn),
                                NodeKind::AfterEach => after_each.push(sn),
                                NodeKind::AfterAll => after_all.push(sn),
                                NodeKind::It => {}
                            }
                        }
                    }
                    // Ginkgo: BeforeAll + BeforeEach sorted by ascending nesting level (stable),
                    // AfterEach + AfterAll sorted by descending nesting level (stable).
                    before.sort_by_key(|n| n.level);
                    let mut after = after_each;
                    after.extend(after_all);
                    after.sort_by_key(|n| std::cmp::Reverse(n.level));
                    out.push(FlatSpec {
                        full_text: texts.join(" "),
                        labels: all_labels,
                        containers: stack.iter().map(|f| f.id).collect(),
                        before,
                        it: SpecNode {
                            kind: NodeKind::It,
                            node: *node,
                            level: stack.len(),
                            pair: None,
                        },
                        after,
                        filtered: false,
                    });
                }
            }
        }
        stack.pop();
    }
    let mut out = Vec::new();
    let mut next_id = 0;
    walk(tree, &mut Vec::new(), &mut next_id, &mut out);
    out
}

// ---------------------------------------------------------------------------------------------
// Label filter (`-ginkgo.label-filter`)
// ---------------------------------------------------------------------------------------------

/// A parsed Ginkgo label filter: labels, `!`, `&&`, `||`, `,` (same as `||`) and parentheses.
/// Labels match case-insensitively. The empty filter matches everything.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LabelFilter {
    All,
    Label(String),
    Not(Box<LabelFilter>),
    And(Box<LabelFilter>, Box<LabelFilter>),
    Or(Box<LabelFilter>, Box<LabelFilter>),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Tok {
    Label(String),
    Not,
    And,
    Or,
    LParen,
    RParen,
}

fn tokenize(s: &str) -> Result<Vec<Tok>, String> {
    let mut toks = Vec::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            c if c.is_whitespace() => i += 1,
            '!' => {
                toks.push(Tok::Not);
                i += 1;
            }
            '(' => {
                toks.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                toks.push(Tok::RParen);
                i += 1;
            }
            ',' => {
                toks.push(Tok::Or);
                i += 1;
            }
            '&' | '|' => {
                if chars.get(i + 1) != Some(&c) {
                    return Err(format!("Invalid token '{c}'.  Did you mean '{c}{c}'?"));
                }
                toks.push(if c == '&' { Tok::And } else { Tok::Or });
                i += 2;
            }
            '/' => return Err("regular expression label filters are not supported".into()),
            _ => {
                let start = i;
                while i < chars.len() && !chars[i].is_whitespace() && !"!(),&|/".contains(chars[i])
                {
                    i += 1;
                }
                let label: String = chars[start..i].iter().collect();
                toks.push(Tok::Label(label.trim().to_lowercase()));
            }
        }
    }
    Ok(toks)
}

impl LabelFilter {
    /// Parse a filter expression.
    pub fn parse(s: &str) -> Result<Self, String> {
        let toks = tokenize(s)?;
        if toks.is_empty() {
            return Ok(LabelFilter::All);
        }
        let mut pos = 0;
        let f = Self::parse_or(&toks, &mut pos)?;
        if pos != toks.len() {
            return Err(format!("Unexpected token in label filter: {s}"));
        }
        Ok(f)
    }

    fn parse_or(t: &[Tok], pos: &mut usize) -> Result<Self, String> {
        let mut lhs = Self::parse_and(t, pos)?;
        while t.get(*pos) == Some(&Tok::Or) {
            *pos += 1;
            let rhs = Self::parse_and(t, pos)?;
            lhs = LabelFilter::Or(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_and(t: &[Tok], pos: &mut usize) -> Result<Self, String> {
        let mut lhs = Self::parse_unary(t, pos)?;
        while t.get(*pos) == Some(&Tok::And) {
            *pos += 1;
            let rhs = Self::parse_unary(t, pos)?;
            lhs = LabelFilter::And(Box::new(lhs), Box::new(rhs));
        }
        Ok(lhs)
    }

    fn parse_unary(t: &[Tok], pos: &mut usize) -> Result<Self, String> {
        match t.get(*pos) {
            Some(Tok::Not) => {
                *pos += 1;
                Ok(LabelFilter::Not(Box::new(Self::parse_unary(t, pos)?)))
            }
            Some(Tok::LParen) => {
                *pos += 1;
                let inner = Self::parse_or(t, pos)?;
                if t.get(*pos) != Some(&Tok::RParen) {
                    return Err("Mismatched '(' - could not find matching ')'.".into());
                }
                *pos += 1;
                Ok(inner)
            }
            Some(Tok::Label(l)) => {
                *pos += 1;
                Ok(LabelFilter::Label(l.clone()))
            }
            Some(other) => Err(format!("Unexpected token {other:?} in label filter")),
            None => Err("Unexpected end of label filter".into()),
        }
    }

    /// Whether a spec with these labels is selected.
    pub fn matches(&self, labels: &[String]) -> bool {
        match self {
            LabelFilter::All => true,
            LabelFilter::Label(l) => labels.iter().any(|x| x.to_lowercase() == *l),
            LabelFilter::Not(f) => !f.matches(labels),
            LabelFilter::And(a, b) => a.matches(labels) && b.matches(labels),
            LabelFilter::Or(a, b) => a.matches(labels) || b.matches(labels),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Go durations (`-ginkgo.timeout=90m`)
// ---------------------------------------------------------------------------------------------

/// `time.ParseDuration` (non-negative durations).
pub fn parse_go_duration(s: &str) -> Result<Duration, String> {
    let orig = s;
    let err = || format!("time: invalid duration \"{orig}\"");
    let mut s = s;
    if let Some(rest) = s.strip_prefix('+') {
        s = rest;
    } else if s.starts_with('-') {
        return Err(format!("negative duration \"{orig}\""));
    }
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    if s.is_empty() {
        return Err(err());
    }
    let mut total = 0f64;
    while !s.is_empty() {
        let num_end = s
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .unwrap_or(s.len());
        if num_end == 0 {
            return Err(err());
        }
        let value: f64 = s[..num_end].parse().map_err(|_| err())?;
        s = &s[num_end..];
        let unit_end = s
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(s.len());
        let unit = &s[..unit_end];
        s = &s[unit_end..];
        let nanos = match unit {
            "ns" => 1.0,
            "us" | "µs" | "μs" => 1e3,
            "ms" => 1e6,
            "s" => 1e9,
            "m" => 60e9,
            "h" => 3600e9,
            "" => return Err(format!("time: missing unit in duration \"{orig}\"")),
            other => {
                return Err(format!(
                    "time: unknown unit \"{other}\" in duration \"{orig}\""
                ));
            }
        };
        total += value * nanos;
    }
    Ok(Duration::from_nanos(total as u64))
}

// ---------------------------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------------------------

/// Final state of a spec or node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Passed,
    Skipped(String),
    Failed(String),
    Interrupted(String),
}

impl State {
    fn is_failure(&self) -> bool {
        matches!(self, State::Failed(_) | State::Interrupted(_))
    }
}

/// Runner configuration (Ginkgo CLI flags).
#[derive(Clone, Debug)]
pub struct Config {
    pub label_filter: LabelFilter,
    pub focus: Option<regex::Regex>,
    pub timeout: Duration,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            label_filter: LabelFilter::All,
            focus: None,
            timeout: DEFAULT_SUITE_TIMEOUT,
        }
    }
}

impl Config {
    /// Read `E2E_LABEL_FILTER`, `E2E_FOCUS` and `E2E_SUITE_TIMEOUT`.
    pub fn from_env() -> Result<Self, String> {
        let label_filter =
            LabelFilter::parse(&std::env::var(LABEL_FILTER_ENV).unwrap_or_default())?;
        let focus = match std::env::var(FOCUS_ENV).unwrap_or_default() {
            f if f.is_empty() => None,
            f => Some(regex::Regex::new(&f).map_err(|e| format!("invalid {FOCUS_ENV}: {e}"))?),
        };
        let timeout = match std::env::var(SUITE_TIMEOUT_ENV).unwrap_or_default() {
            t if t.is_empty() => DEFAULT_SUITE_TIMEOUT,
            t => parse_go_duration(&t)?,
        };
        Ok(Self {
            label_filter,
            focus,
            timeout,
        })
    }

    fn selects(&self, full_text: &str, labels: &[String]) -> bool {
        self.label_filter.matches(labels)
            && self.focus.as_ref().is_none_or(|r| r.is_match(full_text))
    }
}

/// Outcome of one reported spec.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecOutcome {
    pub full_text: String,
    pub state: State,
}

/// Executes containers with Ginkgo semantics and collects the report.
pub struct Runner {
    pub config: Config,
    suite_description: String,
    deadline: Instant,
    total_specs: usize,
    pub outcomes: Vec<SpecOutcome>,
    suite_failures: Vec<String>,
    skip_all: bool,
    started: Instant,
}

impl Runner {
    pub fn new(suite_description: impl Into<String>, config: Config) -> Self {
        let now = Instant::now();
        Self {
            deadline: now + config.timeout,
            config,
            suite_description: suite_description.into(),
            total_specs: 0,
            outcomes: Vec::new(),
            suite_failures: Vec::new(),
            skip_all: false,
            started: now,
        }
    }

    fn plan<N: Copy>(&self, tree: &Tree<N>) -> Vec<FlatSpec<N>> {
        let mut specs = flatten(tree);
        for s in &mut specs {
            s.filtered = !self.config.selects(&s.full_text, &s.labels);
        }
        specs
    }

    /// Whether any spec of these trees passes the filters (Ginkgo skips the suite setup nodes
    /// when no spec will run).
    pub fn any_selected<N: Copy>(&self, tree: &Tree<N>) -> bool {
        self.plan(tree).iter().any(|s| !s.filtered)
    }

    fn deadline_passed(&self) -> bool {
        Instant::now() >= self.deadline
    }

    fn node_deadline(&self) -> Instant {
        if self.deadline_passed() {
            Instant::now() + GRACE_PERIOD
        } else {
            self.deadline
        }
    }

    async fn guard<F: Future<Output = NodeResult>>(&self, fut: F) -> State {
        let deadline = self.node_deadline();
        let fut = AssertUnwindSafe(fut).catch_unwind();
        match tokio::time::timeout_at(deadline, fut).await {
            Err(_) => State::Interrupted("Interrupted by Suite Timeout".into()),
            Ok(Err(panic)) => {
                let msg = panic
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| panic.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "panic".into());
                State::Failed(format!("[PANICKED] Test Panicked\n{msg}"))
            }
            Ok(Ok(Ok(()))) => State::Passed,
            Ok(Ok(Err(Interrupt::Fail(m)))) => State::Failed(m),
            Ok(Ok(Err(Interrupt::Skip(m)))) => State::Skipped(m),
        }
    }

    /// Run a suite-level node (`BeforeSuite`/`AfterSuite`). Returns the value on success.
    pub async fn run_suite_node<T, F>(&mut self, name: &str, fut: F) -> Option<T>
    where
        F: Future<Output = NodeResult<T>>,
    {
        write(&format!("------------------------------\n[{name}] \n"));
        let started = Instant::now();
        let mut out = None;
        let state = self
            .guard(async {
                out = Some(fut.await?);
                Ok(())
            })
            .await;
        let secs = started.elapsed().as_secs_f64();
        match &state {
            State::Passed => write(&format!("[{name}] PASSED [{secs:.3} seconds]\n")),
            State::Skipped(m) => write(&format!("[{name}] SKIPPED [{secs:.3} seconds]\n  {m}\n")),
            State::Failed(m) | State::Interrupted(m) => {
                write(&format!("[{name}] [FAILED] [{secs:.3} seconds]\n  {m}\n"));
                self.suite_failures.push(format!("[{name}] {m}"));
                if name == "BeforeSuite" {
                    self.skip_all = true;
                }
            }
        }
        if state == State::Passed { out } else { None }
    }

    /// Record a container whose specs cannot run (e.g. BeforeSuite failed): every selected spec
    /// is reported skipped.
    pub fn skip_container<N: Copy>(&mut self, tree: &Tree<N>) {
        for spec in self.plan(tree) {
            self.total_specs += 1;
            if spec.filtered {
                continue;
            }
            self.outcomes.push(SpecOutcome {
                full_text: spec.full_text,
                state: State::Skipped(String::new()),
            });
        }
    }

    /// Run an ordered container (one Ginkgo "group").
    pub async fn run_container<C: Container>(&mut self, c: &mut C) {
        let tree = c.tree();
        let specs = self.plan(&tree);
        self.total_specs += specs.len();
        let mut succeeded = true;
        let mut tracker: HashMap<PairId, State> = HashMap::new();

        let is_last_spec_with_pair = |idx: usize, pair: PairId| -> bool {
            let mut last = None;
            for (i, s) in specs.iter().enumerate() {
                if s.filtered {
                    continue;
                }
                if s.containers.contains(&pair.0) {
                    last = Some(i);
                }
            }
            last == Some(idx)
        };

        for (idx, spec) in specs.iter().enumerate() {
            if spec.filtered {
                continue;
            }
            let mut report = SpecReport {
                full_text: spec.full_text.clone(),
                labels: spec.labels.clone(),
                failed: false,
            };

            // evaluateSkipStatus
            let mut state = State::Passed;
            if self.skip_all || self.deadline_passed() {
                state = State::Skipped(String::new());
            } else if !succeeded {
                state = State::Skipped(
                    "Spec skipped because an earlier spec in an ordered container failed".into(),
                );
            } else if spec.before.iter().any(|n| {
                n.pair
                    .is_some_and(|p| matches!(tracker.get(&p), Some(State::Skipped(_))))
            }) {
                let kind = spec
                    .before
                    .iter()
                    .find(|n| {
                        n.pair
                            .is_some_and(|p| matches!(tracker.get(&p), Some(State::Skipped(_))))
                    })
                    .map(|n| n.kind.label())
                    .unwrap_or("BeforeAll");
                state = State::Skipped(format!("Spec skipped because Skip() was called in {kind}"));
            }

            let labels = if spec.labels.is_empty() {
                String::new()
            } else {
                format!(" [{}]", spec.labels.join(", "))
            };
            write(&format!(
                "------------------------------\n{}{}\n",
                spec.full_text, labels
            ));
            let started = Instant::now();

            if state == State::Passed {
                // attemptSpec: before nodes + It
                let mut terminating: Option<&SpecNode<C::Node>> = None;
                for n in spec.before.iter().chain(std::iter::once(&spec.it)) {
                    if let Some(p) = n.pair
                        && tracker.get(&p) == Some(&State::Passed)
                    {
                        continue;
                    }
                    let s = self.guard(c.run(n.node, &report)).await;
                    if let Some(p) = n.pair {
                        tracker.insert(p, s.clone());
                    }
                    if s != State::Passed {
                        if s.is_failure() {
                            write(&format!("  [FAILED] in [{}]\n", n.kind.label()));
                        }
                        state = s;
                        terminating = Some(n);
                        break;
                    }
                }

                // after nodes
                let after: Vec<&SpecNode<C::Node>> = spec
                    .after
                    .iter()
                    .filter(|n| terminating.is_none_or(|t| n.level <= t.level))
                    .filter(|n| {
                        let Some(pair) = n.pair else { return true };
                        let last = is_last_spec_with_pair(idx, pair);
                        match &state {
                            State::Passed => last,
                            State::Skipped(_) => {
                                last || terminating
                                    .is_some_and(|t| t.pair.is_some() && t.level == n.level)
                            }
                            State::Failed(_) | State::Interrupted(_) => true,
                        }
                    })
                    .collect();
                for n in after {
                    report.failed = state.is_failure();
                    let s = self.guard(c.run(n.node, &report)).await;
                    if state == State::Passed {
                        if s.is_failure() {
                            write(&format!("  [FAILED] in [{}]\n", n.kind.label()));
                        }
                        state = s;
                    } else if s.is_failure() {
                        write(&format!(
                            "  [FAILED] additional failure in [{}]: {}\n",
                            n.kind.label(),
                            match &s {
                                State::Failed(m) | State::Interrupted(m) => m.as_str(),
                                _ => "",
                            }
                        ));
                    }
                }
            }

            let secs = started.elapsed().as_secs_f64();
            match &state {
                State::Passed => write(&format!("• [{secs:.3} seconds]\n")),
                State::Skipped(m) => write(&format!("S [SKIPPED] [{secs:.3} seconds]\n  {m}\n")),
                State::Failed(m) => write(&format!("• [FAILED] [{secs:.3} seconds]\n  {m}\n")),
                State::Interrupted(m) => {
                    write(&format!("• [INTERRUPTED] [{secs:.3} seconds]\n  {m}\n"))
                }
            }
            if state.is_failure() {
                succeeded = false;
            }
            self.outcomes.push(SpecOutcome {
                full_text: spec.full_text.clone(),
                state,
            });
        }
    }

    /// Print the summary (`Ran X of Y Specs ...`) and return an error if the suite failed.
    pub fn finish(&self) -> Result<(), String> {
        let ran = self
            .outcomes
            .iter()
            .filter(|o| !matches!(o.state, State::Skipped(_)))
            .count();
        let passed = self
            .outcomes
            .iter()
            .filter(|o| o.state == State::Passed)
            .count();
        let failed: Vec<&SpecOutcome> = self
            .outcomes
            .iter()
            .filter(|o| o.state.is_failure())
            .collect();
        let skipped = self.total_specs - ran;
        let secs = self.started.elapsed().as_secs_f64();
        let ok = failed.is_empty() && self.suite_failures.is_empty();
        let mut s = String::from("------------------------------\n\n");
        if !ok {
            s.push_str("Summarizing failures:\n");
            for f in &self.suite_failures {
                s.push_str(&format!("  [FAILED] {f}\n"));
            }
            for f in &failed {
                s.push_str(&format!("  [FAILED] {}\n", f.full_text));
            }
            s.push('\n');
        }
        s.push_str(&format!(
            "Ran {ran} of {} Specs in {secs:.3} seconds\n{} -- {passed} Passed | {} Failed | 0 Pending | {skipped} Skipped\n",
            self.total_specs,
            if ok { "SUCCESS!" } else { "FAIL!" },
            failed.len(),
        ));
        write(&s);
        if ok {
            Ok(())
        } else {
            Err(format!("{} failed", self.suite_description))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[test]
    fn label_filter_parsing_and_matching() {
        let ocp = vec!["ocp-required".to_string()];
        let none: Vec<String> = vec![];
        let f = LabelFilter::parse("!ocp-required").unwrap();
        assert!(!f.matches(&ocp));
        assert!(f.matches(&none));
        let f = LabelFilter::parse("OCP-Required").unwrap();
        assert!(f.matches(&ocp));
        assert!(!f.matches(&none));
        assert!(LabelFilter::parse("").unwrap().matches(&ocp));
        let f = LabelFilter::parse("a && (b || !c)").unwrap();
        assert!(f.matches(&["a".into(), "b".into(), "c".into()]));
        assert!(f.matches(&["a".into()]));
        assert!(!f.matches(&["a".into(), "c".into()]));
        let f = LabelFilter::parse("a, b").unwrap();
        assert!(f.matches(&["b".into()]));
        assert!(LabelFilter::parse("a & b").is_err());
        assert!(LabelFilter::parse("(a").is_err());
        assert!(LabelFilter::parse("/re/").is_err());
    }

    #[test]
    fn go_durations() {
        assert_eq!(parse_go_duration("90m").unwrap(), Duration::from_secs(5400));
        assert_eq!(parse_go_duration("10m").unwrap(), Duration::from_secs(600));
        assert_eq!(
            parse_go_duration("1h30m").unwrap(),
            Duration::from_secs(5400)
        );
        assert_eq!(
            parse_go_duration("1.5h").unwrap(),
            Duration::from_secs(5400)
        );
        assert_eq!(
            parse_go_duration("300ms").unwrap(),
            Duration::from_millis(300)
        );
        assert_eq!(parse_go_duration("0").unwrap(), Duration::ZERO);
        assert!(parse_go_duration("10").is_err());
        assert!(parse_go_duration("").is_err());
        assert!(parse_go_duration("5x").is_err());
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    enum N {
        BeforeAll,
        AfterAll,
        AfterEach,
        CtxBeforeEach,
        CtxAfterEach,
        A,
        B,
        C,
        D,
    }

    struct Fake {
        log: Arc<Mutex<Vec<String>>>,
        behaviour: HashMap<String, Interrupt>,
        panic_on: Option<String>,
        labels_on_ctx: Option<&'static str>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                log: Arc::new(Mutex::new(Vec::new())),
                behaviour: HashMap::new(),
                panic_on: None,
                labels_on_ctx: None,
            }
        }
        fn with(mut self, node: N, i: Interrupt) -> Self {
            self.behaviour.insert(format!("{node:?}"), i);
            self
        }
        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    impl Container for Fake {
        type Node = N;
        fn tree(&self) -> Tree<N> {
            let mut ctx = Tree::describe("Ctx")
                .before_each(N::CtxBeforeEach)
                .after_each(N::CtxAfterEach)
                .it("c", N::C);
            if let Some(l) = self.labels_on_ctx {
                ctx = ctx.label(l);
            }
            Tree::describe("Top")
                .ordered()
                .before_all(N::BeforeAll)
                .after_all(N::AfterAll)
                .after_each(N::AfterEach)
                .context(Tree::describe("First").it("a", N::A).it("b", N::B))
                .context(ctx)
                .context(Tree::describe("Last").it("d", N::D))
        }
        async fn run(&mut self, node: N, spec: &SpecReport) -> NodeResult {
            let name = format!("{node:?}");
            let entry = if node == N::AfterEach && spec.failed() {
                format!("{name}(failed)")
            } else {
                name.clone()
            };
            self.log.lock().unwrap().push(entry);
            if self.panic_on.as_deref() == Some(name.as_str()) {
                panic!("boom");
            }
            match self.behaviour.get(&name) {
                Some(i) => Err(i.clone()),
                None => Ok(()),
            }
        }
    }

    fn states(r: &Runner) -> Vec<String> {
        r.outcomes
            .iter()
            .map(|o| match &o.state {
                State::Passed => "P".to_string(),
                State::Skipped(_) => "S".to_string(),
                State::Failed(_) => "F".to_string(),
                State::Interrupted(_) => "I".to_string(),
            })
            .collect()
    }

    #[tokio::test]
    async fn ordered_container_happy_path() {
        let mut c = Fake::new();
        let mut r = Runner::new("t", Config::default());
        r.run_container(&mut c).await;
        assert_eq!(
            c.log(),
            vec![
                "BeforeAll",
                "A",
                "AfterEach",
                "B",
                "AfterEach",
                "CtxBeforeEach",
                "C",
                "CtxAfterEach",
                "AfterEach",
                "D",
                "AfterEach",
                "AfterAll"
            ]
        );
        assert_eq!(states(&r), vec!["P", "P", "P", "P"]);
        assert!(r.finish().is_ok());
    }

    #[tokio::test]
    async fn failure_skips_remaining_and_runs_after_all() {
        let mut c = Fake::new().with(N::B, Interrupt::Fail("nope".into()));
        let mut r = Runner::new("t", Config::default());
        r.run_container(&mut c).await;
        assert_eq!(
            c.log(),
            vec![
                "BeforeAll",
                "A",
                "AfterEach",
                "B",
                "AfterEach(failed)",
                "AfterAll"
            ]
        );
        assert_eq!(states(&r), vec!["P", "F", "S", "S"]);
        assert!(r.finish().is_err());
    }

    #[tokio::test]
    async fn skip_in_before_all_skips_everything_but_runs_after_all() {
        let mut c = Fake::new().with(N::BeforeAll, Interrupt::Skip("kind only".into()));
        let mut r = Runner::new("t", Config::default());
        r.run_container(&mut c).await;
        assert_eq!(c.log(), vec!["BeforeAll", "AfterEach", "AfterAll"]);
        assert_eq!(states(&r), vec!["S", "S", "S", "S"]);
        assert_eq!(
            r.outcomes[1].state,
            State::Skipped("Spec skipped because Skip() was called in BeforeAll".into())
        );
        assert!(r.finish().is_ok());
    }

    #[tokio::test]
    async fn skip_in_before_each_only_skips_that_spec() {
        let mut c = Fake::new().with(N::CtxBeforeEach, Interrupt::Skip("not ready".into()));
        let mut r = Runner::new("t", Config::default());
        r.run_container(&mut c).await;
        assert_eq!(
            c.log(),
            vec![
                "BeforeAll",
                "A",
                "AfterEach",
                "B",
                "AfterEach",
                "CtxBeforeEach",
                "CtxAfterEach",
                "AfterEach",
                "D",
                "AfterEach",
                "AfterAll"
            ]
        );
        assert_eq!(states(&r), vec!["P", "P", "S", "P"]);
    }

    #[tokio::test]
    async fn before_all_failure_runs_only_outer_after_nodes() {
        let mut c = Fake::new().with(N::BeforeAll, Interrupt::Fail("x".into()));
        let mut r = Runner::new("t", Config::default());
        r.run_container(&mut c).await;
        assert_eq!(c.log(), vec!["BeforeAll", "AfterEach(failed)", "AfterAll"]);
        assert_eq!(states(&r), vec!["F", "S", "S", "S"]);
    }

    #[tokio::test]
    async fn panics_are_recovered_as_failures() {
        let mut c = Fake::new();
        c.panic_on = Some("A".into());
        let mut r = Runner::new("t", Config::default());
        r.run_container(&mut c).await;
        assert_eq!(
            c.log(),
            vec!["BeforeAll", "A", "AfterEach(failed)", "AfterAll"]
        );
        assert!(matches!(&r.outcomes[0].state, State::Failed(m) if m.contains("boom")));
    }

    #[tokio::test]
    async fn filtered_specs_do_not_run_and_after_all_follows_last_selected() {
        let mut c = Fake::new();
        let config = Config {
            focus: Some(regex::Regex::new("First").unwrap()),
            ..Config::default()
        };
        let mut r = Runner::new("t", config);
        r.run_container(&mut c).await;
        assert_eq!(
            c.log(),
            vec!["BeforeAll", "A", "AfterEach", "B", "AfterEach", "AfterAll"]
        );
        assert_eq!(states(&r), vec!["P", "P"]);
    }

    #[tokio::test]
    async fn label_filter_excludes_container() {
        let c = Fake::new();
        let tree = Tree::describe("X")
            .ordered()
            .label("ocp-required")
            .it("a", N::A);
        let config = Config {
            label_filter: LabelFilter::parse("!ocp-required").unwrap(),
            ..Config::default()
        };
        let r = Runner::new("t", config);
        assert!(!r.any_selected(&tree));
        assert!(r.any_selected(&c.tree()));
    }

    #[tokio::test]
    async fn context_labels_select_subset() {
        let mut c = Fake::new();
        c.labels_on_ctx = Some("ctx");
        let config = Config {
            label_filter: LabelFilter::parse("ctx").unwrap(),
            ..Config::default()
        };
        let mut r = Runner::new("t", config);
        r.run_container(&mut c).await;
        assert_eq!(
            c.log(),
            vec![
                "BeforeAll",
                "CtxBeforeEach",
                "C",
                "CtxAfterEach",
                "AfterEach",
                "AfterAll"
            ]
        );
    }

    #[tokio::test]
    async fn before_suite_failure_skips_all() {
        let mut r = Runner::new("t", Config::default());
        let v: Option<()> = r
            .run_suite_node("BeforeSuite", async { fail::<()>("no cluster") })
            .await;
        assert!(v.is_none());
        let mut c = Fake::new();
        r.run_container(&mut c).await;
        assert!(c.log().is_empty());
        assert_eq!(states(&r), vec!["S", "S", "S", "S"]);
        assert!(r.finish().is_err());
    }

    #[tokio::test(start_paused = true)]
    async fn suite_timeout_interrupts_and_cleans_up() {
        struct Slow(Vec<&'static str>);
        impl Container for Slow {
            type Node = &'static str;
            fn tree(&self) -> Tree<&'static str> {
                Tree::describe("S")
                    .ordered()
                    .after_all("after_all")
                    .it("slow", "slow")
                    .it("next", "next")
            }
            async fn run(&mut self, node: &'static str, _: &SpecReport) -> NodeResult {
                self.0.push(node);
                if node == "slow" {
                    tokio::time::sleep(Duration::from_secs(3600)).await;
                }
                Ok(())
            }
        }
        let mut c = Slow(Vec::new());
        let config = Config {
            timeout: Duration::from_secs(10),
            ..Config::default()
        };
        let mut r = Runner::new("t", config);
        r.run_container(&mut c).await;
        assert_eq!(c.0, vec!["slow", "after_all"]);
        assert!(matches!(r.outcomes[0].state, State::Interrupted(_)));
        assert!(matches!(r.outcomes[1].state, State::Skipped(_)));
    }
}
