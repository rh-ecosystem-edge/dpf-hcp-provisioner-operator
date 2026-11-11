//! Port of Go's `archive/tar` Reader (V7 / USTAR / PAX / GNU / STAR headers, PAX
//! records, GNU long names), reproducing its end-of-archive and error semantics, which
//! `mutate.Extract` depends on (e.g. a stream truncated inside the padding after an
//! entry is a clean end of archive, a partial header block is `unexpected EOF`).
//!
//! Sparse files are not expanded (their physical data is returned as-is).

use std::collections::BTreeMap;
use std::io::{self, Read};

use crate::mutate::{Header, typeflag};

const BLOCK_SIZE: usize = 512;
const MAX_SPECIAL_FILE_SIZE: u64 = 1 << 20;
/// `time.Time{}.Unix()`
const GO_ZERO_TIME_UNIX: i64 = -62135596800;

/// `tar.ErrHeader`
pub const ERR_HEADER: &str = "archive/tar: invalid tar header";
/// `tar.ErrFieldTooLong`
pub const ERR_FIELD_TOO_LONG: &str = "archive/tar: header field too long";

fn err_header() -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, ERR_HEADER)
}

fn unexpected_eof() -> io::Error {
    io::Error::new(io::ErrorKind::UnexpectedEof, "unexpected EOF")
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Format {
    Unknown,
    V7,
    UstarPax,
    Gnu,
    Star,
}

/// Outcome of a full read: `Eof` = 0 bytes then EOF, `Short` = some bytes then EOF.
enum Full {
    Ok,
    Eof,
    Short,
}

fn read_full<R: Read>(r: &mut R, b: &mut [u8]) -> io::Result<Full> {
    let mut n = 0;
    while n < b.len() {
        match r.read(&mut b[n..]) {
            Ok(0) => return Ok(if n == 0 { Full::Eof } else { Full::Short }),
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(Full::Ok)
}

fn parse_string(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

#[derive(Default)]
struct Parser {
    err: bool,
}

impl Parser {
    fn parse_numeric(&mut self, b: &[u8]) -> i64 {
        if !b.is_empty() && b[0] & 0x80 != 0 {
            let inv: u8 = if b[0] & 0x40 != 0 { 0xff } else { 0 };
            let mut x: u64 = 0;
            for (i, &c) in b.iter().enumerate() {
                let mut c = c ^ inv;
                if i == 0 {
                    c &= 0x7f;
                }
                if (x >> 56) > 0 {
                    self.err = true;
                    return 0;
                }
                x = (x << 8) | u64::from(c);
            }
            if (x >> 63) > 0 {
                self.err = true;
                return 0;
            }
            if inv == 0xff {
                return !(x as i64);
            }
            return x as i64;
        }
        self.parse_octal(b)
    }

    fn parse_octal(&mut self, b: &[u8]) -> i64 {
        let start = b.iter().position(|&c| c != b' ' && c != 0).unwrap_or(b.len());
        let end = b.iter().rposition(|&c| c != b' ' && c != 0).map(|i| i + 1).unwrap_or(start);
        let b = &b[start..end.max(start)];
        if b.is_empty() {
            return 0;
        }
        let s = parse_string(b);
        match u64::from_str_radix(&s, 8) {
            Ok(x) if !s.starts_with('+') => x as i64,
            _ => {
                self.err = true;
                0
            }
        }
    }
}

fn compute_checksum(b: &[u8; BLOCK_SIZE]) -> (i64, i64) {
    let (mut unsigned, mut signed) = (0i64, 0i64);
    for (i, &c) in b.iter().enumerate() {
        let c = if (148..156).contains(&i) { b' ' } else { c };
        unsigned += i64::from(c);
        signed += i64::from(c as i8);
    }
    (unsigned, signed)
}

fn get_format(b: &[u8; BLOCK_SIZE]) -> Format {
    let mut p = Parser::default();
    let value = p.parse_octal(&b[148..156]);
    let (c1, c2) = compute_checksum(b);
    if p.err || (value != c1 && value != c2) {
        return Format::Unknown;
    }
    let magic = &b[257..263];
    let version = &b[263..265];
    let trailer = &b[508..512];
    if magic == b"ustar\0" && trailer == b"tar\0" {
        Format::Star
    } else if magic == b"ustar\0" {
        Format::UstarPax
    } else if magic == b"ustar " && version == b" \0" {
        Format::Gnu
    } else {
        Format::V7
    }
}

fn is_header_only_type(flag: u8) -> bool {
    matches!(
        flag,
        typeflag::LINK | typeflag::SYMLINK | typeflag::CHAR | typeflag::BLOCK | typeflag::DIR | typeflag::FIFO
    )
}

fn parse_pax_record(s: &[u8]) -> Option<(Vec<u8>, Vec<u8>, &[u8])> {
    let sp = s.iter().position(|&c| c == b' ')?;
    let (n_str, rest) = (&s[..sp], &s[sp + 1..]);
    let n: i64 = std::str::from_utf8(n_str).ok()?.parse().ok()?;
    if n < 5 || n > s.len() as i64 {
        return None;
    }
    let n = n - (n_str.len() as i64 + 1);
    if n <= 0 || n as usize > rest.len() {
        return None;
    }
    let n = n as usize;
    let (rec, nl, rem) = (&rest[..n - 1], &rest[n - 1..n], &rest[n..]);
    if nl != b"\n" {
        return None;
    }
    let eq = rec.iter().position(|&c| c == b'=')?;
    let (k, v) = (&rec[..eq], &rec[eq + 1..]);
    if k.is_empty() {
        return None;
    }
    let bad = match k {
        b"path" | b"linkpath" | b"uname" | b"gname" => v.contains(&0),
        _ => k.contains(&0),
    };
    if bad {
        return None;
    }
    Some((k.to_vec(), v.to_vec(), rem))
}

fn parse_pax_time(s: &str) -> Option<i64> {
    let (ss, sn) = match s.split_once('.') {
        Some((a, b)) => (a, Some(b)),
        None => (s, None),
    };
    let secs: i64 = ss.parse().ok()?;
    if let Some(sn) = sn
        && !sn.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
    Some(secs)
}

/// Go `tar.Reader`.
pub struct TarReader<R: Read> {
    r: R,
    pad: u64,
    /// Remaining bytes of the current entry's data section.
    nb: u64,
    blk: [u8; BLOCK_SIZE],
}

impl<R: Read> TarReader<R> {
    pub fn new(r: R) -> Self {
        TarReader {
            r,
            pad: 0,
            nb: 0,
            blk: [0; BLOCK_SIZE],
        }
    }

    /// `discard(r, n)`
    fn discard(&mut self, mut n: u64) -> io::Result<()> {
        let mut buf = [0u8; 8192];
        while n > 0 {
            let want = (n as usize).min(buf.len());
            match self.r.read(&mut buf[..want]) {
                Ok(0) => return Err(unexpected_eof()),
                Ok(k) => n -= k as u64,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    fn handle_regular_file(&mut self, hdr: &Header) -> io::Result<()> {
        let nb = if is_header_only_type(hdr.typeflag) { 0 } else { hdr.size };
        if nb < 0 {
            return Err(err_header());
        }
        self.nb = nb as u64;
        self.pad = (nb as u64).wrapping_neg() & (BLOCK_SIZE as u64 - 1);
        Ok(())
    }

    /// `readSpecialFile` from the current entry.
    fn read_special_file(&mut self) -> io::Result<Vec<u8>> {
        let mut out = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            let n = self.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.extend_from_slice(&buf[..n]);
            if out.len() as u64 > MAX_SPECIAL_FILE_SIZE {
                return Err(io::Error::new(io::ErrorKind::InvalidData, ERR_FIELD_TOO_LONG));
            }
        }
        Ok(out)
    }

    /// `readHeader`; `Ok(None)` is `io.EOF`.
    fn read_header(&mut self) -> io::Result<Option<(Header, [u8; BLOCK_SIZE])>> {
        let mut blk = self.blk;
        match read_full(&mut self.r, &mut blk)? {
            Full::Ok => {}
            Full::Eof => return Ok(None),
            Full::Short => return Err(unexpected_eof()),
        }
        if blk.iter().all(|&b| b == 0) {
            match read_full(&mut self.r, &mut blk)? {
                Full::Ok => {}
                Full::Eof => return Ok(None),
                Full::Short => return Err(unexpected_eof()),
            }
            if blk.iter().all(|&b| b == 0) {
                return Ok(None);
            }
            return Err(err_header());
        }

        let format = get_format(&blk);
        if format == Format::Unknown {
            return Err(err_header());
        }
        let mut p = Parser::default();
        let mut hdr = Header {
            typeflag: blk[156],
            name: parse_string(&blk[0..100]),
            linkname: parse_string(&blk[157..257]),
            size: p.parse_numeric(&blk[124..136]),
            mode: p.parse_numeric(&blk[100..108]),
            uid: p.parse_numeric(&blk[108..116]),
            gid: p.parse_numeric(&blk[116..124]),
            mtime: p.parse_numeric(&blk[136..148]),
            ..Default::default()
        };
        if format != Format::V7 {
            hdr.uname = parse_string(&blk[265..297]);
            hdr.gname = parse_string(&blk[297..329]);
            hdr.devmajor = p.parse_numeric(&blk[329..337]);
            hdr.devminor = p.parse_numeric(&blk[337..345]);
            let mut prefix = String::new();
            match format {
                Format::UstarPax => prefix = parse_string(&blk[345..500]),
                Format::Star => prefix = parse_string(&blk[345..476]),
                Format::Gnu => {
                    let mut p2 = Parser::default();
                    if blk[345] != 0 {
                        p2.parse_numeric(&blk[345..357]);
                    }
                    if blk[357] != 0 {
                        p2.parse_numeric(&blk[357..369]);
                    }
                    if p2.err {
                        let s = parse_string(&blk[345..500]);
                        if s.is_ascii() {
                            prefix = s;
                        }
                    }
                }
                _ => {}
            }
            if !prefix.is_empty() {
                hdr.name = format!("{prefix}/{}", hdr.name);
            }
        }
        if p.err {
            return Err(err_header());
        }
        Ok(Some((hdr, blk)))
    }

    /// `Next()`; `Ok(None)` is `io.EOF`.
    pub fn next_header(&mut self) -> io::Result<Option<Header>> {
        let mut pax: BTreeMap<String, String> = BTreeMap::new();
        let mut gnu_long_name = String::new();
        let mut gnu_long_link = String::new();
        loop {
            // Discard the remainder of the file and any padding.
            let rem = self.nb;
            self.nb = 0;
            self.discard(rem)?;
            if self.pad > 0 {
                let mut pad = vec![0u8; self.pad as usize];
                self.pad = 0;
                match read_full(&mut self.r, &mut pad)? {
                    Full::Ok => {}
                    // tryReadFull: a short read is io.EOF.
                    Full::Eof | Full::Short => return Ok(None),
                }
            }

            let Some((mut hdr, _raw)) = self.read_header()? else {
                return Ok(None);
            };
            self.handle_regular_file(&hdr)?;

            match hdr.typeflag {
                b'x' | b'g' => {
                    pax = self.parse_pax()?;
                    if hdr.typeflag == b'g' {
                        merge_pax(&mut hdr, &pax)?;
                        return Ok(Some(Header {
                            name: hdr.name,
                            typeflag: hdr.typeflag,
                            // Go leaves ModTime as the zero time.Time.
                            mtime: GO_ZERO_TIME_UNIX,
                            ..Default::default()
                        }));
                    }
                    continue;
                }
                b'L' | b'K' => {
                    let real = self.read_special_file()?;
                    let s = parse_string(&real);
                    if hdr.typeflag == b'L' {
                        gnu_long_name = s;
                    } else {
                        gnu_long_link = s;
                    }
                    continue;
                }
                _ => {
                    merge_pax(&mut hdr, &pax)?;
                    if !gnu_long_name.is_empty() {
                        hdr.name = gnu_long_name;
                    }
                    if !gnu_long_link.is_empty() {
                        hdr.linkname = gnu_long_link;
                    }
                    if hdr.typeflag == typeflag::REG_A {
                        hdr.typeflag = if hdr.name.ends_with('/') {
                            typeflag::DIR
                        } else {
                            typeflag::REG
                        };
                    }
                    self.handle_regular_file(&hdr)?;
                    return Ok(Some(hdr));
                }
            }
        }
    }

    fn parse_pax(&mut self) -> io::Result<BTreeMap<String, String>> {
        let buf = self.read_special_file()?;
        let mut s: &[u8] = &buf;
        let mut out = BTreeMap::new();
        let mut sparse_map: Vec<String> = Vec::new();
        while !s.is_empty() {
            let Some((k, v, rest)) = parse_pax_record(s) else {
                return Err(err_header());
            };
            s = rest;
            let k = String::from_utf8_lossy(&k).into_owned();
            let v = String::from_utf8_lossy(&v).into_owned();
            match k.as_str() {
                "GNU.sparse.offset" | "GNU.sparse.numbytes" => {
                    if (sparse_map.len().is_multiple_of(2) && k != "GNU.sparse.offset")
                        || (sparse_map.len() % 2 == 1 && k != "GNU.sparse.numbytes")
                        || v.contains(',')
                    {
                        return Err(err_header());
                    }
                    sparse_map.push(v);
                }
                _ => {
                    out.insert(k, v);
                }
            }
        }
        if !sparse_map.is_empty() {
            out.insert("GNU.sparse.map".to_string(), sparse_map.join(","));
        }
        Ok(out)
    }

    /// `Reader.Read` for the current entry: `Ok(0)` at the end of the entry,
    /// `unexpected EOF` when the archive ends early.
    pub fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        if self.nb == 0 || b.is_empty() {
            return Ok(0);
        }
        let want = (self.nb as usize).min(b.len());
        loop {
            match self.r.read(&mut b[..want]) {
                Ok(0) => return Err(unexpected_eof()),
                Ok(n) => {
                    self.nb -= n as u64;
                    return Ok(n);
                }
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }
}

/// `mergePAX`
fn merge_pax(hdr: &mut Header, pax: &BTreeMap<String, String>) -> io::Result<()> {
    for (k, v) in pax {
        if v.is_empty() {
            continue;
        }
        let ok = match k.as_str() {
            "path" => {
                hdr.name = v.clone();
                true
            }
            "linkpath" => {
                hdr.linkname = v.clone();
                true
            }
            "uname" => {
                hdr.uname = v.clone();
                true
            }
            "gname" => {
                hdr.gname = v.clone();
                true
            }
            "uid" => v.parse::<i64>().map(|x| hdr.uid = x).is_ok(),
            "gid" => v.parse::<i64>().map(|x| hdr.gid = x).is_ok(),
            "mtime" => parse_pax_time(v).map(|x| hdr.mtime = x).is_some(),
            "atime" | "ctime" => parse_pax_time(v).is_some(),
            "size" => v.parse::<i64>().map(|x| hdr.size = x).is_ok(),
            _ => true,
        };
        if !ok {
            return Err(err_header());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_and_base256() {
        let mut p = Parser::default();
        assert_eq!(p.parse_numeric(b"0000644\0"), 0o644);
        assert_eq!(p.parse_numeric(&[0x80, 0, 0, 0, 0, 0, 0, 1]), 1);
        assert_eq!(p.parse_numeric(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]), -1);
        assert!(!p.err);
        p.parse_numeric(b"0009\0");
        assert!(p.err);
    }

    #[test]
    fn pax_record() {
        let (k, v, r) = parse_pax_record(b"30 mtime=1432668921.098285006\n30 ctime=").unwrap();
        assert_eq!((&k[..], &v[..], r), (&b"mtime"[..], &b"1432668921.098285006"[..], &b"30 ctime="[..]));
        assert!(parse_pax_record(b"3 a=\n").is_none());
    }
}
