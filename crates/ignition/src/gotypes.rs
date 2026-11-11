//! Reflection model of the Go `github.com/coreos/ignition/v2/config/v3_4/types` package.
//!
//! The official Rust types (`ignition_config::v3_4`) differ from the Go structs in ways that
//! matter for byte-exact behaviour (alphabetical field order, `Option` for Go non-pointer
//! structs, no embedding, no merge/validate). Go's merge, validation and `encoding/json` code
//! is reflection based, so it is ported here over a dynamic value ([`GoVal`]) described by a
//! static schema ([`StructDef`]) that mirrors the Go declarations exactly: field order, Go
//! field names, JSON tags, `omitempty`, embedding and the methods (`Key`, `IgnoreDuplicates`,
//! `MergedKeys`, `Validate`) each Go type has (including promoted methods).

use ignition_config::v3_4::Config;
use serde_json::{Map, Value};

use crate::validate::{ContextPath, Report};

/// A Go primitive kind (used for pointer elements).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Prim {
    Str,
    Int,
    Bool,
}

/// A Go type of the schema.
#[derive(Clone, Copy)]
pub(crate) enum Ty {
    /// `string` or a named string type (the Go type name, e.g. `types.Device`).
    Str(&'static str),
    /// `int`.
    Int,
    /// `*string`, `*int`, `*bool`.
    Ptr(Prim),
    /// A slice type.
    Slice(&'static SliceDef),
    /// A struct type.
    Struct(&'static StructDef),
}

pub(crate) struct SliceDef {
    /// Go type name (`[]types.File`, `types.HTTPHeaders`).
    pub name: &'static str,
    pub elem: Ty,
}

pub(crate) type KeyFn = fn(&SRef<'_>) -> String;
pub(crate) type ValidateFn = fn(&SRef<'_>, &ContextPath) -> Report;

pub(crate) struct StructDef {
    /// Go type name without package (`File`).
    pub name: &'static str,
    pub fields: &'static [Field],
    /// `Key()` (own or promoted).
    pub key: Option<KeyFn>,
    /// `IgnoreDuplicates()` (own or promoted).
    pub ignore_dups: &'static [&'static str],
    /// `MergedKeys()`.
    pub merged_keys: &'static [(&'static str, &'static str)],
    /// `Validate(path.ContextPath)` with a value receiver (own or promoted).
    pub validate: Option<ValidateFn>,
}

pub(crate) struct Field {
    /// Go field name.
    pub name: &'static str,
    /// JSON name (empty for embedded fields).
    pub json: &'static str,
    pub omitempty: bool,
    /// Anonymous (embedded) struct field.
    pub embedded: bool,
    pub ty: Ty,
}

/// A Go value of a schema type.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GoVal {
    Str(String),
    Int(i64),
    /// A pointer to a primitive (`None` = nil).
    Ptr(Option<Box<GoVal>>),
    /// `None` = nil slice.
    Slice(Option<Vec<GoVal>>),
    /// Field values in declaration order (embedded structs are nested `Struct`s).
    Struct(Vec<GoVal>),
    Bool(bool),
}

impl Ty {
    /// Go `reflect.Type.String()`.
    pub fn go_name(&self) -> String {
        match self {
            Ty::Str(n) => (*n).to_string(),
            Ty::Int => "int".into(),
            Ty::Ptr(p) => prim_name(*p).into(),
            Ty::Slice(d) => d.name.into(),
            Ty::Struct(d) => format!("types.{}", d.name),
        }
    }
}

pub(crate) fn prim_name(p: Prim) -> &'static str {
    match p {
        Prim::Str => "string",
        Prim::Int => "int",
        Prim::Bool => "bool",
    }
}

/// The zero value of a type.
pub(crate) fn zero(ty: Ty) -> GoVal {
    match ty {
        Ty::Str(_) => GoVal::Str(String::new()),
        Ty::Int => GoVal::Int(0),
        Ty::Ptr(_) => GoVal::Ptr(None),
        Ty::Slice(_) => GoVal::Slice(None),
        Ty::Struct(d) => GoVal::Struct(d.fields.iter().map(|f| zero(f.ty)).collect()),
    }
}

/// Go `reflect.Value.IsZero` (a non-nil empty slice is not zero).
pub(crate) fn is_zero(v: &GoVal) -> bool {
    match v {
        GoVal::Str(s) => s.is_empty(),
        GoVal::Int(i) => *i == 0,
        GoVal::Bool(b) => !*b,
        GoVal::Ptr(p) => p.is_none(),
        GoVal::Slice(s) => s.is_none(),
        GoVal::Struct(fs) => fs.iter().all(is_zero),
    }
}

impl GoVal {
    pub fn as_str(&self) -> &str {
        match self {
            GoVal::Str(s) => s,
            _ => "",
        }
    }
    pub fn as_int(&self) -> i64 {
        match self {
            GoVal::Int(i) => *i,
            _ => 0,
        }
    }
    pub fn as_slice(&self) -> &[GoVal] {
        match self {
            GoVal::Slice(Some(v)) => v,
            _ => &[],
        }
    }
    pub fn fields(&self) -> &[GoVal] {
        match self {
            GoVal::Struct(f) => f,
            _ => &[],
        }
    }
    /// `*string` value.
    pub fn ptr_str(&self) -> Option<&str> {
        match self {
            GoVal::Ptr(Some(b)) => Some(b.as_str()),
            _ => None,
        }
    }
    /// `*int` value.
    pub fn ptr_int(&self) -> Option<i64> {
        match self {
            GoVal::Ptr(Some(b)) => Some(b.as_int()),
            _ => None,
        }
    }
    /// `*bool` value.
    pub fn ptr_bool(&self) -> Option<bool> {
        match self {
            GoVal::Ptr(Some(b)) => match **b {
                GoVal::Bool(x) => Some(x),
                _ => None,
            },
            _ => None,
        }
    }
}

/// A struct value with its definition, giving access to fields by Go name (including fields
/// promoted from embedded structs).
#[derive(Clone, Copy)]
pub(crate) struct SRef<'a> {
    pub def: &'static StructDef,
    pub vals: &'a [GoVal],
}

impl<'a> SRef<'a> {
    pub fn new(def: &'static StructDef, v: &'a GoVal) -> SRef<'a> {
        SRef {
            def,
            vals: v.fields(),
        }
    }

    fn lookup(&self, name: &str) -> Option<(Ty, &'a GoVal)> {
        for (f, v) in self.def.fields.iter().zip(self.vals) {
            if f.name == name {
                return Some((f.ty, v));
            }
        }
        for (f, v) in self.def.fields.iter().zip(self.vals) {
            if f.embedded
                && let Ty::Struct(d) = f.ty
                && let Some(r) = SRef::new(d, v).lookup(name)
            {
                return Some(r);
            }
        }
        None
    }

    /// The field value (panics on schema programming errors only).
    pub fn get(&self, name: &str) -> &'a GoVal {
        match self.lookup(name) {
            Some((_, v)) => v,
            None => panic!("no field {name} in {}", self.def.name),
        }
    }

    /// A struct-typed field.
    pub fn sub(&self, name: &str) -> SRef<'a> {
        match self.lookup(name) {
            Some((Ty::Struct(d), v)) => SRef::new(d, v),
            _ => panic!("no struct field {name} in {}", self.def.name),
        }
    }

    /// A slice-of-struct field's elements.
    pub fn structs(&self, name: &str) -> Vec<SRef<'a>> {
        match self.lookup(name) {
            Some((
                Ty::Slice(SliceDef {
                    elem: Ty::Struct(d),
                    ..
                }),
                v,
            )) => v.as_slice().iter().map(|e| SRef::new(d, e)).collect(),
            _ => panic!("no struct slice field {name} in {}", self.def.name),
        }
    }

    pub fn str(&self, name: &str) -> &'a str {
        self.get(name).as_str()
    }
    pub fn int(&self, name: &str) -> i64 {
        self.get(name).as_int()
    }
    pub fn ptr_str(&self, name: &str) -> Option<&'a str> {
        self.get(name).ptr_str()
    }
    pub fn ptr_int(&self, name: &str) -> Option<i64> {
        self.get(name).ptr_int()
    }
    pub fn ptr_bool(&self, name: &str) -> Option<bool> {
        self.get(name).ptr_bool()
    }
    pub fn slice(&self, name: &str) -> &'a [GoVal] {
        self.get(name).as_slice()
    }
}

/// Go `util.CallKey`: strings are their own key, structs use `Key()`.
pub(crate) fn call_key(ty: Ty, v: &GoVal) -> String {
    match (ty, v) {
        (Ty::Str(_), GoVal::Str(s)) => s.clone(),
        (Ty::Struct(d), v) => match d.key {
            Some(k) => k(&SRef::new(d, v)),
            None => panic!("type {} has no Key()", d.name),
        },
        _ => panic!("value has no key"),
    }
}

// -------------------------------------------------------------------------------------------
// Schema
// -------------------------------------------------------------------------------------------

macro_rules! fld {
    ($name:literal, $json:literal, $omit:expr, $ty:expr) => {
        Field {
            name: $name,
            json: $json,
            omitempty: $omit,
            embedded: false,
            ty: $ty,
        }
    };
}
macro_rules! emb {
    ($name:literal, $def:expr) => {
        Field {
            name: $name,
            json: "",
            omitempty: false,
            embedded: true,
            ty: Ty::Struct($def),
        }
    };
}
macro_rules! sdef {
    ($name:literal, [$($f:expr),* $(,)?] $(, key = $key:expr)? $(, ignore = [$($ig:literal),*])? $(, merged = [$(($mk:literal, $mv:literal)),*])? $(, validate = $val:expr)?) => {
        StructDef {
            name: $name,
            fields: &[$($f),*],
            key: sdef!(@opt $($key)?),
            ignore_dups: &[$($($ig),*)?],
            merged_keys: &[$($(($mk, $mv)),*)?],
            validate: sdef!(@optv $($val)?),
        }
    };
    (@opt) => { None };
    (@opt $k:expr) => { Some($k as KeyFn) };
    (@optv) => { None };
    (@optv $k:expr) => { Some($k as ValidateFn) };
}

const S: Ty = Ty::Str("string");
const PS: Ty = Ty::Ptr(Prim::Str);
const PI: Ty = Ty::Ptr(Prim::Int);
const PB: Ty = Ty::Ptr(Prim::Bool);

const fn slice(def: &'static SliceDef) -> Ty {
    Ty::Slice(def)
}

use crate::validate::rules as v;

pub(crate) static CLEVIS: StructDef = sdef!(
    "Clevis",
    [
        fld!("Custom", "custom", true, Ty::Struct(&CLEVIS_CUSTOM)),
        fld!(
            "Tang",
            "tang",
            true,
            slice(&SliceDef {
                name: "[]types.Tang",
                elem: Ty::Struct(&TANG)
            })
        ),
        fld!("Threshold", "threshold", true, PI),
        fld!("Tpm2", "tpm2", true, PB),
    ]
);
pub(crate) static CLEVIS_CUSTOM: StructDef = sdef!(
    "ClevisCustom",
    [
        fld!("Config", "config", true, PS),
        fld!("NeedsNetwork", "needsNetwork", true, PB),
        fld!("Pin", "pin", true, PS),
    ],
    validate = v::clevis_custom
);
pub(crate) static CONFIG: StructDef = sdef!(
    "Config",
    [
        fld!("Ignition", "ignition", false, Ty::Struct(&IGNITION)),
        fld!(
            "KernelArguments",
            "kernelArguments",
            true,
            Ty::Struct(&KERNEL_ARGUMENTS)
        ),
        fld!("Passwd", "passwd", true, Ty::Struct(&PASSWD)),
        fld!("Storage", "storage", true, Ty::Struct(&STORAGE)),
        fld!("Systemd", "systemd", true, Ty::Struct(&SYSTEMD)),
    ],
    validate = v::config
);
pub(crate) static DIRECTORY: StructDef = sdef!(
    "Directory",
    [
        emb!("Node", &NODE),
        emb!("DirectoryEmbedded1", &DIRECTORY_EMBEDDED1),
    ],
    key = node_key,
    validate = v::directory
);
pub(crate) static DIRECTORY_EMBEDDED1: StructDef =
    sdef!("DirectoryEmbedded1", [fld!("Mode", "mode", true, PI),]);
pub(crate) static DISK: StructDef = sdef!(
    "Disk",
    [
        fld!("Device", "device", false, S),
        fld!(
            "Partitions",
            "partitions",
            true,
            slice(&SliceDef {
                name: "[]types.Partition",
                elem: Ty::Struct(&PARTITION)
            })
        ),
        fld!("WipeTable", "wipeTable", true, PB),
    ],
    key = |s| s.str("Device").to_string(),
    validate = v::disk
);
pub(crate) static DROPIN: StructDef = sdef!(
    "Dropin",
    [
        fld!("Contents", "contents", true, PS),
        fld!("Name", "name", false, S),
    ],
    key = |s| s.str("Name").to_string(),
    validate = v::dropin
);
pub(crate) static FILE: StructDef = sdef!(
    "File",
    [emb!("Node", &NODE), emb!("FileEmbedded1", &FILE_EMBEDDED1),],
    key = node_key,
    ignore = ["Append"],
    validate = v::file
);
pub(crate) static FILE_EMBEDDED1: StructDef = sdef!(
    "FileEmbedded1",
    [
        fld!(
            "Append",
            "append",
            true,
            slice(&SliceDef {
                name: "[]types.Resource",
                elem: Ty::Struct(&RESOURCE)
            })
        ),
        fld!("Contents", "contents", true, Ty::Struct(&RESOURCE)),
        fld!("Mode", "mode", true, PI),
    ],
    ignore = ["Append"]
);
pub(crate) static FILESYSTEM: StructDef = sdef!(
    "Filesystem",
    [
        fld!("Device", "device", false, S),
        fld!("Format", "format", true, PS),
        fld!("Label", "label", true, PS),
        fld!(
            "MountOptions",
            "mountOptions",
            true,
            slice(&SliceDef {
                name: "[]types.MountOption",
                elem: Ty::Str("types.MountOption")
            })
        ),
        fld!(
            "Options",
            "options",
            true,
            slice(&SliceDef {
                name: "[]types.FilesystemOption",
                elem: Ty::Str("types.FilesystemOption")
            })
        ),
        fld!("Path", "path", true, PS),
        fld!("UUID", "uuid", true, PS),
        fld!("WipeFilesystem", "wipeFilesystem", true, PB),
    ],
    key = |s| s.str("Device").to_string(),
    ignore = ["Options", "MountOptions"],
    validate = v::filesystem
);
pub(crate) static HTTP_HEADER: StructDef = sdef!(
    "HTTPHeader",
    [
        fld!("Name", "name", false, S),
        fld!("Value", "value", true, PS),
    ],
    key = |s| s.str("Name").to_string(),
    validate = v::http_header
);
pub(crate) static IGNITION: StructDef = sdef!(
    "Ignition",
    [
        fld!("Config", "config", true, Ty::Struct(&IGNITION_CONFIG)),
        fld!("Proxy", "proxy", true, Ty::Struct(&PROXY)),
        fld!("Security", "security", true, Ty::Struct(&SECURITY)),
        fld!("Timeouts", "timeouts", true, Ty::Struct(&TIMEOUTS)),
        fld!("Version", "version", false, S),
    ],
    validate = v::ignition
);
pub(crate) static IGNITION_CONFIG: StructDef = sdef!(
    "IgnitionConfig",
    [
        fld!(
            "Merge",
            "merge",
            true,
            slice(&SliceDef {
                name: "[]types.Resource",
                elem: Ty::Struct(&RESOURCE)
            })
        ),
        fld!("Replace", "replace", true, Ty::Struct(&RESOURCE)),
    ],
    validate = v::ignition_config
);
pub(crate) static KERNEL_ARGUMENTS: StructDef = sdef!(
    "KernelArguments",
    [
        fld!(
            "ShouldExist",
            "shouldExist",
            true,
            slice(&SliceDef {
                name: "[]types.KernelArgument",
                elem: Ty::Str("types.KernelArgument")
            })
        ),
        fld!(
            "ShouldNotExist",
            "shouldNotExist",
            true,
            slice(&SliceDef {
                name: "[]types.KernelArgument",
                elem: Ty::Str("types.KernelArgument")
            })
        ),
    ],
    merged = [
        ("ShouldExist", "KernelArgument"),
        ("ShouldNotExist", "KernelArgument")
    ]
);
pub(crate) static LINK: StructDef = sdef!(
    "Link",
    [emb!("Node", &NODE), emb!("LinkEmbedded1", &LINK_EMBEDDED1),],
    key = node_key,
    validate = v::node
);
pub(crate) static LINK_EMBEDDED1: StructDef = sdef!(
    "LinkEmbedded1",
    [
        fld!("Hard", "hard", true, PB),
        fld!("Target", "target", true, PS),
    ]
);
pub(crate) static LUKS: StructDef = sdef!(
    "Luks",
    [
        fld!("Clevis", "clevis", true, Ty::Struct(&CLEVIS)),
        fld!("Device", "device", true, PS),
        fld!("Discard", "discard", true, PB),
        fld!("KeyFile", "keyFile", true, Ty::Struct(&RESOURCE)),
        fld!("Label", "label", true, PS),
        fld!("Name", "name", false, S),
        fld!(
            "OpenOptions",
            "openOptions",
            true,
            slice(&SliceDef {
                name: "[]types.OpenOption",
                elem: Ty::Str("types.OpenOption")
            })
        ),
        fld!(
            "Options",
            "options",
            true,
            slice(&SliceDef {
                name: "[]types.LuksOption",
                elem: Ty::Str("types.LuksOption")
            })
        ),
        fld!("UUID", "uuid", true, PS),
        fld!("WipeVolume", "wipeVolume", true, PB),
    ],
    key = |s| s.str("Name").to_string(),
    ignore = ["Options"],
    validate = v::luks
);
pub(crate) static NODE: StructDef = sdef!(
    "Node",
    [
        fld!("Group", "group", true, Ty::Struct(&NODE_GROUP)),
        fld!("Overwrite", "overwrite", true, PB),
        fld!("Path", "path", false, S),
        fld!("User", "user", true, Ty::Struct(&NODE_USER)),
    ],
    key = node_key,
    validate = v::node
);
pub(crate) static NODE_GROUP: StructDef = sdef!(
    "NodeGroup",
    [fld!("ID", "id", true, PI), fld!("Name", "name", true, PS),],
    validate = v::node_owner
);
pub(crate) static NODE_USER: StructDef = sdef!(
    "NodeUser",
    [fld!("ID", "id", true, PI), fld!("Name", "name", true, PS),],
    validate = v::node_owner
);
pub(crate) static PARTITION: StructDef = sdef!(
    "Partition",
    [
        fld!("GUID", "guid", true, PS),
        fld!("Label", "label", true, PS),
        fld!("Number", "number", true, Ty::Int),
        fld!("Resize", "resize", true, PB),
        fld!("ShouldExist", "shouldExist", true, PB),
        fld!("SizeMiB", "sizeMiB", true, PI),
        fld!("StartMiB", "startMiB", true, PI),
        fld!("TypeGUID", "typeGuid", true, PS),
        fld!("WipePartitionEntry", "wipePartitionEntry", true, PB),
    ],
    key = v::partition_key,
    validate = v::partition
);
pub(crate) static PASSWD: StructDef = sdef!(
    "Passwd",
    [
        fld!(
            "Groups",
            "groups",
            true,
            slice(&SliceDef {
                name: "[]types.PasswdGroup",
                elem: Ty::Struct(&PASSWD_GROUP)
            })
        ),
        fld!(
            "Users",
            "users",
            true,
            slice(&SliceDef {
                name: "[]types.PasswdUser",
                elem: Ty::Struct(&PASSWD_USER)
            })
        ),
    ]
);
pub(crate) static PASSWD_GROUP: StructDef = sdef!(
    "PasswdGroup",
    [
        fld!("Gid", "gid", true, PI),
        fld!("Name", "name", false, S),
        fld!("PasswordHash", "passwordHash", true, PS),
        fld!("ShouldExist", "shouldExist", true, PB),
        fld!("System", "system", true, PB),
    ],
    key = |s| s.str("Name").to_string()
);
pub(crate) static PASSWD_USER: StructDef = sdef!(
    "PasswdUser",
    [
        fld!("Gecos", "gecos", true, PS),
        fld!(
            "Groups",
            "groups",
            true,
            slice(&SliceDef {
                name: "[]types.Group",
                elem: Ty::Str("types.Group")
            })
        ),
        fld!("HomeDir", "homeDir", true, PS),
        fld!("Name", "name", false, S),
        fld!("NoCreateHome", "noCreateHome", true, PB),
        fld!("NoLogInit", "noLogInit", true, PB),
        fld!("NoUserGroup", "noUserGroup", true, PB),
        fld!("PasswordHash", "passwordHash", true, PS),
        fld!("PrimaryGroup", "primaryGroup", true, PS),
        fld!(
            "SSHAuthorizedKeys",
            "sshAuthorizedKeys",
            true,
            slice(&SliceDef {
                name: "[]types.SSHAuthorizedKey",
                elem: Ty::Str("types.SSHAuthorizedKey")
            })
        ),
        fld!("Shell", "shell", true, PS),
        fld!("ShouldExist", "shouldExist", true, PB),
        fld!("System", "system", true, PB),
        fld!("UID", "uid", true, PI),
    ],
    key = |s| s.str("Name").to_string()
);
pub(crate) static PROXY: StructDef = sdef!(
    "Proxy",
    [
        fld!("HTTPProxy", "httpProxy", true, PS),
        fld!("HTTPSProxy", "httpsProxy", true, PS),
        fld!(
            "NoProxy",
            "noProxy",
            true,
            slice(&SliceDef {
                name: "[]types.NoProxyItem",
                elem: Ty::Str("types.NoProxyItem")
            })
        ),
    ],
    validate = v::proxy
);
pub(crate) static RAID: StructDef = sdef!(
    "Raid",
    [
        fld!(
            "Devices",
            "devices",
            true,
            slice(&SliceDef {
                name: "[]types.Device",
                elem: Ty::Str("types.Device")
            })
        ),
        fld!("Level", "level", true, PS),
        fld!("Name", "name", false, S),
        fld!(
            "Options",
            "options",
            true,
            slice(&SliceDef {
                name: "[]types.RaidOption",
                elem: Ty::Str("types.RaidOption")
            })
        ),
        fld!("Spares", "spares", true, PI),
    ],
    key = |s| s.str("Name").to_string(),
    ignore = ["Options"],
    validate = v::raid
);
pub(crate) static RESOURCE: StructDef = sdef!(
    "Resource",
    [
        fld!("Compression", "compression", true, PS),
        fld!(
            "HTTPHeaders",
            "httpHeaders",
            true,
            slice(&SliceDef {
                name: "types.HTTPHeaders",
                elem: Ty::Struct(&HTTP_HEADER)
            })
        ),
        fld!("Source", "source", true, PS),
        fld!(
            "Verification",
            "verification",
            true,
            Ty::Struct(&VERIFICATION)
        ),
    ],
    key = |s| s.ptr_str("Source").unwrap_or("").to_string(),
    validate = v::resource
);
pub(crate) static SECURITY: StructDef =
    sdef!("Security", [fld!("TLS", "tls", true, Ty::Struct(&TLS)),]);
pub(crate) static STORAGE: StructDef = sdef!(
    "Storage",
    [
        fld!(
            "Directories",
            "directories",
            true,
            slice(&SliceDef {
                name: "[]types.Directory",
                elem: Ty::Struct(&DIRECTORY)
            })
        ),
        fld!(
            "Disks",
            "disks",
            true,
            slice(&SliceDef {
                name: "[]types.Disk",
                elem: Ty::Struct(&DISK)
            })
        ),
        fld!(
            "Files",
            "files",
            true,
            slice(&SliceDef {
                name: "[]types.File",
                elem: Ty::Struct(&FILE)
            })
        ),
        fld!(
            "Filesystems",
            "filesystems",
            true,
            slice(&SliceDef {
                name: "[]types.Filesystem",
                elem: Ty::Struct(&FILESYSTEM)
            })
        ),
        fld!(
            "Links",
            "links",
            true,
            slice(&SliceDef {
                name: "[]types.Link",
                elem: Ty::Struct(&LINK)
            })
        ),
        fld!(
            "Luks",
            "luks",
            true,
            slice(&SliceDef {
                name: "[]types.Luks",
                elem: Ty::Struct(&LUKS)
            })
        ),
        fld!(
            "Raid",
            "raid",
            true,
            slice(&SliceDef {
                name: "[]types.Raid",
                elem: Ty::Struct(&RAID)
            })
        ),
    ],
    merged = [
        ("Directories", "Node"),
        ("Files", "Node"),
        ("Links", "Node")
    ],
    validate = v::storage
);
pub(crate) static SYSTEMD: StructDef = sdef!(
    "Systemd",
    [fld!(
        "Units",
        "units",
        true,
        slice(&SliceDef {
            name: "[]types.Unit",
            elem: Ty::Struct(&UNIT)
        })
    ),],
    validate = v::systemd
);
pub(crate) static TLS: StructDef = sdef!(
    "TLS",
    [fld!(
        "CertificateAuthorities",
        "certificateAuthorities",
        true,
        slice(&SliceDef {
            name: "[]types.Resource",
            elem: Ty::Struct(&RESOURCE)
        })
    ),],
    validate = v::tls
);
pub(crate) static TANG: StructDef = sdef!(
    "Tang",
    [
        fld!("Advertisement", "advertisement", true, PS),
        fld!("Thumbprint", "thumbprint", true, PS),
        fld!("URL", "url", true, S),
    ],
    key = |s| s.str("URL").to_string(),
    validate = v::tang
);
pub(crate) static TIMEOUTS: StructDef = sdef!(
    "Timeouts",
    [
        fld!("HTTPResponseHeaders", "httpResponseHeaders", true, PI),
        fld!("HTTPTotal", "httpTotal", true, PI),
    ]
);
pub(crate) static UNIT: StructDef = sdef!(
    "Unit",
    [
        fld!("Contents", "contents", true, PS),
        fld!(
            "Dropins",
            "dropins",
            true,
            slice(&SliceDef {
                name: "[]types.Dropin",
                elem: Ty::Struct(&DROPIN)
            })
        ),
        fld!("Enabled", "enabled", true, PB),
        fld!("Mask", "mask", true, PB),
        fld!("Name", "name", false, S),
    ],
    key = |s| s.str("Name").to_string(),
    validate = v::unit
);
pub(crate) static VERIFICATION: StructDef = sdef!(
    "Verification",
    [fld!("Hash", "hash", true, PS),],
    validate = v::verification
);

fn node_key(s: &SRef<'_>) -> String {
    s.str("Path").to_string()
}

/// The root type.
pub(crate) const CONFIG_TY: Ty = Ty::Struct(&CONFIG);

// -------------------------------------------------------------------------------------------
// Conversion to/from the ignition-config crate types
// -------------------------------------------------------------------------------------------

/// Converts a crate value (serialized with serde: keys are the Go JSON names) to a Go value.
/// `None`/missing/`null` give the Go zero value.
fn from_json(ty: Ty, v: Option<&Value>) -> GoVal {
    let v = match v {
        None | Some(Value::Null) => return zero(ty),
        Some(v) => v,
    };
    match ty {
        Ty::Str(_) => GoVal::Str(v.as_str().unwrap_or_default().to_string()),
        Ty::Int => GoVal::Int(v.as_i64().unwrap_or_default()),
        Ty::Ptr(p) => GoVal::Ptr(Some(Box::new(match p {
            Prim::Str => GoVal::Str(v.as_str().unwrap_or_default().to_string()),
            Prim::Int => GoVal::Int(v.as_i64().unwrap_or_default()),
            Prim::Bool => GoVal::Bool(v.as_bool().unwrap_or_default()),
        }))),
        Ty::Slice(d) => GoVal::Slice(Some(
            v.as_array()
                .map(|a| a.iter().map(|e| from_json(d.elem, Some(e))).collect())
                .unwrap_or_default(),
        )),
        Ty::Struct(d) => from_json_struct(d, v.as_object()),
    }
}

fn from_json_struct(d: &'static StructDef, obj: Option<&Map<String, Value>>) -> GoVal {
    GoVal::Struct(
        d.fields
            .iter()
            .map(|f| {
                if f.embedded {
                    let Ty::Struct(ed) = f.ty else { unreachable!() };
                    from_json_struct(ed, obj)
                } else {
                    from_json(f.ty, obj.and_then(|o| o.get(f.json)))
                }
            })
            .collect(),
    )
}

/// Converts a Go value to the serde JSON form of the crate types. Zero values of
/// `omitempty` fields are left out, so Go zero structs become `None`.
fn to_json(ty: Ty, v: &GoVal) -> Value {
    match (ty, v) {
        (_, GoVal::Str(s)) => Value::String(s.clone()),
        (_, GoVal::Int(i)) => Value::from(*i),
        (_, GoVal::Bool(b)) => Value::Bool(*b),
        (Ty::Ptr(p), GoVal::Ptr(Some(b))) => to_json(
            match p {
                Prim::Str => Ty::Str("string"),
                _ => Ty::Int,
            },
            b,
        ),
        (_, GoVal::Ptr(None)) | (_, GoVal::Slice(None)) => Value::Null,
        (Ty::Slice(d), GoVal::Slice(Some(items))) => {
            Value::Array(items.iter().map(|e| to_json(d.elem, e)).collect())
        }
        (Ty::Struct(d), GoVal::Struct(_)) => {
            let mut m = Map::new();
            to_json_struct(d, v, &mut m);
            Value::Object(m)
        }
        _ => Value::Null,
    }
}

fn to_json_struct(d: &'static StructDef, v: &GoVal, m: &mut Map<String, Value>) {
    for (f, fv) in d.fields.iter().zip(v.fields()) {
        if f.embedded {
            let Ty::Struct(ed) = f.ty else { unreachable!() };
            to_json_struct(ed, fv, m);
            continue;
        }
        let emit = match fv {
            GoVal::Ptr(p) => p.is_some(),
            GoVal::Slice(s) => s.is_some(),
            other => !f.omitempty || !is_zero(other),
        };
        if emit {
            m.insert(f.json.to_string(), to_json(f.ty, fv));
        }
    }
}

/// Crate `Config` → Go value.
pub(crate) fn from_config(cfg: &Config) -> GoVal {
    let v = serde_json::to_value(cfg).unwrap_or(Value::Null);
    from_json(CONFIG_TY, Some(&v))
}

/// Go value → crate `Config`.
pub(crate) fn to_config(v: &GoVal) -> Config {
    let j = to_json(CONFIG_TY, v);
    match serde_json::from_value(j) {
        Ok(c) => c,
        // The JSON produced by `to_json` always matches the crate schema.
        Err(e) => panic!("internal error converting ignition config: {e}"),
    }
}

// -------------------------------------------------------------------------------------------
// encoding/json Marshal
// -------------------------------------------------------------------------------------------

/// Go `json.Marshal` of a typed value.
pub(crate) fn marshal(ty: Ty, v: &GoVal) -> Vec<u8> {
    let mut out = Vec::new();
    marshal_into(ty, v, &mut out);
    out
}

fn marshal_into(ty: Ty, v: &GoVal, out: &mut Vec<u8>) {
    match v {
        GoVal::Str(s) => gocompat::json::append_string(out, s.as_bytes(), true),
        GoVal::Int(i) => out.extend_from_slice(i.to_string().as_bytes()),
        GoVal::Bool(b) => out.extend_from_slice(if *b { b"true" } else { b"false" }),
        GoVal::Ptr(None) | GoVal::Slice(None) => out.extend_from_slice(b"null"),
        GoVal::Ptr(Some(b)) => marshal_into(ty, b, out),
        GoVal::Slice(Some(items)) => {
            let Ty::Slice(d) = ty else { unreachable!() };
            out.push(b'[');
            for (i, e) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                marshal_into(d.elem, e, out);
            }
            out.push(b']');
        }
        GoVal::Struct(_) => {
            let Ty::Struct(d) = ty else { unreachable!() };
            out.push(b'{');
            let mut first = true;
            marshal_fields(d, v, out, &mut first);
            out.push(b'}');
        }
    }
}

fn marshal_fields(d: &'static StructDef, v: &GoVal, out: &mut Vec<u8>, first: &mut bool) {
    for (f, fv) in d.fields.iter().zip(v.fields()) {
        if f.embedded {
            let Ty::Struct(ed) = f.ty else { unreachable!() };
            marshal_fields(ed, fv, out, first);
            continue;
        }
        if f.omitempty {
            let empty = match fv {
                GoVal::Str(s) => s.is_empty(),
                GoVal::Int(i) => *i == 0,
                GoVal::Bool(b) => !*b,
                GoVal::Ptr(p) => p.is_none(),
                GoVal::Slice(s) => s.as_ref().is_none_or(|s| s.is_empty()),
                // omitempty never omits structs
                GoVal::Struct(_) => false,
            };
            if empty {
                continue;
            }
        }
        if !*first {
            out.push(b',');
        }
        *first = false;
        gocompat::json::append_string(out, f.json.as_bytes(), true);
        out.push(b':');
        marshal_into(f.ty, fv, out);
    }
}

// -------------------------------------------------------------------------------------------
// encoding/json Unmarshal
// -------------------------------------------------------------------------------------------

/// Go `json.Unmarshal(data, &cfg)` with `var cfg types.Config` (errors carry Go's messages).
pub(crate) fn unmarshal(data: &[u8]) -> Result<GoVal, gocompat::json::Error> {
    gocompat::json::check_valid(data)?;
    let mut d = Decoder {
        d: data,
        i: 0,
        err: None,
        ctx: None,
    };
    let mut v = zero(CONFIG_TY);
    d.ws();
    d.value(Some(CONFIG_TY), Some(&mut v));
    match d.err {
        Some(e) => Err(gocompat::json::Error::UnmarshalType(e)),
        None => Ok(v),
    }
}

struct ErrCtx {
    struct_name: Option<&'static str>,
    stack: Vec<String>,
}

struct Decoder<'a> {
    d: &'a [u8],
    i: usize,
    err: Option<gocompat::json::UnmarshalTypeError>,
    ctx: Option<ErrCtx>,
}

/// Go `foldName` for a key compared against ASCII field names: ASCII letters are
/// upper-cased; the only non-ASCII runes folding to ASCII are U+017F (ſ → S) and
/// U+212A (Kelvin → K); other runes are kept (they can never match an ASCII name).
fn fold_name(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' => c.to_ascii_uppercase(),
            '\u{17f}' => 'S',
            '\u{212a}' => 'K',
            _ => c,
        })
        .collect()
}

/// Finds a JSON key among the (flattened) fields of `d`: exact match first, then
/// case-insensitive. Returns the embedded-field path (Go names) and the field index path.
fn find_field(
    d: &'static StructDef,
    key: &str,
) -> Option<(Vec<&'static str>, Vec<usize>, &'static Field)> {
    fn walk(
        d: &'static StructDef,
        pred: &dyn Fn(&str) -> bool,
        names: &mut Vec<&'static str>,
        idx: &mut Vec<usize>,
    ) -> Option<&'static Field> {
        // Go's typeFields orders by depth: direct fields first, then embedded ones.
        for (i, f) in d.fields.iter().enumerate() {
            if !f.embedded && pred(f.json) {
                idx.push(i);
                return Some(f);
            }
        }
        for (i, f) in d.fields.iter().enumerate() {
            if f.embedded {
                let Ty::Struct(ed) = f.ty else { unreachable!() };
                names.push(f.name);
                idx.push(i);
                if let Some(r) = walk(ed, pred, names, idx) {
                    return Some(r);
                }
                names.pop();
                idx.pop();
            }
        }
        None
    }
    let mut names = Vec::new();
    let mut idx = Vec::new();
    if let Some(f) = walk(d, &|n| n == key, &mut names, &mut idx) {
        return Some((names, idx, f));
    }
    let folded = fold_name(key);
    let mut names = Vec::new();
    let mut idx = Vec::new();
    walk(d, &|n| fold_name(n) == folded, &mut names, &mut idx).map(|f| (names, idx, f))
}

impl Decoder<'_> {
    fn ws(&mut self) {
        while self.i < self.d.len() && matches!(self.d[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn save_error(&mut self, value: String, ty: Ty) {
        if self.err.is_some() {
            return;
        }
        let mut e = gocompat::json::UnmarshalTypeError {
            value,
            type_name: ty.go_name(),
            offset: self.i as i64,
            struct_name: String::new(),
            field: String::new(),
        };
        if let Some(c) = &self.ctx
            && (c.struct_name.is_some() || !c.stack.is_empty())
        {
            e.struct_name = c.struct_name.unwrap_or_default().to_string();
            e.field = c.stack.join(".");
        }
        self.err = Some(e);
    }

    /// Scans a literal starting at `self.i` and returns its bytes.
    fn literal(&mut self) -> &[u8] {
        let start = self.i;
        let d = self.d;
        match d[self.i] {
            b'"' => {
                self.i += 1;
                while self.i < d.len() {
                    match d[self.i] {
                        b'\\' => self.i += 2,
                        b'"' => {
                            self.i += 1;
                            break;
                        }
                        _ => self.i += 1,
                    }
                }
            }
            b't' | b'n' => self.i += 4,
            b'f' => self.i += 5,
            _ => {
                while self.i < d.len()
                    && matches!(d[self.i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-')
                {
                    self.i += 1;
                }
            }
        }
        &d[start..self.i]
    }

    fn skip(&mut self) {
        self.value(None, None);
    }

    /// Decodes the value at `self.i` into `target` (of type `ty`); `None` skips it.
    fn value(&mut self, ty: Option<Ty>, target: Option<&mut GoVal>) {
        match self.d[self.i] {
            b'{' => self.object(ty, target),
            b'[' => self.array(ty, target),
            _ => {
                let lit = self.literal().to_vec();
                if let (Some(ty), Some(t)) = (ty, target) {
                    self.literal_store(&lit, ty, t);
                }
            }
        }
    }

    fn literal_store(&mut self, lit: &[u8], ty: Ty, t: &mut GoVal) {
        match lit[0] {
            b'n' => match t {
                GoVal::Ptr(p) => *p = None,
                GoVal::Slice(s) => *s = None,
                _ => {}
            },
            b't' | b'f' => {
                let b = lit[0] == b't';
                match ty {
                    Ty::Ptr(Prim::Bool) => *t = GoVal::Ptr(Some(Box::new(GoVal::Bool(b)))),
                    _ => self.save_error("bool".into(), ty),
                }
            }
            b'"' => {
                let s = gocompat::json::unquote(lit).unwrap_or_default();
                match ty {
                    Ty::Str(_) => *t = GoVal::Str(s),
                    Ty::Ptr(Prim::Str) => *t = GoVal::Ptr(Some(Box::new(GoVal::Str(s)))),
                    _ => self.save_error("string".into(), ty),
                }
            }
            _ => {
                let lit_s = String::from_utf8_lossy(lit).into_owned();
                match ty {
                    Ty::Int | Ty::Ptr(Prim::Int) => match gocompat::strconv::parse_int(lit, 10, 64)
                    {
                        Ok(n) => {
                            *t = if matches!(ty, Ty::Int) {
                                GoVal::Int(n)
                            } else {
                                GoVal::Ptr(Some(Box::new(GoVal::Int(n))))
                            }
                        }
                        Err(_) => self.save_error(format!("number {lit_s}"), ty),
                    },
                    _ => self.save_error("number".into(), ty),
                }
            }
        }
    }

    fn array(&mut self, ty: Option<Ty>, target: Option<&mut GoVal>) {
        let (def, items) = match (ty, target) {
            (Some(Ty::Slice(d)), Some(GoVal::Slice(s))) => (Some(d), Some(s)),
            (Some(ty), Some(_)) => {
                self.save_error("array".into(), ty);
                (None, None)
            }
            _ => (None, None),
        };
        self.i += 1; // [
        let mut existing = items
            .as_ref()
            .map(|s| (*s).clone().unwrap_or_default())
            .unwrap_or_default();
        let mut n = 0;
        loop {
            self.ws();
            if self.d[self.i] == b']' {
                self.i += 1;
                break;
            }
            match def {
                Some(d) => {
                    if n >= existing.len() {
                        existing.push(zero(d.elem));
                    }
                    let mut el = std::mem::replace(&mut existing[n], GoVal::Int(0));
                    self.value(Some(d.elem), Some(&mut el));
                    existing[n] = el;
                }
                None => self.skip(),
            }
            n += 1;
            self.ws();
            if self.d[self.i] == b',' {
                self.i += 1;
            }
        }
        if let Some(s) = items {
            existing.truncate(n);
            *s = Some(existing);
        }
    }

    fn object(&mut self, ty: Option<Ty>, target: Option<&mut GoVal>) {
        let (def, target) = match (ty, target) {
            (Some(Ty::Struct(d)), Some(t)) => (Some(d), Some(t)),
            (Some(Ty::Ptr(p)), Some(_)) => {
                self.save_error("object".into(), Ty::Ptr(p));
                (None, None)
            }
            (Some(ty), Some(_)) => {
                self.save_error("object".into(), ty);
                (None, None)
            }
            _ => (None, None),
        };
        self.i += 1; // {
        let mut target = target;
        loop {
            self.ws();
            if self.d[self.i] == b'}' {
                self.i += 1;
                break;
            }
            let key_lit = self.literal().to_vec();
            let key = gocompat::json::unquote(&key_lit).unwrap_or_default();
            self.ws();
            self.i += 1; // :
            self.ws();
            let found = def.and_then(|d| find_field(d, &key).map(|r| (d, r)));
            match (found, target.as_deref_mut()) {
                (Some((d, (names, idx, f))), Some(t)) => {
                    let ctx = self.ctx.get_or_insert(ErrCtx {
                        struct_name: None,
                        stack: Vec::new(),
                    });
                    let orig_len = ctx.stack.len();
                    let orig_struct = ctx.struct_name;
                    ctx.stack.extend(names.iter().map(|s| s.to_string()));
                    ctx.struct_name = Some(d.name);
                    ctx.stack.push(f.json.to_string());
                    let mut slot = t;
                    for &i in &idx {
                        let GoVal::Struct(fs) = slot else {
                            unreachable!()
                        };
                        slot = &mut fs[i];
                    }
                    self.value(Some(f.ty), Some(slot));
                    if let Some(ctx) = self.ctx.as_mut() {
                        ctx.stack.truncate(orig_len);
                        ctx.struct_name = orig_struct;
                    }
                }
                _ => self.skip(),
            }
            self.ws();
            if self.d[self.i] == b',' {
                self.i += 1;
            }
        }
    }
}
