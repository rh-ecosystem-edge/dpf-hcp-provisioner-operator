//! Port of `mutate.Extract` from `github.com/google/go-containerregistry/pkg/v1/mutate`
//! (plus the layer decompression of `partial.CompressedToLayer(...).Uncompressed()`).
//!
//! Go streams the flattened filesystem as a tar through an `io.Pipe` written by a
//! goroutine. Here a blocking worker thread (the "goroutine") parses the layer tars and
//! sends entries over a bounded channel; [`Extract`] is the reading side and offers the
//! `tar.Reader` operations the operator uses (`Next` + reading the entry data).
//!
//! Faithfully reproduced Go quirk: when extraction fails *between* entries (manifest
//! errors, a layer that cannot be fetched or decompressed, a corrupt tar header) Go's
//! deferred `tarWriter.Close()` still writes the end-of-archive marker before the pipe
//! is closed with the error, so the reader sees a clean end of archive. Only failures in
//! the middle of an entry's data surface as read errors.

use std::collections::HashMap;
use std::io::{self, Read};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use tokio::runtime::Handle;
use tokio::sync::mpsc;

use crate::goutil;
use crate::transport::BoxAsyncRead;
use crate::transport::http::go_io_message;

const WHITEOUT_PREFIX: &str = ".wh.";
const CHUNK: usize = 32 * 1024;

/// A layer whose compressed (or uncompressed) blob can be streamed.
#[async_trait]
pub trait Layer: Send + Sync {
    /// `Compressed()`: the raw blob; gzip / zstd / uncompressed tar are detected by magic.
    async fn compressed(&self) -> anyhow::Result<BoxAsyncRead>;
}

/// The part of `v1.Image` needed by [`extract`].
#[async_trait]
pub trait LayeredImage: Send + Sync {
    /// `Layers()`, bottom-most first.
    async fn layers(&self) -> anyhow::Result<Vec<Arc<dyn Layer>>>;
}

#[async_trait]
impl<T: LayeredImage + ?Sized> LayeredImage for Arc<T> {
    async fn layers(&self) -> anyhow::Result<Vec<Arc<dyn Layer>>> {
        (**self).layers().await
    }
}

/// An in-memory layer (handy for tests and for already-downloaded blobs).
#[derive(Debug, Clone)]
pub struct StaticLayer(pub Bytes);

#[async_trait]
impl Layer for StaticLayer {
    async fn compressed(&self) -> anyhow::Result<BoxAsyncRead> {
        Ok(Box::pin(std::io::Cursor::new(self.0.clone())))
    }
}

/// An image made of in-memory layers.
#[derive(Debug, Clone, Default)]
pub struct StaticImage(pub Vec<StaticLayer>);

#[async_trait]
impl LayeredImage for StaticImage {
    async fn layers(&self) -> anyhow::Result<Vec<Arc<dyn Layer>>> {
        Ok(self
            .0
            .iter()
            .map(|l| Arc::new(l.clone()) as Arc<dyn Layer>)
            .collect())
    }
}

/// Typeflags (`archive/tar`).
pub mod typeflag {
    pub const REG: u8 = b'0';
    pub const REG_A: u8 = b'\0';
    pub const LINK: u8 = b'1';
    pub const SYMLINK: u8 = b'2';
    pub const CHAR: u8 = b'3';
    pub const BLOCK: u8 = b'4';
    pub const DIR: u8 = b'5';
    pub const FIFO: u8 = b'6';
    pub const CONT: u8 = b'7';
    pub const X_GLOBAL_HEADER: u8 = b'g';
}

/// A tar header of the flattened filesystem (`tar.Header` subset).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Header {
    /// Cleaned path (`filepath.Clean`).
    pub name: String,
    pub typeflag: u8,
    pub linkname: String,
    pub size: i64,
    pub mode: i64,
    pub uid: i64,
    pub gid: i64,
    pub uname: String,
    pub gname: String,
    /// Modification time, seconds since the Unix epoch.
    pub mtime: i64,
    pub devmajor: i64,
    pub devminor: i64,
}

enum Event {
    Header(Header),
    Data(Bytes),
    End,
    Error(anyhow::Error),
}

/// Reading side of `mutate.Extract(img)`.
pub struct Extract {
    rx: mpsc::Receiver<Event>,
    lookahead: Option<Event>,
    /// Sticky error message (Go `tar.Reader` keeps returning the same error).
    err: Option<String>,
    finished: bool,
}

/// `mutate.Extract(img)`: flatten the image filesystem, top-most layer first, applying
/// whiteouts. Must be called within a Tokio runtime.
pub fn extract<I: LayeredImage + Clone + 'static>(img: &I) -> Extract {
    let (tx, rx) = mpsc::channel(16);
    match Handle::try_current() {
        Ok(handle) => {
            let img = img.clone();
            let h = handle.clone();
            handle.spawn_blocking(move || produce(&img, &h, &tx));
        }
        Err(e) => {
            let _ = tx.try_send(Event::Error(anyhow::anyhow!(
                "mutate.Extract requires a Tokio runtime: {e}"
            )));
        }
    }
    Extract {
        rx,
        lookahead: None,
        err: None,
        finished: false,
    }
}

impl Extract {
    async fn recv(&mut self) -> Event {
        if let Some(e) = self.lookahead.take() {
            return e;
        }
        // A dropped sender without End means the worker died: Go's pipe would report
        // io.ErrClosedPipe.
        self.rx
            .recv()
            .await
            .unwrap_or_else(|| Event::Error(anyhow::anyhow!("io: read/write on closed pipe")))
    }

    fn fail(&mut self, e: anyhow::Error) -> anyhow::Error {
        self.err = Some(e.to_string());
        e
    }

    /// `tar.Reader.Next()`: advance to the next entry, skipping any unread data of the
    /// current one. Returns `Ok(None)` at the end of the archive (`io.EOF`).
    pub async fn next_entry(&mut self) -> anyhow::Result<Option<Header>> {
        if let Some(e) = &self.err {
            return Err(anyhow::anyhow!(e.clone()));
        }
        if self.finished {
            return Ok(None);
        }
        loop {
            match self.recv().await {
                Event::Data(_) => continue,
                Event::Header(h) => return Ok(Some(h)),
                Event::End => {
                    self.finished = true;
                    return Ok(None);
                }
                Event::Error(e) => return Err(self.fail(e)),
            }
        }
    }

    /// Read the next chunk of the current entry's data; `Ok(None)` at the end of the entry.
    pub async fn read_chunk(&mut self) -> anyhow::Result<Option<Bytes>> {
        if let Some(e) = &self.err {
            return Err(anyhow::anyhow!(e.clone()));
        }
        if self.finished {
            return Ok(None);
        }
        match self.recv().await {
            Event::Data(b) => Ok(Some(b)),
            Event::Error(e) => Err(self.fail(e)),
            other => {
                self.lookahead = Some(other);
                Ok(None)
            }
        }
    }

    /// `io.ReadAll(tarReader)` for the current entry.
    pub async fn read_to_end(&mut self) -> anyhow::Result<Vec<u8>> {
        let mut out = Vec::new();
        while let Some(b) = self.read_chunk().await? {
            out.extend_from_slice(&b);
        }
        Ok(out)
    }
}

enum Stop {
    /// The reader went away (Go: `io.ErrClosedPipe` on write).
    Closed,
    Failed(anyhow::Error),
}

fn produce<I: LayeredImage>(img: &I, handle: &Handle, tx: &mpsc::Sender<Event>) {
    let mut entry_open = false;
    match run(img, handle, tx, &mut entry_open) {
        Ok(()) => {
            let _ = tx.blocking_send(Event::End);
        }
        Err(Stop::Closed) => {}
        Err(Stop::Failed(e)) => {
            // tarWriter.Close() writes the trailer unless an entry is incomplete.
            let ev = if entry_open {
                Event::Error(e)
            } else {
                Event::End
            };
            let _ = tx.blocking_send(ev);
        }
    }
}

fn send(tx: &mpsc::Sender<Event>, ev: Event) -> Result<(), Stop> {
    tx.blocking_send(ev).map_err(|_| Stop::Closed)
}

fn failed(msg: String) -> Stop {
    Stop::Failed(anyhow::anyhow!(msg))
}

/// Go `inWhiteoutDir`.
fn in_whiteout_dir(file_map: &HashMap<String, bool>, file: &str) -> bool {
    let mut file = file.to_string();
    while !file.is_empty() {
        let dirname = goutil::dir(&file);
        if file == dirname {
            break;
        }
        if file_map.get(&dirname).copied().unwrap_or(false) {
            return true;
        }
        file = dirname;
    }
    false
}

fn is_header_only_type(flag: u8) -> bool {
    matches!(
        flag,
        typeflag::LINK
            | typeflag::SYMLINK
            | typeflag::CHAR
            | typeflag::BLOCK
            | typeflag::DIR
            | typeflag::FIFO
    )
}

fn run<I: LayeredImage>(
    img: &I,
    handle: &Handle,
    tx: &mpsc::Sender<Event>,
    entry_open: &mut bool,
) -> Result<(), Stop> {
    let mut file_map: HashMap<String, bool> = HashMap::new();
    let layers = handle
        .block_on(img.layers())
        .map_err(|e| failed(format!("retrieving image layers: {e}")))?;

    for layer in layers.iter().rev() {
        let rc = handle
            .block_on(layer.compressed())
            .map_err(|e| failed(format!("reading layer contents: {e}")))?;
        let sync = tokio_util::io::SyncIoBridge::new_with_handle(rc, handle.clone());
        let layer_reader = uncompressed(sync)
            .map_err(|e| failed(format!("reading layer contents: {}", go_io_message(&e))))?;
        let mut tr = crate::gotar::TarReader::new(layer_reader);
        loop {
            let mut header = match tr.next_header() {
                Ok(Some(h)) => h,
                Ok(None) => break,
                Err(e) => return Err(failed(format!("reading tar: {}", go_io_message(&e)))),
            };

            // Some tools prepend everything with "./".
            header.name = goutil::clean_path(&header.name);

            let mut basename = goutil::base(&header.name);
            let dirname = goutil::dir(&header.name);
            let tombstone = basename.starts_with(WHITEOUT_PREFIX);
            if tombstone {
                basename = basename[WHITEOUT_PREFIX.len()..].to_string();
            }

            let name = if header.typeflag == typeflag::DIR {
                header.name.clone()
            } else {
                goutil::join(&[&dirname, &basename])
            };

            if file_map.contains_key(&name) && !tombstone {
                continue;
            }
            if in_whiteout_dir(&file_map, &name) {
                continue;
            }

            // Non-directory implicitly tombstones any entries with a matching (or child) name.
            file_map.insert(name, tombstone || header.typeflag != typeflag::DIR);
            if tombstone {
                continue;
            }
            let size = header.size;
            let header_only = is_header_only_type(header.typeflag);
            send(tx, Event::Header(header))?;
            if size > 0 {
                if header_only {
                    // Go's tar.Reader yields no data for header-only types, so CopyN fails
                    // with io.EOF; the tar.Writer considers such entries complete (no data
                    // section), hence the error ends the archive cleanly.
                    return Err(failed("EOF".to_string()));
                }
                *entry_open = true;
                let mut remaining = size as u64;
                let mut buf = vec![0u8; CHUNK];
                while remaining > 0 {
                    let want = (remaining as usize).min(CHUNK);
                    let n = match tr.read(&mut buf[..want]) {
                        Ok(0) => return Err(failed("unexpected EOF".to_string())),
                        Ok(n) => n,
                        Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                        Err(e) => return Err(failed(go_io_message(&e))),
                    };
                    send(tx, Event::Data(Bytes::copy_from_slice(&buf[..n])))?;
                    remaining -= n as u64;
                }
                *entry_open = false;
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ decompression

/// `bufio.Reader` with Go `Peek(n)` semantics.
struct PeekReader<R> {
    inner: R,
    buf: Vec<u8>,
    pos: usize,
}

impl<R: Read> PeekReader<R> {
    fn new(inner: R) -> Self {
        PeekReader {
            inner,
            buf: Vec::new(),
            pos: 0,
        }
    }

    /// Returns up to `n` bytes; fewer only at EOF (`Ok`), errors otherwise.
    fn peek(&mut self, n: usize) -> io::Result<&[u8]> {
        while self.buf.len() - self.pos < n {
            let mut tmp = [0u8; 4096];
            match self.inner.read(&mut tmp) {
                Ok(0) => break,
                Ok(k) => self.buf.extend_from_slice(&tmp[..k]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
        let end = (self.pos + n).min(self.buf.len());
        Ok(&self.buf[self.pos..end])
    }
}

impl<R: Read> Read for PeekReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos < self.buf.len() {
            let n = (self.buf.len() - self.pos).min(out.len());
            out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
            self.pos += n;
            if self.pos == self.buf.len() {
                self.buf.clear();
                self.pos = 0;
            }
            return Ok(n);
        }
        self.inner.read(out)
    }
}

const GZIP_MAGIC: &[u8] = &[0x1f, 0x8b];
const ZSTD_MAGIC: &[u8] = &[0x28, 0xb5, 0x2f, 0xfd];

/// `compression.PeekCompression` + the matching decompressor.
fn uncompressed<R: Read + Send + 'static>(r: R) -> io::Result<Box<dyn Read + Send>> {
    let mut pr = PeekReader::new(r);
    if pr.peek(GZIP_MAGIC.len())? == GZIP_MAGIC {
        return Ok(Box::new(flate2::read::MultiGzDecoder::new(pr)));
    }
    if pr.peek(ZSTD_MAGIC.len())? == ZSTD_MAGIC {
        return Ok(Box::new(ZstdMultiFrame::new(pr)));
    }
    Ok(Box::new(pr))
}

/// Streaming zstd decoder that, like klauspost/compress, decodes concatenated frames.
struct ZstdMultiFrame<R: Read> {
    src: Option<PeekReader<R>>,
    cur: Option<ruzstd::decoding::StreamingDecoder<PeekReader<R>, ruzstd::decoding::FrameDecoder>>,
}

impl<R: Read> ZstdMultiFrame<R> {
    fn new(src: PeekReader<R>) -> Self {
        ZstdMultiFrame {
            src: Some(src),
            cur: None,
        }
    }
}

fn zstd_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, format!("zstd: {e}"))
}

impl<R: Read> Read for ZstdMultiFrame<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        loop {
            if let Some(dec) = &mut self.cur {
                let n = dec.read(out)?;
                if n > 0 || out.is_empty() {
                    return Ok(n);
                }
                // Frame finished: continue with the next one, if any.
                let Some(dec) = self.cur.take() else {
                    return Ok(0);
                };
                self.src = Some(dec.into_inner());
            }
            let Some(mut src) = self.src.take() else {
                return Ok(0);
            };
            if src.peek(1)?.is_empty() {
                return Ok(0);
            }
            match ruzstd::decoding::StreamingDecoder::new(src) {
                Ok(dec) => self.cur = Some(dec),
                Err(e) => return Err(zstd_err(e)),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whiteout_dir() {
        let mut m = HashMap::new();
        m.insert("opt/app".to_string(), true);
        assert!(in_whiteout_dir(&m, "opt/app/a.txt"));
        assert!(!in_whiteout_dir(&m, "opt/other"));
        m.insert("etc".to_string(), false);
        assert!(!in_whiteout_dir(&m, "etc/passwd"));
    }
}
