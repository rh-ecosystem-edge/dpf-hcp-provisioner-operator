//! The `Validate` methods of `github.com/coreos/ignition/v2/config/v3_4/types` and their
//! helpers (`config/shared/errors`, `config/shared/parse`, `config/shared/validations`).

use std::collections::{HashMap, HashSet};

use super::{ContextPath, Report, cp, dataurl, gonet, gopath, systemd};
use crate::gotypes::SRef;

// config/shared/errors
pub(crate) const ERR_DUPLICATE: &str = "duplicate entry defined";
const ERR_INVALID_VERSION: &str = "invalid config version (couldn't parse)";
const ERR_UNKNOWN_VERSION: &str = "unsupported config version";
const ERR_COMPRESSION_INVALID: &str = "invalid compression method";
const ERR_FILE_USED_SYMLINK: &str = "file path includes link in config";
const ERR_DIRECTORY_USED_SYMLINK: &str = "directory path includes link in config";
const ERR_LINK_USED_SYMLINK: &str = "link path includes link in config";
const ERR_LINK_TARGET_REQUIRED: &str = "link target is required";
const ERR_HARD_LINK_TO_DIRECTORY: &str = "hard link target is a directory";
const ERR_HARD_LINK_SPECIFIES_OWNER: &str = "user/group ignored for hard link";
const ERR_DISK_DEVICE_REQUIRED: &str = "disk device is required";
const ERR_PARTITION_NUMBERS_COLLIDE: &str = "partition numbers collide";
const ERR_PARTITIONS_OVERLAP: &str = "partitions overlap";
const ERR_OVERWRITE_AND_NIL_SOURCE: &str = "overwrite must be false if source is unspecified";
const ERR_VERIFICATION_AND_NIL_SOURCE: &str =
    "source must be specified if verification is specified";
const ERR_FILESYSTEM_INVALID_FORMAT: &str = "invalid filesystem format";
const ERR_LABEL_NEEDS_FORMAT: &str = "filesystem must specify format if label is specified";
const ERR_FORMAT_NIL_WITH_OTHERS: &str = "format cannot be empty when path, label, uuid, wipeFilesystem, options, or mountOptions is specified";
const ERR_EXT4_LABEL_TOO_LONG: &str =
    "filesystem labels cannot be longer than 16 characters when using ext4";
const ERR_BTRFS_LABEL_TOO_LONG: &str =
    "filesystem labels cannot be longer than 256 characters when using btrfs";
const ERR_XFS_LABEL_TOO_LONG: &str =
    "filesystem labels cannot be longer than 12 characters when using xfs";
const ERR_SWAP_LABEL_TOO_LONG: &str =
    "filesystem labels cannot be longer than 15 characters when using swap";
const ERR_VFAT_LABEL_TOO_LONG: &str =
    "filesystem labels cannot be longer than 11 characters when using vfat";
const ERR_LUKS_LABEL_TOO_LONG: &str = "luks device labels cannot be longer than 47 characters";
const ERR_LUKS_NAME_CONTAINS_SLASH: &str = "device names cannot contain slashes";
const ERR_INVALID_LUKS_KEY_FILE: &str = "invalid key-file source";
const ERR_CLEVIS_PIN_REQUIRED: &str = "missing required custom clevis pin";
const ERR_UNKNOWN_CLEVIS_PIN: &str = "unsupported clevis pin";
const ERR_CLEVIS_CONFIG_REQUIRED: &str = "missing required custom clevis config";
const ERR_CLEVIS_CUSTOM_WITH_OTHERS: &str =
    "cannot use custom clevis config with tpm2, tang, or threshold";
const ERR_TANG_THUMBPRINT_REQUIRED: &str = "thumbprint is required";
const ERR_INVALID_TANG_ADVERTISEMENT: &str = "advertisement is not valid JSON";
const ERR_FILE_ILLEGAL_MODE: &str = "illegal file mode";
const ERR_MODE_SPECIAL_BITS: &str =
    "setuid/setgid/sticky bits are not supported or functional in spec versions older than 3.6.0";
const ERR_BOTH_ID_AND_NAME_SET: &str = "cannot set both id and name";
const ERR_LABEL_TOO_LONG: &str = "partition labels may not exceed 36 characters";
const ERR_DOESNT_MATCH_GUID_REGEX: &str =
    "doesn't match the form \"01234567-89AB-CDEF-EDCB-A98765432101\"";
const ERR_LABEL_CONTAINS_COLON: &str = "partition label will be truncated to text before the colon";
const ERR_NO_PATH: &str = "path not specified";
const ERR_PATH_RELATIVE: &str = "path not absolute";
const ERR_DIRTY_PATH: &str = "path is not fully simplified";
const ERR_PARTITIONS_OVERWRITTEN: &str = "filesystem overwrites partitioned device";
const ERR_FILESYSTEM_IMPLICIT_WIPE: &str =
    "device matches disk with wipeTable enabled; filesystem will be wiped";
const ERR_RAID_LEVEL_REQUIRED: &str = "raid level is required";
const ERR_SPARES_UNSUPPORTED_FOR_LEVEL: &str = "spares unsupported for linear and raid0 arrays";
const ERR_UNRECOGNIZED_RAID_LEVEL: &str = "unrecognized raid level";
const ERR_RAID_DEVICES_REQUIRED: &str = "raid devices required";
const ERR_SHOULD_NOT_EXIST_WITH_OTHERS: &str =
    "shouldExist specified false with other options also specified";
const ERR_ZEROES_WITH_SHOULD_NOT_EXIST: &str =
    "shouldExist is false for a partition and other partition(s) has start or size 0";
const ERR_NEED_LABEL_OR_NUMBER: &str = "a partition number >= 1 or a label must be specified";
const ERR_DUPLICATE_LABELS: &str = "cannot use the same partition label twice";
const ERR_INVALID_PROXY: &str = "proxies must be http(s)";
const ERR_INSECURE_PROXY: &str = "insecure plaintext HTTP proxy specified for HTTPS resources";
const ERR_PATH_CONFLICTS_SYSTEMD: &str = "path conflicts with systemd unit or dropin";
const ERR_INVALID_SYSTEMD_EXT: &str = "invalid systemd unit extension";
const ERR_INVALID_SYSTEMD_DROPIN_EXT: &str = "invalid systemd drop-in extension";
const ERR_SOURCE_REQUIRED: &str = "source is required";
const ERR_INVALID_SCHEME: &str = "invalid url scheme";
const ERR_INVALID_URL: &str = "unable to parse url";
const ERR_INVALID_HTTP_HEADER: &str = "unable to parse HTTP header";
const ERR_EMPTY_HTTP_HEADER_NAME: &str = "HTTP header name can't be empty";
const ERR_UNSUPPORTED_SCHEME_FOR_HTTP_HEADERS: &str =
    "cannot use HTTP headers with this source scheme";
const ERR_HASH_MALFORMED: &str = "malformed hash specifier";
const ERR_HASH_WRONG_SIZE: &str = "incorrect size for hash sum";
const ERR_HASH_UNRECOGNIZED: &str = "unrecognized hash function";
const ERR_INVALID_S3_ARN: &str = "invalid S3 ARN format";
const ERR_INVALID_S3_OBJECT_VERSION_ID: &str = "invalid S3 object VersionId";

type GoErr = Option<String>;

fn err(s: &str) -> GoErr {
    Some(s.to_string())
}

// config/util helpers
fn nil_or_empty(s: Option<&str>) -> bool {
    s.is_none_or(str::is_empty)
}
fn not_empty(s: Option<&str>) -> bool {
    s.is_some_and(|s| !s.is_empty())
}
fn is_true(b: Option<bool>) -> bool {
    b == Some(true)
}
fn is_false(b: Option<bool>) -> bool {
    b == Some(false)
}

// ------------------------------------------------------------------------------- path.go

pub(crate) fn validate_path(p: &str) -> GoErr {
    if p.is_empty() {
        return err(ERR_NO_PATH);
    }
    if !gopath::is_abs(p) {
        return err(ERR_PATH_RELATIVE);
    }
    if gopath::clean(p) != p {
        return err(ERR_DIRTY_PATH);
    }
    None
}

fn validate_path_nil_ok(p: Option<&str>) -> GoErr {
    if nil_or_empty(p) {
        return None;
    }
    validate_path(p.unwrap_or_default())
}

// -------------------------------------------------------------------------------- url.go

fn validate_url(s: &str) -> GoErr {
    let Ok(u) = gonet::Url::parse(s) else {
        return err(ERR_INVALID_URL);
    };
    match u.scheme.as_str() {
        "http" | "https" | "tftp" | "gs" => None,
        "s3" => {
            if let Some(v) = gonet::parse_query(&u.raw_query).get("versionId")
                && (v.is_empty() || v[0].is_empty())
            {
                return err(ERR_INVALID_S3_OBJECT_VERSION_ID);
            }
            None
        }
        "arn" => {
            let full_url = format!("{}:{}", u.scheme, u.opaque);
            if !gonet::arn_is_arn(&full_url) {
                return err(ERR_INVALID_S3_ARN);
            }
            let s3arn = match gonet::arn_parse(&full_url) {
                Ok(a) => a,
                Err(e) => return Some(e),
            };
            if s3arn.service != "s3" {
                return err(ERR_INVALID_S3_ARN);
            }
            let url_split: Vec<&str> = full_url.split('/').collect();
            if (s3arn.resource.starts_with("accesspoint/") && url_split.len() < 3)
                || url_split.len() < 2
            {
                return err(ERR_INVALID_S3_ARN);
            }
            if let Some(v) = gonet::parse_query(&u.raw_query).get("versionId")
                && (v.is_empty() || v[0].is_empty())
            {
                return err(ERR_INVALID_S3_OBJECT_VERSION_ID);
            }
            None
        }
        "data" => dataurl::decode_string(s).err(),
        _ => err(ERR_INVALID_SCHEME),
    }
}

fn validate_url_nil_ok(s: Option<&str>) -> GoErr {
    if nil_or_empty(s) {
        return None;
    }
    validate_url(s.unwrap_or_default())
}

// ------------------------------------------------------------------------------- mode.go

fn validate_mode(m: Option<i64>) -> GoErr {
    if let Some(m) = m
        && !(0..=0o7777).contains(&m)
    {
        return err(ERR_FILE_ILLEGAL_MODE);
    }
    None
}

fn validate_mode_special_bits(m: Option<i64>) -> GoErr {
    if let Some(m) = m {
        let mode = m as u32;
        if mode & 0o7000 != 0 {
            return err(ERR_MODE_SPECIAL_BITS);
        }
    }
    None
}

// ---------------------------------------------------------------------------- config.go

pub(crate) fn config(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    let systemd_path = "/etc/systemd/system/";
    let mut unit_paths = HashSet::new();
    for unit in s.sub("Systemd").structs("Units") {
        if !nil_or_empty(unit.ptr_str("Contents")) {
            unit_paths.insert(format!("{systemd_path}{}", unit.str("Name")));
        }
        for dropin in unit.structs("Dropins") {
            if !nil_or_empty(dropin.ptr_str("Contents")) {
                unit_paths.insert(format!(
                    "{systemd_path}{}.d/{}",
                    unit.str("Name"),
                    dropin.str("Name")
                ));
            }
        }
    }
    let storage = s.sub("Storage");
    for (list, json) in [
        ("Files", "files"),
        ("Directories", "directories"),
        ("Links", "links"),
    ] {
        for (i, f) in storage.structs(list).iter().enumerate() {
            if unit_paths.contains(f.str("Path")) {
                r.add_on_error(
                    &cp!(c, "storage", json, i, "path"),
                    err(ERR_PATH_CONFLICTS_SYSTEMD),
                );
            }
        }
    }
    r
}

// ---------------------------------------------------------------------------- ignition.go

pub(crate) fn ignition(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    let c = c.append("version");
    match gocompat::semver::coreos::Version::new(s.str("Version")) {
        Err(_) => r.add_on_error(&c, err(ERR_INVALID_VERSION)),
        Ok(tv) => {
            let max = gocompat::semver::coreos::Version::new("3.4.0").ok();
            if max.as_ref() != Some(&tv) {
                r.add_on_error(&c, err(ERR_UNKNOWN_VERSION));
            }
        }
    }
    r
}

pub(crate) fn ignition_config(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    for (i, res) in s.structs("Merge").iter().enumerate() {
        r.add_on_error(&cp!(c, "merge", i), validate_required_source(res));
    }
    r
}

// ------------------------------------------------------------------------------- tls.go

pub(crate) fn tls(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    for (i, ca) in s.structs("CertificateAuthorities").iter().enumerate() {
        r.add_on_error(
            &cp!(c, "certificateAuthorities", i),
            validate_required_source(ca),
        );
    }
    r
}

// ----------------------------------------------------------------------------- proxy.go

pub(crate) fn proxy(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    validate_proxy_url(s.ptr_str("HTTPProxy"), &c.append("httpProxy"), &mut r, true);
    validate_proxy_url(
        s.ptr_str("HTTPSProxy"),
        &c.append("httpsProxy"),
        &mut r,
        false,
    );
    r
}

fn validate_proxy_url(s: Option<&str>, p: &ContextPath, r: &mut Report, http_ok: bool) {
    let Some(s) = s else { return };
    let Ok(u) = gonet::Url::parse(s) else {
        r.add_on_error(p, err(ERR_INVALID_URL));
        return;
    };
    if u.scheme != "https" && u.scheme != "http" {
        r.add_on_error(p, err(ERR_INVALID_PROXY));
        return;
    }
    if u.scheme == "http" && !http_ok {
        r.add_on_warn(p, err(ERR_INSECURE_PROXY));
    }
}

// -------------------------------------------------------------------------- resource.go

pub(crate) fn resource(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    r.add_on_error(&c.append("compression"), validate_compression(s));
    r.add_on_error(&cp!(c, "verification", "hash"), validate_verification(s));
    r.add_on_error(
        &c.append("source"),
        validate_url_nil_ok(s.ptr_str("Source")),
    );
    r.add_on_error(
        &c.append("httpHeaders"),
        validate_scheme_for_http_headers(s),
    );
    r
}

fn validate_compression(s: &SRef<'_>) -> GoErr {
    match s.ptr_str("Compression") {
        Some("") | Some("gzip") | None => None,
        Some(_) => err(ERR_COMPRESSION_INVALID),
    }
}

fn validate_verification(s: &SRef<'_>) -> GoErr {
    if s.sub("Verification").ptr_str("Hash").is_some() && s.ptr_str("Source").is_none() {
        return err(ERR_VERIFICATION_AND_NIL_SOURCE);
    }
    None
}

fn validate_scheme_for_http_headers(s: &SRef<'_>) -> GoErr {
    if s.slice("HTTPHeaders").is_empty() {
        return None;
    }
    let src = s.ptr_str("Source");
    if nil_or_empty(src) {
        return err(ERR_INVALID_URL);
    }
    let Ok(u) = gonet::Url::parse(src.unwrap_or_default()) else {
        return err(ERR_INVALID_URL);
    };
    match u.scheme.as_str() {
        "http" | "https" => None,
        _ => err(ERR_UNSUPPORTED_SCHEME_FOR_HTTP_HEADERS),
    }
}

fn validate_required_source(s: &SRef<'_>) -> GoErr {
    let src = s.ptr_str("Source");
    if nil_or_empty(src) {
        return err(ERR_SOURCE_REQUIRED);
    }
    validate_url(src.unwrap_or_default())
}

// ---------------------------------------------------------------------- verification.go

pub(crate) fn verification(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    let c = c.append("hash");
    let Some(hash) = s.ptr_str("Hash") else {
        return r;
    };
    let Some((function, sum)) = hash.split_once('-') else {
        r.add_on_error(&c, err(ERR_HASH_MALFORMED));
        return r;
    };
    let size = match function {
        "sha512" => 64,
        "sha256" => 32,
        _ => {
            r.add_on_error(&c, err(ERR_HASH_UNRECOGNIZED));
            return r;
        }
    };
    if sum.len() != size * 2 {
        r.add_on_error(&c, err(ERR_HASH_WRONG_SIZE));
    }
    r
}

// --------------------------------------------------------------------------- headers.go

pub(crate) fn http_header(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    r.add_on_error(
        &c.append("name"),
        if s.str("Name").is_empty() {
            err(ERR_EMPTY_HTTP_HEADER_NAME)
        } else {
            None
        },
    );
    r.add_on_error(
        &c.append("value"),
        match s.ptr_str("Value") {
            Some("") => err(ERR_INVALID_HTTP_HEADER),
            _ => None,
        },
    );
    r
}

// ------------------------------------------------------------------------------ node.go

pub(crate) fn node(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    r.add_on_error(&c.append("path"), validate_path(s.str("Path")));
    r
}

pub(crate) fn node_owner(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    if s.ptr_int("ID").is_some() && not_empty(s.ptr_str("Name")) {
        r.add_on_error(c, err(ERR_BOTH_ID_AND_NAME_SET));
    }
    r
}

// ------------------------------------------------------------------------------ file.go

pub(crate) fn file(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = node(s, c);
    let mode = s.ptr_int("Mode");
    r.add_on_error(&c.append("mode"), validate_mode(mode));
    r.add_on_warn(&c.append("mode"), validate_mode_special_bits(mode));
    let overwrite =
        if is_true(s.ptr_bool("Overwrite")) && s.sub("Contents").ptr_str("Source").is_none() {
            err(ERR_OVERWRITE_AND_NIL_SOURCE)
        } else {
            None
        };
    r.add_on_error(&c.append("overwrite"), overwrite);
    r
}

// ------------------------------------------------------------------------- directory.go

pub(crate) fn directory(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = node(s, c);
    let mode = s.ptr_int("Mode");
    r.add_on_error(&c.append("mode"), validate_mode(mode));
    r.add_on_warn(&c.append("mode"), validate_mode_special_bits(mode));
    r
}

// --------------------------------------------------------------------------- storage.go

pub(crate) fn storage(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    let dirs = s.structs("Directories");
    let files = s.structs("Files");
    let links = s.structs("Links");
    for (i, d) in dirs.iter().enumerate() {
        for l in &links {
            if d.str("Path").starts_with(&format!("{}/", l.str("Path"))) {
                r.add_on_error(&cp!(c, "directories", i), err(ERR_DIRECTORY_USED_SYMLINK));
            }
        }
    }
    for (i, f) in files.iter().enumerate() {
        for l in &links {
            if f.str("Path").starts_with(&format!("{}/", l.str("Path"))) {
                r.add_on_error(&cp!(c, "files", i), err(ERR_FILE_USED_SYMLINK));
            }
        }
    }
    for (i, l1) in links.iter().enumerate() {
        for l2 in &links {
            if l1.str("Path").starts_with(&format!("{}/", l2.str("Path"))) {
                r.add_on_error(&cp!(c, "links", i), err(ERR_LINK_USED_SYMLINK));
            }
        }
        let target = l1.ptr_str("Target");
        if nil_or_empty(target) {
            r.add_on_error(&cp!(c, "links", i, "target"), err(ERR_LINK_TARGET_REQUIRED));
            continue;
        }
        if !is_true(l1.ptr_bool("Hard")) {
            continue;
        }
        let raw_target = target.unwrap_or_default();
        let mut target = gopath::clean(raw_target);
        if !gopath::is_abs(&target) {
            target = gopath::join(&[l1.str("Path"), raw_target]);
        }
        for d in &dirs {
            if target == d.str("Path") {
                r.add_on_error(&cp!(c, "links", i), err(ERR_HARD_LINK_TO_DIRECTORY));
            }
        }
        let user = l1.sub("User");
        let group = l1.sub("Group");
        let mut owner_check = |ok: bool, p: ContextPath| {
            if !ok {
                r.add_on_warn(&p, err(ERR_HARD_LINK_SPECIFIES_OWNER));
            }
        };
        owner_check(
            user.ptr_int("ID").is_none(),
            cp!(c, "links", i, "user", "id"),
        );
        owner_check(
            user.ptr_str("Name").is_none(),
            cp!(c, "links", i, "user", "name"),
        );
        owner_check(
            group.ptr_int("ID").is_none(),
            cp!(c, "links", i, "group", "id"),
        );
        owner_check(
            group.ptr_str("Name").is_none(),
            cp!(c, "links", i, "group", "name"),
        );
    }
    // validateFilesystems
    let mut disks: HashMap<&str, SRef<'_>> = HashMap::new();
    for d in s.structs("Disks") {
        disks.insert(d.str("Device"), d);
    }
    for (i, f) in s.structs("Filesystems").iter().enumerate() {
        if let Some(disk) = disks.get(f.str("Device")) {
            if !disk.slice("Partitions").is_empty() {
                r.add_on_warn(
                    &cp!(c, "filesystems", i, "device"),
                    err(ERR_PARTITIONS_OVERWRITTEN),
                );
            } else if !is_true(f.ptr_bool("WipeFilesystem")) && is_true(disk.ptr_bool("WipeTable"))
            {
                r.add_on_warn(
                    &cp!(c, "filesystems", i, "device"),
                    err(ERR_FILESYSTEM_IMPLICIT_WIPE),
                );
            }
        }
    }
    r
}

// ------------------------------------------------------------------------------ disk.go

pub(crate) fn disk(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    let device = s.str("Device");
    if device.is_empty() {
        r.add_on_error(&c.append("device"), err(ERR_DISK_DEVICE_REQUIRED));
        return r;
    }
    r.add_on_error(&c.append("device"), validate_path(device));
    let parts = s.structs("Partitions");
    if let Some(p) = partition_numbers_collide(&parts) {
        r.add_on_error(&cp!(c, "partitions", p), err(ERR_PARTITION_NUMBERS_COLLIDE));
    }
    if let Some(p) = partitions_overlap(&parts) {
        r.add_on_error(&cp!(c, "partitions", p), err(ERR_PARTITIONS_OVERLAP));
    }
    if partitions_mix_zeroes_and_nonexistence(&parts) {
        r.add_on_error(
            &c.append("partitions"),
            err(ERR_ZEROES_WITH_SHOULD_NOT_EXIST),
        );
    }
    if let Some(p) = partition_labels_collide(&parts) {
        r.add_on_error(&cp!(c, "partitions", p), err(ERR_DUPLICATE_LABELS));
    }
    r
}

/// Go iterates a map of number -> indexes and returns the second index of the first
/// colliding number it meets (random order when several numbers collide). Numbers are
/// visited here in order of first appearance.
fn partition_numbers_collide(parts: &[SRef<'_>]) -> Option<usize> {
    let mut order: Vec<i64> = Vec::new();
    let mut m: HashMap<i64, Vec<usize>> = HashMap::new();
    for (i, p) in parts.iter().enumerate() {
        let n = p.int("Number");
        if n != 0 {
            if !m.contains_key(&n) {
                order.push(n);
            }
            m.entry(n).or_default().push(i);
        }
    }
    order.iter().find_map(|n| {
        let v = &m[n];
        (v.len() > 1).then(|| v[1])
    })
}

fn partition_labels_collide(parts: &[SRef<'_>]) -> Option<usize> {
    let mut m = HashSet::new();
    for (i, p) in parts.iter().enumerate() {
        if let Some(l) = p.ptr_str("Label") {
            if m.contains(l) {
                return Some(i);
            }
            m.insert(l);
        }
    }
    None
}

fn partition_end(start: i64, size: i64) -> i64 {
    if size == 0 { start } else { start + size - 1 }
}

fn partitions_overlap(parts: &[SRef<'_>]) -> Option<usize> {
    for (pi, p) in parts.iter().enumerate() {
        let (Some(ps), Some(psz)) = (p.ptr_int("StartMiB"), p.ptr_int("SizeMiB")) else {
            continue;
        };
        if ps == 0 {
            continue;
        }
        let pend = partition_end(ps, psz);
        for (i, o) in parts.iter().enumerate() {
            // Go compares the structs by value (pointer fields by address): only the
            // partition itself compares equal.
            let (Some(os), Some(osz)) = (o.ptr_int("StartMiB"), o.ptr_int("SizeMiB")) else {
                continue;
            };
            if pi == i || os == 0 {
                continue;
            }
            let oend = partition_end(os, osz);
            if ps >= os && ps <= oend {
                return Some(i);
            }
            if pend >= os && pend <= oend {
                return Some(i);
            }
            if ps < os && pend > oend {
                return Some(i);
            }
        }
    }
    None
}

fn partitions_mix_zeroes_and_nonexistence(parts: &[SRef<'_>]) -> bool {
    let mut has_zero = false;
    let mut has_should_not_exist = false;
    for p in parts {
        has_should_not_exist = has_should_not_exist || is_false(p.ptr_bool("ShouldExist"));
        has_zero = has_zero || p.int("Number") == 0;
    }
    has_zero && has_should_not_exist
}

// ------------------------------------------------------------------------- partition.go

pub(crate) fn partition_key(s: &SRef<'_>) -> String {
    let n = s.int("Number");
    if n != 0 {
        format!("number:{n}")
    } else if let Some(l) = s.ptr_str("Label") {
        format!("label:{l}")
    } else {
        String::new()
    }
}

pub(crate) fn partition(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    let label = s.ptr_str("Label");
    if is_false(s.ptr_bool("ShouldExist"))
        && (label.is_some()
            || not_empty(s.ptr_str("TypeGUID"))
            || not_empty(s.ptr_str("GUID"))
            || s.ptr_int("StartMiB").is_some()
            || s.ptr_int("SizeMiB").is_some())
    {
        r.add_on_error(c, err(ERR_SHOULD_NOT_EXIST_WITH_OTHERS));
    }
    if s.int("Number") == 0 && label.is_none() {
        r.add_on_error(c, err(ERR_NEED_LABEL_OR_NUMBER));
    }
    let label_err = match label {
        None => None,
        Some(l) if l.len() > 36 => err(ERR_LABEL_TOO_LONG),
        Some(l) if l.contains(':') => err(ERR_LABEL_CONTAINS_COLON),
        Some(_) => None,
    };
    r.add_on_error(&c.append("label"), label_err);
    r.add_on_error(&c.append("guid"), validate_guid(s.ptr_str("GUID")));
    r.add_on_error(&c.append("typeGuid"), validate_guid(s.ptr_str("TypeGUID")));
    r
}

/// `^(|[[:xdigit:]]{8}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{4}-[[:xdigit:]]{12})$`
fn validate_guid(g: Option<&str>) -> GoErr {
    let g = g?;
    if g.is_empty() {
        return None;
    }
    let groups: Vec<&str> = g.split('-').collect();
    let ok = groups.len() == 5
        && groups
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.bytes().all(|b| b.is_ascii_hexdigit()));
    if ok {
        None
    } else {
        err(ERR_DOESNT_MATCH_GUID_REGEX)
    }
}

// ------------------------------------------------------------------------ filesystem.go

pub(crate) fn filesystem(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    r.add_on_error(&c.append("path"), validate_path_nil_ok(s.ptr_str("Path")));
    r.add_on_error(&c.append("device"), validate_path(s.str("Device")));
    r.add_on_error(&c.append("format"), validate_format(s));
    r.add_on_error(&c.append("label"), validate_fs_label(s));
    r
}

fn validate_format(s: &SRef<'_>) -> GoErr {
    let format = s.ptr_str("Format");
    if nil_or_empty(format) {
        if not_empty(s.ptr_str("Path"))
            || not_empty(s.ptr_str("Label"))
            || not_empty(s.ptr_str("UUID"))
            || is_true(s.ptr_bool("WipeFilesystem"))
            || !s.slice("MountOptions").is_empty()
            || !s.slice("Options").is_empty()
        {
            return err(ERR_FORMAT_NIL_WITH_OTHERS);
        }
    } else {
        match format.unwrap_or_default() {
            "ext4" | "btrfs" | "xfs" | "swap" | "vfat" | "none" => {}
            _ => return err(ERR_FILESYSTEM_INVALID_FORMAT),
        }
    }
    None
}

fn validate_fs_label(s: &SRef<'_>) -> GoErr {
    let label = s.ptr_str("Label");
    if nil_or_empty(label) {
        return None;
    }
    let format = s.ptr_str("Format");
    if nil_or_empty(format) {
        return err(ERR_LABEL_NEEDS_FORMAT);
    }
    let n = label.unwrap_or_default().len();
    match format.unwrap_or_default() {
        "ext4" if n > 16 => err(ERR_EXT4_LABEL_TOO_LONG),
        "btrfs" if n > 256 => err(ERR_BTRFS_LABEL_TOO_LONG),
        "xfs" if n > 12 => err(ERR_XFS_LABEL_TOO_LONG),
        "swap" if n > 15 => err(ERR_SWAP_LABEL_TOO_LONG),
        "vfat" if n > 11 => err(ERR_VFAT_LABEL_TOO_LONG),
        _ => None,
    }
}

// ------------------------------------------------------------------------------ luks.go

pub(crate) fn luks(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    if s.str("Name").contains('/') {
        r.add_on_error(&c.append("name"), err(ERR_LUKS_NAME_CONTAINS_SLASH));
    }
    let label = s.ptr_str("Label");
    let label_err = if !nil_or_empty(label) && label.unwrap_or_default().len() > 47 {
        err(ERR_LUKS_LABEL_TOO_LONG)
    } else {
        None
    };
    r.add_on_error(&c.append("label"), label_err);
    let device = s.ptr_str("Device");
    if nil_or_empty(device) {
        r.add_on_error(&c.append("device"), err(ERR_DISK_DEVICE_REQUIRED));
    } else {
        r.add_on_error(
            &c.append("device"),
            validate_path(device.unwrap_or_default()),
        );
    }
    let clevis = s.sub("Clevis");
    if not_empty(clevis.sub("Custom").ptr_str("Pin"))
        && (!clevis.slice("Tang").is_empty()
            || is_true(clevis.ptr_bool("Tpm2"))
            || clevis.ptr_int("Threshold").is_some_and(|t| t != 0))
    {
        r.add_on_error(&c.append("clevis"), err(ERR_CLEVIS_CUSTOM_WITH_OTHERS));
    }
    if validate_url_nil_ok(s.sub("KeyFile").ptr_str("Source")).is_some() {
        r.add_on_error(&c.append("keys"), err(ERR_INVALID_LUKS_KEY_FILE));
    }
    r
}

// ---------------------------------------------------------------------------- clevis.go

pub(crate) fn clevis_custom(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    let pin = s.ptr_str("Pin");
    let config = s.ptr_str("Config");
    if nil_or_empty(pin) && nil_or_empty(config) && !is_true(s.ptr_bool("NeedsNetwork")) {
        return r;
    }
    if not_empty(pin) {
        match pin.unwrap_or_default() {
            "tpm2" | "tang" | "sss" => {}
            _ => r.add_on_error(&c.append("pin"), err(ERR_UNKNOWN_CLEVIS_PIN)),
        }
    } else {
        r.add_on_error(&c.append("pin"), err(ERR_CLEVIS_PIN_REQUIRED));
    }
    if nil_or_empty(config) {
        r.add_on_error(&c.append("config"), err(ERR_CLEVIS_CONFIG_REQUIRED));
    }
    r
}

// ------------------------------------------------------------------------------ tang.go

pub(crate) fn tang(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    let url_err = match gonet::Url::parse(s.str("URL")) {
        Err(_) => err(ERR_INVALID_URL),
        Ok(u) if u.scheme == "http" || u.scheme == "https" => None,
        Ok(_) => err(ERR_INVALID_SCHEME),
    };
    r.add_on_error(&c.append("url"), url_err);
    if nil_or_empty(s.ptr_str("Thumbprint")) {
        r.add_on_error(&c.append("thumbprint"), err(ERR_TANG_THUMBPRINT_REQUIRED));
    }
    let adv = s.ptr_str("Advertisement");
    let adv_err = if not_empty(adv)
        && gocompat::json::unmarshal_any(adv.unwrap_or_default().as_bytes()).is_err()
    {
        err(ERR_INVALID_TANG_ADVERTISEMENT)
    } else {
        None
    };
    r.add_on_error(&c.append("advertisement"), adv_err);
    r
}

// ------------------------------------------------------------------------------ raid.go

pub(crate) fn raid(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    let level = s.ptr_str("Level");
    let level_err = if nil_or_empty(level) {
        err(ERR_RAID_LEVEL_REQUIRED)
    } else {
        match level.unwrap_or_default() {
            "linear" | "raid0" | "0" | "stripe" => {
                if s.ptr_int("Spares").is_some_and(|x| x != 0) {
                    err(ERR_SPARES_UNSUPPORTED_FOR_LEVEL)
                } else {
                    None
                }
            }
            "raid1" | "1" | "mirror" | "raid4" | "4" | "raid5" | "5" | "raid6" | "6" | "raid10"
            | "10" => None,
            _ => err(ERR_UNRECOGNIZED_RAID_LEVEL),
        }
    };
    r.add_on_error(&c.append("level"), level_err);
    if s.slice("Devices").is_empty() {
        r.add_on_error(&c.append("devices"), err(ERR_RAID_DEVICES_REQUIRED));
    }
    r
}

/// `Device.Validate`.
pub(crate) fn device(d: &str, c: &ContextPath) -> Report {
    let mut r = Report::default();
    r.add_on_error(c, validate_path(d));
    r
}

// ------------------------------------------------------------------- systemd.go, unit.go

/// `parse.ParseUnitContents`: `Ok(None)` stands for the non-nil empty option list Go
/// returns for nil contents.
fn parse_unit_contents(content: Option<&str>) -> Result<Vec<systemd::UnitOption>, String> {
    match content {
        None => Ok(Vec::new()),
        Some(c) => {
            systemd::deserialize(c.as_bytes()).map_err(|e| format!("invalid unit content: {e}"))
        }
    }
}

/// `validations.ValidateInstallSection`.
fn validate_install_section(
    name: &str,
    enabled: bool,
    contents_empty: bool,
    sections: Option<&[systemd::UnitOption]>,
) -> GoErr {
    if !enabled || contents_empty {
        return None;
    }
    let sections = sections?;
    if sections.iter().any(|s| s.section == "Install") {
        return None;
    }
    Some(format!(
        "unit {} is enabled, but has no install section so enable does nothing",
        gocompat::strconv::quote(name)
    ))
}

pub(crate) fn unit(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    let name = s.str("Name");
    let name_err = match gopath::ext(name) {
        ".service" | ".socket" | ".device" | ".mount" | ".automount" | ".swap" | ".target"
        | ".path" | ".timer" | ".snapshot" | ".slice" | ".scope" => None,
        _ => err(ERR_INVALID_SYSTEMD_EXT),
    };
    r.add_on_error(&c.append("name"), name_err);
    let c = c.append("contents");
    let contents = s.ptr_str("Contents");
    let (opts, perr) = match parse_unit_contents(contents) {
        Ok(o) => (Some(o), None),
        Err(e) => (None, Some(e)),
    };
    r.add_on_error(&c, perr);
    r.add_on_warn(
        &c,
        validate_install_section(
            name,
            is_true(s.ptr_bool("Enabled")),
            nil_or_empty(contents),
            opts.as_deref(),
        ),
    );
    r
}

pub(crate) fn dropin(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    r.add_on_error(
        &c.append("contents"),
        parse_unit_contents(s.ptr_str("Contents")).err(),
    );
    if gopath::ext(s.str("Name")) != ".conf" {
        r.add_on_error(&c.append("name"), err(ERR_INVALID_SYSTEMD_DROPIN_EXT));
    }
    r
}

/// `^(.+?)@(.+?)\.service$` (`.` does not match `\n`): returns the first group.
fn instance_unit_prefix(name: &str) -> Option<&str> {
    let body = name.strip_suffix(".service")?;
    for (i, ch) in body.char_indices() {
        if ch == '\n' {
            return None;
        }
        if ch == '@' && i > 0 {
            let rest = &body[i + 1..];
            if !rest.is_empty() && !rest.contains('\n') {
                return Some(&body[..i]);
            }
        }
    }
    None
}

pub(crate) fn systemd(s: &SRef<'_>, c: &ContextPath) -> Report {
    let mut r = Report::default();
    let units = s.structs("Units");
    let mut by_name: HashMap<&str, SRef<'_>> = HashMap::new();
    for u in &units {
        by_name.insert(u.str("Name"), *u);
    }
    for (index, unit) in units.iter().enumerate() {
        let Some(prefix) = instance_unit_prefix(unit.str("Name")) else {
            continue;
        };
        if !is_true(unit.ptr_bool("Enabled")) {
            continue;
        }
        let instantiable = format!("{prefix}@.service");
        if let Some(inst) = by_name.get(instantiable.as_str())
            && not_empty(inst.ptr_str("Contents"))
        {
            let Ok(opts) = parse_unit_contents(inst.ptr_str("Contents")) else {
                continue;
            };
            if !opts.iter().any(|o| o.section == "Install") {
                r.add_on_warn(
                    &cp!(c, "units", index, "contents"),
                    Some(format!(
                        "template unit {} for {} doesn't have Install section",
                        gocompat::strconv::quote(&instantiable),
                        gocompat::strconv::quote(unit.str("Name"))
                    )),
                );
            }
        }
    }
    r
}
