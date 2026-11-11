//! Port of `internal/common/keychain.go`: an `authn.Keychain` backed by a
//! `.dockerconfigjson` pull secret.

use std::collections::HashMap;
use std::sync::Arc;

use ctrlrt::k8s_openapi::api::core::v1::Secret;
use ctrlrt::{Client, ObjectKey, Result, wrapf};
use registry::authn::{self, AuthConfig, Authenticator, Keychain, Resource};
use registry::goutil;

/// `dockerConfigKeychain`: resolves credentials by exact repository match first
/// (`target.String()`), then by registry (`target.RegistryStr()`), else anonymous.
#[derive(Clone, Debug, Default)]
pub struct DockerConfigKeychain {
    auths: HashMap<String, AuthConfig>,
}

impl Keychain for DockerConfigKeychain {
    fn resolve(&self, target: &dyn Resource) -> anyhow::Result<Arc<dyn Authenticator>> {
        if let Some(cfg) = self.auths.get(&target.string()) {
            return Ok(authn::from_config(cfg.clone()));
        }
        if let Some(cfg) = self.auths.get(&target.registry_str()) {
            return Ok(authn::from_config(cfg.clone()));
        }
        Ok(authn::anonymous())
    }
}

/// Decode `struct{ Auths map[string]authn.AuthConfig `json:"auths"` }` with Go's
/// `encoding/json` error messages.
fn decode_docker_config(data: &[u8]) -> std::result::Result<HashMap<String, AuthConfig>, String> {
    let v: serde_json::Value =
        serde_json::from_slice(data).map_err(|e| goutil::json_syntax_error(data, &e))?;
    let obj = match &v {
        serde_json::Value::Null => return Ok(HashMap::new()),
        serde_json::Value::Object(o) => o,
        other => {
            return Err(format!(
                "json: cannot unmarshal {} into Go value of type common.dockerConfig",
                goutil::json_kind(other)
            ));
        }
    };
    let mut out = HashMap::new();
    match goutil::json_field(obj, "auths") {
        None | Some(serde_json::Value::Null) => {}
        Some(serde_json::Value::Object(m)) => {
            for (k, entry) in m {
                let cfg = AuthConfig::from_json_value(entry).map_err(|e| e.to_string())?;
                out.insert(k.clone(), cfg);
            }
        }
        Some(other) => {
            return Err(format!(
                "json: cannot unmarshal {} into Go struct field dockerConfig.auths of type map[string]authn.AuthConfig",
                goutil::json_kind(other)
            ));
        }
    }
    Ok(out)
}

/// `NewKeychainFromDockerConfig`
pub fn new_keychain_from_docker_config(docker_config_json: &[u8]) -> Result<Arc<dyn Keychain>> {
    match decode_docker_config(docker_config_json) {
        Ok(auths) => Ok(Arc::new(DockerConfigKeychain { auths })),
        Err(msg) => Err(anyhow::anyhow!("failed to unmarshal docker config: {msg}")),
    }
}

/// `KeychainFromPullSecret` reads `.dockerconfigjson` from the named secret.
pub async fn keychain_from_pull_secret(
    c: &Client,
    name: &str,
    namespace: &str,
) -> Result<Arc<dyn Keychain>> {
    let secret: Secret = match c.get(&ObjectKey::new(namespace, name)).await {
        Ok(s) => s,
        Err(e) => return Err(wrapf!(e, "getting secret {}/{}: {}", namespace, name, e)),
    };
    const KEY: &str = ".dockerconfigjson";
    let Some(data) = secret.data.as_ref().and_then(|d| d.get(KEY)) else {
        return Err(anyhow::anyhow!(
            "secret {}/{} missing key {}",
            namespace,
            name,
            ctrlrt::error::go_quote(KEY)
        ));
    };
    new_keychain_from_docker_config(&data.0)
}
