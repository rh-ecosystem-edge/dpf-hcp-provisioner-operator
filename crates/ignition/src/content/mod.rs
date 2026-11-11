//! Port of `internal/ignition/content` (`types.go`, `loader.go`).

use std::borrow::Cow;

use anyhow::anyhow;
use ignition_config::v3_4::{Config, File, Link, Resource, Unit};

/// The content of a [`FileDefinition`] (Go `ContentSource interface{}`).
#[derive(Clone, Debug, PartialEq)]
pub enum ContentSource {
    /// Go `string`: a ready data URI, used as the file source as-is.
    DataUri(String),
    /// Go `[]byte`: raw content, gzip-compressed and base64-encoded into a data URI.
    Bytes(Cow<'static, [u8]>),
    /// Any other Go dynamic type (rejected with `invalid content source type`). The string is
    /// only a description of the value.
    Other(String),
}

/// `FileDefinition`: a file to be added to an ignition config.
#[derive(Clone, Debug, PartialEq)]
pub struct FileDefinition {
    pub path: String,
    pub mode: i64,
    pub content_source: ContentSource,
}

/// `LinkDefinition`: a symlink to be added to an ignition config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkDefinition {
    pub path: String,
    pub target: String,
}

/// `SystemdUnitDefinition`: a systemd unit to be added.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SystemdUnitDefinition {
    pub name: String,
    pub contents: Vec<u8>,
}

/// `ContentProvider`.
pub trait ContentProvider {
    fn get_files(&self) -> Vec<FileDefinition>;
    fn get_links(&self) -> Vec<LinkDefinition>;
    fn get_systemd_units(&self) -> anyhow::Result<Vec<SystemdUnitDefinition>>;
}

/// An embedded directory (Go `embed.FS` for one `//go:embed dir/*` pattern): the entries
/// sorted by name, as `fs.ReadDir` returns them.
#[derive(Debug)]
pub struct EmbeddedDir {
    pub entries: &'static [(&'static str, &'static [u8])],
}

impl EmbeddedDir {
    /// Reads a file by name.
    pub fn read_file(&self, name: &str) -> Option<&'static [u8]> {
        self.entries
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, d)| *d)
    }
}

/// `EmbeddedProvider`: a reusable [`ContentProvider`] backed by embedded files.
#[derive(Clone, Debug, Default)]
pub struct EmbeddedProvider {
    pub files: Vec<FileDefinition>,
    pub links: Vec<LinkDefinition>,
    /// `None` if this provider has no systemd units.
    pub systemd_fs: Option<&'static EmbeddedDir>,
    /// Unit names to exclude from systemd loading.
    pub skip_units: Vec<String>,
}

impl ContentProvider for EmbeddedProvider {
    fn get_files(&self) -> Vec<FileDefinition> {
        self.files.clone()
    }

    fn get_links(&self) -> Vec<LinkDefinition> {
        self.links.clone()
    }

    fn get_systemd_units(&self) -> anyhow::Result<Vec<SystemdUnitDefinition>> {
        let Some(fs) = self.systemd_fs else {
            return Ok(Vec::new());
        };
        let units = load_systemd_units(fs)?;
        if self.skip_units.is_empty() {
            return Ok(units);
        }
        Ok(units
            .into_iter()
            .filter(|u| !self.skip_units.contains(&u.name))
            .collect())
    }
}

/// `LoadSystemdUnits`: all unit files of an embedded directory, sorted by name.
pub fn load_systemd_units(fs: &EmbeddedDir) -> anyhow::Result<Vec<SystemdUnitDefinition>> {
    let mut entries: Vec<_> = fs.entries.to_vec();
    entries.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    Ok(entries
        .into_iter()
        .map(|(name, data)| SystemdUnitDefinition {
            name: name.to_string(),
            contents: data.to_vec(),
        })
        .collect())
}

/// `EmbedFile`: reads a file from an embedded directory. Panics if it does not exist, as
/// Go does (embedded content is fixed at build time).
pub fn embed_file(fs: &EmbeddedDir, name: &str) -> &'static [u8] {
    match fs.read_file(name) {
        Some(d) => d,
        None => panic!(
            "embedded file {:?} not found: open {name}: file does not exist",
            name
        ),
    }
}

fn files_mut(ign: &mut Config) -> &mut Vec<File> {
    ign.storage
        .get_or_insert_with(Default::default)
        .files
        .get_or_insert_with(Vec::new)
}

/// `AddFiles`: adds files from a content provider to an ignition config.
pub fn add_files(ign: &mut Config, provider: &dyn ContentProvider) -> anyhow::Result<()> {
    for def in provider.get_files() {
        let contents = match &def.content_source {
            ContentSource::DataUri(s) => Resource {
                source: Some(s.clone()),
                ..Default::default()
            },
            ContentSource::Bytes(b) => crate::encode_gzip_file(b)
                .map_err(|e| ctrlrt::wrapf!(e, "failed to encode content for {}: {}", def.path, e))?,
            ContentSource::Other(_) => {
                return Err(anyhow!("invalid content source type for {}", def.path));
            }
        };
        let mut f = File::new(def.path.clone());
        f.overwrite = Some(true);
        f.contents = Some(contents);
        f.mode = Some(def.mode);
        files_mut(ign).push(f);
    }
    Ok(())
}

/// `AddLinks`: adds symlinks from a content provider to an ignition config.
pub fn add_links(ign: &mut Config, provider: &dyn ContentProvider) -> anyhow::Result<()> {
    for def in provider.get_links() {
        let mut l = Link::new(def.path.clone());
        l.overwrite = Some(true);
        l.target = Some(def.target.clone());
        ign.storage
            .get_or_insert_with(Default::default)
            .links
            .get_or_insert_with(Vec::new)
            .push(l);
    }
    Ok(())
}

/// `AddSystemdUnits`: adds systemd units from a content provider to an ignition config.
pub fn add_systemd_units(ign: &mut Config, provider: &dyn ContentProvider) -> anyhow::Result<()> {
    let units = provider
        .get_systemd_units()
        .map_err(|e| ctrlrt::wrapf!(e, "failed to get systemd units: {}", e))?;
    for def in units {
        let mut u = Unit::new(def.name.clone());
        u.enabled = Some(true);
        // Go string(bytes): invalid UTF-8 is replaced by U+FFFD when JSON-encoded anyway.
        u.contents = Some(String::from_utf8_lossy(&def.contents).into_owned());
        ign.systemd
            .get_or_insert_with(Default::default)
            .units
            .get_or_insert_with(Vec::new)
            .push(u);
    }
    Ok(())
}

/// `AddKernelArgs`: adds kernel arguments templating to the ignition config.
pub fn add_kernel_args(ign: &mut Config) {
    ign.kernel_arguments
        .get_or_insert_with(Default::default)
        .should_exist
        .get_or_insert_with(Vec::new)
        .push("{{.KernelParameters}}".to_string());
}

/// `AddContent`: adds files, links and systemd units.
pub fn add_content(ign: &mut Config, provider: &dyn ContentProvider) -> anyhow::Result<()> {
    add_files(ign, provider)?;
    add_links(ign, provider)?;
    add_systemd_units(ign, provider)?;
    Ok(())
}
