//! Port of `remote/transport/bearer.go`.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};

use ::http::Method;
use async_trait::async_trait;
use bytes::Bytes;

use super::challenge::response_challenges;
use super::ping::Challenge;
use super::{
    BasicTransport, Client, DEFAULT_USER_AGENT, Request, Response, RoundTripper, check_error,
    matches_host,
};
use crate::authn::{AuthConfig, Authenticator, from_config};
use crate::gourl::{self, GoUrl};
use crate::goutil;
use crate::name::Registry;

/// `transport.Token`
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Token {
    pub token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
}

impl Token {
    /// `json.Unmarshal(content, &response)` with Go's error messages for the common cases.
    pub fn from_json(data: &[u8]) -> anyhow::Result<Token> {
        let v: serde_json::Value = match serde_json::from_slice(data) {
            Ok(v) => v,
            Err(e) => anyhow::bail!(goutil::json_syntax_error(data, &e)),
        };
        let obj = match v {
            serde_json::Value::Null => return Ok(Token::default()),
            serde_json::Value::Object(o) => o,
            other => anyhow::bail!(
                "json: cannot unmarshal {} into Go value of type transport.Token",
                goutil::json_kind(&other)
            ),
        };
        let mut first_err: Option<String> = None;
        let mut s = |name: &str| match goutil::json_string_field(&obj, "Token", name) {
            Ok(v) => v.unwrap_or_default(),
            Err(e) => {
                first_err.get_or_insert(e);
                String::new()
            }
        };
        let mut tok = Token {
            token: s("token"),
            access_token: s("access_token"),
            refresh_token: s("refresh_token"),
            expires_in: 0,
        };
        match goutil::json_field(&obj, "expires_in") {
            None | Some(serde_json::Value::Null) => {}
            Some(serde_json::Value::Number(n)) => match n.as_i64() {
                Some(i) => tok.expires_in = i,
                None => {
                    first_err.get_or_insert(format!(
                        "json: cannot unmarshal number {n} into Go struct field Token.expires_in of type int"
                    ));
                }
            },
            Some(other) => {
                first_err.get_or_insert(format!(
                    "json: cannot unmarshal {} into Go struct field Token.expires_in of type int",
                    goutil::json_kind(other)
                ));
            }
        }
        if let Some(e) = first_err {
            anyhow::bail!(e);
        }
        Ok(tok)
    }
}

struct BearerState {
    /// Basic credentials that we exchange for bearer tokens.
    basic: Arc<dyn Authenticator>,
    /// Holds the bearer response from the token service.
    bearer: AuthConfig,
    scopes: Vec<String>,
}

/// `bearerTransport`
pub(crate) struct BearerTransport {
    inner: Arc<dyn RoundTripper>,
    state: Mutex<BearerState>,
    registry: Registry,
    realm: String,
    service: String,
    scheme: &'static str,
}

fn go_map_string(m: &std::collections::BTreeMap<String, String>) -> String {
    let parts: Vec<String> = m.iter().map(|(k, v)| format!("{k}:{v}")).collect();
    format!("map[{}]", parts.join(" "))
}

impl BearerTransport {
    /// `fromChallenge`
    pub(crate) fn from_challenge(
        reg: &Registry,
        auth: Arc<dyn Authenticator>,
        t: Arc<dyn RoundTripper>,
        pr: &Challenge,
        scopes: Vec<String>,
    ) -> anyhow::Result<BearerTransport> {
        let Some(realm) = pr.parameters.get("realm") else {
            anyhow::bail!(
                "malformed www-authenticate, missing realm: {}",
                go_map_string(&pr.parameters)
            );
        };
        let service = pr.parameters.get("service").cloned().unwrap_or_default();
        Ok(BearerTransport {
            inner: t,
            state: Mutex::new(BearerState {
                basic: auth,
                bearer: AuthConfig::default(),
                scopes,
            }),
            registry: reg.clone(),
            realm: realm.clone(),
            service,
            scheme: if pr.insecure { "http" } else { "https" },
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, BearerState> {
        self.state.lock().unwrap_or_else(|p| p.into_inner())
    }

    async fn send_request(&self, mut req: Request) -> anyhow::Result<Response> {
        if matches_host(self.registry.registry_str(), &req, self.scheme) {
            let token = self.lock().bearer.registry_token.clone();
            req.set_header(::http::header::AUTHORIZATION, &format!("Bearer {token}"));
        }
        self.inner.round_trip(req).await
    }

    /// `refresh`: exchange credentials for a bearer token.
    pub(crate) async fn refresh(&self) -> anyhow::Result<()> {
        let basic = self.lock().basic.clone();
        let auth = basic.authorization()?;
        if !auth.registry_token.is_empty() {
            self.lock().bearer.registry_token = auth.registry_token;
            return Ok(());
        }
        let mut response = self.refresh_token(&auth).await?;
        // Some registries set access_token instead of token.
        if !response.access_token.is_empty() {
            response.token = response.access_token.clone();
        }
        let mut st = self.lock();
        if !response.token.is_empty() {
            st.bearer.registry_token = response.token.clone();
        }
        // If we obtained a refresh token from the oauth flow, use that for refresh() now.
        if !response.refresh_token.is_empty() {
            st.basic = from_config(AuthConfig {
                identity_token: response.refresh_token.clone(),
                ..Default::default()
            });
        }
        Ok(())
    }

    /// `Refresh(ctx, auth)`
    async fn refresh_token(&self, auth: &AuthConfig) -> anyhow::Result<Token> {
        let content = if !auth.identity_token.is_empty() {
            match self.refresh_oauth().await {
                Err(e)
                    if e.chain().any(|c| {
                        c.downcast_ref::<super::Error>()
                            .is_some_and(|t| t.status_code == 404)
                    }) =>
                {
                    self.refresh_basic().await?
                }
                other => other?,
            }
        } else {
            self.refresh_basic().await?
        };
        let response = Token::from_json(&content)?;
        if response.token.is_empty() && response.access_token.is_empty() {
            anyhow::bail!(
                "no token in bearer response:\n{}",
                String::from_utf8_lossy(&content)
            );
        }
        Ok(response)
    }

    /// `refreshOauth` — https://docs.docker.com/registry/spec/auth/oauth/
    async fn refresh_oauth(&self) -> anyhow::Result<Bytes> {
        let (basic, scopes) = {
            let st = self.lock();
            (st.basic.clone(), st.scopes.clone())
        };
        let auth = basic.authorization()?;
        let u = GoUrl::parse(&self.realm)?;
        let mut v = gourl::Values::new();
        v.insert("scope".into(), vec![scopes.join(" ")]);
        if !self.service.is_empty() {
            v.insert("service".into(), vec![self.service.clone()]);
        }
        v.insert("client_id".into(), vec![DEFAULT_USER_AGENT.to_string()]);
        if !auth.identity_token.is_empty() {
            v.insert("grant_type".into(), vec!["refresh_token".into()]);
            v.insert("refresh_token".into(), vec![auth.identity_token.clone()]);
        } else if !auth.username.is_empty() && !auth.password.is_empty() {
            v.insert("grant_type".into(), vec!["password".into()]);
            v.insert("username".into(), vec![auth.username.clone()]);
            v.insert("password".into(), vec![auth.password.clone()]);
            v.insert("access_type".into(), vec!["offline".into()]);
        }
        let client = Client::new(self.inner.clone());
        let mut req = Request::new(Method::POST, &u.string())?;
        req.set_header(
            ::http::header::CONTENT_TYPE,
            "application/x-www-form-urlencoded",
        );
        req.body = Some(Bytes::from(gourl::encode_query(&v)));
        let resp = client.do_request(req).await?;
        let mut resp = check_error(resp, &[200]).await?;
        resp.body
            .read_all()
            .await
            .map_err(super::http::NetError::from_io)
    }

    /// `refreshBasic` — https://docs.docker.com/registry/spec/auth/token/
    async fn refresh_basic(&self) -> anyhow::Result<Bytes> {
        let mut u = GoUrl::parse(&self.realm)?;
        let (basic, scopes) = {
            let st = self.lock();
            (st.basic.clone(), st.scopes.clone())
        };
        let b = BasicTransport {
            inner: self.inner.clone(),
            auth: basic,
            target: u.host.clone(),
        };
        let client = Client::new(Arc::new(b));
        let mut v = u.query();
        if scopes.is_empty() {
            v.remove("scope");
        } else {
            v.insert("scope".into(), scopes);
        }
        v.insert("service".into(), vec![self.service.clone()]);
        u.raw_query = gourl::encode_query(&v);
        let req = Request::new(Method::GET, &u.string())?;
        let resp = client.do_request(req).await?;
        let mut resp = check_error(resp, &[200]).await?;
        resp.body
            .read_all()
            .await
            .map_err(super::http::NetError::from_io)
    }
}

#[async_trait]
impl RoundTripper for BearerTransport {
    async fn round_trip(&self, req: Request) -> anyhow::Result<Response> {
        let res = self.send_request(req.clone()).await?;
        // If we hit a WWW-Authenticate challenge, it might be due to expired tokens or
        // insufficient scope.
        let challenges = response_challenges(&res);
        if !challenges.is_empty() {
            drop(res);
            {
                let mut st = self.lock();
                let got: BTreeSet<String> = st.scopes.iter().cloned().collect();
                let mut new_scopes: Vec<String> = Vec::new();
                for wac in &challenges {
                    if let Some(want) = wac.parameters.get("scope")
                        && !got.contains(want)
                    {
                        new_scopes.push(want.clone());
                    }
                }
                // Missing scopes first: some registries only look at the first one.
                new_scopes.extend(st.scopes.iter().cloned());
                st.scopes = new_scopes;
            }
            self.refresh().await?;
            return self.send_request(req).await;
        }
        Ok(res)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_json() {
        let t = Token::from_json(br#"{"token":"a","expires_in":300}"#).unwrap();
        assert_eq!(t.token, "a");
        assert_eq!(t.expires_in, 300);
        assert_eq!(
            Token::from_json(b"<html>").unwrap_err().to_string(),
            "invalid character '<' looking for beginning of value"
        );
        assert_eq!(
            Token::from_json(br#"{"expires_in":1.5}"#)
                .unwrap_err()
                .to_string(),
            "json: cannot unmarshal number 1.5 into Go struct field Token.expires_in of type int"
        );
    }
}
