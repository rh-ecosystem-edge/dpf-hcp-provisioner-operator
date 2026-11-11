//! Go `compress/gzip`: [`Writer`] produces byte-identical output to `gzip.NewWriterLevel`
//! (default header: no name/comment/extra, mtime 0, OS 255); [`Reader`] mirrors
//! `gzip.NewReader` + reading to EOF (multistream), including Go's error values.

use std::fmt;

use crate::crc32;
use crate::flate::{self, BEST_COMPRESSION, BEST_SPEED, HUFFMAN_ONLY, InflateError, Inflater};

pub use crate::flate::{DEFAULT_COMPRESSION, NO_COMPRESSION};

const GZIP_ID1: u8 = 0x1f;
const GZIP_ID2: u8 = 0x8b;
const GZIP_DEFLATE: u8 = 8;
const FLAG_HDR_CRC: u8 = 1 << 1;
const FLAG_EXTRA: u8 = 1 << 2;
const FLAG_NAME: u8 = 1 << 3;
const FLAG_COMMENT: u8 = 1 << 4;

/// Errors from the gzip writer and reader, with Go's messages.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `gzip: invalid compression level: N`.
    InvalidLevel(i32),
    /// `gzip.ErrHeader`.
    Header,
    /// `gzip.ErrChecksum`.
    Checksum,
    /// `io.EOF` (empty input to `NewReader`).
    Eof,
    /// `io.ErrUnexpectedEOF`.
    UnexpectedEof,
    /// A decompression error from `compress/flate`.
    Flate(InflateError),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidLevel(l) => write!(f, "gzip: invalid compression level: {l}"),
            Error::Header => f.write_str("gzip: invalid header"),
            Error::Checksum => f.write_str("gzip: invalid checksum"),
            Error::Eof => f.write_str("EOF"),
            Error::UnexpectedEof => f.write_str("unexpected EOF"),
            Error::Flate(e) => e.fmt(f),
        }
    }
}

impl std::error::Error for Error {}

impl From<InflateError> for Error {
    fn from(e: InflateError) -> Self {
        match e {
            InflateError::UnexpectedEof => Error::UnexpectedEof,
            e => Error::Flate(e),
        }
    }
}

/// Go `gzip.Writer` with the default `Header` (as created by `NewWriterLevel`).
pub struct Writer {
    level: i32,
    out: Vec<u8>,
    wrote_header: bool,
    closed: bool,
    compressor: Option<flate::Writer>,
    digest: u32,
    size: u32,
}

impl Writer {
    /// Go `gzip.NewWriterLevel`.
    pub fn new(level: i32) -> Result<Writer, Error> {
        if !(HUFFMAN_ONLY..=BEST_COMPRESSION).contains(&level) {
            return Err(Error::InvalidLevel(level));
        }
        Ok(Writer {
            level,
            out: Vec::new(),
            wrote_header: false,
            closed: false,
            compressor: None,
            digest: 0,
            size: 0,
        })
    }

    fn drain(&mut self) {
        if let Some(c) = self.compressor.as_mut() {
            let o = c.take_output();
            self.out.extend_from_slice(&o);
        }
    }

    /// Go `(*gzip.Writer).Write`.
    pub fn write(&mut self, p: &[u8]) -> usize {
        if self.closed {
            return 0;
        }
        if !self.wrote_header {
            self.wrote_header = true;
            let mut buf = [0u8; 10];
            buf[0] = GZIP_ID1;
            buf[1] = GZIP_ID2;
            buf[2] = GZIP_DEFLATE;
            if self.level == BEST_COMPRESSION {
                buf[8] = 2;
            } else if self.level == BEST_SPEED {
                buf[8] = 4;
            }
            buf[9] = 255;
            self.out.extend_from_slice(&buf);
            if self.compressor.is_none() {
                self.compressor = flate::Writer::new(self.level).ok();
            }
        }
        self.size = self.size.wrapping_add(p.len() as u32);
        self.digest = crc32::update(self.digest, p);
        let n = match self.compressor.as_mut() {
            Some(c) => c.write(p),
            None => 0,
        };
        self.drain();
        n
    }

    /// Go `(*gzip.Writer).Flush`.
    pub fn flush(&mut self) {
        if self.closed {
            return;
        }
        if !self.wrote_header {
            self.write(&[]);
        }
        if let Some(c) = self.compressor.as_mut() {
            c.flush();
        }
        self.drain();
    }

    /// Go `(*gzip.Writer).Close`.
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        if !self.wrote_header {
            self.write(&[]);
        }
        if let Some(c) = self.compressor.as_mut() {
            c.close();
        }
        self.drain();
        self.out.extend_from_slice(&self.digest.to_le_bytes());
        self.out.extend_from_slice(&self.size.to_le_bytes());
    }

    /// Takes the bytes written to the underlying writer so far.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }

    /// Closes the writer and returns all output.
    pub fn finish(mut self) -> Vec<u8> {
        self.close();
        self.take_output()
    }
}

/// `w, _ := gzip.NewWriterLevel(&buf, level); w.Write(data); w.Close(); buf.Bytes()`.
pub fn compress(data: &[u8], level: i32) -> Result<Vec<u8>, Error> {
    let mut w = Writer::new(level)?;
    w.write(data);
    Ok(w.finish())
}

/// The gzip header fields Go exposes after reading a header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Header {
    /// `Header.Comment`.
    pub comment: String,
    /// `Header.Extra` (`None` when absent).
    pub extra: Option<Vec<u8>>,
    /// `Header.ModTime` as Unix seconds (0 when unset).
    pub mod_time: i64,
    /// `Header.Name`.
    pub name: String,
    /// `Header.OS`.
    pub os: u8,
}

/// Go `gzip.Reader` over an in-memory input.
pub struct Reader<'a> {
    input: &'a [u8],
    pos: usize,
    /// The header of the first member.
    pub header: Header,
}

fn read_full<'a>(input: &'a [u8], pos: &mut usize, n: usize) -> Result<&'a [u8], Error> {
    let avail = input.len() - *pos;
    if avail < n {
        *pos = input.len();
        return Err(if avail == 0 {
            Error::Eof
        } else {
            Error::UnexpectedEof
        });
    }
    let s = &input[*pos..*pos + n];
    *pos += n;
    Ok(s)
}

fn no_eof(e: Error) -> Error {
    if e == Error::Eof {
        Error::UnexpectedEof
    } else {
        e
    }
}

fn read_string(input: &[u8], pos: &mut usize, digest: &mut u32) -> Result<String, Error> {
    let start = *pos;
    let mut i = 0;
    loop {
        if i >= 512 {
            return Err(Error::Header);
        }
        let Some(&c) = input.get(*pos) else {
            return Err(Error::Eof);
        };
        *pos += 1;
        if c == 0 {
            let b = &input[start..start + i + 1];
            *digest = crc32::update(*digest, b);
            // Latin-1 decoding.
            return Ok(input[start..start + i].iter().map(|&v| v as char).collect());
        }
        i += 1;
    }
}

fn read_header(input: &[u8], pos: &mut usize) -> Result<Header, Error> {
    let buf = read_full(input, pos, 10)?;
    if buf[0] != GZIP_ID1 || buf[1] != GZIP_ID2 || buf[2] != GZIP_DEFLATE {
        return Err(Error::Header);
    }
    let flg = buf[3];
    let mut hdr = Header::default();
    let t = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]) as i64;
    if t > 0 {
        hdr.mod_time = t;
    }
    hdr.os = buf[9];
    let mut digest = crc32::checksum_ieee(buf);
    if flg & FLAG_EXTRA != 0 {
        let b = read_full(input, pos, 2).map_err(no_eof)?;
        digest = crc32::update(digest, b);
        let n = u16::from_le_bytes([b[0], b[1]]) as usize;
        let data = read_full(input, pos, n).map_err(no_eof)?;
        digest = crc32::update(digest, data);
        hdr.extra = Some(data.to_vec());
    }
    if flg & FLAG_NAME != 0 {
        hdr.name = read_string(input, pos, &mut digest).map_err(no_eof)?;
    }
    if flg & FLAG_COMMENT != 0 {
        hdr.comment = read_string(input, pos, &mut digest).map_err(no_eof)?;
    }
    if flg & FLAG_HDR_CRC != 0 {
        let b = read_full(input, pos, 2).map_err(no_eof)?;
        if u16::from_le_bytes([b[0], b[1]]) != digest as u16 {
            return Err(Error::Header);
        }
    }
    Ok(hdr)
}

impl<'a> Reader<'a> {
    /// Go `gzip.NewReader(bytes.NewReader(input))`: reads and validates the first header.
    pub fn new(input: &'a [u8]) -> Result<Reader<'a>, Error> {
        let mut pos = 0;
        let header = read_header(input, &mut pos)?;
        Ok(Reader { input, pos, header })
    }

    /// Reads the (multistream) decompressed content to EOF, like `io.ReadAll(z)` /
    /// `bytes.Buffer.ReadFrom(z)`. On error the data read so far is discarded.
    pub fn read_all(self) -> Result<Vec<u8>, Error> {
        let mut out = Vec::new();
        let mut pos = self.pos;
        loop {
            let mut f = Inflater::new(self.input, pos);
            let res = f.run();
            let data = f.take_output();
            pos = f.pos();
            res?;
            let digest = crc32::checksum_ieee(&data);
            let size = data.len() as u32;
            out.extend_from_slice(&data);
            let trailer = read_full(self.input, &mut pos, 8).map_err(no_eof)?;
            let want_digest = u32::from_le_bytes([trailer[0], trailer[1], trailer[2], trailer[3]]);
            let want_size = u32::from_le_bytes([trailer[4], trailer[5], trailer[6], trailer[7]]);
            if want_digest != digest || want_size != size {
                return Err(Error::Checksum);
            }
            match read_header(self.input, &mut pos) {
                Ok(_) => continue,
                Err(Error::Eof) => return Ok(out),
                Err(e) => return Err(e),
            }
        }
    }
}

/// `gzip.NewReader` followed by reading everything (see [`Reader`]).
pub fn decompress(input: &[u8]) -> Result<Vec<u8>, Error> {
    Reader::new(input)?.read_all()
}
