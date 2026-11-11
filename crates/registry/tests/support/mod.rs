//! In-process fake OCI registry (axum on 127.0.0.1) with token / basic auth, manifests
//! (images + indexes) and blobs, plus request logging and fault injection.

#![allow(dead_code)]

use std::collections::{BTreeMap, HashMap};
use std::io::Write;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::body::{Body, Bytes};
use axum::extract::State;
use axum::http::{HeaderMap, Method, Response, StatusCode, Uri};
use sha2::{Digest, Sha256};

pub fn sha256(b: &[u8]) -> String {
    format!("sha256:{}", hex::encode(Sha256::digest(b)))
}

#[derive(Clone, Debug)]
pub struct Logged {
    pub method: String,
    pub path: String,
    pub query: String,
    pub authorization: String,
    pub user_agent: String,
    pub accept: String,
    pub content_type: String,
    pub body: String,
}

#[derive(Clone, Debug, Default)]
pub enum AuthMode {
    #[default]
    None,
    Basic {
        user: String,
        pass: String,
    },
    Bearer {
        service: String,
        /// Credentials the token endpoint requires; `None` = anonymous tokens.
        creds: Option<(String, String)>,
        /// Omit the realm parameter from the challenge.
        omit_realm: bool,
        /// Token endpoint response body override.
        token_body: Option<String>,
        /// Use `access_token` instead of `token`.
        access_token: bool,
    },
}

/// A hook may answer a request instead of the default logic.
pub type Hook = Arc<dyn Fn(&Logged, usize) -> Option<Response<Body>> + Send + Sync>;

#[derive(Default)]
pub struct Inner {
    pub manifests: HashMap<(String, String), (String, Bytes)>,
    pub blobs: HashMap<String, Bytes>,
    pub auth: AuthMode,
    pub issued: Vec<String>,
    /// Tokens that will be rejected once (forcing a re-auth with a new scope).
    pub reject_once: Vec<String>,
    pub log: Vec<Logged>,
    pub hooks: Vec<Hook>,
    pub counter: usize,
}

#[derive(Clone, Default)]
pub struct FakeRegistry {
    pub inner: Arc<Mutex<Inner>>,
    pub addr: Option<SocketAddr>,
}

impl FakeRegistry {
    pub async fn start(auth: AuthMode) -> FakeRegistry {
        let inner = Arc::new(Mutex::new(Inner {
            auth,
            ..Default::default()
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().fallback(handle).with_state(inner.clone());
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        FakeRegistry {
            inner,
            addr: Some(addr),
        }
    }

    pub fn host(&self) -> String {
        self.addr.unwrap().to_string()
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap()
    }

    pub fn add_blob(&self, data: &[u8]) -> String {
        let d = sha256(data);
        self.lock().blobs.insert(d.clone(), Bytes::copy_from_slice(data));
        d
    }

    /// Store a manifest under its digest and (optionally) a tag; returns the digest.
    pub fn add_manifest(&self, repo: &str, tag: Option<&str>, media_type: &str, body: &[u8]) -> String {
        let d = sha256(body);
        let mut st = self.lock();
        let v = (media_type.to_string(), Bytes::copy_from_slice(body));
        st.manifests.insert((repo.to_string(), d.clone()), v.clone());
        if let Some(t) = tag {
            st.manifests.insert((repo.to_string(), t.to_string()), v);
        }
        d
    }

    pub fn hook(&self, h: impl Fn(&Logged, usize) -> Option<Response<Body>> + Send + Sync + 'static) {
        self.lock().hooks.push(Arc::new(h));
    }

    pub fn log(&self) -> Vec<Logged> {
        self.lock().log.clone()
    }

    pub fn requests_to(&self, path_prefix: &str) -> Vec<Logged> {
        self.log().into_iter().filter(|l| l.path.starts_with(path_prefix)).collect()
    }

    /// Push an image (config + layers) and return the manifest digest.
    pub fn push_image(
        &self,
        repo: &str,
        tag: Option<&str>,
        media_type: &str,
        config: &[u8],
        layers: &[Vec<u8>],
    ) -> String {
        let cd = self.add_blob(config);
        let (cfg_mt, layer_mt) = if media_type == "application/vnd.oci.image.manifest.v1+json" {
            (
                "application/vnd.oci.image.config.v1+json",
                "application/vnd.oci.image.layer.v1.tar+gzip",
            )
        } else {
            (
                "application/vnd.docker.container.image.v1+json",
                "application/vnd.docker.image.rootfs.diff.tar.gzip",
            )
        };
        let layer_descs: Vec<serde_json::Value> = layers
            .iter()
            .map(|l| {
                let d = self.add_blob(l);
                serde_json::json!({"mediaType": layer_mt, "size": l.len(), "digest": d})
            })
            .collect();
        let m = serde_json::json!({
            "schemaVersion": 2,
            "mediaType": media_type,
            "config": {"mediaType": cfg_mt, "size": config.len(), "digest": cd},
            "layers": layer_descs,
        });
        let body = serde_json::to_vec(&m).unwrap();
        self.add_manifest(repo, tag, media_type, &body)
    }
}

fn header(h: &HeaderMap, k: &str) -> String {
    h.get(k).and_then(|v| v.to_str().ok()).unwrap_or("").to_string()
}

fn json_error(status: StatusCode, code: &str, message: &str, detail: serde_json::Value) -> Response<Body> {
    let body = serde_json::json!({"errors": [{"code": code, "message": message, "detail": detail}]});
    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn unauthorized(inner: &Inner, host: &str, scope: Option<&str>) -> Response<Body> {
    let challenge = match &inner.auth {
        AuthMode::Basic { .. } => "Basic realm=\"fake\"".to_string(),
        AuthMode::Bearer {
            service, omit_realm, ..
        } => {
            let mut c = String::from("Bearer ");
            if !omit_realm {
                c.push_str(&format!("realm=\"http://{host}/token\","));
            }
            c.push_str(&format!("service=\"{service}\""));
            if let Some(s) = scope {
                c.push_str(&format!(",scope=\"{s}\""));
            }
            c
        }
        AuthMode::None => String::new(),
    };
    let mut r = json_error(
        StatusCode::UNAUTHORIZED,
        "UNAUTHORIZED",
        "authentication required",
        serde_json::Value::Null,
    );
    r.headers_mut().insert("WWW-Authenticate", challenge.parse().unwrap());
    r
}

fn basic_ok(authz: &str, user: &str, pass: &str) -> bool {
    let want = format!("{user}:{pass}");
    let mut enc = Vec::new();
    {
        // base64 encode without extra deps
        const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        for chunk in want.as_bytes().chunks(3) {
            let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
            let v = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
            enc.write_all(&[T[(v >> 18) as usize & 63], T[(v >> 12) as usize & 63]]).unwrap();
            enc.push(if chunk.len() > 1 { T[(v >> 6) as usize & 63] } else { b'=' });
            enc.push(if chunk.len() > 2 { T[v as usize & 63] } else { b'=' });
        }
    }
    authz == format!("Basic {}", String::from_utf8(enc).unwrap())
}

async fn handle(
    State(state): State<Arc<Mutex<Inner>>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response<Body> {
    let logged = Logged {
        method: method.to_string(),
        path: uri.path().to_string(),
        query: uri.query().unwrap_or("").to_string(),
        authorization: header(&headers, "authorization"),
        user_agent: header(&headers, "user-agent"),
        accept: header(&headers, "accept"),
        content_type: header(&headers, "content-type"),
        body: String::from_utf8_lossy(&body).into_owned(),
    };
    let host = header(&headers, "host");
    let mut st = state.lock().unwrap();
    st.log.push(logged.clone());
    st.counter += 1;
    let n = st.counter;
    for h in st.hooks.clone() {
        if let Some(r) = h(&logged, n) {
            return r;
        }
    }

    let path = logged.path.clone();
    if path == "/token" {
        let AuthMode::Bearer {
            creds,
            token_body,
            access_token,
            ..
        } = st.auth.clone()
        else {
            return Response::builder().status(404).body(Body::empty()).unwrap();
        };
        if let Some((u, p)) = &creds
            && !basic_ok(&logged.authorization, u, p) {
                return json_error(
                    StatusCode::UNAUTHORIZED,
                    "UNAUTHORIZED",
                    "access to the requested resource is not authorized",
                    serde_json::json!([{"Type": "repository", "Action": "pull"}]),
                );
            }
        if let Some(b) = token_body {
            return Response::builder()
                .status(200)
                .header("Content-Type", "application/json")
                .body(Body::from(b))
                .unwrap();
        }
        let tok = format!("tok-{}", st.issued.len() + 1);
        st.issued.push(tok.clone());
        let key = if access_token { "access_token" } else { "token" };
        let body = serde_json::json!({key: tok, "expires_in": 300});
        return Response::builder()
            .status(200)
            .header("Content-Type", "application/json")
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
    }

    // Auth check for /v2/ endpoints.
    let authorized = match &st.auth {
        AuthMode::None => true,
        AuthMode::Basic { user, pass } => basic_ok(&logged.authorization, user, pass),
        AuthMode::Bearer { .. } => {
            let tok = logged.authorization.strip_prefix("Bearer ").unwrap_or("");
            st.issued.iter().any(|t| t == tok) && !tok.is_empty()
        }
    };
    if !authorized {
        return unauthorized(&st, &host, None);
    }
    if let AuthMode::Bearer { .. } = &st.auth {
        let tok = logged.authorization.strip_prefix("Bearer ").unwrap_or("").to_string();
        if let Some(i) = st.reject_once.iter().position(|t| *t == tok) {
            st.reject_once.remove(i);
            return unauthorized(&st, &host, Some("repository:extra/scope:pull"));
        }
    }

    if path == "/v2/" {
        return Response::builder().status(200).body(Body::from("{}")).unwrap();
    }
    let Some(rest) = path.strip_prefix("/v2/") else {
        return Response::builder().status(404).body(Body::empty()).unwrap();
    };
    let is_head = method == Method::HEAD;
    if let Some(i) = rest.rfind("/manifests/") {
        let repo = &rest[..i];
        let reference = &rest[i + "/manifests/".len()..];
        match st.manifests.get(&(repo.to_string(), reference.to_string())) {
            Some((mt, body)) => {
                let b = Response::builder()
                    .status(200)
                    .header("Content-Type", mt.as_str())
                    .header("Docker-Content-Digest", sha256(body))
                    .header("Content-Length", body.len());
                if is_head {
                    b.body(Body::empty()).unwrap()
                } else {
                    b.body(Body::from(body.clone())).unwrap()
                }
            }
            None => {
                if is_head {
                    Response::builder().status(404).body(Body::empty()).unwrap()
                } else {
                    json_error(
                        StatusCode::NOT_FOUND,
                        "MANIFEST_UNKNOWN",
                        "manifest unknown",
                        serde_json::json!({"Tag": reference}),
                    )
                }
            }
        }
    } else if let Some(i) = rest.rfind("/blobs/") {
        let digest = &rest[i + "/blobs/".len()..];
        match st.blobs.get(digest) {
            Some(b) => Response::builder()
                .status(200)
                .header("Content-Length", b.len())
                .body(if is_head { Body::empty() } else { Body::from(b.clone()) })
                .unwrap(),
            None => json_error(
                StatusCode::NOT_FOUND,
                "BLOB_UNKNOWN",
                "blob unknown to registry",
                serde_json::Value::String(digest.to_string()),
            ),
        }
    } else {
        Response::builder().status(404).body(Body::empty()).unwrap()
    }
}

// ------------------------------------------------------------------ layer helpers

/// Build a gzip-compressed tar layer from (path, typeflag, content) entries.
pub fn gz_layer(entries: &[(&str, u8, &str)]) -> Vec<u8> {
    let tar = tar_bytes(entries);
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(&tar).unwrap();
    enc.finish().unwrap()
}

/// Minimal USTAR writer (names < 100 bytes).
pub fn tar_bytes(entries: &[(&str, u8, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, flag, content) in entries {
        let mut h = [0u8; 512];
        h[..name.len()].copy_from_slice(name.as_bytes());
        let mode = if *flag == b'5' { "0000755\0" } else { "0000644\0" };
        h[100..108].copy_from_slice(mode.as_bytes());
        h[108..116].copy_from_slice(b"0000000\0");
        h[116..124].copy_from_slice(b"0000000\0");
        let size = if *flag == b'0' { content.len() } else { 0 };
        h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
        h[136..148].copy_from_slice(b"00000000000\0");
        h[156] = *flag;
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        h[148..156].copy_from_slice(b"        ");
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        out.extend_from_slice(&h);
        if size > 0 {
            out.extend_from_slice(content.as_bytes());
            out.resize(out.len() + (512 - size % 512) % 512, 0);
        }
    }
    out.resize(out.len() + 1024, 0);
    out
}

pub fn config_json(labels: &BTreeMap<&str, &str>) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "architecture": "amd64",
        "os": "linux",
        "config": {"Labels": labels},
        "rootfs": {"type": "layers", "diff_ids": []},
    }))
    .unwrap()
}
