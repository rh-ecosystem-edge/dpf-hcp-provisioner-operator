//! Port of `github.com/google/go-containerregistry/pkg/v1/remote` (read path:
//! `Head`, `Get`, `Image`, `Descriptor.Image`, config and layer blob fetching).

use std::fmt;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use ::http::Method;
use async_trait::async_trait;
use bytes::Bytes;
use futures::future::BoxFuture;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};
use tokio::sync::OnceCell;

use crate::authn::{Authenticator, Keychain, anonymous};
use crate::gourl::{self, GoUrl};
use crate::goutil;
use crate::name::{Reference, Registry, Repository};
use crate::transport::{
    self, Backoff, BoxAsyncRead, Client, HttpTransport, NetError, Request, RetryTransport,
    RoundTripper, UrlError, UserAgentTransport, check_error,
};
use crate::v1::{self, ConfigFile, Hash, Manifest, MediaType, Platform};

const MANIFEST_LIMIT: usize = 100 * 1024 * 1024;

/// `allManifestMediaTypes`
fn all_manifest_media_types() -> Vec<MediaType> {
    vec![
        MediaType::DOCKER_MANIFEST_SCHEMA1.into(),
        MediaType::DOCKER_MANIFEST_SCHEMA1_SIGNED.into(),
        MediaType::DOCKER_MANIFEST_SCHEMA2.into(),
        MediaType::OCI_MANIFEST_SCHEMA1.into(),
        MediaType::DOCKER_MANIFEST_LIST.into(),
        MediaType::OCI_IMAGE_INDEX.into(),
    ]
}

// ------------------------------------------------------------------ options

/// `remote.Option`
#[derive(Clone)]
pub enum RemoteOption {
    /// `remote.WithAuth`
    Auth(Arc<dyn Authenticator>),
    /// `remote.WithAuthFromKeychain`
    AuthFromKeychain(Arc<dyn Keychain>),
    /// `remote.WithPlatform`
    Platform(Platform),
    /// `remote.WithRetryBackoff`
    RetryBackoff(Backoff),
    /// `remote.WithRetryStatusCodes`
    RetryStatusCodes(Vec<u16>),
    /// `remote.WithRetryPredicate`
    RetryPredicate(transport::Predicate),
    /// `remote.WithUserAgent`
    UserAgent(String),
    /// `remote.WithTransport`
    Transport(Arc<dyn RoundTripper>),
}

impl fmt::Debug for RemoteOption {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RemoteOption::Auth(a) => write!(f, "Auth({a:?})"),
            RemoteOption::AuthFromKeychain(_) => f.write_str("AuthFromKeychain"),
            RemoteOption::Platform(p) => write!(f, "Platform({p:?})"),
            RemoteOption::RetryBackoff(b) => write!(f, "RetryBackoff({b:?})"),
            RemoteOption::RetryStatusCodes(c) => write!(f, "RetryStatusCodes({c:?})"),
            RemoteOption::RetryPredicate(_) => f.write_str("RetryPredicate"),
            RemoteOption::UserAgent(u) => write!(f, "UserAgent({u:?})"),
            RemoteOption::Transport(_) => f.write_str("Transport"),
        }
    }
}

/// `remote.WithAuthFromKeychain(keys)`
pub fn with_auth_from_keychain(keys: Arc<dyn Keychain>) -> RemoteOption {
    RemoteOption::AuthFromKeychain(keys)
}

/// `remote.WithAuth(auth)`
pub fn with_auth(auth: Arc<dyn Authenticator>) -> RemoteOption {
    RemoteOption::Auth(auth)
}

/// `remote.WithPlatform(p)`
pub fn with_platform(p: Platform) -> RemoteOption {
    RemoteOption::Platform(p)
}

/// `remote.WithRetryBackoff(b)`
pub fn with_retry_backoff(b: Backoff) -> RemoteOption {
    RemoteOption::RetryBackoff(b)
}

/// `remote.WithRetryStatusCodes(codes...)`
pub fn with_retry_status_codes(codes: Vec<u16>) -> RemoteOption {
    RemoteOption::RetryStatusCodes(codes)
}

/// `remote.WithUserAgent(ua)`
pub fn with_user_agent(ua: impl Into<String>) -> RemoteOption {
    RemoteOption::UserAgent(ua.into())
}

/// `remote.WithTransport(t)`
pub fn with_transport(t: Arc<dyn RoundTripper>) -> RemoteOption {
    RemoteOption::Transport(t)
}

/// `remote.defaultRetryStatusCodes`
pub const DEFAULT_RETRY_STATUS_CODES: &[u16] = &[408, 500, 502, 503, 504, 499, 522];

struct Options {
    auth: Arc<dyn Authenticator>,
    keychain: Option<Arc<dyn Keychain>>,
    transport: Arc<dyn RoundTripper>,
    platform: Platform,
}

fn make_options(opts: &[RemoteOption]) -> anyhow::Result<Options> {
    let mut auth: Option<Arc<dyn Authenticator>> = None;
    let mut keychain: Option<Arc<dyn Keychain>> = None;
    let mut transport_opt: Option<Arc<dyn RoundTripper>> = None;
    let mut platform = v1::default_platform();
    let mut backoff = Backoff::remote_default();
    let mut predicate: transport::Predicate =
        Arc::new(transport::default_retry_predicate);
    let mut codes: Vec<u16> = DEFAULT_RETRY_STATUS_CODES.to_vec();
    let mut user_agent = String::new();
    for o in opts {
        match o {
            RemoteOption::Auth(a) => auth = Some(a.clone()),
            RemoteOption::AuthFromKeychain(k) => keychain = Some(k.clone()),
            RemoteOption::Platform(p) => platform = p.clone(),
            RemoteOption::RetryBackoff(b) => backoff = b.clone(),
            RemoteOption::RetryStatusCodes(c) => codes = c.clone(),
            RemoteOption::RetryPredicate(p) => predicate = p.clone(),
            RemoteOption::UserAgent(u) => user_agent = u.clone(),
            RemoteOption::Transport(t) => transport_opt = Some(t.clone()),
        }
    }
    if auth.is_some() && keychain.is_some() {
        anyhow::bail!("provide an option for either authn.Authenticator or authn.Keychain, not both");
    }
    let auth = auth.unwrap_or_else(anonymous);
    let mut t: Arc<dyn RoundTripper> = transport_opt.unwrap_or_else(|| Arc::new(HttpTransport));
    if !t.is_wrapper() {
        t = Arc::new(RetryTransport::new(t, backoff, predicate, codes));
        if !user_agent.is_empty() {
            t = Arc::new(UserAgentTransport::new(t, &user_agent));
        }
    }
    Ok(Options {
        auth,
        keychain,
        transport: t,
        platform,
    })
}

// ------------------------------------------------------------------ fetcher

/// `remote.fetcher`: an authenticated client for one repository.
pub struct Fetcher {
    target: Repository,
    client: Client,
}

impl Fetcher {
    /// `makeFetcher`
    async fn new(target: &Repository, o: &Options) -> anyhow::Result<Fetcher> {
        let mut auth = o.auth.clone();
        if let Some(kc) = &o.keychain {
            auth = kc.resolve(target)?;
        }
        let reg: &Registry = target.registry();
        let tr = transport::new_with_context(
            reg,
            auth,
            o.transport.clone(),
            vec![target.scope(transport::PULL_SCOPE)],
        )
        .await?;
        Ok(Fetcher {
            target: target.clone(),
            client: Client::new(tr),
        })
    }

    /// `fetcher.url(resource, identifier)`
    fn url(&self, resource: &str, identifier: &str) -> GoUrl {
        GoUrl {
            scheme: self.target.scheme().to_string(),
            host: self.target.registry_str().to_string(),
            path: format!(
                "/v2/{}/{}/{}",
                self.target.repository_str(),
                resource,
                identifier
            ),
            ..Default::default()
        }
    }

    async fn do_request(&self, req: Request) -> anyhow::Result<transport::Response> {
        self.client.do_request(req).await
    }

    /// `fetchManifest`
    async fn fetch_manifest(
        &self,
        r: &Reference,
        acceptable: &[MediaType],
    ) -> anyhow::Result<(Bytes, v1::Descriptor)> {
        let u = self.url("manifests", r.identifier());
        let mut req = Request::new(Method::GET, &u.string())?;
        let accept: Vec<&str> = acceptable.iter().map(|m| m.as_str()).collect();
        req.set_header(::http::header::ACCEPT, &accept.join(","));

        let resp = self.do_request(req).await?;
        let mut resp = check_error(resp, &[200]).await?;

        let manifest = resp
            .body
            .read_limited(MANIFEST_LIMIT)
            .await
            .map_err(NetError::from_io)?;

        let (mut digest, size) = (Hash::sha256(&manifest), manifest.len() as i64);

        let media_type = MediaType::new(resp.header("Content-Type"));
        if let Ok(content_digest) = Hash::new(&resp.header("Docker-Content-Digest"))
            && media_type == MediaType::DOCKER_MANIFEST_SCHEMA1_SIGNED {
                digest = content_digest;
            }

        if let Some(dgst) = r.as_digest()
            && digest.to_string() != dgst.digest_str() {
                anyhow::bail!(
                    "manifest digest: {} does not match requested digest: {} for {}",
                    goutil::quote(&digest.to_string()),
                    goutil::quote(dgst.digest_str()),
                    goutil::quote(&r.to_string())
                );
            }

        let mut artifact_type = String::new();
        if let Ok(mf) = v1::parse_manifest(&manifest)
            && !mf.config.media_type.is_config() {
                artifact_type = mf.config.media_type.0.clone();
            }

        Ok((
            manifest,
            v1::Descriptor {
                digest,
                size,
                media_type,
                artifact_type,
                ..Default::default()
            },
        ))
    }

    /// `headManifest`
    async fn head_manifest(
        &self,
        r: &Reference,
        acceptable: &[MediaType],
    ) -> anyhow::Result<v1::Descriptor> {
        let u = self.url("manifests", r.identifier());
        let mut req = Request::new(Method::HEAD, &u.string())?;
        let accept: Vec<&str> = acceptable.iter().map(|m| m.as_str()).collect();
        req.set_header(::http::header::ACCEPT, &accept.join(","));

        let resp = self.do_request(req).await?;
        let resp = check_error(resp, &[200]).await?;

        let mth = resp.header("Content-Type");
        if mth.is_empty() {
            anyhow::bail!(
                "HEAD {}: response did not include Content-Type header",
                u.string()
            );
        }
        let media_type = MediaType::new(mth);

        let size = resp.content_length;
        if size == -1 {
            anyhow::bail!(
                "GET {}: response did not include Content-Length header",
                u.string()
            );
        }

        let dh = resp.header("Docker-Content-Digest");
        if dh.is_empty() {
            anyhow::bail!(
                "HEAD {}: response did not include Docker-Content-Digest header",
                u.string()
            );
        }
        let digest = Hash::new(&dh)?;

        if let Some(dgst) = r.as_digest()
            && digest.to_string() != dgst.digest_str() {
                anyhow::bail!(
                    "manifest digest: {} does not match requested digest: {} for {}",
                    goutil::quote(&digest.to_string()),
                    goutil::quote(dgst.digest_str()),
                    goutil::quote(&r.to_string())
                );
            }

        Ok(v1::Descriptor {
            digest,
            size,
            media_type,
            ..Default::default()
        })
    }

    /// `fetchBlob`
    async fn fetch_blob(&self, size: i64, h: &Hash) -> anyhow::Result<BoxAsyncRead> {
        let u = self.url("blobs", &h.to_string());
        let req = Request::new(Method::GET, &u.string())?;
        let resp = match self.do_request(req).await {
            Ok(r) => r,
            Err(e) => return Err(redact_error(e)),
        };
        let resp = check_error(resp, &[200]).await?;
        let mut size = size;
        let hsize = resp.content_length;
        if hsize != -1 {
            if size == SIZE_UNKNOWN {
                size = hsize;
            } else if hsize != size {
                anyhow::bail!(
                    "GET {}: Content-Length header {} does not match expected size {}",
                    u.string(),
                    hsize,
                    size
                );
            }
        }
        verify_read_closer(resp.body.into_async_read(), size, h)
    }
}

/// `redact.Error`: redact the query of a `*url.Error`'s URL.
fn redact_error(e: anyhow::Error) -> anyhow::Error {
    match e.downcast::<UrlError>() {
        Ok(mut ue) => {
            if let Ok(u) = GoUrl::parse(&ue.url) {
                ue.url = gourl::redact_url(&u);
            }
            anyhow::Error::new(ue)
        }
        Err(e) => e,
    }
}

// ------------------------------------------------------------------ verify

/// `verify.SizeUnknown`
pub const SIZE_UNKNOWN: i64 = -1;

/// `verify.Error` (and the size mismatch error).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyError {
    Checksum { got: String, want: Hash, got_size: i64 },
    Size { got: i64, want: i64 },
}

impl fmt::Display for VerifyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            VerifyError::Checksum {
                got,
                want,
                got_size,
            } => write!(
                f,
                "error verifying {} checksum after reading {} bytes; got {}, want {}",
                want.algorithm,
                got_size,
                goutil::quote(got),
                goutil::quote(&want.to_string())
            ),
            VerifyError::Size { got, want } => {
                write!(f, "error verifying size; got {got}, want {want}")
            }
        }
    }
}

impl std::error::Error for VerifyError {}

/// `verify.ReadCloser`: tee into a hasher, limit to `size` when known, and check the
/// size and digest once the stream reaches EOF.
pub struct VerifyReader {
    inner: BoxAsyncRead,
    hasher: Sha256,
    expected: Hash,
    got_size: i64,
    want_size: i64,
    done: bool,
    scratch: Vec<u8>,
}

/// `verify.ReadCloser(r, size, h)`
pub fn verify_read_closer(r: BoxAsyncRead, size: i64, h: &Hash) -> anyhow::Result<BoxAsyncRead> {
    if h.algorithm != "sha256" {
        anyhow::bail!("unsupported hash: {}", goutil::quote(&h.algorithm));
    }
    Ok(Box::pin(VerifyReader {
        inner: r,
        hasher: Sha256::new(),
        expected: h.clone(),
        got_size: 0,
        want_size: size,
        done: false,
        scratch: vec![0u8; 32 * 1024],
    }))
}

impl AsyncRead for VerifyReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = &mut *self;
        if this.done || buf.remaining() == 0 {
            return Poll::Ready(Ok(()));
        }
        // io.LimitReader(r, size) when the size is known.
        let remaining = if this.want_size != SIZE_UNKNOWN {
            (this.want_size - this.got_size).max(0) as usize
        } else {
            usize::MAX
        };
        if remaining > 0 {
            let limit = remaining.min(buf.remaining()).min(this.scratch.len());
            let mut tmp = ReadBuf::new(&mut this.scratch[..limit]);
            match Pin::new(&mut this.inner).poll_read(cx, &mut tmp) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(Err(e)) => return Poll::Ready(Err(e)),
                Poll::Ready(Ok(())) => {
                    let n = tmp.filled().len();
                    if n > 0 {
                        this.hasher.update(tmp.filled());
                        buf.put_slice(tmp.filled());
                        this.got_size += n as i64;
                        return Poll::Ready(Ok(()));
                    }
                }
            }
        }
        // EOF (or the size limit was reached): verify.
        this.done = true;
        if this.want_size != SIZE_UNKNOWN && this.got_size != this.want_size {
            return Poll::Ready(Err(std::io::Error::other(VerifyError::Size {
                got: this.got_size,
                want: this.want_size,
            })));
        }
        let got = hex::encode(this.hasher.clone().finalize());
        if got != this.expected.hex {
            return Poll::Ready(Err(std::io::Error::other(VerifyError::Checksum {
                got: format!("{}:{}", this.expected.algorithm, got),
                want: this.expected.clone(),
                got_size: this.got_size,
            })));
        }
        Poll::Ready(Ok(()))
    }
}

/// `verify.Descriptor`
fn verify_descriptor(d: &v1::Descriptor) -> anyhow::Result<()> {
    let Some(data) = &d.data else {
        anyhow::bail!("error verifying descriptor; Data == nil");
    };
    let h = Hash::sha256(data);
    if h != d.digest {
        anyhow::bail!(
            "error verifying Digest; got {}, want {}",
            goutil::quote(&h.to_string()),
            goutil::quote(&d.digest.to_string())
        );
    }
    if data.len() as i64 != d.size {
        anyhow::bail!("error verifying Size; got {}, want {}", data.len(), d.size);
    }
    Ok(())
}

async fn read_all(mut r: BoxAsyncRead) -> anyhow::Result<Bytes> {
    let mut out = Vec::new();
    r.read_to_end(&mut out).await.map_err(NetError::from_io)?;
    Ok(Bytes::from(out))
}

// ------------------------------------------------------------------ public API

/// `remote.Head`: HEAD the manifest and return its descriptor.
pub async fn head(r: &Reference, opts: &[RemoteOption]) -> anyhow::Result<v1::Descriptor> {
    let o = make_options(opts)?;
    let f = Fetcher::new(r.context(), &o).await?;
    f.head_manifest(r, &all_manifest_media_types()).await
}

/// `remote.Get`: fetch the manifest and return a [`Descriptor`].
pub async fn get(r: &Reference, opts: &[RemoteOption]) -> anyhow::Result<Descriptor> {
    let o = make_options(opts)?;
    let f = Arc::new(Fetcher::new(r.context(), &o).await?);
    let (manifest, desc) = f.fetch_manifest(r, &all_manifest_media_types()).await?;
    Ok(Descriptor {
        fetcher: f,
        descriptor: desc,
        reference: r.clone(),
        manifest,
        platform: o.platform,
    })
}

/// `remote.Image`: `Get` + `Descriptor.Image()`.
pub async fn image(r: &Reference, opts: &[RemoteOption]) -> anyhow::Result<Image> {
    let desc = get(r, opts).await?;
    desc.image().await
}

/// `remote.Descriptor`
pub struct Descriptor {
    fetcher: Arc<Fetcher>,
    /// The embedded `v1.Descriptor`.
    pub descriptor: v1::Descriptor,
    reference: Reference,
    /// The raw manifest bytes.
    pub manifest: Bytes,
    platform: Platform,
}

impl fmt::Debug for Descriptor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Descriptor")
            .field("descriptor", &self.descriptor)
            .field("reference", &self.reference)
            .finish()
    }
}

impl Descriptor {
    /// `RawManifest()`
    pub fn raw_manifest(&self) -> &[u8] {
        &self.manifest
    }

    /// The media type of the manifest.
    pub fn media_type(&self) -> &MediaType {
        &self.descriptor.media_type
    }

    /// `Descriptor.Image()`: resolve indexes to the configured platform (default
    /// linux/amd64) and return the image.
    pub fn image(&self) -> BoxFuture<'_, anyhow::Result<Image>> {
        Box::pin(async move {
            let mt = self.descriptor.media_type.as_str();
            match mt {
                MediaType::DOCKER_MANIFEST_SCHEMA1 | MediaType::DOCKER_MANIFEST_SCHEMA1_SIGNED => {
                    anyhow::bail!(
                        "unsupported MediaType: {}, see https://github.com/google/go-containerregistry/issues/377",
                        goutil::quote(mt)
                    );
                }
                MediaType::OCI_IMAGE_INDEX | MediaType::DOCKER_MANIFEST_LIST => {
                    let child = self.child_by_platform(&self.platform).await?;
                    return child.image().await;
                }
                _ => {}
            }
            Ok(Image {
                fetcher: self.fetcher.clone(),
                reference: self.reference.clone(),
                manifest: self.manifest.clone(),
                media_type: self.descriptor.media_type.clone(),
                descriptor: self.descriptor.clone(),
                config: Arc::new(OnceCell::new()),
            })
        })
    }

    /// `remoteIndex.IndexManifest()`
    pub fn index_manifest(&self) -> anyhow::Result<v1::IndexManifest> {
        v1::parse_index_manifest(&self.manifest)
    }

    /// `remoteIndex.childByPlatform`
    async fn child_by_platform(&self, platform: &Platform) -> anyhow::Result<Descriptor> {
        let index = self.index_manifest()?;
        for child in &index.manifests {
            let p = child.platform.clone().unwrap_or_else(v1::default_platform);
            if matches_platform(&p, platform) {
                return self.child_descriptor(child.clone(), platform.clone()).await;
            }
        }
        anyhow::bail!(
            "no child with platform {} in index {}",
            platform.string(),
            self.reference
        )
    }

    /// `remoteIndex.childDescriptor`
    async fn child_descriptor(
        &self,
        mut child: v1::Descriptor,
        platform: Platform,
    ) -> anyhow::Result<Descriptor> {
        let r: Reference = self
            .reference
            .context()
            .digest(&child.digest.to_string())
            .into();
        let manifest = if child.data.is_some() {
            verify_descriptor(&child)?;
            Bytes::from(child.data.clone().unwrap_or_default())
        } else {
            let (m, _) = self
                .fetcher
                .fetch_manifest(&r, std::slice::from_ref(&child.media_type))
                .await?;
            m
        };
        if child.media_type.is_image()
            && let Ok(mf) = v1::parse_manifest(&manifest)
                && !mf.config.media_type.is_config() {
                    child.artifact_type = mf.config.media_type.0.clone();
                }
        Ok(Descriptor {
            fetcher: self.fetcher.clone(),
            descriptor: child,
            reference: r,
            manifest,
            platform,
        })
    }
}

/// `matchesPlatform`
fn matches_platform(given: &Platform, required: &Platform) -> bool {
    if given.architecture != required.architecture || given.os != required.os {
        return false;
    }
    if !required.os_version.is_empty() && given.os_version != required.os_version {
        return false;
    }
    if !required.variant.is_empty() && given.variant != required.variant {
        return false;
    }
    let is_subset = |lst: &[String], req: &[String]| req.iter().all(|r| lst.contains(r));
    is_subset(&given.os_features, &required.os_features)
        && is_subset(&given.features, &required.features)
}

/// A remote image (`remote.remoteImage` wrapped by `partial.CompressedToImage`).
#[derive(Clone)]
pub struct Image {
    fetcher: Arc<Fetcher>,
    reference: Reference,
    manifest: Bytes,
    media_type: MediaType,
    descriptor: v1::Descriptor,
    config: Arc<OnceCell<Bytes>>,
}

impl fmt::Debug for Image {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("reference", &self.reference)
            .field("media_type", &self.media_type)
            .finish()
    }
}

impl Image {
    /// `MediaType()`
    pub fn media_type(&self) -> MediaType {
        if self.media_type.0.is_empty() {
            return MediaType::DOCKER_MANIFEST_SCHEMA2.into();
        }
        self.media_type.clone()
    }

    /// `RawManifest()`
    pub fn raw_manifest(&self) -> &[u8] {
        &self.manifest
    }

    /// `Manifest()`
    pub fn manifest(&self) -> anyhow::Result<Manifest> {
        v1::parse_manifest(&self.manifest)
    }

    /// `Digest()`
    pub fn digest(&self) -> Hash {
        Hash::sha256(&self.manifest)
    }

    /// `Size()`
    pub fn size(&self) -> i64 {
        self.manifest.len() as i64
    }

    /// `Descriptor()`
    pub fn descriptor(&self) -> &v1::Descriptor {
        &self.descriptor
    }

    /// The reference this image was resolved from.
    pub fn reference(&self) -> &Reference {
        &self.reference
    }

    /// `RawConfigFile()`
    pub async fn raw_config_file(&self) -> anyhow::Result<Bytes> {
        self.config
            .get_or_try_init(|| async {
                let m = self.manifest()?;
                if m.config.data.is_some() {
                    verify_descriptor(&m.config)?;
                    return Ok(Bytes::from(m.config.data.clone().unwrap_or_default()));
                }
                let body = self.fetcher.fetch_blob(m.config.size, &m.config.digest).await?;
                read_all(body).await
            })
            .await
            .cloned()
    }

    /// `ConfigFile()`
    pub async fn config_file(&self) -> anyhow::Result<ConfigFile> {
        let b = self.raw_config_file().await?;
        v1::parse_config_file(&b)
    }

    /// `ConfigName()`
    pub async fn config_name(&self) -> anyhow::Result<Hash> {
        Ok(Hash::sha256(&self.raw_config_file().await?))
    }

    /// `Layers()`
    pub fn layers(&self) -> anyhow::Result<Vec<RemoteLayer>> {
        let m = self.manifest()?;
        Ok(m.layers
            .iter()
            .map(|l| RemoteLayer {
                image: self.clone(),
                digest: l.digest.clone(),
            })
            .collect())
    }
}

/// A layer of a remote image (`remoteImageLayer`).
#[derive(Clone, Debug)]
pub struct RemoteLayer {
    image: Image,
    digest: Hash,
}

impl RemoteLayer {
    /// `Digest()`
    pub fn digest(&self) -> &Hash {
        &self.digest
    }

    /// `partial.BlobDescriptor`
    fn blob_descriptor(&self) -> anyhow::Result<v1::Descriptor> {
        let m = self.image.manifest()?;
        if m.config.digest == self.digest {
            return Ok(m.config);
        }
        for l in m.layers {
            if l.digest == self.digest {
                return Ok(l);
            }
        }
        anyhow::bail!("blob {} not found", self.digest)
    }

    /// `MediaType()`
    pub fn media_type(&self) -> anyhow::Result<MediaType> {
        Ok(self.blob_descriptor()?.media_type)
    }

    /// `Size()`
    pub fn size(&self) -> anyhow::Result<i64> {
        Ok(self.blob_descriptor()?.size)
    }

    /// `Compressed()`: the verified blob stream (registry URL, then descriptor URLs).
    pub async fn compressed(&self) -> anyhow::Result<BoxAsyncRead> {
        let f = &self.image.fetcher;
        let mut urls = vec![f.url("blobs", &self.digest.to_string())];
        let d = self.blob_descriptor()?;
        if let Some(data) = &d.data {
            let r: BoxAsyncRead = Box::pin(std::io::Cursor::new(Bytes::from(data.clone())));
            return verify_read_closer(r, d.size, &d.digest);
        }
        for s in &d.urls {
            urls.push(GoUrl::parse(s)?);
        }
        let mut last_err: Option<anyhow::Error> = None;
        for u in urls {
            let req = Request::new(Method::GET, &u.string())?;
            let resp = match f.do_request(req).await {
                Ok(r) => r,
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            };
            let resp = match check_error(resp, &[200]).await {
                Ok(r) => r,
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
            };
            return verify_read_closer(resp.body.into_async_read(), d.size, &self.digest);
        }
        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no URLs to fetch blob {}", self.digest)))
    }
}

#[async_trait]
impl crate::mutate::Layer for RemoteLayer {
    async fn compressed(&self) -> anyhow::Result<BoxAsyncRead> {
        RemoteLayer::compressed(self).await
    }
}

#[async_trait]
impl crate::mutate::LayeredImage for Image {
    async fn layers(&self) -> anyhow::Result<Vec<Arc<dyn crate::mutate::Layer>>> {
        Ok(Image::layers(self)?
            .into_iter()
            .map(|l| Arc::new(l) as Arc<dyn crate::mutate::Layer>)
            .collect())
    }
}
