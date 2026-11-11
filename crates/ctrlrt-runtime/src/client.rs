//! Direct API client (`client.New` typed client + client-go `rest.Request` error semantics) and
//! the lazy discovery-based RESTMapper (`apiutil.NewDynamicRESTMapper`).

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use ctrlrt::error::{ApiError, NoKindMatchError, StatusCause, StatusDetails};
use ctrlrt::{ClientBackend, DeleteOptions, ListOptions, Patch, ResourceInfo};
use http::{Method, Request};
use http_body_util::BodyExt;
use kube::client::Body;
use serde_json::Value;

use crate::Result;

// ---------------------------------------------------------------------------------------------
// Raw REST client with client-go error mapping
// ---------------------------------------------------------------------------------------------

/// A thin request helper over `kube::Client` reproducing client-go's `rest.Request`
/// result handling (`transformResponse`, `newUnstructuredResponseError`, `Result.Error()`).
#[derive(Clone)]
pub struct RestClient {
    client: kube::Client,
    base: String,
}

/// Description of a request, used for error messages like client-go.
#[derive(Clone, Debug, Default)]
pub struct RequestInfo {
    /// API group of the resource (for generic error details).
    pub group: String,
    /// Resource (plural) name, empty for non-resource requests.
    pub resource: String,
    /// Object name, if any.
    pub name: String,
}

impl RestClient {
    pub fn new(client: kube::Client) -> Self {
        Self {
            client,
            base: String::new(),
        }
    }

    /// Set the server base URL used in transport error messages (`Get "<url>": ...`).
    pub fn with_base(mut self, base: impl Into<String>) -> Self {
        self.base = base.into().trim_end_matches('/').to_string();
        self
    }

    pub fn kube_client(&self) -> &kube::Client {
        &self.client
    }

    /// GET a path and decode JSON.
    pub async fn get_json(&self, path: &str, info: Option<&RequestInfo>) -> Result<Value> {
        self.request(Method::GET, path, None, None, None, info).await
    }

    /// Perform a request; returns the decoded JSON body (Null for an empty body).
    pub async fn request(
        &self,
        method: Method,
        path_and_query: &str,
        body: Option<Vec<u8>>,
        content_type: Option<&str>,
        accept: Option<&str>,
        info: Option<&RequestInfo>,
    ) -> Result<Value> {
        let (status, headers, bytes) = self
            .raw(method.clone(), path_and_query, body, content_type, accept)
            .await?;
        handle_warnings(&headers);
        if !(200..=206).contains(&status) {
            return Err(response_error(status, &method, &headers, &bytes, info).into());
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Perform a request and return the raw status, headers and body.
    pub async fn raw(
        &self,
        method: Method,
        path_and_query: &str,
        body: Option<Vec<u8>>,
        content_type: Option<&str>,
        accept: Option<&str>,
    ) -> Result<(u16, http::HeaderMap, Bytes)> {
        let mut builder = Request::builder().method(method.clone()).uri(path_and_query);
        builder = builder.header(http::header::ACCEPT, accept.unwrap_or("application/json, */*"));
        if let Some(ct) = content_type {
            builder = builder.header(http::header::CONTENT_TYPE, ct);
        }
        let req = builder
            .body(Body::from(body.unwrap_or_default()))
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let res = self
            .client
            .send(req)
            .await
            .map_err(|e| transport_error(&method, &self.base, path_and_query, &e))?;
        let status = res.status().as_u16();
        let headers = res.headers().clone();
        let bytes = res
            .into_body()
            .collect()
            .await
            .map_err(|e| {
                anyhow::anyhow!(
                    "unexpected error when reading response body. Please retry. Original error: {e}"
                )
            })?
            .to_bytes();
        Ok((status, headers, bytes))
    }
}

/// `url.Error` rendering: `Get "https://host/path": <cause>`.
fn transport_error(method: &Method, base: &str, path: &str, e: &kube::Error) -> anyhow::Error {
    let m = method.as_str();
    let op = format!("{}{}", &m[..1], m[1..].to_lowercase());
    // innermost cause gives the most Go-like text (e.g. "connection refused")
    let mut cause: &dyn std::error::Error = e;
    let mut text = e.to_string();
    while let Some(src) = cause.source() {
        cause = src;
        text = match src.downcast_ref::<std::io::Error>() {
            Some(io) => crate::config::io_error_text(io),
            None => src.to_string(),
        };
    }
    if let Some(t) = find_timeout(e) {
        text = t;
    }
    anyhow::anyhow!("{} {}: {}", op, ctrlrt::error::go_quote(&format!("{base}{path}")), text)
}

fn find_timeout(e: &kube::Error) -> Option<String> {
    let mut cur: Option<&dyn std::error::Error> = Some(e);
    while let Some(c) = cur {
        if c.downcast_ref::<crate::config::ClientTimeoutError>().is_some() {
            return Some(c.to_string());
        }
        cur = c.source();
    }
    None
}

/// `KubeAPIWarningLogger` (controller-runtime surfaces server warnings, no deduplication).
fn handle_warnings(headers: &http::HeaderMap) {
    for v in headers.get_all(http::header::WARNING) {
        let Ok(s) = v.to_str() else { continue };
        for (code, msg) in parse_warning_header(s) {
            if code == 299 && !msg.is_empty() {
                ctrlrt::Logger::root().info(&msg, ctrlrt::kv![]);
            }
        }
    }
}

/// Parse `Warning` header values (`299 - "message"`, comma separated).
fn parse_warning_header(s: &str) -> Vec<(u16, String)> {
    let mut out = Vec::new();
    let mut rest = s.trim();
    while !rest.is_empty() {
        let Some(sp) = rest.find(' ') else { break };
        let code: u16 = rest[..sp].parse().unwrap_or(0);
        rest = rest[sp + 1..].trim_start();
        // agent
        let Some(sp) = rest.find(' ') else { break };
        rest = rest[sp + 1..].trim_start();
        if !rest.starts_with('"') {
            break;
        }
        let mut msg = String::new();
        let mut chars = rest[1..].char_indices();
        let mut end = None;
        while let Some((i, c)) = chars.next() {
            match c {
                '\\' => {
                    if let Some((_, n)) = chars.next() {
                        msg.push(n);
                    }
                }
                '"' => {
                    end = Some(i + 2);
                    break;
                }
                c => msg.push(c),
            }
        }
        out.push((code, msg));
        let Some(end) = end else { break };
        rest = rest[end..].trim_start();
        // optional date
        if rest.starts_with('"')
            && let Some(close) = rest[1..].find('"')
        {
            rest = rest[close + 2..].trim_start();
        }
        rest = rest.trim_start_matches(',').trim_start();
    }
    out
}

fn is_text_response(headers: &http::HeaderMap) -> bool {
    match headers.get(http::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) {
        None => true,
        Some("") => true,
        Some(ct) => ct
            .split(';')
            .next()
            .map(|m| m.trim().to_ascii_lowercase().starts_with("text/"))
            .unwrap_or(false),
    }
}

/// Build the error for a non-2xx response exactly like client-go: decode a `metav1.Status`
/// body when possible (`Result.Error()`), otherwise `NewGenericServerResponse`.
pub(crate) fn response_error(
    status: u16,
    method: &Method,
    headers: &http::HeaderMap,
    body: &[u8],
    info: Option<&RequestInfo>,
) -> ApiError {
    if !body.is_empty()
        && let Ok(v) = serde_json::from_slice::<Value>(body)
        && v.get("kind").and_then(Value::as_str) == Some("Status")
        && v.get("status").and_then(Value::as_str) == Some("Failure")
    {
        let details = v
            .get("details")
            .filter(|d| d.is_object())
            .and_then(|d| serde_json::from_value::<StatusDetails>(d.clone()).ok());
        return ApiError {
            status: "Failure".into(),
            message: v.get("message").and_then(Value::as_str).unwrap_or("").into(),
            reason: v.get("reason").and_then(Value::as_str).unwrap_or("").into(),
            details,
            code: v.get("code").and_then(Value::as_u64).unwrap_or(0) as u16,
        };
    }
    let mut text = body;
    if text.len() > 1024 {
        // maxUnstructuredResponseTextBytes
        text = &text[..1024];
    }
    let server_message = if is_text_response(headers) {
        String::from_utf8_lossy(text).trim().to_string()
    } else {
        "unknown".to_string()
    };
    let retry_after = headers
        .get(http::header::RETRY_AFTER)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<i32>().ok())
        .unwrap_or(0);
    let info = info.cloned().unwrap_or_default();
    generic_server_response(
        status,
        method.as_str(),
        &info.group,
        &info.resource,
        &info.name,
        &server_message,
        retry_after,
    )
}

/// `apierrors.NewGenericServerResponse(code, verb, gr, name, serverMessage, retryAfter, true)`.
pub fn generic_server_response(
    code: u16,
    verb: &str,
    group: &str,
    resource: &str,
    name: &str,
    server_message: &str,
    retry_after_seconds: i32,
) -> ApiError {
    let mut reason = "";
    let mut message =
        format!("the server responded with the status code {code} but did not return more information");
    match code {
        409 => {
            reason = if verb == "POST" { "AlreadyExists" } else { "Conflict" };
            message = "the server reported a conflict".into();
        }
        404 => {
            reason = "NotFound";
            message = "the server could not find the requested resource".into();
        }
        400 => {
            reason = "BadRequest";
            message = "the server rejected our request for an unknown reason".into();
        }
        401 => {
            reason = "Unauthorized";
            message = "the server has asked for the client to provide credentials".into();
        }
        403 => {
            reason = "Forbidden";
            message = server_message.into();
        }
        406 => {
            reason = "NotAcceptable";
            message = if server_message.is_empty() || server_message == "unknown" {
                "the server was unable to respond with a content type that the client supports".into()
            } else {
                server_message.into()
            };
        }
        415 => {
            reason = "UnsupportedMediaType";
            message = server_message.into();
        }
        405 => {
            reason = "MethodNotAllowed";
            message = "the server does not allow this method on the requested resource".into();
        }
        422 => {
            reason = "Invalid";
            message = "the server rejected our request due to an error in our request".into();
        }
        503 => {
            reason = "ServiceUnavailable";
            message = "the server is currently unable to handle the request".into();
        }
        504 => {
            reason = "Timeout";
            message = "the server was unable to return a response in the time allotted, but may still be processing the request".into();
        }
        429 => {
            reason = "TooManyRequests";
            message = "the server has received too many requests and has asked us to try again later".into();
        }
        c if c >= 500 => {
            reason = "InternalError";
            message = format!(
                "an error on the server ({}) has prevented the request from succeeding",
                ctrlrt::error::go_quote(server_message)
            );
        }
        _ => {}
    }
    let gr = if group.is_empty() {
        resource.to_string()
    } else {
        format!("{resource}.{group}")
    };
    let gr_empty = group.is_empty() && resource.is_empty();
    if !gr_empty && !name.is_empty() {
        message = format!("{} ({} {} {})", message, verb.to_lowercase(), gr, name);
    } else if !gr_empty {
        message = format!("{} ({} {})", message, verb.to_lowercase(), gr);
    }
    ApiError {
        status: "Failure".into(),
        message,
        reason: reason.into(),
        details: Some(StatusDetails {
            name: name.into(),
            group: group.into(),
            kind: resource.into(),
            uid: String::new(),
            causes: vec![StatusCause {
                type_: "UnexpectedServerResponse".into(),
                message: server_message.into(),
                field: String::new(),
            }],
            retry_after_seconds,
        }),
        code,
    }
}

// ---------------------------------------------------------------------------------------------
// RESTMapper
// ---------------------------------------------------------------------------------------------

/// Result of a RESTMapping lookup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestMapping {
    pub group: String,
    pub version: String,
    pub kind: String,
    /// Resource (plural) name.
    pub resource: String,
    pub namespaced: bool,
}

impl RestMapping {
    /// The mapping as a [`ResourceInfo`].
    pub fn resource_info(&self) -> ResourceInfo {
        ResourceInfo {
            group: self.group.clone(),
            version: self.version.clone(),
            kind: self.kind.clone(),
            plural: self.resource.clone(),
            namespaced: self.namespaced,
        }
    }
}

/// `meta.NoResourceMatchError` (defined in the core so `ctrlrt::error::is_no_match` sees it).
pub use ctrlrt::error::NoResourceMatchError;

/// `apiutil.ErrResourceDiscoveryFailed`
#[derive(Debug)]
pub struct ErrResourceDiscoveryFailed {
    /// GroupVersion string -> error
    pub failures: BTreeMap<String, Arc<anyhow::Error>>,
}

impl fmt::Display for ErrResourceDiscoveryFailed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut sub: Vec<String> = self
            .failures
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect();
        sub.sort();
        write!(
            f,
            "unable to retrieve the complete list of server APIs: {}",
            sub.join(", ")
        )
    }
}

impl std::error::Error for ErrResourceDiscoveryFailed {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        // Go's Unwrap() []error; expose the first failure so `errors.Is`-like checks can see it.
        self.failures
            .values()
            .next()
            .map(|e| e.as_ref().as_ref() as &(dyn std::error::Error + 'static))
    }
}

#[derive(Clone, Debug)]
struct ApiResource {
    name: String,
    kind: String,
    namespaced: bool,
}

#[derive(Clone, Debug, Default)]
struct GroupResources {
    /// discovery versions, in priority order
    versions: Vec<String>,
    versioned_resources: HashMap<String, Vec<ApiResource>>,
}

#[derive(Clone, Debug, Default)]
struct ApiGroup {
    versions: Vec<String>,
    preferred_version: String,
}

#[derive(Default)]
struct MapperState {
    known_groups: HashMap<String, GroupResources>,
    api_groups: HashMap<String, ApiGroup>,
    initial_discovery_done: bool,
}

/// controller-runtime's lazy dynamic RESTMapper: starts empty, discovers a group/version on the
/// first lookup miss, and re-discovers on later misses (so CRDs installed later are found).
pub struct RestMapper {
    rest: RestClient,
    state: tokio::sync::Mutex<MapperState>,
}

const AGGREGATED_ACCEPT: &str = "application/json;g=apidiscovery.k8s.io;v=v2;as=APIGroupDiscoveryList,application/json;g=apidiscovery.k8s.io;v=v2beta1;as=APIGroupDiscoveryList,application/json";

impl RestMapper {
    pub fn new(rest: RestClient) -> Self {
        Self {
            rest,
            state: tokio::sync::Mutex::new(MapperState::default()),
        }
    }

    fn lookup(st: &MapperState, group: &str, kind: &str, version: &str) -> Option<RestMapping> {
        let gr = st.known_groups.get(group)?;
        if !gr.versions.iter().any(|v| v == version) {
            return None;
        }
        let resources = gr.versioned_resources.get(version)?;
        resources
            .iter()
            .filter(|r| !r.name.contains('/'))
            .find(|r| r.kind == kind || r.kind.to_lowercase() == kind)
            .map(|r| RestMapping {
                group: group.to_string(),
                version: version.to_string(),
                kind: r.kind.clone(),
                resource: r.name.clone(),
                namespaced: r.namespaced,
            })
    }

    /// `RESTMapping(GroupKind{group, kind}, version)`.
    pub async fn rest_mapping(&self, group: &str, kind: &str, version: &str) -> Result<RestMapping> {
        let mut st = self.state.lock().await;
        if let Some(m) = Self::lookup(&st, group, kind, version) {
            return Ok(m);
        }
        let versions: Vec<String> = if version.is_empty() {
            vec![]
        } else {
            vec![version.to_string()]
        };
        self.add_known_group_and_reload(&mut st, group, versions).await?;
        Self::lookup(&st, group, kind, version).ok_or_else(|| {
            NoKindMatchError {
                group: group.to_string(),
                kind: kind.to_string(),
                searched_versions: if version.is_empty() {
                    vec![]
                } else {
                    vec![version.to_string()]
                },
            }
            .into()
        })
    }

    /// `apiutil.IsGVKNamespaced`
    pub async fn is_namespaced(&self, res: &ResourceInfo) -> Result<bool> {
        match self.rest_mapping(&res.group, &res.kind, &res.version).await {
            Ok(m) => Ok(m.namespaced),
            Err(e) => {
                let msg = format!("failed to get restmapping: {e}");
                Err(ctrlrt::error::wrap(e, msg))
            }
        }
    }

    async fn add_known_group_and_reload(
        &self,
        st: &mut MapperState,
        group: &str,
        mut versions: Vec<String>,
    ) -> Result<()> {
        if versions.is_empty() || !st.initial_discovery_done {
            let (api_group, did_aggregated) = self.find_api_group(st, group).await?;
            if let Some(g) = &api_group
                && versions.is_empty()
            {
                versions = g.versions.clone();
            }
            if did_aggregated {
                let mut failures = BTreeMap::new();
                for v in &versions {
                    let known = st
                        .known_groups
                        .get(group)
                        .is_some_and(|g| g.versioned_resources.contains_key(v));
                    if !known {
                        failures.insert(
                            gv_string(group, v),
                            Arc::new(anyhow::Error::new(NoResourceMatchError {
                                group: group.into(),
                                version: v.clone(),
                                resource: String::new(),
                            })),
                        );
                    }
                }
                if !failures.is_empty() {
                    return Err(ErrResourceDiscoveryFailed { failures }.into());
                }
                return Ok(());
            }
        }

        let gvr = match self.fetch_group_version_resources(st, group, &versions).await {
            Ok(g) => g,
            Err(e) => {
                let msg = format!("failed to get API group resources: {e}");
                return Err(ctrlrt::error::wrap(e, msg));
            }
        };
        Self::add_resources(st, gvr);
        Ok(())
    }

    fn add_resources(st: &mut MapperState, gvr: Vec<(String, String, Vec<ApiResource>)>) {
        for (group, version, resources) in gvr {
            let preferred = st
                .api_groups
                .get(&group)
                .map(|g| g.preferred_version.clone())
                .unwrap_or_default();
            let gr = st.known_groups.entry(group.clone()).or_default();
            gr.versioned_resources.insert(version.clone(), resources);
            if !gr.versions.contains(&version) {
                if preferred == version {
                    gr.versions.insert(0, version);
                } else {
                    gr.versions.push(version);
                }
            }
        }
    }

    async fn find_api_group(
        &self,
        st: &mut MapperState,
        group: &str,
    ) -> Result<(Option<ApiGroup>, bool)> {
        if let Some(g) = st.api_groups.get(group) {
            return Ok((Some(g.clone()), false));
        }
        let (groups, resources, aggregated) = match self.groups_and_maybe_resources().await {
            Ok(r) => r,
            Err(e) => {
                let msg = format!("failed to get server groups: {e}");
                return Err(ctrlrt::error::wrap(e, msg));
            }
        };
        if groups.is_empty() {
            return Err(anyhow::anyhow!("received an empty API groups list"));
        }
        st.initial_discovery_done = true;
        for (name, g) in groups {
            st.api_groups.insert(name, g);
        }
        let did_aggregated = aggregated && !resources.is_empty();
        if did_aggregated {
            Self::add_resources(st, resources);
        }
        Ok((st.api_groups.get(group).cloned(), did_aggregated))
    }

    /// `discovery.GroupsAndMaybeResources()`: aggregated discovery when the server supports it.
    #[allow(clippy::type_complexity)]
    async fn groups_and_maybe_resources(
        &self,
    ) -> Result<(
        Vec<(String, ApiGroup)>,
        Vec<(String, String, Vec<ApiResource>)>,
        bool,
    )> {
        let mut groups = Vec::new();
        let mut resources = Vec::new();
        let mut aggregated = true;
        for path in ["/api", "/apis"] {
            let v = match self
                .rest
                .request(Method::GET, path, None, None, Some(AGGREGATED_ACCEPT), None)
                .await
            {
                Ok(v) => v,
                // the legacy API may legitimately be absent behind some proxies
                Err(e) if path == "/api" && ctrlrt::error::is_not_found(&e) => continue,
                Err(e) => return Err(e),
            };
            match v.get("kind").and_then(Value::as_str) {
                Some("APIGroupDiscoveryList") => {
                    for item in v.get("items").and_then(Value::as_array).into_iter().flatten() {
                        let name = item
                            .pointer("/metadata/name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let mut g = ApiGroup::default();
                        for ver in item.get("versions").and_then(Value::as_array).into_iter().flatten() {
                            let version = ver.get("version").and_then(Value::as_str).unwrap_or("").to_string();
                            if version.is_empty() {
                                continue;
                            }
                            g.versions.push(version.clone());
                            if ver.get("freshness").and_then(Value::as_str) == Some("Stale") {
                                continue;
                            }
                            let mut rs = Vec::new();
                            for r in ver.get("resources").and_then(Value::as_array).into_iter().flatten() {
                                let rname = r.get("resource").and_then(Value::as_str).unwrap_or("").to_string();
                                let kind = r
                                    .pointer("/responseKind/kind")
                                    .and_then(Value::as_str)
                                    .unwrap_or("")
                                    .to_string();
                                let namespaced = r.get("scope").and_then(Value::as_str) == Some("Namespaced");
                                rs.push(ApiResource {
                                    name: rname,
                                    kind,
                                    namespaced,
                                });
                            }
                            resources.push((name.clone(), version, rs));
                        }
                        g.preferred_version = g.versions.first().cloned().unwrap_or_default();
                        groups.push((name, g));
                    }
                }
                Some("APIVersions") => {
                    aggregated = false;
                    let versions: Vec<String> = v
                        .get("versions")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|x| x.as_str().map(String::from))
                        .collect();
                    groups.push((
                        String::new(),
                        ApiGroup {
                            preferred_version: versions.first().cloned().unwrap_or_default(),
                            versions,
                        },
                    ));
                }
                Some("APIGroupList") => {
                    aggregated = false;
                    for g in v.get("groups").and_then(Value::as_array).into_iter().flatten() {
                        let name = g.get("name").and_then(Value::as_str).unwrap_or("").to_string();
                        let versions: Vec<String> = g
                            .get("versions")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|x| x.get("version").and_then(Value::as_str).map(String::from))
                            .collect();
                        let preferred = g
                            .pointer("/preferredVersion/version")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        groups.push((
                            name,
                            ApiGroup {
                                versions,
                                preferred_version: preferred,
                            },
                        ));
                    }
                }
                _ => {}
            }
        }
        Ok((groups, resources, aggregated))
    }

    async fn fetch_group_version_resources(
        &self,
        st: &mut MapperState,
        group: &str,
        versions: &[String],
    ) -> Result<Vec<(String, String, Vec<ApiResource>)>> {
        let mut out = Vec::new();
        let mut failures = BTreeMap::new();
        for version in versions {
            let path = if group.is_empty() {
                format!("/api/{version}")
            } else {
                format!("/apis/{group}/{version}")
            };
            match self.rest.get_json(&path, None).await {
                Err(e) if ctrlrt::error::is_not_found(&e) => {
                    // If the version is not found, we remove the group from the cache so it
                    // gets refreshed on the next call.
                    if st
                        .api_groups
                        .get(group)
                        .is_some_and(|g| g.versions.iter().any(|v| v == version))
                    {
                        st.api_groups.remove(group);
                    }
                    if st
                        .known_groups
                        .get(group)
                        .is_some_and(|g| g.versioned_resources.contains_key(version))
                    {
                        st.known_groups.remove(group);
                    }
                    continue;
                }
                Err(e) => {
                    failures.insert(gv_string(group, version), Arc::new(e));
                }
                Ok(v) => {
                    let rs = v
                        .get("resources")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|r| ApiResource {
                            name: r.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                            kind: r.get("kind").and_then(Value::as_str).unwrap_or("").to_string(),
                            namespaced: r.get("namespaced").and_then(Value::as_bool).unwrap_or(false),
                        })
                        .collect();
                    out.push((group.to_string(), version.clone(), rs));
                }
            }
        }
        if !failures.is_empty() {
            return Err(ErrResourceDiscoveryFailed { failures }.into());
        }
        Ok(out)
    }
}

fn gv_string(group: &str, version: &str) -> String {
    if group.is_empty() {
        version.to_string()
    } else {
        format!("{group}/{version}")
    }
}

// ---------------------------------------------------------------------------------------------
// KubeBackend
// ---------------------------------------------------------------------------------------------

type MappingCache = HashMap<(String, String, String), RestMapping>;

/// Direct (uncached) `ClientBackend` talking to the API server (`client.New(...)` typed client).
#[derive(Clone)]
pub struct KubeBackend {
    rest: RestClient,
    mapper: Arc<RestMapper>,
    mappings: Arc<Mutex<MappingCache>>,
}

/// `rest.IsValidPathSegmentName`
fn is_valid_path_segment_name(name: &str) -> Vec<String> {
    for illegal in [".", ".."] {
        if name == illegal {
            return vec![format!("may not be '{illegal}'")];
        }
    }
    let mut errors = Vec::new();
    for illegal in ["/", "%"] {
        if name.contains(illegal) {
            errors.push(format!("may not contain '{illegal}'"));
        }
    }
    errors
}

fn validate_namespace(ns: &str) -> Result<()> {
    let msgs = is_valid_path_segment_name(ns);
    if !msgs.is_empty() {
        return Err(anyhow::anyhow!(
            "invalid namespace {}: {}",
            ctrlrt::error::go_quote(ns),
            go_string_slice(&msgs)
        ));
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(anyhow::anyhow!("resource name may not be empty"));
    }
    let msgs = is_valid_path_segment_name(name);
    if !msgs.is_empty() {
        return Err(anyhow::anyhow!(
            "invalid resource name {}: {}",
            ctrlrt::error::go_quote(name),
            go_string_slice(&msgs)
        ));
    }
    Ok(())
}

/// Go's `%v` of a `[]string`.
fn go_string_slice(v: &[String]) -> String {
    format!("[{}]", v.join(" "))
}

/// Remove `apiVersion`/`kind` like client-go's `WithoutVersionDecoder` does for typed objects.
fn strip_type_meta(mut v: Value) -> Value {
    if let Value::Object(m) = &mut v {
        m.remove("apiVersion");
        m.remove("kind");
    }
    v
}

impl KubeBackend {
    /// Backend with its own lazy RESTMapper.
    pub fn new(client: kube::Client) -> Self {
        let rest = RestClient::new(client);
        let mapper = Arc::new(RestMapper::new(rest.clone()));
        Self::with_mapper(rest, mapper)
    }

    /// Backend sharing a RESTMapper (the manager's).
    pub fn with_mapper(rest: RestClient, mapper: Arc<RestMapper>) -> Self {
        Self {
            rest,
            mapper,
            mappings: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn mapper(&self) -> &Arc<RestMapper> {
        &self.mapper
    }

    pub fn rest(&self) -> &RestClient {
        &self.rest
    }

    /// Resolve (and cache, like the client's `resourceByType`) the REST mapping of a type.
    pub async fn mapping(&self, res: &ResourceInfo) -> Result<RestMapping> {
        let key = (res.group.clone(), res.version.clone(), res.kind.clone());
        if let Some(m) = self
            .mappings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
        {
            return Ok(m.clone());
        }
        let m = self
            .mapper
            .rest_mapping(&res.group, &res.kind, &res.version)
            .await?;
        self.mappings
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key, m.clone());
        Ok(m)
    }

    fn path(m: &RestMapping, ns: &str, name: Option<&str>, sub: Option<&str>) -> String {
        let mut p = if m.group.is_empty() {
            format!("/api/{}", m.version)
        } else {
            format!("/apis/{}/{}", m.group, m.version)
        };
        if m.namespaced && !ns.is_empty() {
            p.push_str("/namespaces/");
            p.push_str(ns);
        }
        p.push('/');
        p.push_str(&m.resource);
        if let Some(n) = name {
            p.push('/');
            p.push_str(n);
        }
        if let Some(s) = sub {
            p.push('/');
            p.push_str(s);
        }
        p
    }

    fn info(m: &RestMapping, name: &str) -> RequestInfo {
        RequestInfo {
            group: m.group.clone(),
            resource: m.resource.clone(),
            name: name.to_string(),
        }
    }

    async fn do_json(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
        content_type: Option<&str>,
        info: &RequestInfo,
    ) -> Result<Value> {
        let body = body.map(serde_json::to_vec).transpose().map_err(|e| anyhow::anyhow!("{e}"))?;
        let ct = content_type.or(body.as_ref().map(|_| "application/json"));
        self.rest
            .request(method, path, body, ct, None, Some(info))
            .await
    }

    fn obj_ns_name(obj: &Value) -> (String, String) {
        ctrlrt::client::value_key(obj)
    }

    /// List with a raw label selector (used by tests / helpers).
    pub async fn list_raw(
        &self,
        res: &ResourceInfo,
        namespace: &str,
        label_selector: Option<&str>,
        field_selector: Option<&str>,
    ) -> Result<Value> {
        let m = self.mapping(res).await?;
        let ns = if m.namespaced { namespace } else { "" };
        validate_namespace(ns)?;
        let mut path = Self::path(&m, ns, None, None);
        let query = {
            let mut q = form_urlencoded::Serializer::new(String::new());
            let mut has = false;
            if let Some(fs) = field_selector.filter(|s| !s.is_empty()) {
                q.append_pair("fieldSelector", fs);
                has = true;
            }
            if let Some(ls) = label_selector.filter(|s| !s.is_empty()) {
                q.append_pair("labelSelector", ls);
                has = true;
            }
            has.then(|| q.finish())
        };
        if let Some(q) = query {
            path.push('?');
            path.push_str(&q);
        }
        self.do_json(Method::GET, &path, None, None, &Self::info(&m, ""))
            .await
    }
}

#[async_trait::async_trait]
impl ClientBackend for KubeBackend {
    async fn get(&self, res: &ResourceInfo, namespace: &str, name: &str) -> Result<Value> {
        let m = self.mapping(res).await?;
        let ns = if m.namespaced { namespace } else { "" };
        validate_namespace(ns)?;
        validate_name(name)?;
        let path = Self::path(&m, ns, Some(name), None);
        let v = self
            .do_json(Method::GET, &path, None, None, &Self::info(&m, name))
            .await?;
        Ok(strip_type_meta(v))
    }

    async fn list(&self, res: &ResourceInfo, opts: &ListOptions) -> Result<Vec<Value>> {
        let ls = opts.label_selector();
        let v = self
            .list_raw(res, &opts.namespace, ls.as_deref(), None)
            .await?;
        Ok(match v {
            Value::Object(mut m) => match m.remove("items") {
                Some(Value::Array(items)) => items.into_iter().map(strip_type_meta).collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        })
    }

    async fn create(&self, res: &ResourceInfo, obj: Value) -> Result<Value> {
        let m = self.mapping(res).await?;
        let (ns, _) = Self::obj_ns_name(&obj);
        let ns = if m.namespaced { ns } else { String::new() };
        validate_namespace(&ns)?;
        let path = Self::path(&m, &ns, None, None);
        let v = self
            .do_json(Method::POST, &path, Some(&obj), None, &Self::info(&m, ""))
            .await?;
        Ok(strip_type_meta(v))
    }

    async fn update(&self, res: &ResourceInfo, obj: Value) -> Result<Value> {
        let m = self.mapping(res).await?;
        let (ns, name) = Self::obj_ns_name(&obj);
        let ns = if m.namespaced { ns } else { String::new() };
        validate_namespace(&ns)?;
        validate_name(&name)?;
        let path = Self::path(&m, &ns, Some(&name), None);
        let v = self
            .do_json(Method::PUT, &path, Some(&obj), None, &Self::info(&m, &name))
            .await?;
        Ok(strip_type_meta(v))
    }

    async fn update_subresource(
        &self,
        res: &ResourceInfo,
        subresource: &str,
        obj: Value,
    ) -> Result<Value> {
        let m = self.mapping(res).await?;
        let (ns, name) = Self::obj_ns_name(&obj);
        let ns = if m.namespaced { ns } else { String::new() };
        validate_namespace(&ns)?;
        validate_name(&name)?;
        let path = Self::path(&m, &ns, Some(&name), Some(subresource));
        let v = self
            .do_json(Method::PUT, &path, Some(&obj), None, &Self::info(&m, &name))
            .await?;
        Ok(strip_type_meta(v))
    }

    async fn patch(
        &self,
        res: &ResourceInfo,
        namespace: &str,
        name: &str,
        patch: &Patch,
    ) -> Result<Value> {
        self.patch_subresource_impl(res, None, namespace, name, patch)
            .await
    }

    async fn patch_subresource(
        &self,
        res: &ResourceInfo,
        subresource: &str,
        namespace: &str,
        name: &str,
        patch: &Patch,
    ) -> Result<Value> {
        self.patch_subresource_impl(res, Some(subresource), namespace, name, patch)
            .await
    }

    async fn delete(
        &self,
        res: &ResourceInfo,
        namespace: &str,
        name: &str,
        opts: &DeleteOptions,
    ) -> Result<()> {
        let m = self.mapping(res).await?;
        let ns = if m.namespaced { namespace } else { "" };
        validate_namespace(ns)?;
        validate_name(name)?;
        let path = Self::path(&m, ns, Some(name), None);
        let mut body = serde_json::Map::new();
        if let Some(g) = opts.grace_period_seconds {
            body.insert("gracePeriodSeconds".into(), Value::from(g));
        }
        if opts.precondition_uid.is_some() || opts.precondition_resource_version.is_some() {
            let mut pre = serde_json::Map::new();
            if let Some(u) = &opts.precondition_uid {
                pre.insert("uid".into(), Value::String(u.clone()));
            }
            if let Some(rv) = &opts.precondition_resource_version {
                pre.insert("resourceVersion".into(), Value::String(rv.clone()));
            }
            body.insert("preconditions".into(), Value::Object(pre));
        }
        if let Some(p) = opts.propagation_policy {
            body.insert("propagationPolicy".into(), Value::String(p.as_str().into()));
        }
        self.do_json(
            Method::DELETE,
            &path,
            Some(&Value::Object(body)),
            None,
            &Self::info(&m, name),
        )
        .await?;
        Ok(())
    }
}

impl KubeBackend {
    async fn patch_subresource_impl(
        &self,
        res: &ResourceInfo,
        sub: Option<&str>,
        namespace: &str,
        name: &str,
        patch: &Patch,
    ) -> Result<Value> {
        let m = self.mapping(res).await?;
        let ns = if m.namespaced { namespace } else { "" };
        validate_namespace(ns)?;
        validate_name(name)?;
        let path = Self::path(&m, ns, Some(name), sub);
        let (ct, body) = match patch {
            Patch::Merge(v) => ("application/merge-patch+json", v),
            Patch::Strategic(v) => ("application/strategic-merge-patch+json", v),
        };
        let v = self
            .do_json(Method::PATCH, &path, Some(body), Some(ct), &Self::info(&m, name))
            .await?;
        Ok(strip_type_meta(v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_not_found_message() {
        let e = generic_server_response(404, "GET", "", "configmaps", "foo", "", 0);
        assert_eq!(
            e.message,
            "the server could not find the requested resource (get configmaps foo)"
        );
        assert_eq!(e.reason, "NotFound");
        assert!(ctrlrt::error::is_not_found(&anyhow::Error::new(e)));
        let e = generic_server_response(409, "POST", "apps", "deployments", "", "", 0);
        assert_eq!(e.reason, "AlreadyExists");
        assert_eq!(e.message, "the server reported a conflict (post deployments.apps)");
        let e = generic_server_response(500, "GET", "", "", "", "boom", 0);
        assert_eq!(
            e.message,
            r#"an error on the server ("boom") has prevented the request from succeeding"#
        );
    }

    #[test]
    fn status_body_is_decoded() {
        let body = br#"{"kind":"Status","apiVersion":"v1","metadata":{},"status":"Failure","message":"configmaps \"x\" not found","reason":"NotFound","details":{"name":"x","kind":"configmaps"},"code":404}"#;
        let mut h = http::HeaderMap::new();
        h.insert(http::header::CONTENT_TYPE, "application/json".parse().unwrap());
        let e = response_error(404, &Method::GET, &h, body, None);
        assert_eq!(e.message, r#"configmaps "x" not found"#);
        assert_eq!(e.code, 404);
        assert_eq!(e.details.as_ref().unwrap().name, "x");
    }

    #[test]
    fn text_body_generic_error() {
        let mut h = http::HeaderMap::new();
        h.insert(http::header::CONTENT_TYPE, "text/plain".parse().unwrap());
        let e = response_error(
            403,
            &Method::GET,
            &h,
            b"forbidden: nope\n",
            Some(&RequestInfo {
                group: String::new(),
                resource: "secrets".into(),
                name: "s".into(),
            }),
        );
        assert_eq!(e.reason, "Forbidden");
        assert_eq!(e.message, "forbidden: nope (get secrets s)");
    }

    #[test]
    fn errors_render_like_go() {
        let e = NoResourceMatchError {
            group: "metallb.io".into(),
            version: "v1beta1".into(),
            resource: String::new(),
        };
        let mut failures = BTreeMap::new();
        failures.insert("metallb.io/v1beta1".to_string(), Arc::new(anyhow::Error::new(e)));
        let d = ErrResourceDiscoveryFailed { failures };
        assert_eq!(
            d.to_string(),
            "unable to retrieve the complete list of server APIs: metallb.io/v1beta1: no matches for metallb.io/v1beta1, Resource="
        );
        assert_eq!(validate_name("").unwrap_err().to_string(), "resource name may not be empty");
        assert_eq!(
            validate_name("a/b").unwrap_err().to_string(),
            r#"invalid resource name "a/b": [may not contain '/']"#
        );
    }

    #[test]
    fn parses_warning_headers() {
        assert_eq!(
            parse_warning_header(r#"299 - "v1beta1 is deprecated", 299 - "second \"q\"""#),
            vec![
                (299, "v1beta1 is deprecated".to_string()),
                (299, "second \"q\"".to_string())
            ]
        );
    }
}
