//! Port of Go `compress/flate` inflate.go (decompressor) over an in-memory input, keeping
//! Go's byte-consumption pattern so error values (`CorruptInputError` offsets,
//! `unexpected EOF`) and the position where the stream ends are identical.

use std::fmt;
use std::sync::LazyLock;

const MAX_CODE_LEN: usize = 16;
const MAX_NUM_LIT: usize = 286;
const MAX_NUM_DIST: usize = 30;
const NUM_CODES: usize = 19;
const END_BLOCK_MARKER: usize = 256;
const MAX_MATCH_OFFSET: usize = 1 << 15;

const HUFFMAN_CHUNK_BITS: u32 = 9;
const HUFFMAN_NUM_CHUNKS: usize = 1 << HUFFMAN_CHUNK_BITS;
const HUFFMAN_COUNT_MASK: u32 = 15;
const HUFFMAN_VALUE_SHIFT: u32 = 4;

/// Errors produced while inflating (the subset of Go errors reachable with in-memory input).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InflateError {
    /// `flate.CorruptInputError`.
    Corrupt(i64),
    /// `io.ErrUnexpectedEOF`.
    UnexpectedEof,
    /// `flate.InternalError`.
    Internal(String),
}

impl fmt::Display for InflateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InflateError::Corrupt(off) => write!(f, "flate: corrupt input before offset {off}"),
            InflateError::UnexpectedEof => f.write_str("unexpected EOF"),
            InflateError::Internal(s) => write!(f, "flate: internal error: {s}"),
        }
    }
}

impl std::error::Error for InflateError {}

#[derive(Clone)]
struct HuffmanDecoder {
    min: u32,
    chunks: Vec<u32>,
    links: Vec<Vec<u32>>,
    link_mask: u32,
}

impl HuffmanDecoder {
    fn new() -> Self {
        HuffmanDecoder {
            min: 0,
            chunks: vec![0; HUFFMAN_NUM_CHUNKS],
            links: Vec::new(),
            link_mask: 0,
        }
    }

    fn init(&mut self, lengths: &[usize]) -> bool {
        if self.min != 0 {
            *self = HuffmanDecoder::new();
        }
        let mut count = [0usize; MAX_CODE_LEN];
        let mut min = 0usize;
        let mut max = 0usize;
        for &n in lengths {
            if n == 0 {
                continue;
            }
            if min == 0 || n < min {
                min = n;
            }
            if n > max {
                max = n;
            }
            count[n] += 1;
        }
        if max == 0 {
            return true;
        }
        let mut code = 0usize;
        let mut nextcode = [0usize; MAX_CODE_LEN];
        for i in min..=max {
            code <<= 1;
            nextcode[i] = code;
            code += count[i];
        }
        if code != 1 << max && !(code == 1 && max == 1) {
            return false;
        }
        self.min = min as u32;
        if max > HUFFMAN_CHUNK_BITS as usize {
            let num_links = 1usize << (max - HUFFMAN_CHUNK_BITS as usize);
            self.link_mask = (num_links - 1) as u32;
            let link = nextcode[HUFFMAN_CHUNK_BITS as usize + 1] >> 1;
            self.links = vec![Vec::new(); HUFFMAN_NUM_CHUNKS - link];
            for j in link..HUFFMAN_NUM_CHUNKS {
                let reverse = ((j as u16).reverse_bits() as usize) >> (16 - HUFFMAN_CHUNK_BITS);
                let off = j - link;
                self.chunks[reverse] = ((off as u32) << HUFFMAN_VALUE_SHIFT) | (HUFFMAN_CHUNK_BITS + 1);
                self.links[off] = vec![0; num_links];
            }
        }
        for (i, &n) in lengths.iter().enumerate() {
            if n == 0 {
                continue;
            }
            let code = nextcode[n];
            nextcode[n] += 1;
            let chunk = ((i as u32) << HUFFMAN_VALUE_SHIFT) | n as u32;
            let mut reverse = ((code as u16).reverse_bits() as usize) >> (16 - n);
            if n <= HUFFMAN_CHUNK_BITS as usize {
                let mut off = reverse;
                while off < self.chunks.len() {
                    self.chunks[off] = chunk;
                    off += 1 << n;
                }
            } else {
                let j = reverse & (HUFFMAN_NUM_CHUNKS - 1);
                let value = (self.chunks[j] >> HUFFMAN_VALUE_SHIFT) as usize;
                reverse >>= HUFFMAN_CHUNK_BITS;
                let step = 1usize << (n - HUFFMAN_CHUNK_BITS as usize);
                if let Some(linktab) = self.links.get_mut(value) {
                    let mut off = reverse;
                    while off < linktab.len() {
                        linktab[off] = chunk;
                        off += step;
                    }
                }
            }
        }
        true
    }
}

static FIXED_HUFFMAN_DECODER: LazyLock<HuffmanDecoder> = LazyLock::new(|| {
    let mut bits = [0usize; 288];
    for (i, b) in bits.iter_mut().enumerate() {
        *b = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    let mut h = HuffmanDecoder::new();
    h.init(&bits);
    h
});

static CODE_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

struct BitReader<'a> {
    input: &'a [u8],
    pos: usize,
    roffset: i64,
    b: u32,
    nb: u32,
}

fn huff_sym(h: &HuffmanDecoder, s: &mut BitReader<'_>) -> Result<usize, InflateError> {
    let mut n = h.min;
    let mut nb = s.nb;
    let mut b = s.b;
    loop {
        while nb < n {
            let Some(c) = s.input.get(s.pos).copied() else {
                s.b = b;
                s.nb = nb;
                return Err(InflateError::UnexpectedEof);
            };
            s.pos += 1;
            s.roffset += 1;
            b |= (c as u32) << (nb & 31);
            nb += 8;
        }
        let mut chunk = h.chunks[(b as usize) & (HUFFMAN_NUM_CHUNKS - 1)];
        n = chunk & HUFFMAN_COUNT_MASK;
        if n > HUFFMAN_CHUNK_BITS {
            let idx = (chunk >> HUFFMAN_VALUE_SHIFT) as usize;
            chunk = h
                .links
                .get(idx)
                .and_then(|l| l.get(((b >> HUFFMAN_CHUNK_BITS) & h.link_mask) as usize))
                .copied()
                .unwrap_or(0);
            n = chunk & HUFFMAN_COUNT_MASK;
        }
        if n <= nb {
            if n == 0 {
                s.b = b;
                s.nb = nb;
                return Err(InflateError::Corrupt(s.roffset));
            }
            s.b = b >> (n & 31);
            s.nb = nb - n;
            return Ok((chunk >> HUFFMAN_VALUE_SHIFT) as usize);
        }
    }
}

/// An inflater over `input` starting at `pos`.
pub(crate) struct Inflater<'a> {
    br: BitReader<'a>,
    h1: HuffmanDecoder,
    h2: HuffmanDecoder,
    bits: Vec<usize>,
    codebits: [usize; NUM_CODES],
    final_block: bool,
    out: Vec<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Which {
    Fixed,
    Dynamic,
}

impl<'a> Inflater<'a> {
    pub(crate) fn new(input: &'a [u8], pos: usize) -> Self {
        Inflater {
            br: BitReader {
                input,
                pos,
                roffset: 0,
                b: 0,
                nb: 0,
            },
            h1: HuffmanDecoder::new(),
            h2: HuffmanDecoder::new(),
            bits: vec![0; MAX_NUM_LIT + MAX_NUM_DIST],
            codebits: [0; NUM_CODES],
            final_block: false,
            out: Vec::new(),
        }
    }

    /// Absolute read position in the input.
    pub(crate) fn pos(&self) -> usize {
        self.br.pos
    }

    fn more_bits(&mut self) -> Result<(), InflateError> {
        let c = *self.br.input.get(self.br.pos).ok_or(InflateError::UnexpectedEof)?;
        self.br.pos += 1;
        self.br.roffset += 1;
        self.br.b |= (c as u32) << self.br.nb;
        self.br.nb += 8;
        Ok(())
    }

    fn huff_sym(&mut self, which: Which, dist: bool) -> Result<usize, InflateError> {
        let h: &HuffmanDecoder = match (which, dist) {
            (Which::Fixed, _) => &FIXED_HUFFMAN_DECODER,
            (Which::Dynamic, false) => &self.h1,
            (Which::Dynamic, true) => &self.h2,
        };
        huff_sym(h, &mut self.br)
    }

    fn read_huffman(&mut self) -> Result<(), InflateError> {
        while self.br.nb < 5 + 5 + 4 {
            self.more_bits()?;
        }
        let nlit = (self.br.b & 0x1F) as usize + 257;
        if nlit > MAX_NUM_LIT {
            return Err(InflateError::Corrupt(self.br.roffset));
        }
        self.br.b >>= 5;
        let ndist = (self.br.b & 0x1F) as usize + 1;
        if ndist > MAX_NUM_DIST {
            return Err(InflateError::Corrupt(self.br.roffset));
        }
        self.br.b >>= 5;
        let nclen = (self.br.b & 0xF) as usize + 4;
        self.br.b >>= 4;
        self.br.nb -= 5 + 5 + 4;
        for &co in CODE_ORDER.iter().take(nclen) {
            while self.br.nb < 3 {
                self.more_bits()?;
            }
            self.codebits[co] = (self.br.b & 0x7) as usize;
            self.br.b >>= 3;
            self.br.nb -= 3;
        }
        for &co in CODE_ORDER.iter().skip(nclen) {
            self.codebits[co] = 0;
        }
        let cb = self.codebits;
        if !self.h1.init(&cb) {
            return Err(InflateError::Corrupt(self.br.roffset));
        }
        let n = nlit + ndist;
        let mut i = 0;
        while i < n {
            let x = self.huff_sym(Which::Dynamic, false)?;
            if x < 16 {
                self.bits[i] = x;
                i += 1;
                continue;
            }
            let (mut rep, nb, b) = match x {
                16 => {
                    if i == 0 {
                        return Err(InflateError::Corrupt(self.br.roffset));
                    }
                    (3usize, 2u32, self.bits[i - 1])
                }
                17 => (3, 3, 0),
                18 => (11, 7, 0),
                _ => return Err(InflateError::Internal("unexpected length code".to_string())),
            };
            while self.br.nb < nb {
                self.more_bits()?;
            }
            rep += (self.br.b & ((1u32 << nb) - 1)) as usize;
            self.br.b >>= nb;
            self.br.nb -= nb;
            if i + rep > n {
                return Err(InflateError::Corrupt(self.br.roffset));
            }
            for _ in 0..rep {
                self.bits[i] = b;
                i += 1;
            }
        }
        let lit = self.bits[0..nlit].to_vec();
        let dist = self.bits[nlit..nlit + ndist].to_vec();
        if !self.h1.init(&lit) || !self.h2.init(&dist) {
            return Err(InflateError::Corrupt(self.br.roffset));
        }
        if (self.h1.min as usize) < self.bits[END_BLOCK_MARKER] {
            self.h1.min = self.bits[END_BLOCK_MARKER] as u32;
        }
        Ok(())
    }

    fn hist_size(&self) -> usize {
        self.out.len().min(MAX_MATCH_OFFSET)
    }

    fn huffman_block(&mut self, which: Which) -> Result<(), InflateError> {
        loop {
            let v = self.huff_sym(which, false)?;
            let mut n: u32;
            let mut length: usize;
            if v < 256 {
                self.out.push(v as u8);
                continue;
            } else if v == 256 {
                return Ok(());
            } else if v < 265 {
                length = v - (257 - 3);
                n = 0;
            } else if v < 269 {
                length = v * 2 - (265 * 2 - 11);
                n = 1;
            } else if v < 273 {
                length = v * 4 - (269 * 4 - 19);
                n = 2;
            } else if v < 277 {
                length = v * 8 - (273 * 8 - 35);
                n = 3;
            } else if v < 281 {
                length = v * 16 - (277 * 16 - 67);
                n = 4;
            } else if v < 285 {
                length = v * 32 - (281 * 32 - 131);
                n = 5;
            } else if v < MAX_NUM_LIT {
                length = 258;
                n = 0;
            } else {
                return Err(InflateError::Corrupt(self.br.roffset));
            }
            if n > 0 {
                while self.br.nb < n {
                    self.more_bits()?;
                }
                length += (self.br.b & ((1u32 << n) - 1)) as usize;
                self.br.b >>= n;
                self.br.nb -= n;
            }
            let mut dist: usize;
            if which == Which::Fixed {
                while self.br.nb < 5 {
                    self.more_bits()?;
                }
                dist = (((self.br.b & 0x1F) << 3) as u8).reverse_bits() as usize;
                self.br.b >>= 5;
                self.br.nb -= 5;
            } else {
                dist = self.huff_sym(which, true)?;
            }
            if dist < 4 {
                dist += 1;
            } else if dist < MAX_NUM_DIST {
                n = ((dist - 2) >> 1) as u32;
                let mut extra = (dist & 1) << n;
                while self.br.nb < n {
                    self.more_bits()?;
                }
                extra |= (self.br.b & ((1u32 << n) - 1)) as usize;
                self.br.b >>= n;
                self.br.nb -= n;
                dist = (1 << (n + 1)) + 1 + extra;
            } else {
                return Err(InflateError::Corrupt(self.br.roffset));
            }
            if dist > self.hist_size() {
                return Err(InflateError::Corrupt(self.br.roffset));
            }
            let start = self.out.len() - dist;
            for k in 0..length {
                let c = self.out[start + k];
                self.out.push(c);
            }
        }
    }

    fn data_block(&mut self) -> Result<(), InflateError> {
        self.br.nb = 0;
        self.br.b = 0;
        let avail = self.br.input.len() - self.br.pos;
        if avail < 4 {
            self.br.pos += avail;
            self.br.roffset += avail as i64;
            return Err(InflateError::UnexpectedEof);
        }
        let buf = &self.br.input[self.br.pos..self.br.pos + 4];
        let n = buf[0] as usize | (buf[1] as usize) << 8;
        let nn = buf[2] as usize | (buf[3] as usize) << 8;
        self.br.pos += 4;
        self.br.roffset += 4;
        if nn as u16 != !(n as u16) {
            return Err(InflateError::Corrupt(self.br.roffset));
        }
        if n == 0 {
            return Ok(());
        }
        let avail = self.br.input.len() - self.br.pos;
        let take = n.min(avail);
        self.out.extend_from_slice(&self.br.input[self.br.pos..self.br.pos + take]);
        self.br.pos += take;
        self.br.roffset += take as i64;
        if take < n {
            return Err(InflateError::UnexpectedEof);
        }
        Ok(())
    }

    /// Inflates one complete deflate stream. On success the read position is just after the
    /// last byte consumed by the decompressor.
    pub(crate) fn run(&mut self) -> Result<(), InflateError> {
        loop {
            while self.br.nb < 1 + 2 {
                self.more_bits()?;
            }
            self.final_block = self.br.b & 1 == 1;
            self.br.b >>= 1;
            let typ = self.br.b & 3;
            self.br.b >>= 2;
            self.br.nb -= 1 + 2;
            match typ {
                0 => self.data_block()?,
                1 => self.huffman_block(Which::Fixed)?,
                2 => {
                    self.read_huffman()?;
                    self.huffman_block(Which::Dynamic)?;
                }
                _ => return Err(InflateError::Corrupt(self.br.roffset)),
            }
            if self.final_block {
                return Ok(());
            }
        }
    }

    pub(crate) fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.out)
    }
}

/// Inflates a raw deflate stream like reading a `flate.NewReader` to EOF. Returns the output
/// produced and the error (if any) Go's reader would report.
pub fn decompress(input: &[u8]) -> Result<Vec<u8>, InflateError> {
    let mut f = Inflater::new(input, 0);
    f.run()?;
    Ok(f.take_output())
}
