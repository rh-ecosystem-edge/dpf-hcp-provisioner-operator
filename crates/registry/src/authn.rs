//! Port of `github.com/google/go-containerregistry/pkg/authn`.

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::gobase64;
use crate::goutil;
use crate::name::{Registry, Repository};

/// `authn.AuthConfig` — the credentials stored in a docker config file.
///
/// JSON (un)marshalling follows the Go custom `UnmarshalJSON` / `MarshalJSON`:
/// a non-empty `auth` is base64-decoded into `username`/`password` (overriding them);
/// otherwise when both username and password are set, `auth` is computed. Marshalling
/// always recomputes `auth` from username/password.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthConfig {
    pub username: String,
    pub password: String,
    pub auth: String,
    /// IdentityToken is used to authenticate the user and get an access token for the
    /// registry.
    pub identity_token: String,
    /// RegistryToken is a bearer token to be sent to a registry.
    pub registry_token: String,
}

/// Error returned by [`AuthConfig::from_json`] / the serde `Deserialize` impl.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthConfigError(pub String);

impl fmt::Display for AuthConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for AuthConfigError {}

impl AuthConfig {
    /// `json.Unmarshal(data, &authConfig)` with Go's exact error messages
    /// (for well-formed JSON input).
    pub fn from_json(data: &[u8]) -> Result<AuthConfig, AuthConfigError> {
        let v: serde_json::Value = serde_json::from_slice(data)
            .map_err(|e| AuthConfigError(goutil::json_syntax_error(data, &e)))?;
        Self::from_json_value(&v)
    }

    /// Decode from an already parsed JSON value (Go `AuthConfig.UnmarshalJSON`).
    pub fn from_json_value(v: &serde_json::Value) -> Result<AuthConfig, AuthConfigError> {
        let obj = match v {
            serde_json::Value::Null => return Ok(AuthConfig::default()),
            serde_json::Value::Object(o) => o,
            other => {
                return Err(AuthConfigError(format!(
                    "json: cannot unmarshal {} into Go value of type authn.authConfig",
                    goutil::json_kind(other)
                )));
            }
        };
        // Decode the shadow struct; Go keeps going after a type error and reports the
        // first one at the end.
        let mut first_err: Option<String> = None;
        let mut field = |name: &str| -> String {
            match goutil::json_string_field(obj, "authConfig", name) {
                Ok(v) => v.unwrap_or_default(),
                Err(e) => {
                    first_err.get_or_insert(e);
                    String::new()
                }
            }
        };
        let mut a = AuthConfig {
            username: field("username"),
            password: field("password"),
            auth: field("auth"),
            identity_token: field("identitytoken"),
            registry_token: field("registrytoken"),
        };
        if let Some(e) = first_err {
            return Err(AuthConfigError(e));
        }

        if !a.auth.is_empty() {
            match decode_docker_config_field_auth(&a.auth) {
                Ok((u, p)) => {
                    a.username = u;
                    a.password = p;
                }
                Err(e) => {
                    return Err(AuthConfigError(format!("unable to decode auth field: {e}")));
                }
            }
        } else if !a.username.is_empty() && !a.password.is_empty() {
            a.auth = encode_docker_config_field_auth(&a.username, &a.password);
        }
        Ok(a)
    }

    /// `json.Marshal(authConfig)` (Go `AuthConfig.MarshalJSON`).
    pub fn to_json(&self) -> String {
        let auth = encode_docker_config_field_auth(&self.username, &self.password);
        let mut fields: Vec<String> = Vec::new();
        let mut push = |k: &str, v: &str| {
            if !v.is_empty() {
                fields.push(format!("\"{k}\":{}", goutil::json_string(v)));
            }
        };
        push("username", &self.username);
        push("password", &self.password);
        push("auth", &auth);
        push("identitytoken", &self.identity_token);
        push("registrytoken", &self.registry_token);
        format!("{{{}}}", fields.join(","))
    }
}

impl<'de> Deserialize<'de> for AuthConfig {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        AuthConfig::from_json_value(&v).map_err(serde::de::Error::custom)
    }
}

impl Serialize for AuthConfig {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let auth = encode_docker_config_field_auth(&self.username, &self.password);
        let mut fields: Vec<(&str, &str)> = Vec::new();
        for (k, v) in [
            ("username", self.username.as_str()),
            ("password", self.password.as_str()),
            ("auth", auth.as_str()),
            ("identitytoken", self.identity_token.as_str()),
            ("registrytoken", self.registry_token.as_str()),
        ] {
            if !v.is_empty() {
                fields.push((k, v));
            }
        }
        use serde::ser::SerializeMap;
        let mut m = s.serialize_map(Some(fields.len()))?;
        for (k, v) in fields {
            m.serialize_entry(k, v)?;
        }
        m.end()
    }
}

fn decode_docker_config_field_auth(field: &str) -> Result<(String, String), String> {
    let decoded = if field.trim().ends_with('=') {
        gobase64::decode_std(field)
    } else {
        gobase64::decode_raw_std(field)
    }
    .map_err(|e| e.to_string())?;
    // Go converts arbitrary bytes to a string; invalid UTF-8 is kept byte-for-byte there.
    let s = String::from_utf8_lossy(&decoded).into_owned();
    match s.split_once(':') {
        Some((u, p)) => Ok((u.to_string(), p.to_string())),
        None => Err("must be formatted as base64(username:password)".to_string()),
    }
}

fn encode_docker_config_field_auth(username: &str, password: &str) -> String {
    gobase64::encode_std(format!("{username}:{password}").as_bytes())
}

// ------------------------------------------------------------------ Authenticator

/// `authn.Authenticator`
pub trait Authenticator: Send + Sync + fmt::Debug {
    /// `Authorization()`
    fn authorization(&self) -> anyhow::Result<AuthConfig>;

    /// Go compares `auth != authn.Anonymous` by identity; only [`Anonymous`] returns true.
    fn is_anonymous(&self) -> bool {
        false
    }
}

/// `authn.Anonymous`
#[derive(Debug, Clone, Copy, Default)]
pub struct Anonymous;

impl Authenticator for Anonymous {
    fn authorization(&self) -> anyhow::Result<AuthConfig> {
        Ok(AuthConfig::default())
    }
    fn is_anonymous(&self) -> bool {
        true
    }
}

/// The `authn.Anonymous` singleton as a shareable authenticator.
pub fn anonymous() -> Arc<dyn Authenticator> {
    Arc::new(Anonymous)
}

/// `authn.FromConfig` result.
#[derive(Debug, Clone)]
struct Auth {
    config: AuthConfig,
}

impl Authenticator for Auth {
    fn authorization(&self) -> anyhow::Result<AuthConfig> {
        Ok(self.config.clone())
    }
}

/// `authn.FromConfig`
pub fn from_config(cfg: AuthConfig) -> Arc<dyn Authenticator> {
    Arc::new(Auth { config: cfg })
}

/// `authn.Basic`
#[derive(Debug, Clone, Default)]
pub struct Basic {
    pub username: String,
    pub password: String,
}

impl Authenticator for Basic {
    fn authorization(&self) -> anyhow::Result<AuthConfig> {
        Ok(AuthConfig {
            username: self.username.clone(),
            password: self.password.clone(),
            ..Default::default()
        })
    }
}

/// `authn.Bearer`
#[derive(Debug, Clone, Default)]
pub struct Bearer {
    pub token: String,
}

impl Authenticator for Bearer {
    fn authorization(&self) -> anyhow::Result<AuthConfig> {
        Ok(AuthConfig {
            registry_token: self.token.clone(),
            ..Default::default()
        })
    }
}

// ------------------------------------------------------------------ Keychain

/// `authn.Resource`
pub trait Resource: Send + Sync {
    /// `String()`
    fn string(&self) -> String;
    /// `RegistryStr()`
    fn registry_str(&self) -> String;
}

impl Resource for Registry {
    fn string(&self) -> String {
        self.to_string()
    }
    fn registry_str(&self) -> String {
        Registry::registry_str(self).to_string()
    }
}

impl Resource for Repository {
    fn string(&self) -> String {
        self.to_string()
    }
    fn registry_str(&self) -> String {
        Repository::registry_str(self).to_string()
    }
}

/// `authn.Keychain`
pub trait Keychain: Send + Sync {
    /// `Resolve(Resource)`
    fn resolve(&self, target: &dyn Resource) -> anyhow::Result<Arc<dyn Authenticator>>;
}

impl<K: Keychain + ?Sized> Keychain for Arc<K> {
    fn resolve(&self, target: &dyn Resource) -> anyhow::Result<Arc<dyn Authenticator>> {
        (**self).resolve(target)
    }
}

impl<K: Keychain + ?Sized> Keychain for Box<K> {
    fn resolve(&self, target: &dyn Resource) -> anyhow::Result<Arc<dyn Authenticator>> {
        (**self).resolve(target)
    }
}

/// `authn.NewMultiKeychain`
pub struct MultiKeychain {
    keychains: Vec<Arc<dyn Keychain>>,
}

impl MultiKeychain {
    pub fn new(keychains: Vec<Arc<dyn Keychain>>) -> Self {
        Self { keychains }
    }
}

impl Keychain for MultiKeychain {
    fn resolve(&self, target: &dyn Resource) -> anyhow::Result<Arc<dyn Authenticator>> {
        for kc in &self.keychains {
            let auth = kc.resolve(target)?;
            if !auth.is_anonymous() {
                return Ok(auth);
            }
        }
        Ok(anonymous())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_decoding() {
        let a = AuthConfig::from_json(br#"{"auth":"dTpw"}"#).unwrap();
        assert_eq!(a.username, "u");
        assert_eq!(a.password, "p");
        let e = AuthConfig::from_json(br#"{"auth":"!!!!"}"#).unwrap_err();
        assert_eq!(
            e.to_string(),
            "unable to decode auth field: illegal base64 data at input byte 0"
        );
        assert_eq!(AuthConfig::default().to_json(), r#"{"auth":"Og=="}"#);
        assert!(anonymous().is_anonymous());
        assert!(!from_config(AuthConfig::default()).is_anonymous());
    }
}
