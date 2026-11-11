//! The command line of `cmd/main.go`: the flags it defines, controller-runtime's
//! `--kubeconfig` (registered by `sigs.k8s.io/controller-runtime/pkg/client/config`'s `init`)
//! and the zap flags (`zap.Options.BindFlags`), parsed with Go `flag` semantics.
//!
//! ```ignore
//! let flags = ManagerFlags::parse_command_line();   // flag.Parse(); exits 0/2 like Go
//! ctrlrt_cmd::setup_log(&flags.zap);                 // ctrl.SetLogger(zap.New(...))
//! ```

use crate::flag::{ErrorHandling, FlagError, FlagSet};
use crate::zap::ZapOptions;

pub const FLAG_METRICS_BIND_ADDRESS: &str = "metrics-bind-address";
pub const FLAG_HEALTH_PROBE_BIND_ADDRESS: &str = "health-probe-bind-address";
pub const FLAG_LEADER_ELECT: &str = "leader-elect";
pub const FLAG_METRICS_SECURE: &str = "metrics-secure";
pub const FLAG_WEBHOOK_CERT_PATH: &str = "webhook-cert-path";
pub const FLAG_WEBHOOK_CERT_NAME: &str = "webhook-cert-name";
pub const FLAG_WEBHOOK_CERT_KEY: &str = "webhook-cert-key";
pub const FLAG_METRICS_CERT_PATH: &str = "metrics-cert-path";
pub const FLAG_METRICS_CERT_NAME: &str = "metrics-cert-name";
pub const FLAG_METRICS_CERT_KEY: &str = "metrics-cert-key";
pub const FLAG_ENABLE_HTTP2: &str = "enable-http2";
pub const FLAG_KUBECONFIG: &str = "kubeconfig";

/// Values of all flags of the manager binary after parsing.
#[derive(Clone, Debug)]
pub struct ManagerFlags {
    pub metrics_addr: String,
    pub probe_addr: String,
    pub enable_leader_election: bool,
    pub secure_metrics: bool,
    pub webhook_cert_path: String,
    pub webhook_cert_name: String,
    pub webhook_cert_key: String,
    pub metrics_cert_path: String,
    pub metrics_cert_name: String,
    pub metrics_cert_key: String,
    pub enable_http2: bool,
    /// `--kubeconfig` (controller-runtime `client/config`); empty = not given.
    pub kubeconfig: String,
    /// `zap.Options{Development: true}` updated from the zap flags.
    pub zap: ZapOptions,
}

/// `flag.StringVar(&kubeconfig, "kubeconfig", "", ...)` from controller-runtime's
/// `pkg/client/config` `init()`.
pub fn register_kubeconfig_flag(fs: &mut FlagSet) {
    fs.string(
        FLAG_KUBECONFIG,
        "",
        "Paths to a kubeconfig. Only required if out-of-cluster.",
    );
}

/// Define every flag of the manager binary on `fs` (the `flag.XxxVar` calls of `main()`,
/// `--kubeconfig` and `opts.BindFlags(flag.CommandLine)` with `Development: true`).
pub fn define_flags(fs: &mut FlagSet) {
    register_kubeconfig_flag(fs);
    fs.string(
        FLAG_METRICS_BIND_ADDRESS,
        "0",
        "The address the metrics endpoint binds to. \
         Use :8443 for HTTPS or :8080 for HTTP, or leave as 0 to disable the metrics service.",
    );
    fs.string(
        FLAG_HEALTH_PROBE_BIND_ADDRESS,
        ":8081",
        "The address the probe endpoint binds to.",
    );
    fs.bool(
        FLAG_LEADER_ELECT,
        false,
        "Enable leader election for controller manager. \
         Enabling this will ensure there is only one active controller manager.",
    );
    fs.bool(
        FLAG_METRICS_SECURE,
        true,
        "If set, the metrics endpoint is served securely via HTTPS. Use --metrics-secure=false to use HTTP instead.",
    );
    fs.string(
        FLAG_WEBHOOK_CERT_PATH,
        "",
        "The directory that contains the webhook certificate.",
    );
    fs.string(
        FLAG_WEBHOOK_CERT_NAME,
        "tls.crt",
        "The name of the webhook certificate file.",
    );
    fs.string(
        FLAG_WEBHOOK_CERT_KEY,
        "tls.key",
        "The name of the webhook key file.",
    );
    fs.string(
        FLAG_METRICS_CERT_PATH,
        "",
        "The directory that contains the metrics server certificate.",
    );
    fs.string(
        FLAG_METRICS_CERT_NAME,
        "tls.crt",
        "The name of the metrics server certificate file.",
    );
    fs.string(
        FLAG_METRICS_CERT_KEY,
        "tls.key",
        "The name of the metrics server key file.",
    );
    fs.bool(
        FLAG_ENABLE_HTTP2,
        false,
        "If set, HTTP/2 will be enabled for the metrics and webhook servers",
    );
    default_zap_options().bind_flags(fs);
}

/// `zap.Options{Development: true}` as in `main()`.
pub fn default_zap_options() -> ZapOptions {
    ZapOptions {
        development: true,
        ..Default::default()
    }
}

impl ManagerFlags {
    /// Read the values of a parsed flag set defined with [`define_flags`].
    pub fn from_flag_set(fs: &FlagSet) -> Self {
        let mut zap = default_zap_options();
        zap.apply_flags(fs);
        Self {
            metrics_addr: fs.get_string(FLAG_METRICS_BIND_ADDRESS),
            probe_addr: fs.get_string(FLAG_HEALTH_PROBE_BIND_ADDRESS),
            enable_leader_election: fs.get_bool(FLAG_LEADER_ELECT),
            secure_metrics: fs.get_bool(FLAG_METRICS_SECURE),
            webhook_cert_path: fs.get_string(FLAG_WEBHOOK_CERT_PATH),
            webhook_cert_name: fs.get_string(FLAG_WEBHOOK_CERT_NAME),
            webhook_cert_key: fs.get_string(FLAG_WEBHOOK_CERT_KEY),
            metrics_cert_path: fs.get_string(FLAG_METRICS_CERT_PATH),
            metrics_cert_name: fs.get_string(FLAG_METRICS_CERT_NAME),
            metrics_cert_key: fs.get_string(FLAG_METRICS_CERT_KEY),
            enable_http2: fs.get_bool(FLAG_ENABLE_HTTP2),
            kubeconfig: fs.get_string(FLAG_KUBECONFIG),
            zap,
        }
    }

    /// Parse `args` (without the program name) on a fresh flag set with the given program
    /// name and error handling. Usage/errors are written to `fs`'s output (stderr unless
    /// `configure` changes it).
    pub fn parse_from<I, S>(
        program: &str,
        error_handling: ErrorHandling,
        args: I,
        configure: impl FnOnce(&mut FlagSet),
    ) -> Result<(Self, Vec<String>), FlagError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut fs = FlagSet::command_line_named(program);
        if error_handling != ErrorHandling::ExitOnError {
            fs = FlagSet::new(program, error_handling);
        }
        define_flags(&mut fs);
        configure(&mut fs);
        fs.parse(args)?;
        Ok((Self::from_flag_set(&fs), fs.args().to_vec()))
    }

    /// `flag.Parse()` on the process command line (`flag.CommandLine`, `ExitOnError`):
    /// prints usage / errors to stderr and exits with 0 (`-h`) or 2 like Go.
    pub fn parse_command_line() -> Self {
        let mut fs = FlagSet::command_line();
        define_flags(&mut fs);
        // ExitOnError: never returns an error.
        let _ = fs.parse_env();
        Self::from_flag_set(&fs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zap::{EncoderKind, TimeEncoding};

    #[test]
    fn defaults() {
        let (f, rest) = ManagerFlags::parse_from(
            "manager",
            ErrorHandling::ContinueOnError,
            Vec::<String>::new(),
            |_| {},
        )
        .unwrap();
        assert!(rest.is_empty());
        assert_eq!(f.metrics_addr, "0");
        assert_eq!(f.probe_addr, ":8081");
        assert!(!f.enable_leader_election);
        assert!(f.secure_metrics);
        assert_eq!(f.webhook_cert_name, "tls.crt");
        assert_eq!(f.metrics_cert_key, "tls.key");
        assert!(!f.enable_http2);
        assert!(f.zap.development);
        assert_eq!(f.zap.encoder, None);
    }

    #[test]
    fn values() {
        let (f, rest) = ManagerFlags::parse_from(
            "manager",
            ErrorHandling::ContinueOnError,
            [
                "--metrics-bind-address=:8443",
                "-leader-elect",
                "--metrics-secure=false",
                "--health-probe-bind-address",
                ":9090",
                "--zap-devel=false",
                "--zap-encoder=CONSOLE",
                "--zap-log-level=5",
                "--zap-stacktrace-level=panic",
                "--zap-time-encoding=iso8601",
                "--kubeconfig=/tmp/kc",
                "--enable-http2",
                "rest",
            ],
            |_| {},
        )
        .unwrap();
        assert_eq!(rest, ["rest"]);
        assert_eq!(f.metrics_addr, ":8443");
        assert_eq!(f.probe_addr, ":9090");
        assert!(f.enable_leader_election);
        assert!(!f.secure_metrics);
        assert!(f.enable_http2);
        assert_eq!(f.kubeconfig, "/tmp/kc");
        assert!(!f.zap.development);
        assert_eq!(f.zap.encoder, Some(EncoderKind::Console));
        assert_eq!(f.zap.level, Some(-5));
        assert_eq!(f.zap.stacktrace_level, Some(4));
        assert_eq!(f.zap.time_encoding, Some(TimeEncoding::Iso8601));
    }
}
