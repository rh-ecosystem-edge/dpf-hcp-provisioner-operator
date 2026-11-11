//! Port of Go `compress/flate` deflate.go and deflatefast.go.

use super::huffman::{HuffmanBitWriter, MAX_STORE_BLOCK_SIZE, Token, literal_token, match_token};

/// `flate.NoCompression`.
pub const NO_COMPRESSION: i32 = 0;
/// `flate.BestSpeed`.
pub const BEST_SPEED: i32 = 1;
/// `flate.BestCompression`.
pub const BEST_COMPRESSION: i32 = 9;
/// `flate.DefaultCompression`.
pub const DEFAULT_COMPRESSION: i32 = -1;
/// `flate.HuffmanOnly`.
pub const HUFFMAN_ONLY: i32 = -2;

const LOG_WINDOW_SIZE: usize = 15;
const WINDOW_SIZE: usize = 1 << LOG_WINDOW_SIZE;
const WINDOW_MASK: usize = WINDOW_SIZE - 1;

const BASE_MATCH_LENGTH: usize = 3;
const MIN_MATCH_LENGTH: usize = 4;
const MAX_MATCH_LENGTH: usize = 258;
const BASE_MATCH_OFFSET: usize = 1;
const MAX_MATCH_OFFSET: usize = 1 << 15;

const MAX_FLATE_BLOCK_TOKENS: usize = 1 << 14;
const HASH_BITS: u32 = 17;
const HASH_SIZE: usize = 1 << HASH_BITS;
const HASH_MASK: u32 = (1 << HASH_BITS) - 1;
const MAX_HASH_OFFSET: i64 = 1 << 24;

const SKIP_NEVER: i64 = i32::MAX as i64;

#[derive(Clone, Copy, Default)]
struct CompressionLevel {
    level: i64,
    good: i64,
    lazy: i64,
    nice: i64,
    chain: i64,
    fast_skip_hashing: i64,
}

const fn lvl(level: i64, good: i64, lazy: i64, nice: i64, chain: i64, fast: i64) -> CompressionLevel {
    CompressionLevel {
        level,
        good,
        lazy,
        nice,
        chain,
        fast_skip_hashing: fast,
    }
}

static LEVELS: [CompressionLevel; 10] = [
    lvl(0, 0, 0, 0, 0, 0),
    lvl(1, 0, 0, 0, 0, 0),
    lvl(2, 4, 0, 16, 8, 5),
    lvl(3, 4, 0, 32, 32, 6),
    lvl(4, 4, 4, 16, 16, SKIP_NEVER),
    lvl(5, 8, 16, 32, 32, SKIP_NEVER),
    lvl(6, 8, 16, 128, 128, SKIP_NEVER),
    lvl(7, 8, 32, 128, 256, SKIP_NEVER),
    lvl(8, 32, 128, 258, 1024, SKIP_NEVER),
    lvl(9, 32, 258, 258, 4096, SKIP_NEVER),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Store,
    HuffOnly,
    Speed,
    Deflate,
}

const HASHMUL: u32 = 0x1e35a7bd;

fn hash4(b: &[u8]) -> u32 {
    ((b[3] as u32 | (b[2] as u32) << 8 | (b[1] as u32) << 16 | (b[0] as u32) << 24).wrapping_mul(HASHMUL))
        >> (32 - HASH_BITS)
}

fn match_len(a: &[u8], b: &[u8], max: usize) -> usize {
    let a = &a[..max];
    let b = &b[..a.len()];
    for (i, &av) in a.iter().enumerate() {
        if b[i] != av {
            return i;
        }
    }
    max
}

/// Error returned by [`Writer::new`] for an unsupported level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvalidLevelError(pub i32);

impl std::fmt::Display for InvalidLevelError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "flate: invalid compression level {}: want value in range [-2, 9]", self.0)
    }
}

impl std::error::Error for InvalidLevelError {}

/// Go `flate.Writer`. Compressed bytes accumulate in an internal buffer; take them with
/// [`Writer::take_output`] (or [`Writer::finish`]).
pub struct Writer {
    cl: CompressionLevel,
    mode: Mode,
    w: HuffmanBitWriter,
    best_speed: Option<Box<DeflateFast>>,

    index: i64,
    window: Vec<u8>,
    window_end: i64,
    block_start: i64,
    byte_available: bool,
    sync: bool,
    tokens: Vec<Token>,
    length: i64,
    offset: i64,
    max_insert_index: i64,
    closed: bool,

    chain_head: i64,
    hash_head: Vec<u32>,
    hash_prev: Vec<u32>,
    hash_offset: i64,
}

impl Writer {
    /// Go `flate.NewWriter(w, level)`.
    pub fn new(level: i32) -> Result<Writer, InvalidLevelError> {
        let mut d = Writer {
            cl: CompressionLevel::default(),
            mode: Mode::Store,
            w: HuffmanBitWriter::new(),
            best_speed: None,
            index: 0,
            window: Vec::new(),
            window_end: 0,
            block_start: 0,
            byte_available: false,
            sync: false,
            tokens: Vec::new(),
            length: 0,
            offset: 0,
            max_insert_index: 0,
            closed: false,
            chain_head: 0,
            hash_head: Vec::new(),
            hash_prev: Vec::new(),
            hash_offset: 0,
        };
        match level {
            NO_COMPRESSION => {
                d.window = vec![0; MAX_STORE_BLOCK_SIZE];
                d.mode = Mode::Store;
            }
            HUFFMAN_ONLY => {
                d.window = vec![0; MAX_STORE_BLOCK_SIZE];
                d.mode = Mode::HuffOnly;
            }
            BEST_SPEED => {
                d.cl = LEVELS[1];
                d.window = vec![0; MAX_STORE_BLOCK_SIZE];
                d.mode = Mode::Speed;
                d.best_speed = Some(Box::new(DeflateFast::new()));
                d.tokens = Vec::with_capacity(MAX_STORE_BLOCK_SIZE);
            }
            DEFAULT_COMPRESSION | 2..=9 => {
                let level = if level == DEFAULT_COMPRESSION { 6 } else { level };
                d.cl = LEVELS[level as usize];
                d.mode = Mode::Deflate;
                d.window = vec![0; 2 * WINDOW_SIZE];
                d.hash_offset = 1;
                d.tokens = Vec::with_capacity(MAX_FLATE_BLOCK_TOKENS + 1);
                d.length = MIN_MATCH_LENGTH as i64 - 1;
                d.offset = 0;
                d.byte_available = false;
                d.index = 0;
                d.chain_head = -1;
                d.hash_head = vec![0; HASH_SIZE];
                d.hash_prev = vec![0; WINDOW_SIZE];
            }
            _ => return Err(InvalidLevelError(level)),
        }
        Ok(d)
    }

    /// Takes the compressed bytes produced so far.
    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.w.out)
    }

    fn step(&mut self) {
        match self.mode {
            Mode::Store => self.store(),
            Mode::HuffOnly => self.store_huff(),
            Mode::Speed => self.enc_speed(),
            Mode::Deflate => self.deflate(),
        }
    }

    fn fill(&mut self, b: &[u8]) -> usize {
        match self.mode {
            Mode::Deflate => self.fill_deflate(b),
            _ => self.fill_store(b),
        }
    }

    fn fill_deflate(&mut self, b: &[u8]) -> usize {
        if self.index >= (2 * WINDOW_SIZE - (MIN_MATCH_LENGTH + MAX_MATCH_LENGTH)) as i64 {
            self.window.copy_within(WINDOW_SIZE..2 * WINDOW_SIZE, 0);
            self.index -= WINDOW_SIZE as i64;
            self.window_end -= WINDOW_SIZE as i64;
            if self.block_start >= WINDOW_SIZE as i64 {
                self.block_start -= WINDOW_SIZE as i64;
            } else {
                self.block_start = i32::MAX as i64;
            }
            self.hash_offset += WINDOW_SIZE as i64;
            if self.hash_offset > MAX_HASH_OFFSET {
                let delta = self.hash_offset - 1;
                self.hash_offset -= delta;
                self.chain_head -= delta;
                for v in self.hash_prev.iter_mut().chain(self.hash_head.iter_mut()) {
                    if *v as i64 > delta {
                        *v = (*v as i64 - delta) as u32;
                    } else {
                        *v = 0;
                    }
                }
            }
        }
        let we = self.window_end as usize;
        let n = b.len().min(self.window.len() - we);
        self.window[we..we + n].copy_from_slice(&b[..n]);
        self.window_end += n as i64;
        n
    }

    fn write_block(&mut self, index: i64) {
        if index > 0 {
            let window = if self.block_start <= index {
                Some(&self.window[self.block_start as usize..index as usize])
            } else {
                None
            };
            self.block_start = index;
            self.w.write_block(&self.tokens, false, window);
        }
    }

    fn find_match(&self, pos: i64, prev_head: i64, prev_length: i64, lookahead: i64) -> (i64, i64, bool) {
        let mut min_match_look = MAX_MATCH_LENGTH as i64;
        if lookahead < min_match_look {
            min_match_look = lookahead;
        }
        let win = &self.window[0..(pos + min_match_look) as usize];
        let mut nice = win.len() as i64 - pos;
        if self.cl.nice < nice {
            nice = self.cl.nice;
        }
        let mut tries = self.cl.chain;
        let mut length = prev_length;
        if length >= self.cl.good {
            tries >>= 2;
        }
        let mut w_end = win[(pos + length) as usize];
        let w_pos = &win[pos as usize..];
        let min_index = pos - WINDOW_SIZE as i64;
        let mut offset = 0;
        let mut ok = false;

        let mut i = prev_head;
        while tries > 0 {
            if w_end == win[(i + length) as usize] {
                let n = match_len(&win[i as usize..], w_pos, min_match_look as usize) as i64;
                if n > length && (n > MIN_MATCH_LENGTH as i64 || pos - i <= 4096) {
                    length = n;
                    offset = pos - i;
                    ok = true;
                    if n >= nice {
                        break;
                    }
                    w_end = win[(pos + n) as usize];
                }
            }
            if i == min_index {
                break;
            }
            i = self.hash_prev[(i as usize) & WINDOW_MASK] as i64 - self.hash_offset;
            if i < min_index || i < 0 {
                break;
            }
            tries -= 1;
        }
        (length, offset, ok)
    }

    fn write_stored_block(&mut self, start: usize, end: usize) {
        self.w.write_stored_header(end - start, false);
        let Writer { w, window, .. } = self;
        w.write_bytes(&window[start..end]);
    }

    fn enc_speed(&mut self) {
        if self.window_end < MAX_STORE_BLOCK_SIZE as i64 {
            if !self.sync {
                return;
            }
            if self.window_end < 128 {
                if self.window_end == 0 {
                    return;
                } else if self.window_end <= 16 {
                    self.write_stored_block(0, self.window_end as usize);
                } else {
                    let Writer { w, window, window_end, .. } = self;
                    w.write_block_huff(false, &window[..*window_end as usize]);
                }
                self.window_end = 0;
                if let Some(bs) = self.best_speed.as_mut() {
                    bs.reset();
                }
                return;
            }
        }
        let we = self.window_end as usize;
        let mut tokens = std::mem::take(&mut self.tokens);
        tokens.clear();
        if let Some(bs) = self.best_speed.as_mut() {
            bs.encode(&mut tokens, &self.window[..we]);
        }
        if tokens.len() > we - (we >> 4) {
            self.w.write_block_huff(false, &self.window[..we]);
        } else {
            self.w.write_block_dynamic(&tokens, false, Some(&self.window[..we]));
        }
        self.tokens = tokens;
        self.window_end = 0;
    }

    fn deflate(&mut self) {
        let mml = MIN_MATCH_LENGTH as i64;
        if self.window_end - self.index < (MIN_MATCH_LENGTH + MAX_MATCH_LENGTH) as i64 && !self.sync {
            return;
        }
        self.max_insert_index = self.window_end - (mml - 1);

        loop {
            let lookahead = self.window_end - self.index;
            if lookahead < (MIN_MATCH_LENGTH + MAX_MATCH_LENGTH) as i64 {
                if !self.sync {
                    break;
                }
                if lookahead == 0 {
                    if self.byte_available {
                        let b = self.window[self.index as usize - 1];
                        self.tokens.push(literal_token(b as u32));
                        self.byte_available = false;
                    }
                    if !self.tokens.is_empty() {
                        self.write_block(self.index);
                        self.tokens.clear();
                    }
                    break;
                }
            }
            if self.index < self.max_insert_index {
                let idx = self.index as usize;
                let hash = hash4(&self.window[idx..idx + MIN_MATCH_LENGTH]);
                let hh = &mut self.hash_head[(hash & HASH_MASK) as usize];
                self.chain_head = *hh as i64;
                self.hash_prev[idx & WINDOW_MASK] = self.chain_head as u32;
                *hh = (self.index + self.hash_offset) as u32;
            }
            let prev_length = self.length;
            let prev_offset = self.offset;
            self.length = mml - 1;
            self.offset = 0;
            let mut min_index = self.index - WINDOW_SIZE as i64;
            if min_index < 0 {
                min_index = 0;
            }

            let fsh = self.cl.fast_skip_hashing;
            if self.chain_head - self.hash_offset >= min_index
                && (fsh != SKIP_NEVER && lookahead > mml - 1
                    || fsh == SKIP_NEVER && lookahead > prev_length && prev_length < self.cl.lazy)
            {
                let (new_length, new_offset, ok) =
                    self.find_match(self.index, self.chain_head - self.hash_offset, mml - 1, lookahead);
                if ok {
                    self.length = new_length;
                    self.offset = new_offset;
                }
            }
            if fsh != SKIP_NEVER && self.length >= mml
                || fsh == SKIP_NEVER && prev_length >= mml && self.length <= prev_length
            {
                if fsh != SKIP_NEVER {
                    self.tokens.push(match_token(
                        (self.length - BASE_MATCH_LENGTH as i64) as u32,
                        (self.offset - BASE_MATCH_OFFSET as i64) as u32,
                    ));
                } else {
                    self.tokens.push(match_token(
                        (prev_length - BASE_MATCH_LENGTH as i64) as u32,
                        (prev_offset - BASE_MATCH_OFFSET as i64) as u32,
                    ));
                }
                if self.length <= fsh {
                    let new_index = if fsh != SKIP_NEVER {
                        self.index + self.length
                    } else {
                        self.index + prev_length - 1
                    };
                    let mut index = self.index + 1;
                    while index < new_index {
                        if index < self.max_insert_index {
                            let idx = index as usize;
                            let hash = hash4(&self.window[idx..idx + MIN_MATCH_LENGTH]);
                            let hh = &mut self.hash_head[(hash & HASH_MASK) as usize];
                            self.hash_prev[idx & WINDOW_MASK] = *hh;
                            *hh = (index + self.hash_offset) as u32;
                        }
                        index += 1;
                    }
                    self.index = index;
                    if fsh == SKIP_NEVER {
                        self.byte_available = false;
                        self.length = mml - 1;
                    }
                } else {
                    self.index += self.length;
                }
                if self.tokens.len() == MAX_FLATE_BLOCK_TOKENS {
                    self.write_block(self.index);
                    self.tokens.clear();
                }
            } else {
                if fsh != SKIP_NEVER || self.byte_available {
                    let i = if fsh != SKIP_NEVER { self.index } else { self.index - 1 };
                    let b = self.window[i as usize];
                    self.tokens.push(literal_token(b as u32));
                    if self.tokens.len() == MAX_FLATE_BLOCK_TOKENS {
                        self.write_block(i + 1);
                        self.tokens.clear();
                    }
                }
                self.index += 1;
                if fsh == SKIP_NEVER {
                    self.byte_available = true;
                }
            }
        }
    }

    fn fill_store(&mut self, b: &[u8]) -> usize {
        let we = self.window_end as usize;
        let n = b.len().min(self.window.len() - we);
        self.window[we..we + n].copy_from_slice(&b[..n]);
        self.window_end += n as i64;
        n
    }

    fn store(&mut self) {
        if self.window_end > 0 && (self.window_end == MAX_STORE_BLOCK_SIZE as i64 || self.sync) {
            self.write_stored_block(0, self.window_end as usize);
            self.window_end = 0;
        }
    }

    fn store_huff(&mut self) {
        if self.window_end < self.window.len() as i64 && !self.sync || self.window_end == 0 {
            return;
        }
        let Writer { w, window, window_end, .. } = self;
        w.write_block_huff(false, &window[..*window_end as usize]);
        self.window_end = 0;
    }

    /// Go `(*flate.Writer).Write`.
    pub fn write(&mut self, data: &[u8]) -> usize {
        if self.closed {
            return 0;
        }
        let mut b = data;
        while !b.is_empty() {
            self.step();
            let n = self.fill(b);
            b = &b[n..];
        }
        data.len()
    }

    /// Go `(*flate.Writer).Flush` (sync flush).
    pub fn flush(&mut self) {
        if self.closed {
            return;
        }
        self.sync = true;
        self.step();
        self.w.write_stored_header(0, false);
        self.w.flush();
        self.sync = false;
    }

    /// Go `(*flate.Writer).Close`.
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.sync = true;
        self.step();
        self.w.write_stored_header(0, true);
        self.w.flush();
        self.closed = true;
    }

    /// Closes the writer and returns all remaining output.
    pub fn finish(mut self) -> Vec<u8> {
        self.close();
        self.take_output()
    }

    /// Go `(*flate.Writer).Reset`: discards state and output, keeping the level.
    pub fn reset(&mut self) {
        self.w.reset();
        self.w.out.clear();
        self.sync = false;
        self.closed = false;
        match self.cl.level {
            0 if self.mode != Mode::Deflate => self.window_end = 0,
            1 => {
                self.window_end = 0;
                self.tokens.clear();
                if let Some(bs) = self.best_speed.as_mut() {
                    bs.reset();
                }
            }
            _ => {
                if self.mode == Mode::Deflate {
                    self.chain_head = -1;
                    self.hash_head.iter_mut().for_each(|v| *v = 0);
                    self.hash_prev.iter_mut().for_each(|v| *v = 0);
                    self.hash_offset = 1;
                    self.index = 0;
                    self.window_end = 0;
                    self.block_start = 0;
                    self.byte_available = false;
                    self.tokens.clear();
                    self.length = MIN_MATCH_LENGTH as i64 - 1;
                    self.offset = 0;
                    self.max_insert_index = 0;
                } else {
                    self.window_end = 0;
                }
            }
        }
    }
}

/// Compresses `data` in a single `Write` followed by `Close`, like
/// `w, _ := flate.NewWriter(&buf, level); w.Write(data); w.Close()`.
pub fn compress(data: &[u8], level: i32) -> Result<Vec<u8>, InvalidLevelError> {
    let mut w = Writer::new(level)?;
    w.write(data);
    Ok(w.finish())
}

// ----- deflatefast.go -----

const TABLE_BITS: u32 = 14;
const TABLE_SIZE: usize = 1 << TABLE_BITS;
const TABLE_MASK: u32 = (TABLE_SIZE - 1) as u32;
const TABLE_SHIFT: u32 = 32 - TABLE_BITS;
const BUFFER_RESET: i32 = i32::MAX - (MAX_STORE_BLOCK_SIZE as i32) * 2;
const INPUT_MARGIN: i32 = 16 - 1;
const MIN_NON_LITERAL_BLOCK_SIZE: usize = 1 + 1 + INPUT_MARGIN as usize;

fn load32(b: &[u8], i: i32) -> u32 {
    let i = i as usize;
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}

fn load64(b: &[u8], i: i32) -> u64 {
    let i = i as usize;
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[i..i + 8]);
    u64::from_le_bytes(a)
}

fn fhash(u: u32) -> u32 {
    u.wrapping_mul(0x1e35a7bd) >> TABLE_SHIFT
}

#[derive(Clone, Copy, Default)]
struct TableEntry {
    val: u32,
    offset: i32,
}

struct DeflateFast {
    table: Vec<TableEntry>,
    prev: Vec<u8>,
    cur: i32,
}

fn emit_literal(dst: &mut Vec<Token>, lit: &[u8]) {
    for &v in lit {
        dst.push(literal_token(v as u32));
    }
}

impl DeflateFast {
    fn new() -> Self {
        DeflateFast {
            table: vec![TableEntry::default(); TABLE_SIZE],
            prev: Vec::with_capacity(MAX_STORE_BLOCK_SIZE),
            cur: MAX_STORE_BLOCK_SIZE as i32,
        }
    }

    fn encode(&mut self, dst: &mut Vec<Token>, src: &[u8]) {
        if self.cur >= BUFFER_RESET {
            self.shift_offsets();
        }
        if src.len() < MIN_NON_LITERAL_BLOCK_SIZE {
            self.cur += MAX_STORE_BLOCK_SIZE as i32;
            self.prev.clear();
            emit_literal(dst, src);
            return;
        }
        let s_limit = src.len() as i32 - INPUT_MARGIN;
        let mut next_emit: i32 = 0;
        let mut s: i32 = 0;
        let mut cv = load32(src, s);
        let mut next_hash = fhash(cv);

        'outer: loop {
            let mut skip: i32 = 32;
            let mut next_s = s;
            let mut candidate;
            loop {
                s = next_s;
                let bytes_between = skip >> 5;
                next_s = s + bytes_between;
                skip += bytes_between;
                if next_s > s_limit {
                    break 'outer;
                }
                candidate = self.table[(next_hash & TABLE_MASK) as usize];
                let now = load32(src, next_s);
                self.table[(next_hash & TABLE_MASK) as usize] = TableEntry {
                    offset: s + self.cur,
                    val: cv,
                };
                next_hash = fhash(now);
                let offset = s - (candidate.offset - self.cur);
                if offset > MAX_MATCH_OFFSET as i32 || cv != candidate.val {
                    cv = now;
                    continue;
                }
                break;
            }
            emit_literal(dst, &src[next_emit as usize..s as usize]);
            loop {
                s += 4;
                let t = candidate.offset - self.cur + 4;
                let l = self.match_len(s, t, src);
                dst.push(match_token(
                    (l + 4 - BASE_MATCH_LENGTH as i32) as u32,
                    (s - t - BASE_MATCH_OFFSET as i32) as u32,
                ));
                s += l;
                next_emit = s;
                if s >= s_limit {
                    break 'outer;
                }
                let mut x = load64(src, s - 1);
                let prev_hash = fhash(x as u32);
                self.table[(prev_hash & TABLE_MASK) as usize] = TableEntry {
                    offset: self.cur + s - 1,
                    val: x as u32,
                };
                x >>= 8;
                let curr_hash = fhash(x as u32);
                candidate = self.table[(curr_hash & TABLE_MASK) as usize];
                self.table[(curr_hash & TABLE_MASK) as usize] = TableEntry {
                    offset: self.cur + s,
                    val: x as u32,
                };
                let offset = s - (candidate.offset - self.cur);
                if offset > MAX_MATCH_OFFSET as i32 || x as u32 != candidate.val {
                    cv = (x >> 8) as u32;
                    next_hash = fhash(cv);
                    s += 1;
                    break;
                }
            }
        }
        // emitRemainder:
        if (next_emit as usize) < src.len() {
            emit_literal(dst, &src[next_emit as usize..]);
        }
        self.cur += src.len() as i32;
        self.prev.clear();
        self.prev.extend_from_slice(src);
    }

    fn match_len(&self, s: i32, t: i32, src: &[u8]) -> i32 {
        let mut s1 = s as usize + MAX_MATCH_LENGTH - 4;
        if s1 > src.len() {
            s1 = src.len();
        }
        if t >= 0 {
            let b = &src[t as usize..];
            let a = &src[s as usize..s1];
            let b = &b[..a.len()];
            for i in 0..a.len() {
                if a[i] != b[i] {
                    return i as i32;
                }
            }
            return a.len() as i32;
        }
        let tp = self.prev.len() as i32 + t;
        if tp < 0 {
            return 0;
        }
        let mut a = &src[s as usize..s1];
        let mut b = &self.prev[tp as usize..];
        if b.len() > a.len() {
            b = &b[..a.len()];
        }
        a = &a[..b.len()];
        for i in 0..b.len() {
            if a[i] != b[i] {
                return i as i32;
            }
        }
        let n = b.len() as i32;
        if (s + n) as usize == s1 {
            return n;
        }
        let a = &src[(s + n) as usize..s1];
        let b = &src[..a.len()];
        for i in 0..a.len() {
            if a[i] != b[i] {
                return i as i32 + n;
            }
        }
        a.len() as i32 + n
    }

    fn reset(&mut self) {
        self.prev.clear();
        self.cur += MAX_MATCH_OFFSET as i32;
        if self.cur >= BUFFER_RESET {
            self.shift_offsets();
        }
    }

    fn shift_offsets(&mut self) {
        if self.prev.is_empty() {
            self.table.iter_mut().for_each(|e| *e = TableEntry::default());
            self.cur = MAX_MATCH_OFFSET as i32 + 1;
            return;
        }
        for e in self.table.iter_mut() {
            let mut v = e.offset - self.cur + MAX_MATCH_OFFSET as i32 + 1;
            if v < 0 {
                v = 0;
            }
            e.offset = v;
        }
        self.cur = MAX_MATCH_OFFSET as i32 + 1;
    }
}
