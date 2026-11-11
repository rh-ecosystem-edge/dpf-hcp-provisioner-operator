//! Port of `github.com/google/go-containerregistry/pkg/name`.

use std::fmt;
use std::net::{Ipv4Addr, Ipv6Addr};

/// `name.DefaultRegistry`
pub const DEFAULT_REGISTRY: &str = "index.docker.io";
const DEFAULT_REGISTRY_ALIAS: &str = "docker.io";
/// `name.DefaultTag`
pub const DEFAULT_TAG: &str = "latest";

const DEFAULT_NAMESPACE: &str = "library";
const REPOSITORY_CHARS: &str = "abcdefghijklmnopqrstuvwxyz0123456789_-./";
const REG_REPO_DELIMITER: &str = "/";
const TAG_CHARS: &str = "abcdefghijklmnopqrstuvwxyz0123456789_-.ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const TAG_DELIM: &str = ":";
const DIGEST_DELIM: &str = "@";

/// `name.ErrBadName`
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrBadName {
    info: String,
}

impl ErrBadName {
    fn new(info: String) -> Self {
        Self { info }
    }
}

impl fmt::Display for ErrBadName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.info)
    }
}

impl std::error::Error for ErrBadName {}

/// Errors returned by the `name` package. Everything is an [`ErrBadName`] except the
/// digest validation errors that go-digest returns from `NewDigest`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    BadName(ErrBadName),
    /// `digest.ErrDigestInvalidLength` / `digest.ErrDigestInvalidFormat`.
    Digest(&'static str),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::BadName(e) => e.fmt(f),
            Error::Digest(s) => f.write_str(s),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::BadName(e) => Some(e),
            Error::Digest(_) => None,
        }
    }
}

impl From<ErrBadName> for Error {
    fn from(e: ErrBadName) -> Self {
        Error::BadName(e)
    }
}

impl Error {
    /// `name.IsErrBadName`
    pub fn is_bad_name(&self) -> bool {
        matches!(self, Error::BadName(_))
    }
}

/// `name.IsErrBadName` for an arbitrary error chain.
pub fn is_err_bad_name(err: &anyhow::Error) -> bool {
    err.chain().any(|e| {
        e.downcast_ref::<ErrBadName>().is_some()
            || e.downcast_ref::<Error>().is_some_and(Error::is_bad_name)
    })
}

fn bad(info: String) -> ErrBadName {
    ErrBadName::new(info)
}

/// `name.Option` — functional options for parsing.
#[derive(Debug, Clone)]
pub enum NameOption {
    /// `name.StrictValidation`
    StrictValidation,
    /// `name.WeakValidation`
    WeakValidation,
    /// `name.Insecure`
    Insecure,
    /// `name.WithDefaultRegistry`
    WithDefaultRegistry(String),
    /// `name.WithDefaultTag`
    WithDefaultTag(String),
}

#[derive(Debug, Clone)]
struct Options {
    strict: bool,
    insecure: bool,
    default_registry: String,
    default_tag: String,
}

fn make_options(opts: &[NameOption]) -> Options {
    let mut o = Options {
        strict: false,
        insecure: false,
        default_registry: DEFAULT_REGISTRY.to_string(),
        default_tag: DEFAULT_TAG.to_string(),
    };
    for opt in opts {
        match opt {
            NameOption::StrictValidation => o.strict = true,
            NameOption::WeakValidation => o.strict = false,
            NameOption::Insecure => o.insecure = true,
            NameOption::WithDefaultRegistry(r) => o.default_registry = r.clone(),
            NameOption::WithDefaultTag(t) => o.default_tag = t.clone(),
        }
    }
    o
}

fn check_element(
    name: &str,
    element: &str,
    allowed_runes: &str,
    min_runes: usize,
    max_runes: usize,
) -> Result<(), ErrBadName> {
    let num_runes = element.chars().count();
    if num_runes < min_runes || max_runes < num_runes {
        return Err(bad(format!(
            "{name} must be between {min_runes} and {max_runes} characters in length: {element}"
        )));
    } else if element.chars().any(|c| !allowed_runes.contains(c)) {
        return Err(bad(format!(
            "{name} can only contain the characters `{allowed_runes}`: {element}"
        )));
    }
    Ok(())
}

// ------------------------------------------------------------------ Registry

/// `name.Registry`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Registry {
    insecure: bool,
    registry: String,
}

impl Registry {
    /// `RegistryStr()`
    pub fn registry_str(&self) -> &str {
        &self.registry
    }

    /// `Name()`
    pub fn name(&self) -> String {
        self.registry.clone()
    }

    /// `Repo(repo...)`
    pub fn repo(&self, repo: &[&str]) -> Repository {
        Repository {
            registry: self.clone(),
            repository: crate::goutil::join(repo),
        }
    }

    /// `Scope(string)`
    pub fn scope(&self, _action: &str) -> String {
        "registry:catalog:*".to_string()
    }

    fn is_rfc1918(&self) -> bool {
        let ip_str = self.registry.split(':').next().unwrap_or("");
        let Ok(ip) = ip_str.parse::<Ipv4Addr>() else {
            return false;
        };
        ip.is_private()
    }

    /// `Scheme()`
    pub fn scheme(&self) -> &'static str {
        if self.insecure {
            return "http";
        }
        if self.is_rfc1918() {
            return "http";
        }
        let name = self.registry.as_str();
        if name.starts_with("localhost:") {
            return "http";
        }
        if re_local(name) {
            return "http";
        }
        if name.contains("127.0.0.1") {
            return "http";
        }
        if name.contains("::1") {
            return "http";
        }
        "https"
    }

    /// Whether this registry was created as insecure.
    pub fn is_insecure(&self) -> bool {
        self.insecure
    }
}

impl fmt::Display for Registry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.registry)
    }
}

/// `.*\.local(?:host)?(?::\d{1,5})?$`
fn re_local(name: &str) -> bool {
    let ends_local = |s: &str| s.ends_with(".local") || s.ends_with(".localhost");
    if ends_local(name) {
        return true;
    }
    if let Some(i) = name.rfind(':') {
        let port = &name[i + 1..];
        if (1..=5).contains(&port.len()) && port.bytes().all(|b| b.is_ascii_digit()) {
            return ends_local(&name[..i]);
        }
    }
    false
}

/// Go: `url.Parse("//" + name)` succeeds and `url.Host == name`.
fn check_registry(name: &str) -> Result<(), ErrBadName> {
    if authority_round_trips(name) {
        Ok(())
    } else {
        Err(bad(format!(
            "registries must be valid RFC 3986 URI authorities: {name}"
        )))
    }
}

fn authority_round_trips(name: &str) -> bool {
    // net/url rejects control characters anywhere in the URL.
    if name.bytes().any(|b| b < 0x20 || b == 0x7f) {
        return false;
    }
    // '#' starts the fragment, '?' the query, '/' ends the authority, '@' separates the
    // userinfo, '%' escapes are either invalid or change the host: all of these make the
    // parsed host differ from the input (or fail to parse).
    if name.contains(['#', '?', '/', '@', '%']) {
        return false;
    }
    if let Some(open) = name.rfind('[') {
        if open > 0 {
            return false;
        }
        let Some(close) = name.rfind(']') else {
            return false;
        };
        let colon_port = &name[close + 1..];
        if !valid_optional_port(colon_port) {
            return false;
        }
        let hostname = &name[1..close];
        if !host_chars_ok(hostname) {
            return false;
        }
        // netip.ParseAddr must succeed and must not be IPv4.
        return hostname.parse::<Ipv6Addr>().is_ok();
    } else if let Some(i) = name.rfind(':')
        && !valid_optional_port(&name[i..])
    {
        return false;
    }
    host_chars_ok(name)
}

fn valid_optional_port(port: &str) -> bool {
    if port.is_empty() {
        return true;
    }
    let Some(rest) = port.strip_prefix(':') else {
        return false;
    };
    rest.bytes().all(|b| b.is_ascii_digit())
}

/// `unescape(host, encodeHost)` character validation (no '%' present at this point).
fn host_chars_ok(s: &str) -> bool {
    s.bytes().all(|c| {
        c >= 0x80
            || c.is_ascii_alphanumeric()
            || matches!(
                c,
                b'!' | b'$'
                    | b'&'
                    | b'\''
                    | b'('
                    | b')'
                    | b'*'
                    | b'+'
                    | b','
                    | b';'
                    | b'='
                    | b':'
                    | b'['
                    | b']'
                    | b'<'
                    | b'>'
                    | b'"'
                    | b'-'
                    | b'_'
                    | b'.'
                    | b'~'
            )
    })
}

/// `name.NewRegistry`
pub fn new_registry(name: &str, opts: &[NameOption]) -> Result<Registry, ErrBadName> {
    let opt = make_options(opts);
    if opt.strict && name.is_empty() {
        return Err(bad(
            "strict validation requires the registry to be explicitly defined".to_string(),
        ));
    }
    check_registry(name)?;
    let mut name = name.to_string();
    if name.is_empty() {
        name = opt.default_registry.clone();
    }
    if name == DEFAULT_REGISTRY_ALIAS {
        name = DEFAULT_REGISTRY.to_string();
    }
    Ok(Registry {
        registry: name,
        insecure: opt.insecure,
    })
}

/// `name.NewInsecureRegistry`
pub fn new_insecure_registry(name: &str, opts: &[NameOption]) -> Result<Registry, ErrBadName> {
    let mut opts = opts.to_vec();
    opts.push(NameOption::Insecure);
    new_registry(name, &opts)
}

// ------------------------------------------------------------------ Repository

/// `name.Repository`
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct Repository {
    registry: Registry,
    repository: String,
}

fn has_implicit_namespace(repo: &str, reg: &Registry) -> bool {
    !repo.contains('/') && reg.registry_str() == DEFAULT_REGISTRY
}

impl Repository {
    /// The embedded `Registry`.
    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    /// `RegistryStr()`
    pub fn registry_str(&self) -> &str {
        self.registry.registry_str()
    }

    /// `RepositoryStr()`
    pub fn repository_str(&self) -> String {
        if has_implicit_namespace(&self.repository, &self.registry) {
            format!("{DEFAULT_NAMESPACE}/{}", self.repository)
        } else {
            self.repository.clone()
        }
    }

    /// `Name()`
    pub fn name(&self) -> String {
        let reg_name = self.registry.name();
        if !reg_name.is_empty() {
            return format!("{reg_name}{REG_REPO_DELIMITER}{}", self.repository_str());
        }
        self.repository_str()
    }

    /// `Scope(action)`
    pub fn scope(&self, action: &str) -> String {
        format!("repository:{}:{}", self.repository_str(), action)
    }

    /// `Scheme()`
    pub fn scheme(&self) -> &'static str {
        self.registry.scheme()
    }

    /// `Tag(identifier)`
    pub fn tag(&self, identifier: &str) -> Tag {
        let mut t = Tag {
            repository: self.clone(),
            tag: identifier.to_string(),
            original: String::new(),
        };
        t.original = t.name();
        t
    }

    /// `Digest(identifier)`
    pub fn digest(&self, identifier: &str) -> Digest {
        let mut d = Digest {
            repository: self.clone(),
            digest: identifier.to_string(),
            original: String::new(),
        };
        d.original = d.name();
        d
    }
}

impl fmt::Display for Repository {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.name())
    }
}

fn check_repository(repository: &str) -> Result<(), ErrBadName> {
    check_element("repository", repository, REPOSITORY_CHARS, 2, 255)
}

/// `name.NewRepository`
pub fn new_repository(name: &str, opts: &[NameOption]) -> Result<Repository, ErrBadName> {
    let opt = make_options(opts);
    if name.is_empty() {
        return Err(bad("a repository name must be specified".to_string()));
    }
    let mut registry = "";
    let mut repo = name;
    if let Some((first, rest)) = name.split_once(REG_REPO_DELIMITER)
        && (first.contains('.') || first.contains(':'))
    {
        registry = first;
        repo = rest;
    }
    check_repository(repo)?;
    let reg = new_registry(registry, opts)?;
    if has_implicit_namespace(repo, &reg) && opt.strict {
        return Err(bad(
            "strict validation requires the full repository path (missing 'library')".to_string(),
        ));
    }
    Ok(Repository {
        registry: reg,
        repository: repo.to_string(),
    })
}

// ------------------------------------------------------------------ Tag

/// `name.Tag`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Tag {
    repository: Repository,
    tag: String,
    original: String,
}

impl Tag {
    /// `Context()`
    pub fn context(&self) -> &Repository {
        &self.repository
    }
    /// `Identifier()`
    pub fn identifier(&self) -> &str {
        &self.tag
    }
    /// `TagStr()`
    pub fn tag_str(&self) -> &str {
        &self.tag
    }
    /// `Name()`
    pub fn name(&self) -> String {
        format!("{}{TAG_DELIM}{}", self.repository.name(), self.tag)
    }
    /// `Scope(action)`
    pub fn scope(&self, action: &str) -> String {
        self.repository.scope(action)
    }
}

impl fmt::Display for Tag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.original)
    }
}

fn check_tag(name: &str) -> Result<(), ErrBadName> {
    check_element("tag", name, TAG_CHARS, 1, 128)
}

/// `name.NewTag`
pub fn new_tag(name: &str, opts: &[NameOption]) -> Result<Tag, ErrBadName> {
    let opt = make_options(opts);
    let mut base = name.to_string();
    let mut tag = String::new();

    let parts: Vec<&str> = name.split(TAG_DELIM).collect();
    if parts.len() > 1 && !parts[parts.len() - 1].contains(REG_REPO_DELIMITER) {
        base = parts[..parts.len() - 1].join(TAG_DELIM);
        tag = parts[parts.len() - 1].to_string();
        if tag.is_empty() {
            return Err(bad(format!(
                "{name} must specify a tag name after the colon"
            )));
        }
    }

    if !tag.is_empty() || opt.strict {
        check_tag(&tag)?;
    }

    if tag.is_empty() {
        tag = opt.default_tag.clone();
    }

    let repo = new_repository(&base, opts)?;
    Ok(Tag {
        repository: repo,
        tag,
        original: name.to_string(),
    })
}

// ------------------------------------------------------------------ Digest

/// `name.Digest`
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Digest {
    repository: Repository,
    digest: String,
    original: String,
}

impl Digest {
    /// `Context()`
    pub fn context(&self) -> &Repository {
        &self.repository
    }
    /// `Identifier()`
    pub fn identifier(&self) -> &str {
        &self.digest
    }
    /// `DigestStr()`
    pub fn digest_str(&self) -> &str {
        &self.digest
    }
    /// `Name()`
    pub fn name(&self) -> String {
        format!("{}{DIGEST_DELIM}{}", self.repository.name(), self.digest)
    }
    /// `Scope(action)`
    pub fn scope(&self, action: &str) -> String {
        self.repository.scope(action)
    }
}

impl fmt::Display for Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.original)
    }
}

/// `digest.Canonical.Validate(hex)` for sha256.
fn validate_sha256_hex(hex: &str) -> Result<(), &'static str> {
    if hex.len() != 64 {
        return Err("invalid checksum digest length");
    }
    if !hex.bytes().all(|b| matches!(b, b'a'..=b'f' | b'0'..=b'9')) {
        return Err("invalid checksum digest format");
    }
    Ok(())
}

/// `name.NewDigest`
pub fn new_digest(name: &str, opts: &[NameOption]) -> Result<Digest, Error> {
    let parts: Vec<&str> = name.split(DIGEST_DELIM).collect();
    if parts.len() != 2 {
        return Err(bad(format!(
            "a digest must contain exactly one '@' separator (e.g. registry/repository@digest) saw: {name}"
        ))
        .into());
    }
    let mut base = parts[0].to_string();
    let dig = parts[1];
    let prefix = "sha256:";
    let Some(hex) = dig.strip_prefix(prefix) else {
        return Err(bad(format!("unsupported digest algorithm: {dig}")).into());
    };
    validate_sha256_hex(hex).map_err(Error::Digest)?;

    if let Ok(tag) = new_tag(&base, opts) {
        base = tag.repository.name();
    }

    let repo = new_repository(&base, opts)?;
    Ok(Digest {
        repository: repo,
        digest: dig.to_string(),
        original: name.to_string(),
    })
}

// ------------------------------------------------------------------ Reference

/// `name.Reference` — either a [`Tag`] or a [`Digest`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Reference {
    Tag(Tag),
    Digest(Digest),
}

impl Reference {
    /// `Context()`
    pub fn context(&self) -> &Repository {
        match self {
            Reference::Tag(t) => t.context(),
            Reference::Digest(d) => d.context(),
        }
    }
    /// `Identifier()`
    pub fn identifier(&self) -> &str {
        match self {
            Reference::Tag(t) => t.identifier(),
            Reference::Digest(d) => d.identifier(),
        }
    }
    /// `Name()`
    pub fn name(&self) -> String {
        match self {
            Reference::Tag(t) => t.name(),
            Reference::Digest(d) => d.name(),
        }
    }
    /// `Scope(action)`
    pub fn scope(&self, action: &str) -> String {
        self.context().scope(action)
    }
    /// The digest, if this is a `name.Digest` (Go: `ref.(name.Digest)`).
    pub fn as_digest(&self) -> Option<&Digest> {
        match self {
            Reference::Digest(d) => Some(d),
            Reference::Tag(_) => None,
        }
    }
    /// The tag, if this is a `name.Tag`.
    pub fn as_tag(&self) -> Option<&Tag> {
        match self {
            Reference::Tag(t) => Some(t),
            Reference::Digest(_) => None,
        }
    }
}

impl fmt::Display for Reference {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reference::Tag(t) => t.fmt(f),
            Reference::Digest(d) => d.fmt(f),
        }
    }
}

impl From<Tag> for Reference {
    fn from(t: Tag) -> Self {
        Reference::Tag(t)
    }
}

impl From<Digest> for Reference {
    fn from(d: Digest) -> Self {
        Reference::Digest(d)
    }
}

/// `name.ParseReference`
pub fn parse_reference(s: &str, opts: &[NameOption]) -> Result<Reference, ErrBadName> {
    if let Ok(t) = new_tag(s, opts) {
        return Ok(Reference::Tag(t));
    }
    if let Ok(d) = new_digest(s, opts) {
        return Ok(Reference::Digest(d));
    }
    Err(bad(format!("could not parse reference: {s}")))
}

impl std::str::FromStr for Reference {
    type Err = ErrBadName;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        parse_reference(s, &[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_defaults() {
        let r = parse_reference("ubuntu", &[]).unwrap();
        assert_eq!(r.to_string(), "ubuntu");
        assert_eq!(r.name(), "index.docker.io/library/ubuntu:latest");
        assert_eq!(r.context().registry_str(), "index.docker.io");
        assert_eq!(r.context().repository_str(), "library/ubuntu");
        assert_eq!(r.identifier(), "latest");
        assert_eq!(r.context().scheme(), "https");

        let err = parse_reference("quay.io/ab::x", &[]).unwrap_err();
        assert_eq!(err.to_string(), "could not parse reference: quay.io/ab::x");
    }

    #[test]
    fn schemes() {
        for (reg, want) in [
            ("localhost:5000", "http"),
            ("localhost", "https"),
            ("foo.local", "http"),
            ("foo.localhost:5000", "http"),
            ("foo.local:123456", "https"),
            ("127.0.0.1:5000", "http"),
            ("[::1]:5000", "http"),
            ("10.0.0.1", "http"),
            ("172.16.3.4:80", "http"),
            ("172.32.0.1", "https"),
            ("192.168.0.1", "http"),
            ("quay.io", "https"),
        ] {
            assert_eq!(new_registry(reg, &[]).unwrap().scheme(), want, "{reg}");
        }
    }
}
