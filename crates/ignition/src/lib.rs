//! Port of `internal/ignition` (and `internal/ignition/content`,
//! `internal/ignition/resources/{common,live,target}`).
//!
//! Ignition configs use the official `ignition-config` crate types ([`types`] =
//! `ignition_config::v3_4`). Behaviour that the Go code gets from
//! `github.com/coreos/ignition/v2` and `encoding/json` (byte-exact JSON encoding, decoding
//! with Go's rules and error messages, `merge.MergeStructTranscribe`,
//! `validate.ValidateWithContext`) is ported on a reflection model of the Go types (see
//! [`gotypes`](crate::gotypes)): a Go non-pointer struct field is `None` or `Some(zero)`
//! in the crate types, both meaning the Go zero value.

pub mod content;
mod gotypes;
mod merge;
pub mod resources;
mod template;
pub mod validate;

use apis::dpf::provisioning::v1alpha1::{ConfigFile, DPUFlavor, DPUFlavorSpec, FILE_APPEND};
use ctrlrt::wrapf;
use ignition_config::v3_4::{Config, File, Ignition, Resource};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

pub use ignition_config::v3_4 as types;
pub use validate::Report;

/// `IgnitionVersion`: a version of Ignition compatible with target RHCOS boot images.
pub const IGNITION_VERSION: &str = "3.4.0";

/// `Ptr`: Go `&v` for an optional field.
pub fn ptr<T>(v: T) -> Option<T> {
    Some(v)
}

/// `NewEmptyIgnition`.
pub fn new_empty_ignition(version: &str) -> Config {
    let version = if version.is_empty() {
        IGNITION_VERSION
    } else {
        version
    };
    Config::new(Ignition::new(version.to_string()))
}

// ------------------------------------------------------------------------------- helpers

/// `ign.Storage.Files` (empty when unset).
pub fn files(ign: &Config) -> &[File] {
    ign.storage
        .as_ref()
        .and_then(|s| s.files.as_deref())
        .unwrap_or_default()
}

/// `&ign.Storage.Files` for appending/modifying (creates the storage/list if unset).
pub fn files_mut(ign: &mut Config) -> &mut Vec<File> {
    ign.storage
        .get_or_insert_with(Default::default)
        .files
        .get_or_insert_with(Vec::new)
}

/// `ign.Systemd.Units` (empty when unset).
pub fn units(ign: &Config) -> &[types::Unit] {
    ign.systemd
        .as_ref()
        .and_then(|s| s.units.as_deref())
        .unwrap_or_default()
}

/// `&ign.Systemd.Units` for appending/modifying.
pub fn units_mut(ign: &mut Config) -> &mut Vec<types::Unit> {
    ign.systemd
        .get_or_insert_with(Default::default)
        .units
        .get_or_insert_with(Vec::new)
}

/// `ign.Storage.Links` (empty when unset).
pub fn links(ign: &Config) -> &[types::Link] {
    ign.storage
        .as_ref()
        .and_then(|s| s.links.as_deref())
        .unwrap_or_default()
}

/// `ign.Passwd.Users` (empty when unset).
pub fn users(ign: &Config) -> &[types::User] {
    ign.passwd
        .as_ref()
        .and_then(|p| p.users.as_deref())
        .unwrap_or_default()
}

/// `f.Contents.Source` of a file.
pub fn file_source(f: &File) -> Option<&str> {
    f.contents.as_ref().and_then(|c| c.source.as_deref())
}

/// Builds a file entry `{Node: {Path, Overwrite: true}, Mode, Contents: {Source}}`.
pub fn new_file(path: &str, mode: i64, source: String) -> File {
    let mut f = File::new(path.to_string());
    f.overwrite = Some(true);
    f.mode = Some(mode);
    f.contents = Some(Resource {
        source: Some(source),
        ..Default::default()
    });
    f
}

/// Go `json.Unmarshal(data, &cfg)` into a `types.Config` (Go decoding rules: case-insensitive
/// keys, unknown keys ignored, `null` leaves values unset; Go error messages).
pub fn unmarshal_config(data: &[u8]) -> Result<Config, gocompat::json::Error> {
    gotypes::unmarshal(data).map(|v| gotypes::to_config(&v))
}

/// Go `json.Marshal(cfg)` of a `types.Config` (struct field order, empty structs emitted).
pub fn marshal_config(cfg: &Config) -> Vec<u8> {
    gotypes::marshal(gotypes::CONFIG_TY, &gotypes::from_config(cfg))
}

/// `merge.MergeStructTranscribe(parent, child)` for v3_4 configs (the merged config).
pub fn merge_configs(parent: &Config, child: &Config) -> Config {
    let merged = merge::merge_struct(
        &gotypes::CONFIG,
        &gotypes::from_config(parent),
        &gotypes::from_config(child),
    );
    gotypes::to_config(&merged)
}

// ------------------------------------------------------------------------------ encoding.go

/// `MarshalJSON`: marshals an ignition config to JSON, stripping empty objects.
pub fn marshal_json(ign: &Config) -> anyhow::Result<Vec<u8>> {
    strip_empty_objects(&marshal_config(ign))
}

fn strip_empty_objects(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut m = gocompat::json::unmarshal_object(data)?.unwrap_or_default();
    strip_empty(&mut m);
    Ok(gocompat::json::marshal(&serde_json::Value::Object(m)))
}

fn strip_empty(m: &mut serde_json::Map<String, serde_json::Value>) {
    let keys: Vec<String> = m.keys().cloned().collect();
    for k in keys {
        let remove = match m.get_mut(&k) {
            Some(serde_json::Value::Object(val)) => {
                strip_empty(val);
                val.is_empty()
            }
            Some(serde_json::Value::Array(items)) => {
                for item in items {
                    if let serde_json::Value::Object(nested) = item {
                        strip_empty(nested);
                    }
                }
                false
            }
            _ => false,
        };
        if remove {
            m.remove(&k);
        }
    }
}

fn gzip_best(data: &[u8]) -> anyhow::Result<Vec<u8>> {
    Ok(gocompat::gzip::compress(data, 9)?)
}

/// `EncodeIgnition`: gzip + base64 of [`marshal_json`].
pub fn encode_ignition(ign: &Config) -> anyhow::Result<String> {
    let json = marshal_json(ign).map_err(|e| wrapf!(e, "failed to marshal ignition: {}", e))?;
    let gz = gzip_best(&json).map_err(|e| wrapf!(e, "failed to gzip ignition: {}", e))?;
    Ok(gocompat::base64::encode_to_string(&gz))
}

/// `EncodeGzipFile`: a resource with gzip-compressed, base64-encoded content.
pub fn encode_gzip_file(content: &[u8]) -> anyhow::Result<Resource> {
    let gz = gzip_best(content).map_err(|e| wrapf!(e, "failed to gzip content: {}", e))?;
    Ok(Resource {
        source: Some(format!(
            "data:;base64,{}",
            gocompat::base64::encode_to_string(&gz)
        )),
        compression: Some("gzip".to_string()),
        ..Default::default()
    })
}

/// `DecodeIgnition`: decodes a base64 + gzip encoded ignition config.
pub fn decode_ignition(encoded: &str) -> anyhow::Result<Config> {
    let compressed = gocompat::base64::decode_string(encoded.as_bytes())
        .map_err(|e| wrapf!(e, "failed to base64 decode: {}", e))?;
    let reader = gocompat::gzip::Reader::new(&compressed)
        .map_err(|e| wrapf!(e, "failed to create gzip reader: {}", e))?;
    let data = reader
        .read_all()
        .map_err(|e| wrapf!(e, "failed to decompress: {}", e))?;
    unmarshal_config(&data).map_err(|e| wrapf!(e, "failed to unmarshal ignition: {}", e))
}

// ------------------------------------------------------------------------------ compress.go

/// `GzipIgnitionFiles`: gzip-compresses all uncompressed data-URL files of the storage.
/// Files that already have compression set are left as-is.
pub fn gzip_ignition_files(ign: &mut Config) -> anyhow::Result<()> {
    let Some(files) = ign.storage.as_mut().and_then(|s| s.files.as_mut()) else {
        return Ok(());
    };
    for f in files.iter_mut() {
        let Some(contents) = f.contents.as_mut() else {
            continue;
        };
        if contents
            .compression
            .as_deref()
            .is_some_and(|c| !c.is_empty())
        {
            continue;
        }
        let source = match contents.source.as_deref() {
            None | Some("") => continue,
            Some(s) => s.to_string(),
        };
        let raw = if let Some((_, b64)) = source.split_once(";base64,") {
            gocompat::base64::decode_string(b64.as_bytes())
                .map_err(|e| wrapf!(e, "failed to base64-decode file source: {}", e))?
        } else if source.starts_with("data:") {
            let Some(idx) = source.find(',') else {
                continue;
            };
            gocompat::url::query_unescape(&source[idx + 1..])
                .map_err(|e| wrapf!(e, "failed to URL-decode file source: {}", e))?
        } else {
            continue;
        };
        let gz = gzip_best(&raw).map_err(|e| wrapf!(e, "failed to gzip file content: {}", e))?;
        contents.source = Some(format!(
            "data:;base64,{}",
            gocompat::base64::encode_to_string(&gz)
        ));
        contents.compression = Some("gzip".to_string());
    }
    Ok(())
}

// --------------------------------------------------------------------------- configfiles.go

const DEFAULT_FILE_PERMISSIONS: i64 = 0o644;

/// `MergeFlavorConfigFiles`: builds an ignition config from the DPUFlavor ConfigFiles and
/// merges it into `base` with Ignition's merge logic (files keyed by path; the flavor wins:
/// `override` replaces the contents, `append` keeps the base contents and adds `append`
/// entries).
pub fn merge_flavor_config_files(
    base: &mut Config,
    flavor_spec: &DPUFlavorSpec,
) -> anyhow::Result<()> {
    if flavor_spec.config_files.is_empty() {
        return Ok(());
    }
    let mut flavor_ign = new_empty_ignition(&base.ignition.version);
    add_flavor_files(&mut flavor_ign, &flavor_spec.config_files)?;
    *base = merge_configs(base, &flavor_ign);
    Ok(())
}

fn add_flavor_files(ign: &mut Config, config_files: &[ConfigFile]) -> anyhow::Result<()> {
    for cf in config_files {
        if cf.path.is_empty() {
            continue;
        }
        let mode = parse_permissions(&cf.permissions).map_err(|e| {
            wrapf!(
                e,
                "invalid permissions {} for config file {}: {}",
                gocompat::strconv::quote(&cf.permissions),
                cf.path,
                e
            )
        })?;
        let Some(raw) = &cf.raw else {
            return Err(anyhow::anyhow!(
                "config file {} has no raw content",
                cf.path
            ));
        };
        let resource = encode_gzip_file(raw.as_bytes())
            .map_err(|e| wrapf!(e, "failed to encode config file {}: {}", cf.path, e))?;
        let mut file = File::new(cf.path.clone());
        file.mode = Some(mode);
        if cf.operation == FILE_APPEND {
            file.append = Some(vec![resource]);
        } else {
            file.overwrite = Some(true);
            file.contents = Some(resource);
        }
        files_mut(ign).push(file);
    }
    Ok(())
}

fn parse_permissions(perms: &str) -> anyhow::Result<i64> {
    if perms.is_empty() {
        return Ok(DEFAULT_FILE_PERMISSIONS);
    }
    gocompat::strconv::parse_int(perms, 8, 32)
        .map_err(|e| wrapf!(e, "failed to parse octal permissions: {}", e))
}

// ----------------------------------------------------------------------------- dpuflavor.go

const DPU_FLAVOR_JSON_PATH: &str = "/etc/dpf/dpuflavor.json";
const DPU_FLAVOR_YAML_PATH: &str = "/etc/dpf/dpuflavor.yaml";

/// `cleanDPUFlavor`: a copy with only apiVersion, kind, name, namespace and spec. Unknown
/// fields preserved by the Rust types (`extra`), which Go drops, are cleared as well.
pub fn clean_dpu_flavor(flavor: &DPUFlavor) -> DPUFlavor {
    let mut spec = flavor.spec.clone();
    clear_spec_extra(&mut spec);
    DPUFlavor {
        kind: flavor.kind.clone(),
        api_version: flavor.api_version.clone(),
        metadata: ObjectMeta {
            name: flavor.metadata.name.clone(),
            namespace: flavor.metadata.namespace.clone(),
            ..Default::default()
        },
        spec,
        extra: Default::default(),
    }
}

fn clear_spec_extra(spec: &mut DPUFlavorSpec) {
    use apis::dpf::provisioning::v1alpha1 as p;
    let DPUFlavorSpec {
        grub,
        sysctl,
        nvconfig,
        ovs,
        bfcfg_parameters: _,
        config_files,
        packages,
        systemd_services,
        containerd_config,
        dpu_resources: _,
        system_reserved_resources: _,
        dpu_mode: _,
        host_network_interface_configs,
        ew_nic_configurations,
        dma,
        service_readiness,
        extra,
    } = spec;
    extra.clear();
    let p::DPUFlavorGrub {
        kernel_parameters: _,
        extra,
    } = grub;
    extra.clear();
    let p::DPUFLavorSysctl {
        parameters: _,
        extra,
    } = sysctl;
    extra.clear();
    let clear_nv = |nv: &mut p::NVConfig| {
        let p::NVConfig {
            device: _,
            parameters: _,
            force: _,
            extra,
        } = nv;
        extra.clear();
    };
    nvconfig.iter_mut().for_each(clear_nv);
    let p::DPUFlavorOVS {
        raw_config_script: _,
        extra,
    } = ovs;
    extra.clear();
    for cf in config_files {
        let p::ConfigFile {
            type_: _,
            path: _,
            operation: _,
            raw: _,
            content_from,
            permissions: _,
            extra,
        } = cf;
        extra.clear();
        if let Some(cs) = content_from {
            let p::ConfigFileContentSource {
                config_map_key_ref,
                extra,
            } = cs;
            extra.clear();
            if let Some(r) = config_map_key_ref {
                let apis::generated::k8s_core_v1::ConfigMapKeySelector {
                    name: _,
                    key: _,
                    optional: _,
                    extra,
                } = r;
                extra.clear();
            }
        }
    }
    for pk in packages {
        let p::PackageSpec {
            name: _,
            version,
            repo_file_ref: _,
            extra,
        } = pk;
        extra.clear();
        if let Some(v) = version {
            let p::PackageVersionSpec {
                value: _,
                match_policy: _,
                extra,
            } = v;
            extra.clear();
        }
    }
    for s in systemd_services {
        let p::SystemdServiceSpec {
            name: _,
            operation: _,
            extra,
        } = s;
        extra.clear();
    }
    let p::ContainerdConfig {
        registry_endpoint: _,
        extra,
    } = containerd_config;
    extra.clear();
    for h in host_network_interface_configs {
        let p::NetworkInterfaceConfig {
            mtu: _,
            dhcp: _,
            port_number: _,
            nvconfig,
            extra,
        } = h;
        extra.clear();
        if let Some(nv) = nvconfig {
            clear_nv(nv);
        }
    }
    for n in ew_nic_configurations {
        let p::NicConfiguration {
            num_vfs: _,
            link_type: _,
            spectrum_x_optimized,
            raw_nv_config,
            network_bay,
            force: _,
            extra,
        } = n;
        extra.clear();
        if let Some(s) = spectrum_x_optimized {
            let apis::generated::nicconfig_v1alpha1::SpectrumXOptimizedSpec {
                enabled: _,
                version: _,
                overlay: _,
                multiplane_mode: _,
                number_of_planes: _,
                extra,
            } = s;
            extra.clear();
        }
        for r in raw_nv_config {
            let apis::generated::nicconfig_v1alpha1::NvConfigParam {
                name: _,
                value: _,
                extra,
            } = r;
            extra.clear();
        }
        if let Some(b) = network_bay {
            let apis::generated::nicconfig_v1alpha1::NetworkBaySpec { conf: _, extra } = b;
            extra.clear();
        }
    }
    if let Some(d) = dma {
        let p::DPUFlavorDMA { enabled: _, extra } = d;
        extra.clear();
    }
    if let Some(s) = service_readiness {
        let p::ServiceReadiness { gate: _, extra } = s;
        extra.clear();
    }
}

/// `AddDPUFlavorYAML`: adds the cleaned DPUFlavor as YAML at `/etc/dpf/dpuflavor.yaml`.
pub fn add_dpu_flavor_yaml(ign: &mut Config, flavor: &DPUFlavor) -> anyhow::Result<()> {
    let data = gocompat::yaml::marshal_typed(&clean_dpu_flavor(flavor))
        .map_err(|e| wrapf!(e, "failed to marshal DPU flavor: {}", e))?;
    add_dpu_flavor_file(ign, &data, DPU_FLAVOR_YAML_PATH);
    Ok(())
}

/// `AddDPUFlavorJSON`: adds the cleaned DPUFlavor as JSON at `/etc/dpf/dpuflavor.json`.
pub fn add_dpu_flavor_json(ign: &mut Config, flavor: &DPUFlavor) -> anyhow::Result<()> {
    let data = gocompat::json::to_go_json(&clean_dpu_flavor(flavor))
        .map_err(|e| wrapf!(e, "failed to marshal DPU flavor: {}", e))?;
    add_dpu_flavor_file(ign, &data, DPU_FLAVOR_JSON_PATH);
    Ok(())
}

fn add_dpu_flavor_file(ign: &mut Config, data: &[u8], path: &str) {
    let source = format!(
        "data:text/plain;charset=utf-8;base64,{}",
        gocompat::base64::encode_to_string(data)
    );
    files_mut(ign).push(new_file(path, 0o644, source));
}

// ----------------------------------------------------------------------------- machineos.go

/// `ReplaceMachineOSURL`: replaces `spec.osImageURL` in the encapsulated machine config.
/// Prints `Replacing machine OS URL` to stdout like the Go code.
pub fn replace_machine_os_url(ign: &mut Config, machine_os_url: &str) -> anyhow::Result<()> {
    let files = ign.storage.as_mut().and_then(|s| s.files.as_mut());
    for file in files.into_iter().flatten() {
        if file.path != "/etc/ignition-machine-config-encapsulated.json" {
            continue;
        }
        println!("Replacing machine OS URL");
        let source = match file.contents.as_ref().and_then(|c| c.source.as_deref()) {
            Some(s) if s.starts_with("data:,") => s,
            _ => return Err(anyhow::anyhow!("unexpected data URI format")),
        };
        let decoded = gocompat::url::query_unescape(&source["data:,".len()..])
            .map_err(|e| wrapf!(e, "failed to decode data URI: {}", e))?;
        let mut config = gocompat::json::unmarshal_object(&decoded)
            .map_err(|e| wrapf!(e, "failed to unmarshal machine config: {}", e))?;
        let Some(serde_json::Value::Object(spec)) = config.as_mut().and_then(|c| c.get_mut("spec"))
        else {
            return Err(anyhow::anyhow!("spec not found in machine config"));
        };
        spec.insert(
            "osImageURL".to_string(),
            serde_json::Value::String(machine_os_url.to_string()),
        );
        let modified =
            gocompat::json::marshal(&serde_json::Value::Object(config.unwrap_or_default()));
        let new_source = format!("data:,{}", gocompat::url::query_escape(&modified));
        file.contents.get_or_insert_with(Default::default).source = Some(new_source);
        return Ok(());
    }
    Err(anyhow::anyhow!("machine OS URL file not found"))
}

// ----------------------------------------------------------------------------------- mtu.go

/// `URLEncode`: a `data:text/plain,` URI with the query-escaped string.
pub fn url_encode(s: &str) -> String {
    format!("data:text/plain,{}", gocompat::url::query_escape(s))
}

fn nm_interface(name: &str, mtu: u16) -> String {
    url_encode(&format!(
        "[connection]\nid={name}\ntype=ethernet\ninterface-name={name}\n\n[ethernet]\nmtu={mtu}\n"
    ))
}

/// `EnableMTU`: adds NetworkManager MTU configuration files and sets the MTU in an existing
/// `pf0vf0.nmconnection`.
pub fn enable_mtu(ign: &mut Config, mtu: u16) {
    for iface in ["p0", "p1", "pf0hpf", "pf1hpf"] {
        let path = format!("/etc/NetworkManager/system-connections/{iface}.nmconnection");
        files_mut(ign).push(new_file(&path, 0o600, nm_interface(iface, mtu)));
    }
    let from = gocompat::url::query_escape("[ethernet]\n");
    let to = gocompat::url::query_escape(format!("[ethernet]\nmtu={mtu}\n"));
    for file in files_mut(ign).iter_mut() {
        if file
            .path
            .ends_with("/etc/NetworkManager/system-connections/pf0vf0.nmconnection")
            && let Some(src) = file.contents.as_mut().and_then(|c| c.source.as_mut())
            && !src.is_empty()
        {
            *src = src.replacen(&from, &to, 1);
        }
    }
}

// ----------------------------------------------------------------------------------- ovs.go

/// `AddFlavorOVSScript`: adds the DPU Flavor OVS script at `/usr/local/bin/dpf-ovs-script.sh`.
pub fn add_flavor_ovs_script(ign: &mut Config, flavor_spec: &DPUFlavorSpec) {
    let encoded = gocompat::base64::encode_to_string(flavor_spec.ovs.raw_config_script.as_bytes());
    let source = format!("data:text/plain;charset=utf-8;base64,{encoded}");
    files_mut(ign).push(new_file("/usr/local/bin/dpf-ovs-script.sh", 0o755, source));
}

// ------------------------------------------------------------------------------ validate.go

/// `ValidateConfig`: validates an ignition config (duplicate paths/units, path conflicts
/// with systemd units, every v3_4 type rule). Any 3.x version is accepted: validation runs on
/// a copy normalized to 3.4.0. `None` is Go's nil config.
pub fn validate_config(cfg: Option<&Config>) -> anyhow::Result<()> {
    let Some(cfg) = cfg else {
        return Err(anyhow::anyhow!("ignition config is nil"));
    };
    let version = &cfg.ignition.version;
    let sv = gocompat::semver::coreos::Version::new(version).map_err(|e| {
        wrapf!(
            e,
            "ignition config has invalid version {}: {}",
            gocompat::strconv::quote(version),
            e
        )
    })?;
    if sv.major != 3 {
        return Err(anyhow::anyhow!(
            "ignition config has unsupported version {}: only spec 3.x is supported",
            gocompat::strconv::quote(version)
        ));
    }
    let mut copy = gotypes::from_config(cfg);
    set_version(&mut copy, "3.4.0");
    let raw = gotypes::marshal(gotypes::CONFIG_TY, &copy);
    let rpt = validate::validate_with_context(&copy, Some(&raw));
    if rpt.is_fatal() {
        return Err(anyhow::anyhow!(
            "ignition config validation failed: {}",
            rpt
        ));
    }
    Ok(())
}

fn set_version(cfg: &mut gotypes::GoVal, version: &str) {
    // Config.Ignition (field 0) .Version (field 4)
    if let gotypes::GoVal::Struct(fs) = cfg
        && let Some(gotypes::GoVal::Struct(ign)) = fs.get_mut(0)
        && let Some(v) = ign.get_mut(4)
    {
        *v = gotypes::GoVal::Str(version.to_string());
    }
}

/// `validate.ValidateWithContext(cfg, json.Marshal(cfg))` for a v3_4 config, without the
/// version normalization of [`validate_config`].
pub fn validate_with_context(cfg: &Config) -> Report {
    let v = gotypes::from_config(cfg);
    let raw = gotypes::marshal(gotypes::CONFIG_TY, &v);
    validate::validate_with_context(&v, Some(&raw))
}
