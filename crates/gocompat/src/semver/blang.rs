//! Port of `github.com/blang/semver/v4` (v4.0.0): `Parse`, `ParseTolerant`, `MustParse`,
//! comparison and `String`.

use std::fmt;

use crate::strconv::{self, NumError};

const NUMBERS: &str = "0123456789";
const ALPHANUM: &str = "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-0123456789";

/// `semver.PRVersion`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PrVersion {
    pub version_str: String,
    pub version_num: u64,
    pub is_num: bool,
}

impl PrVersion {
    /// `semver.NewPRVersion`.
    pub fn new(s: &str) -> Result<PrVersion, Error> {
        if s.is_empty() {
            return Err(Error::Msg("Prerelease is empty".to_string()));
        }
        let mut v = PrVersion::default();
        if contains_only(s, NUMBERS) {
            if has_leading_zeroes(s) {
                return Err(Error::Msg(format!(
                    "Numeric PreRelease version must not contain leading zeroes {}",
                    strconv::quote(s)
                )));
            }
            v.version_num = strconv::parse_uint(s, 10, 64).map_err(Error::Num)?;
            v.is_num = true;
        } else if contains_only(s, ALPHANUM) {
            v.version_str = s.to_string();
            v.is_num = false;
        } else {
            return Err(Error::Msg(format!(
                "Invalid character(s) found in prerelease {}",
                strconv::quote(s)
            )));
        }
        Ok(v)
    }

    /// `(PRVersion).Compare`.
    pub fn compare(&self, o: &PrVersion) -> i32 {
        if self.is_num && !o.is_num {
            -1
        } else if !self.is_num && o.is_num {
            1
        } else if self.is_num && o.is_num {
            match self.version_num.cmp(&o.version_num) {
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
                std::cmp::Ordering::Less => -1,
            }
        } else {
            match self.version_str.as_bytes().cmp(o.version_str.as_bytes()) {
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Greater => 1,
                std::cmp::Ordering::Less => -1,
            }
        }
    }
}

impl fmt::Display for PrVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.is_num {
            write!(f, "{}", self.version_num)
        } else {
            f.write_str(&self.version_str)
        }
    }
}

/// `semver.Version`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Vec<PrVersion>,
    pub build: Vec<String>,
}

/// Errors (messages as in Go).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `errors.New` / `fmt.Errorf` message.
    Msg(String),
    /// A `*strconv.NumError` returned unwrapped.
    Num(NumError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Msg(m) => f.write_str(m),
            Error::Num(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

fn contains_only(s: &str, set: &str) -> bool {
    s.chars().all(|c| set.contains(c))
}

fn has_leading_zeroes(s: &str) -> bool {
    s.len() > 1 && s.as_bytes()[0] == b'0'
}

/// `semver.Parse`.
pub fn parse(s: &str) -> Result<Version, Error> {
    if s.is_empty() {
        return Err(Error::Msg("Version string empty".to_string()));
    }
    let parts: Vec<&str> = s.splitn(3, '.').collect();
    if parts.len() != 3 {
        return Err(Error::Msg(
            "No Major.Minor.Patch elements found".to_string(),
        ));
    }
    if !contains_only(parts[0], NUMBERS) {
        return Err(Error::Msg(format!(
            "Invalid character(s) found in major number {}",
            strconv::quote(parts[0])
        )));
    }
    if has_leading_zeroes(parts[0]) {
        return Err(Error::Msg(format!(
            "Major number must not contain leading zeroes {}",
            strconv::quote(parts[0])
        )));
    }
    let major = strconv::parse_uint(parts[0], 10, 64).map_err(Error::Num)?;
    if !contains_only(parts[1], NUMBERS) {
        return Err(Error::Msg(format!(
            "Invalid character(s) found in minor number {}",
            strconv::quote(parts[1])
        )));
    }
    if has_leading_zeroes(parts[1]) {
        return Err(Error::Msg(format!(
            "Minor number must not contain leading zeroes {}",
            strconv::quote(parts[1])
        )));
    }
    let minor = strconv::parse_uint(parts[1], 10, 64).map_err(Error::Num)?;

    let mut v = Version {
        major,
        minor,
        ..Default::default()
    };
    let mut patch_str = parts[2];
    let mut build: Vec<&str> = Vec::new();
    let mut prerelease: Vec<&str> = Vec::new();
    if let Some(bi) = patch_str.find('+') {
        build = patch_str[bi + 1..].split('.').collect();
        patch_str = &patch_str[..bi];
    }
    if let Some(pi) = patch_str.find('-') {
        prerelease = patch_str[pi + 1..].split('.').collect();
        patch_str = &patch_str[..pi];
    }
    if !contains_only(patch_str, NUMBERS) {
        return Err(Error::Msg(format!(
            "Invalid character(s) found in patch number {}",
            strconv::quote(patch_str)
        )));
    }
    if has_leading_zeroes(patch_str) {
        return Err(Error::Msg(format!(
            "Patch number must not contain leading zeroes {}",
            strconv::quote(patch_str)
        )));
    }
    v.patch = strconv::parse_uint(patch_str, 10, 64).map_err(Error::Num)?;
    for prstr in prerelease {
        v.pre.push(PrVersion::new(prstr)?);
    }
    for s in build {
        if s.is_empty() {
            return Err(Error::Msg("Build meta data is empty".to_string()));
        }
        if !contains_only(s, ALPHANUM) {
            return Err(Error::Msg(format!(
                "Invalid character(s) found in build meta data {}",
                strconv::quote(s)
            )));
        }
        v.build.push(s.to_string());
    }
    Ok(v)
}

/// `semver.ParseTolerant`.
pub fn parse_tolerant(s: &str) -> Result<Version, Error> {
    let s = crate::unicode::trim_space(s.as_bytes());
    let s = std::str::from_utf8(s).unwrap_or_default();
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts: Vec<String> = s.splitn(3, '.').map(|p| p.to_string()).collect();
    for p in parts.iter_mut() {
        if p.len() > 1 {
            let mut q = p.trim_start_matches('0').to_string();
            if q.is_empty() || !q.as_bytes()[0].is_ascii_digit() {
                q = format!("0{q}");
            }
            *p = q;
        }
    }
    if parts.len() < 3 {
        if parts.last().is_some_and(|p| p.contains(['+', '-'])) {
            return Err(Error::Msg(
                "Short version cannot contain PreRelease/Build meta data".to_string(),
            ));
        }
        while parts.len() < 3 {
            parts.push("0".to_string());
        }
    }
    parse(&parts.join("."))
}

/// `semver.MustParse`: panics like Go on invalid input (use only on constants).
pub fn must_parse(s: &str) -> Version {
    match parse(s) {
        Ok(v) => v,
        Err(e) => panic!("semver: Parse({s}): {e}"),
    }
}

impl Version {
    /// `(Version).Compare`: -1, 0 or 1.
    pub fn compare(&self, o: &Version) -> i32 {
        for (a, b) in [
            (self.major, o.major),
            (self.minor, o.minor),
            (self.patch, o.patch),
        ] {
            if a != b {
                return if a > b { 1 } else { -1 };
            }
        }
        if self.pre.is_empty() && o.pre.is_empty() {
            return 0;
        } else if self.pre.is_empty() && !o.pre.is_empty() {
            return 1;
        } else if !self.pre.is_empty() && o.pre.is_empty() {
            return -1;
        }
        let mut i = 0;
        while i < self.pre.len() && i < o.pre.len() {
            match self.pre[i].compare(&o.pre[i]) {
                0 => {}
                1 => return 1,
                _ => return -1,
            }
            i += 1;
        }
        if i == self.pre.len() && i == o.pre.len() {
            0
        } else if i == self.pre.len() && i < o.pre.len() {
            -1
        } else {
            1
        }
    }

    pub fn equals(&self, o: &Version) -> bool {
        self.compare(o) == 0
    }
    pub fn ne(&self, o: &Version) -> bool {
        self.compare(o) != 0
    }
    pub fn gt(&self, o: &Version) -> bool {
        self.compare(o) == 1
    }
    pub fn gte(&self, o: &Version) -> bool {
        self.compare(o) >= 0
    }
    pub fn lt(&self, o: &Version) -> bool {
        self.compare(o) == -1
    }
    pub fn lte(&self, o: &Version) -> bool {
        self.compare(o) <= 0
    }

    /// `(Version).FinalizeVersion`.
    pub fn finalize_version(&self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre.is_empty() {
            f.write_str("-")?;
            for (i, p) in self.pre.iter().enumerate() {
                if i > 0 {
                    f.write_str(".")?;
                }
                write!(f, "{p}")?;
            }
        }
        if !self.build.is_empty() {
            f.write_str("+")?;
            f.write_str(&self.build.join("."))?;
        }
        Ok(())
    }
}
