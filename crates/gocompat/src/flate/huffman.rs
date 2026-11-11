//! Port of Go `compress/flate` huffman_code.go, huffman_bit_writer.go and token.go.

use std::sync::LazyLock;

pub(crate) const MAX_NUM_LIT: usize = 286;
pub(crate) const OFFSET_CODE_COUNT: usize = 30;
pub(crate) const END_BLOCK_MARKER: usize = 256;
pub(crate) const LENGTH_CODES_START: usize = 257;
pub(crate) const CODEGEN_CODE_COUNT: usize = 19;
const BAD_CODE: u8 = 255;
pub(crate) const MAX_STORE_BLOCK_SIZE: usize = 65535;

// ----- token.go -----

const LENGTH_SHIFT: u32 = 22;
const OFFSET_MASK: u32 = (1 << LENGTH_SHIFT) - 1;
pub(crate) const MATCH_TYPE: u32 = 1 << 30;

pub(crate) type Token = u32;

pub(crate) fn literal_token(literal: u32) -> Token {
    literal
}

pub(crate) fn match_token(xlength: u32, xoffset: u32) -> Token {
    MATCH_TYPE
        .wrapping_add(xlength << LENGTH_SHIFT)
        .wrapping_add(xoffset)
}

fn tok_literal(t: Token) -> u32 {
    t
}

fn tok_offset(t: Token) -> u32 {
    t & OFFSET_MASK
}

fn tok_length(t: Token) -> u32 {
    t.wrapping_sub(MATCH_TYPE) >> LENGTH_SHIFT
}

static LENGTH_BASE: [u32; 29] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 20, 24, 28, 32, 40, 48, 56, 64, 80, 96, 112, 128,
    160, 192, 224, 255,
];

static LENGTH_EXTRA_BITS: [i8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

static OFFSET_EXTRA_BITS: [i8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

static OFFSET_BASE: [u32; 30] = [
    0x000000, 0x000001, 0x000002, 0x000003, 0x000004, 0x000006, 0x000008, 0x00000c, 0x000010,
    0x000018, 0x000020, 0x000030, 0x000040, 0x000060, 0x000080, 0x0000c0, 0x000100, 0x000180,
    0x000200, 0x000300, 0x000400, 0x000600, 0x000800, 0x000c00, 0x001000, 0x001800, 0x002000,
    0x003000, 0x004000, 0x006000,
];

static CODEGEN_ORDER: [u32; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

static LENGTH_CODES: LazyLock<[u32; 256]> = LazyLock::new(|| {
    let mut t = [0u32; 256];
    for (len, slot) in t.iter_mut().enumerate() {
        let mut code = 0;
        for (c, &base) in LENGTH_BASE.iter().enumerate() {
            if base as usize <= len {
                code = c as u32;
            }
        }
        *slot = code;
    }
    t
});

static OFFSET_CODES: LazyLock<[u32; 256]> = LazyLock::new(|| {
    let mut t = [0u32; 256];
    for (off, slot) in t.iter_mut().enumerate() {
        let mut code = 0;
        for (c, &base) in OFFSET_BASE.iter().enumerate().take(16) {
            if base as usize <= off {
                code = c as u32;
            }
        }
        *slot = code;
    }
    t
});

fn length_code(len: u32) -> u32 {
    LENGTH_CODES[len as usize]
}

fn offset_code(off: u32) -> u32 {
    if off < 256 {
        return OFFSET_CODES[off as usize];
    }
    if off >> 7 < 256 {
        return OFFSET_CODES[(off >> 7) as usize] + 14;
    }
    OFFSET_CODES[(off >> 14) as usize] + 28
}

// ----- huffman_code.go -----

#[derive(Clone, Copy, Default, Debug)]
pub(crate) struct HCode {
    pub(crate) code: u16,
    pub(crate) len: u16,
}

#[derive(Clone, Copy, Default, Debug)]
struct LiteralNode {
    literal: u16,
    freq: i32,
}

#[derive(Clone, Copy, Default)]
struct LevelInfo {
    level: i32,
    last_freq: i32,
    next_char_freq: i32,
    next_pair_freq: i32,
    needed: i32,
}

pub(crate) struct HuffmanEncoder {
    pub(crate) codes: Vec<HCode>,
    bit_count: [i32; 17],
}

fn reverse_bits(number: u16, bit_length: u8) -> u16 {
    (number << (16 - bit_length as u32)).reverse_bits()
}

impl HuffmanEncoder {
    pub(crate) fn new(size: usize) -> Self {
        HuffmanEncoder {
            codes: vec![HCode::default(); size],
            bit_count: [0; 17],
        }
    }

    pub(crate) fn bit_length(&self, freq: &[i32]) -> i64 {
        let mut total: i64 = 0;
        for (i, &f) in freq.iter().enumerate() {
            if f != 0 {
                total += f as i64 * self.codes[i].len as i64;
            }
        }
        total
    }

    fn bit_counts(&mut self, list: &[LiteralNode], max_bits: i32) -> Vec<i32> {
        let n = list.len() as i32;
        let mut list = list.to_vec();
        list.push(LiteralNode {
            literal: u16::MAX,
            freq: i32::MAX,
        });
        let mut max_bits = max_bits;
        if max_bits > n - 1 {
            max_bits = n - 1;
        }
        let mut levels = [LevelInfo::default(); 16];
        let mut leaf_counts = [[0i32; 16]; 16];
        for level in 1..=max_bits {
            levels[level as usize] = LevelInfo {
                level,
                last_freq: list[1].freq,
                next_char_freq: list[2].freq,
                next_pair_freq: list[0].freq.wrapping_add(list[1].freq),
                needed: 0,
            };
            leaf_counts[level as usize][level as usize] = 2;
            if level == 1 {
                levels[level as usize].next_pair_freq = i32::MAX;
            }
        }
        levels[max_bits as usize].needed = 2 * n - 4;

        let mut level = max_bits as usize;
        loop {
            let l = levels[level];
            if l.next_pair_freq == i32::MAX && l.next_char_freq == i32::MAX {
                levels[level].needed = 0;
                levels[level + 1].next_pair_freq = i32::MAX;
                level += 1;
                continue;
            }
            let prev_freq = l.last_freq;
            if l.next_char_freq < l.next_pair_freq {
                let nn = leaf_counts[level][level] + 1;
                levels[level].last_freq = l.next_char_freq;
                leaf_counts[level][level] = nn;
                levels[level].next_char_freq = list[nn as usize].freq;
            } else {
                levels[level].last_freq = l.next_pair_freq;
                let (lo, hi) = leaf_counts.split_at_mut(level);
                hi[0][..level].copy_from_slice(&lo[level - 1][..level]);
                levels[l.level as usize - 1].needed = 2;
            }
            levels[level].needed -= 1;
            if levels[level].needed == 0 {
                let lv = levels[level].level;
                if lv == max_bits {
                    break;
                }
                levels[lv as usize + 1].next_pair_freq =
                    prev_freq.wrapping_add(levels[level].last_freq);
                level += 1;
            } else {
                while levels[level - 1].needed > 0 {
                    level -= 1;
                }
            }
        }

        let counts = &leaf_counts[max_bits as usize];
        let mut bits = 1usize;
        let mut lvl = max_bits as usize;
        while lvl > 0 {
            self.bit_count[bits] = counts[lvl] - counts[lvl - 1];
            bits += 1;
            lvl -= 1;
        }
        self.bit_count[..=max_bits as usize].to_vec()
    }

    fn assign_encoding_and_size(&mut self, bit_count: &[i32], list: &mut [LiteralNode]) {
        let mut code: u16 = 0;
        let mut end = list.len();
        for (n, &bits) in bit_count.iter().enumerate() {
            code <<= 1;
            if n == 0 || bits == 0 {
                continue;
            }
            let chunk = &mut list[end - bits as usize..end];
            chunk.sort_by_key(|node| node.literal);
            for node in chunk.iter() {
                self.codes[node.literal as usize] = HCode {
                    code: reverse_bits(code, n as u8),
                    len: n as u16,
                };
                code = code.wrapping_add(1);
            }
            end -= bits as usize;
        }
    }

    pub(crate) fn generate(&mut self, freq: &[i32], max_bits: i32) {
        let mut list: Vec<LiteralNode> = Vec::with_capacity(freq.len());
        for (i, &f) in freq.iter().enumerate() {
            if f != 0 {
                list.push(LiteralNode {
                    literal: i as u16,
                    freq: f,
                });
            } else {
                self.codes[i].len = 0;
            }
        }
        if list.len() <= 2 {
            for (i, node) in list.iter().enumerate() {
                self.codes[node.literal as usize] = HCode {
                    code: i as u16,
                    len: 1,
                };
            }
            return;
        }
        list.sort_by(|a, b| {
            if a.freq == b.freq {
                a.literal.cmp(&b.literal)
            } else {
                a.freq.cmp(&b.freq)
            }
        });
        let bit_count = self.bit_counts(&list, max_bits);
        self.assign_encoding_and_size(&bit_count, &mut list);
    }
}

fn generate_fixed_literal_encoding() -> HuffmanEncoder {
    let mut h = HuffmanEncoder::new(MAX_NUM_LIT);
    for ch in 0..MAX_NUM_LIT as u16 {
        let (bits, size): (u16, u16) = match ch {
            0..=143 => (ch + 48, 8),
            144..=255 => (ch + 400 - 144, 9),
            256..=279 => (ch - 256, 7),
            _ => (ch + 192 - 280, 8),
        };
        h.codes[ch as usize] = HCode {
            code: reverse_bits(bits, size as u8),
            len: size,
        };
    }
    h
}

fn generate_fixed_offset_encoding() -> HuffmanEncoder {
    let mut h = HuffmanEncoder::new(30);
    for ch in 0..30u16 {
        h.codes[ch as usize] = HCode {
            code: reverse_bits(ch, 5),
            len: 5,
        };
    }
    h
}

static FIXED_LITERAL_ENCODING: LazyLock<HuffmanEncoder> =
    LazyLock::new(generate_fixed_literal_encoding);
static FIXED_OFFSET_ENCODING: LazyLock<HuffmanEncoder> =
    LazyLock::new(generate_fixed_offset_encoding);
static HUFF_OFFSET: LazyLock<HuffmanEncoder> = LazyLock::new(|| {
    let mut freq = vec![0i32; OFFSET_CODE_COUNT];
    freq[0] = 1;
    let mut h = HuffmanEncoder::new(OFFSET_CODE_COUNT);
    h.generate(&freq, 15);
    h
});

// ----- huffman_bit_writer.go -----

pub(crate) struct HuffmanBitWriter {
    pub(crate) out: Vec<u8>,
    bits: u64,
    nbits: u32,
    codegen_freq: [i32; CODEGEN_CODE_COUNT],
    literal_freq: Vec<i32>,
    offset_freq: Vec<i32>,
    codegen: Vec<u8>,
    literal_encoding: HuffmanEncoder,
    offset_encoding: HuffmanEncoder,
    codegen_encoding: HuffmanEncoder,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Enc {
    Fixed,
    Dynamic,
}

impl HuffmanBitWriter {
    pub(crate) fn new() -> Self {
        HuffmanBitWriter {
            out: Vec::new(),
            bits: 0,
            nbits: 0,
            codegen_freq: [0; CODEGEN_CODE_COUNT],
            literal_freq: vec![0; MAX_NUM_LIT],
            offset_freq: vec![0; OFFSET_CODE_COUNT],
            codegen: vec![0; MAX_NUM_LIT + OFFSET_CODE_COUNT + 1],
            literal_encoding: HuffmanEncoder::new(MAX_NUM_LIT),
            offset_encoding: HuffmanEncoder::new(OFFSET_CODE_COUNT),
            codegen_encoding: HuffmanEncoder::new(CODEGEN_CODE_COUNT),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.bits = 0;
        self.nbits = 0;
    }

    pub(crate) fn flush(&mut self) {
        while self.nbits != 0 {
            self.out.push(self.bits as u8);
            self.bits >>= 8;
            if self.nbits > 8 {
                self.nbits -= 8;
            } else {
                self.nbits = 0;
            }
        }
        self.bits = 0;
    }

    fn emit48(&mut self) {
        let bits = self.bits;
        self.bits >>= 48;
        self.nbits -= 48;
        self.out.extend_from_slice(&bits.to_le_bytes()[..6]);
    }

    fn write_bits(&mut self, b: i32, nb: u32) {
        self.bits |= (b as i64 as u64) << self.nbits;
        self.nbits += nb;
        if self.nbits >= 48 {
            self.emit48();
        }
    }

    pub(crate) fn write_bytes(&mut self, bytes: &[u8]) {
        // nbits is always a multiple of 8 here (after write_stored_header)
        while self.nbits != 0 {
            self.out.push(self.bits as u8);
            self.bits >>= 8;
            self.nbits = self.nbits.saturating_sub(8);
        }
        self.out.extend_from_slice(bytes);
    }

    fn write_code(&mut self, c: HCode) {
        self.bits |= (c.code as u64) << self.nbits;
        self.nbits += c.len as u32;
        if self.nbits >= 48 {
            self.emit48();
        }
    }

    fn generate_codegen(
        &mut self,
        num_literals: usize,
        num_offsets: usize,
        lit: &HuffmanEncoder,
        off: &HuffmanEncoder,
    ) {
        self.codegen_freq = [0; CODEGEN_CODE_COUNT];
        let codegen = &mut self.codegen;
        for (dst, code) in codegen[..num_literals].iter_mut().zip(&lit.codes) {
            *dst = code.len as u8;
        }
        for (dst, code) in codegen[num_literals..num_literals + num_offsets]
            .iter_mut()
            .zip(&off.codes)
        {
            *dst = code.len as u8;
        }
        codegen[num_literals + num_offsets] = BAD_CODE;

        let mut size = codegen[0];
        let mut count: i32 = 1;
        let mut out_index = 0usize;
        let mut in_index = 1usize;
        while size != BAD_CODE {
            let next_size = codegen[in_index];
            if next_size == size {
                count += 1;
                in_index += 1;
                continue;
            }
            if size != 0 {
                codegen[out_index] = size;
                out_index += 1;
                self.codegen_freq[size as usize] += 1;
                count -= 1;
                while count >= 3 {
                    let n = 6.min(count);
                    codegen[out_index] = 16;
                    out_index += 1;
                    codegen[out_index] = (n - 3) as u8;
                    out_index += 1;
                    self.codegen_freq[16] += 1;
                    count -= n;
                }
            } else {
                while count >= 11 {
                    let n = 138.min(count);
                    codegen[out_index] = 18;
                    out_index += 1;
                    codegen[out_index] = (n - 11) as u8;
                    out_index += 1;
                    self.codegen_freq[18] += 1;
                    count -= n;
                }
                if count >= 3 {
                    codegen[out_index] = 17;
                    out_index += 1;
                    codegen[out_index] = (count - 3) as u8;
                    out_index += 1;
                    self.codegen_freq[17] += 1;
                    count = 0;
                }
            }
            count -= 1;
            while count >= 0 {
                codegen[out_index] = size;
                out_index += 1;
                self.codegen_freq[size as usize] += 1;
                count -= 1;
            }
            size = next_size;
            count = 1;
            in_index += 1;
        }
        codegen[out_index] = BAD_CODE;
    }

    fn dynamic_size(
        &self,
        lit: &HuffmanEncoder,
        off: &HuffmanEncoder,
        extra_bits: i64,
    ) -> (i64, usize) {
        let mut num_codegens = self.codegen_freq.len();
        while num_codegens > 4 && self.codegen_freq[CODEGEN_ORDER[num_codegens - 1] as usize] == 0 {
            num_codegens -= 1;
        }
        let header = 3
            + 5
            + 5
            + 4
            + (3 * num_codegens as i64)
            + self.codegen_encoding.bit_length(&self.codegen_freq)
            + self.codegen_freq[16] as i64 * 2
            + self.codegen_freq[17] as i64 * 3
            + self.codegen_freq[18] as i64 * 7;
        let size = header
            + lit.bit_length(&self.literal_freq)
            + off.bit_length(&self.offset_freq)
            + extra_bits;
        (size, num_codegens)
    }

    fn fixed_size(&self, extra_bits: i64) -> i64 {
        3 + FIXED_LITERAL_ENCODING.bit_length(&self.literal_freq)
            + FIXED_OFFSET_ENCODING.bit_length(&self.offset_freq)
            + extra_bits
    }

    fn stored_size(input: Option<&[u8]>) -> (i64, bool) {
        match input {
            None => (0, false),
            Some(i) if i.len() <= MAX_STORE_BLOCK_SIZE => ((i.len() as i64 + 5) * 8, true),
            Some(_) => (0, false),
        }
    }

    fn write_dynamic_header(
        &mut self,
        num_literals: usize,
        num_offsets: usize,
        num_codegens: usize,
        is_eof: bool,
    ) {
        let first_bits = if is_eof { 5 } else { 4 };
        self.write_bits(first_bits, 3);
        self.write_bits(num_literals as i32 - 257, 5);
        self.write_bits(num_offsets as i32 - 1, 5);
        self.write_bits(num_codegens as i32 - 4, 4);
        for &order in &CODEGEN_ORDER[..num_codegens] {
            let value = self.codegen_encoding.codes[order as usize].len;
            self.write_bits(value as i32, 3);
        }
        let mut i = 0;
        loop {
            let code_word = self.codegen[i];
            i += 1;
            if code_word == BAD_CODE {
                break;
            }
            let c = self.codegen_encoding.codes[code_word as usize];
            self.write_code(c);
            match code_word {
                16 => {
                    self.write_bits(self.codegen[i] as i32, 2);
                    i += 1;
                }
                17 => {
                    self.write_bits(self.codegen[i] as i32, 3);
                    i += 1;
                }
                18 => {
                    self.write_bits(self.codegen[i] as i32, 7);
                    i += 1;
                }
                _ => {}
            }
        }
    }

    pub(crate) fn write_stored_header(&mut self, length: usize, is_eof: bool) {
        let flag = if is_eof { 1 } else { 0 };
        self.write_bits(flag, 3);
        self.flush();
        self.write_bits(length as i32, 16);
        self.write_bits(!(length as u16) as i32, 16);
    }

    fn write_fixed_header(&mut self, is_eof: bool) {
        let value = if is_eof { 3 } else { 2 };
        self.write_bits(value, 3);
    }

    /// Go `writeBlock`. `tokens` must not contain the end-of-block marker.
    pub(crate) fn write_block(&mut self, tokens: &[Token], eof: bool, input: Option<&[u8]>) {
        let (num_literals, num_offsets) = self.index_tokens(tokens);

        let mut extra_bits: i64 = 0;
        let (stored_size, storable) = Self::stored_size(input);
        if storable {
            for length_code in LENGTH_CODES_START + 8..num_literals {
                extra_bits += self.literal_freq[length_code] as i64
                    * LENGTH_EXTRA_BITS[length_code - LENGTH_CODES_START] as i64;
            }
            for (freq, extra) in self.offset_freq[..num_offsets]
                .iter()
                .zip(&OFFSET_EXTRA_BITS)
                .skip(4)
            {
                extra_bits += *freq as i64 * *extra as i64;
            }
        }

        let mut enc = Enc::Fixed;
        let mut size = self.fixed_size(extra_bits);

        let lit = std::mem::replace(&mut self.literal_encoding, HuffmanEncoder::new(0));
        let off = std::mem::replace(&mut self.offset_encoding, HuffmanEncoder::new(0));
        self.generate_codegen(num_literals, num_offsets, &lit, &off);
        let freq = self.codegen_freq;
        self.codegen_encoding.generate(&freq, 7);
        let (dynamic_size, num_codegens) = self.dynamic_size(&lit, &off, extra_bits);
        self.literal_encoding = lit;
        self.offset_encoding = off;

        if dynamic_size < size {
            size = dynamic_size;
            enc = Enc::Dynamic;
        }

        if storable && stored_size < size {
            let input = input.unwrap_or_default();
            self.write_stored_header(input.len(), eof);
            self.write_bytes(input);
            return;
        }

        if enc == Enc::Fixed {
            self.write_fixed_header(eof);
            self.write_tokens(tokens, Enc::Fixed);
        } else {
            self.write_dynamic_header(num_literals, num_offsets, num_codegens, eof);
            self.write_tokens(tokens, Enc::Dynamic);
        }
    }

    /// Go `writeBlockDynamic`.
    pub(crate) fn write_block_dynamic(
        &mut self,
        tokens: &[Token],
        eof: bool,
        input: Option<&[u8]>,
    ) {
        let (num_literals, num_offsets) = self.index_tokens(tokens);
        let lit = std::mem::replace(&mut self.literal_encoding, HuffmanEncoder::new(0));
        let off = std::mem::replace(&mut self.offset_encoding, HuffmanEncoder::new(0));
        self.generate_codegen(num_literals, num_offsets, &lit, &off);
        let freq = self.codegen_freq;
        self.codegen_encoding.generate(&freq, 7);
        let (size, num_codegens) = self.dynamic_size(&lit, &off, 0);
        self.literal_encoding = lit;
        self.offset_encoding = off;

        let (ssize, storable) = Self::stored_size(input);
        if storable && ssize < size + (size >> 4) {
            let input = input.unwrap_or_default();
            self.write_stored_header(input.len(), eof);
            self.write_bytes(input);
            return;
        }
        self.write_dynamic_header(num_literals, num_offsets, num_codegens, eof);
        self.write_tokens(tokens, Enc::Dynamic);
    }

    /// Go `indexTokens` over `tokens` followed by the end-of-block marker.
    fn index_tokens(&mut self, tokens: &[Token]) -> (usize, usize) {
        self.literal_freq.iter_mut().for_each(|f| *f = 0);
        self.offset_freq.iter_mut().for_each(|f| *f = 0);
        for &t in tokens
            .iter()
            .chain(std::iter::once(&(END_BLOCK_MARKER as Token)))
        {
            if t < MATCH_TYPE {
                self.literal_freq[tok_literal(t) as usize] += 1;
                continue;
            }
            let length = tok_length(t);
            let offset = tok_offset(t);
            self.literal_freq[LENGTH_CODES_START + length_code(length) as usize] += 1;
            self.offset_freq[offset_code(offset) as usize] += 1;
        }
        let mut num_literals = self.literal_freq.len();
        while self.literal_freq[num_literals - 1] == 0 {
            num_literals -= 1;
        }
        let mut num_offsets = self.offset_freq.len();
        while num_offsets > 0 && self.offset_freq[num_offsets - 1] == 0 {
            num_offsets -= 1;
        }
        if num_offsets == 0 {
            self.offset_freq[0] = 1;
            num_offsets = 1;
        }
        let lf = std::mem::take(&mut self.literal_freq);
        self.literal_encoding.generate(&lf, 15);
        self.literal_freq = lf;
        let of = std::mem::take(&mut self.offset_freq);
        self.offset_encoding.generate(&of, 15);
        self.offset_freq = of;
        (num_literals, num_offsets)
    }

    fn write_tokens(&mut self, tokens: &[Token], enc: Enc) {
        let (le, oe): (Vec<HCode>, Vec<HCode>) = match enc {
            Enc::Fixed => (
                FIXED_LITERAL_ENCODING.codes.clone(),
                FIXED_OFFSET_ENCODING.codes.clone(),
            ),
            Enc::Dynamic => (
                self.literal_encoding.codes.clone(),
                self.offset_encoding.codes.clone(),
            ),
        };
        for &t in tokens
            .iter()
            .chain(std::iter::once(&(END_BLOCK_MARKER as Token)))
        {
            if t < MATCH_TYPE {
                self.write_code(le[tok_literal(t) as usize]);
                continue;
            }
            let length = tok_length(t);
            let lc = length_code(length);
            self.write_code(le[lc as usize + LENGTH_CODES_START]);
            let extra_length_bits = LENGTH_EXTRA_BITS[lc as usize] as u32;
            if extra_length_bits > 0 {
                let extra_length = (length - LENGTH_BASE[lc as usize]) as i32;
                self.write_bits(extra_length, extra_length_bits);
            }
            let offset = tok_offset(t);
            let oc = offset_code(offset);
            self.write_code(oe[oc as usize]);
            let extra_offset_bits = OFFSET_EXTRA_BITS[oc as usize] as u32;
            if extra_offset_bits > 0 {
                let extra_offset = (offset - OFFSET_BASE[oc as usize]) as i32;
                self.write_bits(extra_offset, extra_offset_bits);
            }
        }
    }

    /// Go `writeBlockHuff`.
    pub(crate) fn write_block_huff(&mut self, eof: bool, input: &[u8]) {
        self.literal_freq.iter_mut().for_each(|f| *f = 0);
        for &b in input {
            self.literal_freq[b as usize] += 1;
        }
        self.literal_freq[END_BLOCK_MARKER] = 1;
        let num_literals = END_BLOCK_MARKER + 1;
        self.offset_freq[0] = 1;
        let num_offsets = 1;

        let lf = std::mem::take(&mut self.literal_freq);
        self.literal_encoding.generate(&lf, 15);
        self.literal_freq = lf;

        let lit = std::mem::replace(&mut self.literal_encoding, HuffmanEncoder::new(0));
        self.generate_codegen(num_literals, num_offsets, &lit, &HUFF_OFFSET);
        let freq = self.codegen_freq;
        self.codegen_encoding.generate(&freq, 7);
        let (size, num_codegens) = self.dynamic_size(&lit, &HUFF_OFFSET, 0);
        self.literal_encoding = lit;

        let (ssize, storable) = Self::stored_size(Some(input));
        if storable && ssize < size + (size >> 4) {
            self.write_stored_header(input.len(), eof);
            self.write_bytes(input);
            return;
        }

        self.write_dynamic_header(num_literals, num_offsets, num_codegens, eof);
        let encoding: Vec<HCode> = self.literal_encoding.codes[..257].to_vec();
        for &t in input {
            let c = encoding[t as usize];
            self.bits |= (c.code as u64) << self.nbits;
            self.nbits += c.len as u32;
            if self.nbits < 48 {
                continue;
            }
            self.emit48();
        }
        self.write_code(encoding[END_BLOCK_MARKER]);
    }
}
