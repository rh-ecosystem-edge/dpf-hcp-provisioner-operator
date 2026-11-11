//! Port of `github.com/coreos/go-semver/semver` (v0.3.1).

use std::cmp::Ordering;
use std::fmt;

use crate::strconv::{self, NumError};

/// `semver.Version`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Version {
    pub major: i64,
    pub minor: i64,
    pub patch: i64,
    pub pre_release: String,
    pub metadata: String,
}

/// Errors returned by [`Version::new`] (messages as in Go).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A formatted error message (`fmt.Errorf`).
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

fn split_off(input: &mut String, delim: char) -> String {
    match input.split_once(delim) {
        Some((a, b)) => {
            let val = b.to_string();
            *input = a.to_string();
            val
        }
        None => String::new(),
    }
}

/// `^[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*$`
fn is_identifier(id: &str) -> bool {
    id.split('.')
        .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'))
}

fn validate_identifier(id: &str) -> Result<(), String> {
    if !id.is_empty() && !is_identifier(id) {
        return Err(format!("{id} is not a valid semver identifier"));
    }
    Ok(())
}

impl Version {
    /// `semver.NewVersion`.
    pub fn new(version: &str) -> Result<Version, Error> {
        let mut v = Version::default();
        v.set(version)?;
        Ok(v)
    }

    /// `(*Version).Set`.
    pub fn set(&mut self, version: &str) -> Result<(), Error> {
        let mut version = version.to_string();
        let metadata = split_off(&mut version, '+');
        let pre_release = split_off(&mut version, '-');
        let dot_parts: Vec<&str> = version.splitn(3, '.').collect();
        if dot_parts.len() != 3 {
            return Err(Error::Msg(format!("{version} is not in dotted-tri format")));
        }
        if let Err(e) = validate_identifier(&pre_release) {
            return Err(Error::Msg(format!("failed to validate pre-release: {e}")));
        }
        if let Err(e) = validate_identifier(&metadata) {
            return Err(Error::Msg(format!("failed to validate metadata: {e}")));
        }
        let mut parsed = [0i64; 3];
        for (i, p) in dot_parts.iter().enumerate() {
            parsed[i] = strconv::parse_int(p, 10, 64).map_err(Error::Num)?;
        }
        self.metadata = metadata;
        self.pre_release = pre_release;
        self.major = parsed[0];
        self.minor = parsed[1];
        self.patch = parsed[2];
        Ok(())
    }

    /// `(Version).Compare`: -1, 0 or 1.
    pub fn compare(&self, b: &Version) -> i32 {
        for (x, y) in [
            (self.major, b.major),
            (self.minor, b.minor),
            (self.patch, b.patch),
        ] {
            match x.cmp(&y) {
                Ordering::Greater => return 1,
                Ordering::Less => return -1,
                Ordering::Equal => {}
            }
        }
        pre_release_compare(self, b)
    }

    /// `(Version).Equal`.
    pub fn equal(&self, b: &Version) -> bool {
        self.compare(b) == 0
    }

    /// `(Version).LessThan`.
    pub fn less_than(&self, b: &Version) -> bool {
        self.compare(b) < 0
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre_release.is_empty() {
            write!(f, "-{}", self.pre_release)?;
        }
        if !self.metadata.is_empty() {
            write!(f, "+{}", self.metadata)?;
        }
        Ok(())
    }
}

fn pre_release_compare(a: &Version, b: &Version) -> i32 {
    let (a, b) = (&a.pre_release, &b.pre_release);
    if a.is_empty() && !b.is_empty() {
        return 1;
    } else if b.is_empty() && !a.is_empty() {
        return -1;
    }
    let av: Vec<&str> = a.split('.').collect();
    let bv: Vec<&str> = b.split('.').collect();
    recursive_pre_release_compare(&av, &bv)
}

fn recursive_pre_release_compare(a: &[&str], b: &[&str]) -> i32 {
    if a.is_empty() {
        if !b.is_empty() {
            return -1;
        }
        return 0;
    } else if b.is_empty() {
        return 1;
    }
    let ai = strconv::atoi(a[0]).ok();
    let bi = strconv::atoi(b[0]).ok();
    match (ai, bi) {
        (Some(_), None) => return -1,
        (None, Some(_)) => return 1,
        (Some(x), Some(y)) => {
            if x > y {
                return 1;
            } else if x < y {
                return -1;
            }
        }
        (None, None) => {}
    }
    if a[0] > b[0] {
        return 1;
    } else if a[0] < b[0] {
        return -1;
    }
    recursive_pre_release_compare(&a[1..], &b[1..])
}
