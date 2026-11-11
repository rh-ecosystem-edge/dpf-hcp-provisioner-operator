//! Port of `github.com/coreos/go-systemd/v22/unit.Deserialize`, including the
//! `bufio.Reader` (4096-byte buffer over a `strings.Reader`) state that decides when the
//! "line too long" check fires.

/// `unit.UnitOption`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct UnitOption {
    pub section: String,
    pub name: String,
    pub value: String,
}

const SYSTEMD_LINE_MAX: usize = 2048;
const BUF_SIZE: usize = 4096;

#[derive(Debug, PartialEq, Eq)]
enum IoErr {
    Eof,
}

/// `bufio.Reader` over an in-memory `strings.Reader`.
struct BufReader<'a> {
    src: &'a [u8],
    src_off: usize,
    buf: Vec<u8>,
    r: usize,
    w: usize,
    err: Option<IoErr>,
    last_rune_size: isize,
}

impl<'a> BufReader<'a> {
    fn new(src: &'a [u8]) -> Self {
        BufReader {
            src,
            src_off: 0,
            buf: vec![0; BUF_SIZE],
            r: 0,
            w: 0,
            err: None,
            last_rune_size: -1,
        }
    }

    fn fill(&mut self) {
        if self.r > 0 {
            self.buf.copy_within(self.r..self.w, 0);
            self.w -= self.r;
            self.r = 0;
        }
        // strings.Reader.Read: copies what fits, (0, EOF) once exhausted.
        let remaining = self.src.len() - self.src_off;
        if remaining == 0 {
            self.err = Some(IoErr::Eof);
            return;
        }
        let n = remaining.min(BUF_SIZE - self.w);
        self.buf[self.w..self.w + n].copy_from_slice(&self.src[self.src_off..self.src_off + n]);
        self.src_off += n;
        self.w += n;
    }

    fn read_err(&mut self) -> Option<IoErr> {
        self.err.take()
    }

    fn buffered(&self) -> usize {
        self.w - self.r
    }

    fn full_rune(b: &[u8]) -> bool {
        if b.is_empty() {
            return false;
        }
        let need = match b[0] {
            0x00..=0x7f => 1,
            0xc2..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf4 => 4,
            _ => 1, // invalid lead byte: decodes as RuneError of width 1
        };
        if b.len() >= need {
            return true;
        }
        // an invalid continuation also makes it a "full" (invalid) rune
        b[1..].iter().any(|&c| !(0x80..=0xbf).contains(&c))
    }

    fn read_rune(&mut self) -> Result<char, IoErr> {
        while self.r + 4 > self.w
            && !Self::full_rune(&self.buf[self.r..self.w])
            && self.err.is_none()
            && self.w - self.r < BUF_SIZE
        {
            self.fill();
        }
        self.last_rune_size = -1;
        if self.r == self.w {
            return Err(self.read_err().unwrap_or(IoErr::Eof));
        }
        let b = &self.buf[self.r..self.w];
        let (c, size) = if b[0] < 0x80 {
            (b[0] as char, 1)
        } else {
            decode_rune(b)
        };
        self.r += size;
        self.last_rune_size = size as isize;
        Ok(c)
    }

    fn unread_rune(&mut self) {
        if self.last_rune_size >= 0 && self.r >= self.last_rune_size as usize {
            self.r -= self.last_rune_size as usize;
            self.last_rune_size = -1;
        }
    }

    /// `ReadSlice`: (bytes, Some(err)) with err = None for found, Some(true) for buffer full,
    /// Some(false) for EOF.
    fn read_slice(&mut self, delim: u8) -> (Vec<u8>, Option<bool>) {
        let mut s = 0;
        loop {
            if let Some(i) = self.buf[self.r + s..self.w]
                .iter()
                .position(|&c| c == delim)
            {
                let i = i + s;
                let line = self.buf[self.r..self.r + i + 1].to_vec();
                self.r += i + 1;
                return (line, None);
            }
            if self.err.is_some() {
                let line = self.buf[self.r..self.w].to_vec();
                self.r = self.w;
                self.read_err();
                return (line, Some(false));
            }
            if self.buffered() >= BUF_SIZE {
                self.r = self.w;
                return (self.buf.clone(), Some(true));
            }
            s = self.w - self.r;
            self.fill();
        }
    }

    /// `ReadBytes`: (bytes, eof).
    fn read_bytes(&mut self, delim: u8) -> (Vec<u8>, bool) {
        let mut out = Vec::new();
        loop {
            let (frag, e) = self.read_slice(delim);
            out.extend_from_slice(&frag);
            match e {
                None => return (out, false),
                Some(false) => return (out, true),
                Some(true) => continue,
            }
        }
    }
}

/// `utf8.DecodeRune` for a non-ASCII lead byte.
fn decode_rune(b: &[u8]) -> (char, usize) {
    for n in (2..=4).rev() {
        if b.len() >= n
            && let Ok(s) = std::str::from_utf8(&b[..n])
            && let Some(c) = s.chars().next()
            && c.len_utf8() == n
        {
            return (c, n);
        }
    }
    ('\u{fffd}', 1)
}

fn is_comment(r: char) -> bool {
    r == '#' || r == ';'
}

fn trim_space(b: &[u8]) -> &[u8] {
    gocompat::unicode::trim_space(b)
}

enum Step {
    NextSection,
    SectionName,
    SectionSuffix(String),
    IgnoreLine(Box<Step>),
    NextSectionOrOption(String),
    OptionName(String),
    OptionValue(String, String, Vec<u8>),
}

/// `unit.Deserialize`: the options, or the error message.
pub(crate) fn deserialize(content: &[u8]) -> Result<Vec<UnitOption>, String> {
    let mut l = BufReader::new(content);
    let mut options = Vec::new();
    let mut next = Some(Step::NextSection);
    while let Some(step) = next {
        if l.buffered() >= SYSTEMD_LINE_MAX {
            let line = &l.buf[l.r..l.r + SYSTEMD_LINE_MAX];
            if !line.iter().any(|&c| c == b'\r' || c == b'\n') {
                return Err(format!("line too long (max {SYSTEMD_LINE_MAX} bytes)"));
            }
        }
        next = match step {
            Step::NextSection => match l.read_rune() {
                Err(IoErr::Eof) => None,
                Ok('[') => Some(Step::SectionName),
                Ok(r) if is_comment(r) => Some(Step::IgnoreLine(Box::new(Step::NextSection))),
                Ok(_) => Some(Step::NextSection),
            },
            Step::SectionName => {
                let (sec, eof) = l.read_bytes(b']');
                if eof {
                    return Err("unable to find end of section".into());
                }
                Some(Step::SectionSuffix(
                    String::from_utf8_lossy(&sec[..sec.len() - 1]).into_owned(),
                ))
            }
            Step::SectionSuffix(section) => {
                let (garbage, _) = to_eol(&mut l);
                let garbage = trim_space(&garbage);
                if !garbage.is_empty() {
                    return Err(format!(
                        "found garbage after section name : {}",
                        gocompat::strconv::quote(garbage)
                    ));
                }
                Some(Step::NextSectionOrOption(section))
            }
            Step::IgnoreLine(next) => {
                loop {
                    let (line, _) = to_eol(&mut l);
                    let line = line.strip_suffix(b" ").unwrap_or(&line);
                    if !line.ends_with(b"\\") {
                        break;
                    }
                }
                Some(*next)
            }
            Step::NextSectionOrOption(section) => match l.read_rune() {
                Err(IoErr::Eof) => None,
                Ok(r) if gocompat::unicode::is_space(r as u32) => {
                    Some(Step::NextSectionOrOption(section))
                }
                Ok('[') => Some(Step::SectionName),
                Ok(r) if is_comment(r) => Some(Step::IgnoreLine(Box::new(
                    Step::NextSectionOrOption(section),
                ))),
                Ok(_) => {
                    l.unread_rune();
                    Some(Step::OptionName(section))
                }
            },
            Step::OptionName(section) => {
                let mut partial = String::new();
                loop {
                    match l.read_rune() {
                        Err(IoErr::Eof) => return Err("EOF".into()),
                        Ok('\n') | Ok('\r') => {
                            return Err(
                                "unexpected newline encountered while parsing option name".into()
                            );
                        }
                        Ok('=') => break,
                        Ok(r) => partial.push(r),
                    }
                }
                let name = String::from_utf8_lossy(trim_space(partial.as_bytes())).into_owned();
                Some(Step::OptionValue(section, name, Vec::new()))
            }
            Step::OptionValue(section, name, mut partial) => {
                let (line, eof) = to_eol(&mut l);
                if !trim_space(&line).is_empty() {
                    partial.extend_from_slice(&line);
                    if line.ends_with(b"\\") {
                        if !eof {
                            partial.push(b'\n');
                        }
                        next = Some(Step::OptionValue(section, name, partial));
                        continue;
                    }
                }
                let val = if partial.ends_with(b"\n") {
                    let mut v = trim_space(&partial).to_vec();
                    v.push(b'\n');
                    v
                } else {
                    trim_space(&partial).to_vec()
                };
                options.push(UnitOption {
                    section: section.clone(),
                    name,
                    value: String::from_utf8_lossy(&val).into_owned(),
                });
                Some(Step::NextSectionOrOption(section))
            }
        };
    }
    Ok(options)
}

/// `toEOL`: reads until the end of line or file; returns (line, eof).
fn to_eol(l: &mut BufReader<'_>) -> (Vec<u8>, bool) {
    let (mut line, eof) = l.read_bytes(b'\n');
    if line.ends_with(b"\r") {
        line.pop();
    }
    if line.ends_with(b"\n") {
        line.pop();
    }
    (line, eof)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic() {
        let o = deserialize(b"[Unit]\nDescription=Test\n[Service]\nExecStart=/bin/true\n[Install]\nWantedBy=multi-user.target").unwrap();
        assert_eq!(o.len(), 3);
        assert_eq!(o[2].section, "Install");
        assert_eq!(o[2].value, "multi-user.target");
        assert_eq!(deserialize(b"[Unit]\nFoo").unwrap_err(), "EOF");
        assert_eq!(
            deserialize(b"[Unit").unwrap_err(),
            "unable to find end of section"
        );
        assert_eq!(
            deserialize(b"[Unit] x\n").unwrap_err(),
            "found garbage after section name : \"x\""
        );
        assert_eq!(
            deserialize(b"[Unit]\nFoo\nBar=1").unwrap_err(),
            "unexpected newline encountered while parsing option name"
        );
        let long = format!("[Unit]\nA={}\n", "x".repeat(5000));
        assert_eq!(
            deserialize(long.as_bytes()).unwrap_err(),
            "line too long (max 2048 bytes)"
        );
    }
}
