//! Port of the subset of Go's reflection-driven `encoding/asn1.Unmarshal` used by
//! `x509.ParseCertificateRequest`.
//!
//! Go decodes into Go structs via reflection; here the target "Go types" are described by a
//! small static schema ([`Ty`]) and [`unmarshal`] mirrors `parseField` / `parseSequenceOf`
//! for exactly those types (RawValue, RawContent, int, bool, OBJECT IDENTIFIER, BIT STRING,
//! []byte, `any`, structs and slices), including Go's DER strictness and error messages.

use std::fmt;

/// Universal tag numbers.
pub const TAG_BOOLEAN: i64 = 1;
pub const TAG_INTEGER: i64 = 2;
pub const TAG_BIT_STRING: i64 = 3;
pub const TAG_OCTET_STRING: i64 = 4;
pub const TAG_OID: i64 = 6;
pub const TAG_UTF8_STRING: i64 = 12;
pub const TAG_SEQUENCE: i64 = 16;
pub const TAG_SET: i64 = 17;
pub const TAG_NUMERIC_STRING: i64 = 18;
pub const TAG_PRINTABLE_STRING: i64 = 19;
pub const TAG_T61_STRING: i64 = 20;
pub const TAG_IA5_STRING: i64 = 22;
pub const TAG_UTC_TIME: i64 = 23;
pub const TAG_GENERALIZED_TIME: i64 = 24;
pub const TAG_GENERAL_STRING: i64 = 27;
pub const TAG_BMP_STRING: i64 = 30;

pub const CLASS_UNIVERSAL: i64 = 0;
pub const CLASS_CONTEXT_SPECIFIC: i64 = 2;

/// `asn1.StructuralError` / `asn1.SyntaxError` / plain errors, rendered like Go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Asn1Error {
    Structural(String),
    Syntax(String),
    Plain(String),
}

impl fmt::Display for Asn1Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Asn1Error::Structural(m) => write!(f, "asn1: structure error: {m}"),
            Asn1Error::Syntax(m) => write!(f, "asn1: syntax error: {m}"),
            Asn1Error::Plain(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Asn1Error {}

fn structural(m: impl Into<String>) -> Asn1Error {
    Asn1Error::Structural(m.into())
}
fn syntax(m: impl Into<String>) -> Asn1Error {
    Asn1Error::Syntax(m.into())
}

pub type Result<T> = std::result::Result<T, Asn1Error>;

/// `tagAndLength`
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TagAndLength {
    pub class: i64,
    pub tag: i64,
    pub length: i64,
    pub is_compound: bool,
}

impl fmt::Display for TagAndLength {
    /// `%+v`
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{{class:{} tag:{} length:{} isCompound:{}}}",
            self.class, self.tag, self.length, self.is_compound
        )
    }
}

/// `fieldParameters` subset (`optional`, `tag:N`, `set`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Params {
    pub optional: bool,
    pub tag: Option<i64>,
    pub set: bool,
}

impl Params {
    pub const NONE: Params = Params {
        optional: false,
        tag: None,
        set: false,
    };
}

impl fmt::Display for Params {
    /// `%+v` of `fieldParameters`. Go prints the `tag *int` pointer as an address; a fixed
    /// placeholder address is used here.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{{optional:{} explicit:false application:false private:false defaultValue:<nil> tag:{} stringType:0 timeType:0 set:{} omitEmpty:false}}",
            self.optional,
            if self.tag.is_some() { "0xc000000000" } else { "<nil>" },
            self.set
        )
    }
}

/// A field of a Go struct: its type and `asn1:"..."` parameters.
#[derive(Debug)]
pub struct Field {
    pub ty: &'static Ty,
    pub params: Params,
}

/// The Go types used by the CSR parser.
#[derive(Debug)]
pub enum Ty {
    /// `asn1.RawValue`
    RawValue,
    /// `int` (64-bit)
    Int,
    /// `bool`
    Bool,
    /// `asn1.ObjectIdentifier`
    Oid,
    /// `asn1.BitString`
    BitString,
    /// `[]byte`
    Bytes,
    /// `any`
    Any,
    /// A struct; `raw_content` = first field is `asn1.RawContent`.
    Struct {
        name: &'static str,
        raw_content: bool,
        fields: &'static [Field],
    },
    /// A slice of `elem`; `name` is the Go type name ("" for unnamed slices). A name ending in
    /// "SET" makes it a SET OF.
    Slice { name: &'static str, elem: &'static Ty },
}

impl Ty {
    /// `fieldType.Name()`
    fn go_name(&self) -> &'static str {
        match self {
            Ty::RawValue => "RawValue",
            Ty::Int => "int",
            Ty::Bool => "bool",
            Ty::Oid => "ObjectIdentifier",
            Ty::BitString => "BitString",
            Ty::Bytes => "",
            Ty::Any => "",
            Ty::Struct { name, .. } => name,
            Ty::Slice { name, .. } => name,
        }
    }

    /// `getUniversalType`: (matchAny, tag, isCompound)
    fn universal_type(&self) -> (bool, i64, bool) {
        match self {
            Ty::RawValue => (true, -1, false),
            Ty::Oid => (false, TAG_OID, false),
            Ty::BitString => (false, TAG_BIT_STRING, false),
            Ty::Bool => (false, TAG_BOOLEAN, false),
            Ty::Int => (false, TAG_INTEGER, false),
            Ty::Struct { .. } => (false, TAG_SEQUENCE, true),
            Ty::Bytes => (false, TAG_OCTET_STRING, false),
            Ty::Slice { name, .. } => {
                if name.ends_with("SET") {
                    (false, TAG_SET, true)
                } else {
                    (false, TAG_SEQUENCE, true)
                }
            }
            Ty::Any => (false, 0, false),
        }
    }
}

/// `asn1.RawValue`
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawValue {
    pub class: i64,
    pub tag: i64,
    pub is_compound: bool,
    pub bytes: Vec<u8>,
    pub full_bytes: Vec<u8>,
}

/// `asn1.BitString`
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct BitString {
    pub bytes: Vec<u8>,
    pub bit_length: usize,
}

impl BitString {
    /// `RightAlign`
    pub fn right_align(&self) -> Vec<u8> {
        let shift = 8 - (self.bit_length % 8) as u32;
        if shift == 8 || self.bytes.is_empty() {
            return self.bytes.clone();
        }
        let mut a = vec![0u8; self.bytes.len()];
        a[0] = self.bytes[0] >> shift;
        for (i, w) in self.bytes.windows(2).enumerate() {
            a[i + 1] = (w[0] << (8 - shift)) | (w[1] >> shift);
        }
        a
    }
}

/// A decoded `any` value (only the string forms matter to callers).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnyValue {
    String(String),
    Other,
}

/// Decoded value of a [`Ty`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Val {
    /// Zero value of an absent OPTIONAL field.
    Absent,
    Raw(RawValue),
    Int(i64),
    Bool(bool),
    Oid(Vec<i64>),
    Bits(BitString),
    Bytes(Vec<u8>),
    /// `any`: `None` = left nil.
    Any(Option<AnyValue>),
    /// Struct fields in order (a leading RawContent field holds `Val::Bytes`).
    Struct(Vec<Val>),
    Slice(Vec<Val>),
}

impl Val {
    pub fn raw(&self) -> RawValue {
        match self {
            Val::Raw(r) => r.clone(),
            _ => RawValue::default(),
        }
    }
    pub fn oid(&self) -> Vec<i64> {
        match self {
            Val::Oid(o) => o.clone(),
            _ => Vec::new(),
        }
    }
    pub fn bits(&self) -> BitString {
        match self {
            Val::Bits(b) => b.clone(),
            _ => BitString::default(),
        }
    }
    pub fn bytes(&self) -> Vec<u8> {
        match self {
            Val::Bytes(b) => b.clone(),
            _ => Vec::new(),
        }
    }
    pub fn fields(&self) -> &[Val] {
        match self {
            Val::Struct(f) => f,
            _ => &[],
        }
    }
    pub fn items(&self) -> &[Val] {
        match self {
            Val::Slice(f) => f,
            _ => &[],
        }
    }
    pub fn any_string(&self) -> Option<&str> {
        match self {
            Val::Any(Some(AnyValue::String(s))) => Some(s),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Primitive parsers
// ---------------------------------------------------------------------------------------------

/// `parseBool`
pub fn parse_bool(bytes: &[u8]) -> Result<bool> {
    if bytes.len() != 1 {
        return Err(syntax("invalid boolean"));
    }
    match bytes[0] {
        0 => Ok(false),
        0xff => Ok(true),
        _ => Err(syntax("invalid boolean")),
    }
}

/// `checkInteger`
fn check_integer(bytes: &[u8]) -> Result<()> {
    if bytes.is_empty() {
        return Err(structural("empty integer"));
    }
    if bytes.len() == 1 {
        return Ok(());
    }
    if (bytes[0] == 0 && bytes[1] & 0x80 == 0) || (bytes[0] == 0xff && bytes[1] & 0x80 == 0x80) {
        return Err(structural("integer not minimally-encoded"));
    }
    Ok(())
}

/// `parseInt64`
pub fn parse_int64(bytes: &[u8]) -> Result<i64> {
    check_integer(bytes)?;
    if bytes.len() > 8 {
        return Err(structural("integer too large"));
    }
    let mut ret: i64 = 0;
    for &b in bytes {
        ret = (ret << 8) | i64::from(b);
    }
    let shift = 64 - (bytes.len() as u32) * 8;
    if shift < 64 {
        ret = (ret << shift) >> shift;
    }
    Ok(ret)
}

/// `parseBitString`
pub fn parse_bit_string(bytes: &[u8]) -> Result<BitString> {
    if bytes.is_empty() {
        return Err(syntax("zero length BIT STRING"));
    }
    let padding_bits = bytes[0] as usize;
    if padding_bits > 7
        || (bytes.len() == 1 && padding_bits > 0)
        || bytes[bytes.len() - 1] & ((1u16 << padding_bits) - 1) as u8 != 0
    {
        return Err(syntax("invalid padding bits in BIT STRING"));
    }
    Ok(BitString {
        bit_length: (bytes.len() - 1) * 8 - padding_bits,
        bytes: bytes[1..].to_vec(),
    })
}

/// `parseBase128Int`
fn parse_base128_int(bytes: &[u8], init_offset: usize) -> Result<(i64, usize)> {
    let mut offset = init_offset;
    let mut ret64: i64 = 0;
    let mut shifted = 0;
    while offset < bytes.len() {
        if shifted == 5 {
            return Err(structural("base 128 integer too large"));
        }
        ret64 <<= 7;
        let b = bytes[offset];
        if shifted == 0 && b == 0x80 {
            return Err(syntax("integer is not minimally encoded"));
        }
        ret64 |= i64::from(b & 0x7f);
        offset += 1;
        if b & 0x80 == 0 {
            if ret64 > i64::from(i32::MAX) {
                return Err(structural("base 128 integer too large"));
            }
            return Ok((ret64, offset));
        }
        shifted += 1;
    }
    Err(syntax("truncated base 128 integer"))
}

/// `parseObjectIdentifier`
pub fn parse_object_identifier(bytes: &[u8]) -> Result<Vec<i64>> {
    if bytes.is_empty() {
        return Err(syntax("zero length OBJECT IDENTIFIER"));
    }
    let mut s = Vec::with_capacity(bytes.len() + 1);
    let (v, mut offset) = parse_base128_int(bytes, 0)?;
    if v < 80 {
        s.push(v / 40);
        s.push(v % 40);
    } else {
        s.push(2);
        s.push(v - 80);
    }
    while offset < bytes.len() {
        let (v, o) = parse_base128_int(bytes, offset)?;
        s.push(v);
        offset = o;
    }
    Ok(s)
}

/// `ObjectIdentifier.String()`
pub fn oid_string(oid: &[i64]) -> String {
    oid.iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join(".")
}

fn is_printable(b: u8) -> bool {
    b.is_ascii_lowercase()
        || b.is_ascii_uppercase()
        || b.is_ascii_digit()
        || (b'\''..=b')').contains(&b)
        || (b'+'..=b'/').contains(&b)
        || b == b' '
        || b == b':'
        || b == b'='
        || b == b'?'
        // allowAsterisk
        || b == b'*'
        // allowAmpersand
        || b == b'&'
}

/// `parsePrintableString`
fn parse_printable_string(bytes: &[u8]) -> Result<String> {
    if bytes.iter().any(|&b| !is_printable(b)) {
        return Err(syntax("PrintableString contains invalid character"));
    }
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

/// `parseNumericString`
fn parse_numeric_string(bytes: &[u8]) -> Result<String> {
    if bytes.iter().any(|&b| !(b.is_ascii_digit() || b == b' ')) {
        return Err(syntax("NumericString contains invalid character"));
    }
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

/// `parseIA5String`
fn parse_ia5_string(bytes: &[u8]) -> Result<String> {
    if bytes.iter().any(|&b| b >= 0x80) {
        return Err(syntax("IA5String contains invalid character"));
    }
    Ok(String::from_utf8_lossy(bytes).into_owned())
}

/// `parseT61String` (treated as Latin-1)
fn parse_t61_string(bytes: &[u8]) -> String {
    bytes.iter().map(|&b| char::from(b)).collect()
}

/// `parseUTF8String`
fn parse_utf8_string(bytes: &[u8]) -> Result<String> {
    std::str::from_utf8(bytes)
        .map(str::to_string)
        .map_err(|_| Asn1Error::Plain("asn1: invalid UTF-8 string".into()))
}

/// `parseBMPString`
fn parse_bmp_string(bmp: &[u8]) -> Result<String> {
    if !bmp.len().is_multiple_of(2) {
        return Err(Asn1Error::Plain("invalid BMPString".into()));
    }
    let mut bmp = bmp;
    if bmp.len() >= 2 && bmp[bmp.len() - 1] == 0 && bmp[bmp.len() - 2] == 0 {
        bmp = &bmp[..bmp.len() - 2];
    }
    let mut s = Vec::with_capacity(bmp.len() / 2);
    for pair in bmp.as_chunks::<2>().0 {
        let point = u16::from_be_bytes(*pair);
        if point == 0xfffe
            || point == 0xffff
            || (0xfdd0..=0xfdef).contains(&point)
            || (0xd800..=0xdfff).contains(&point)
        {
            return Err(Asn1Error::Plain("invalid BMPString".into()));
        }
        s.push(point);
    }
    Ok(String::from_utf16_lossy(&s))
}

// ---------------------------------------------------------------------------------------------
// Time validation (UTCTime / GeneralizedTime inside `any` values)
// ---------------------------------------------------------------------------------------------

fn two_digits(s: &[u8], i: usize) -> Option<u32> {
    let a = *s.get(i)?;
    let b = *s.get(i + 1)?;
    if a.is_ascii_digit() && b.is_ascii_digit() {
        Some(u32::from(a - b'0') * 10 + u32::from(b - b'0'))
    } else {
        None
    }
}

fn days_in(month: u32, year: i64) -> u32 {
    match month {
        2 => {
            if (year % 4 == 0 && year % 100 != 0) || year % 400 == 0 {
                29
            } else {
                28
            }
        }
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Validates `MMDDhhmm[ss]` starting at `i`; returns the new offset.
fn check_date_time(s: &[u8], mut i: usize, year: i64, with_seconds: bool) -> Option<usize> {
    let month = two_digits(s, i)?;
    i += 2;
    let day = two_digits(s, i)?;
    i += 2;
    let hour = two_digits(s, i)?;
    i += 2;
    let minute = two_digits(s, i)?;
    i += 2;
    if !(1..=12).contains(&month) || day < 1 || day > days_in(month, year) || hour > 23 || minute > 59
    {
        return None;
    }
    if with_seconds {
        let sec = two_digits(s, i)?;
        i += 2;
        if sec > 59 {
            return None;
        }
    }
    Some(i)
}

/// `Z0700` that serializes back unchanged: `Z`, or `±hhmm` with a non-zero offset.
fn check_zone(s: &[u8], i: usize) -> bool {
    match s.get(i) {
        Some(b'Z') => i + 1 == s.len(),
        Some(b'+') | Some(b'-') => {
            if s.len() != i + 5 {
                return false;
            }
            let (Some(h), Some(m)) = (two_digits(s, i + 1), two_digits(s, i + 3)) else {
                return false;
            };
            h <= 24 && m < 60 && (h, m) != (0, 0)
        }
        _ => false,
    }
}

/// `parseUTCTime` (validity only).
fn parse_utc_time(bytes: &[u8]) -> Result<()> {
    let ok = |with_seconds: bool| -> bool {
        let Some(yy) = two_digits(bytes, 0) else {
            return false;
        };
        let year = if yy >= 69 { 1900 + i64::from(yy) } else { 2000 + i64::from(yy) };
        match check_date_time(bytes, 2, year, with_seconds) {
            Some(i) => check_zone(bytes, i),
            None => false,
        }
    };
    if ok(false) || ok(true) {
        Ok(())
    } else {
        Err(Asn1Error::Plain(format!(
            "asn1: invalid UTCTime {}",
            ctrlrt::error::go_quote(&String::from_utf8_lossy(bytes))
        )))
    }
}

/// `parseGeneralizedTime` (validity only).
fn parse_generalized_time(bytes: &[u8]) -> Result<()> {
    let ok = || -> bool {
        let (Some(y1), Some(y2)) = (two_digits(bytes, 0), two_digits(bytes, 2)) else {
            return false;
        };
        let year = i64::from(y1) * 100 + i64::from(y2);
        let Some(mut i) = check_date_time(bytes, 4, year, true) else {
            return false;
        };
        if bytes.get(i) == Some(&b'.') {
            let start = i + 1;
            let mut j = start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            let n = j - start;
            // Fraction must serialize back: 1..=9 digits without trailing zeros.
            if n == 0 || n > 9 || bytes[j - 1] == b'0' {
                return false;
            }
            i = j;
        }
        check_zone(bytes, i)
    };
    if ok() {
        Ok(())
    } else {
        Err(Asn1Error::Plain(format!(
            "asn1: invalid GeneralizedTime {}",
            ctrlrt::error::go_quote(&String::from_utf8_lossy(bytes))
        )))
    }
}

// ---------------------------------------------------------------------------------------------
// Tag/length and field parsing
// ---------------------------------------------------------------------------------------------

/// `parseTagAndLength`
pub fn parse_tag_and_length(bytes: &[u8], init_offset: usize) -> Result<(TagAndLength, usize)> {
    let mut offset = init_offset;
    if offset >= bytes.len() {
        return Err(Asn1Error::Plain(
            "asn1: internal error in parseTagAndLength".into(),
        ));
    }
    let b = bytes[offset];
    offset += 1;
    let mut ret = TagAndLength {
        class: i64::from(b >> 6),
        is_compound: b & 0x20 == 0x20,
        tag: i64::from(b & 0x1f),
        length: 0,
    };
    if ret.tag == 0x1f {
        let (t, o) = parse_base128_int(bytes, offset)?;
        ret.tag = t;
        offset = o;
        if ret.tag < 0x1f {
            return Err(syntax("non-minimal tag"));
        }
    }
    if offset >= bytes.len() {
        return Err(syntax("truncated tag or length"));
    }
    let b = bytes[offset];
    offset += 1;
    if b & 0x80 == 0 {
        ret.length = i64::from(b & 0x7f);
    } else {
        let num_bytes = (b & 0x7f) as usize;
        if num_bytes == 0 {
            return Err(syntax("indefinite length found (not DER)"));
        }
        ret.length = 0;
        for _ in 0..num_bytes {
            if offset >= bytes.len() {
                return Err(syntax("truncated tag or length"));
            }
            let b = bytes[offset];
            offset += 1;
            if ret.length >= 1 << 23 {
                return Err(structural("length too large"));
            }
            ret.length = (ret.length << 8) | i64::from(b);
            if ret.length == 0 {
                return Err(structural("superfluous leading zeros in length"));
            }
        }
        if ret.length < 0x80 {
            return Err(structural("non-minimal length"));
        }
    }
    Ok((ret, offset))
}

fn invalid_length(offset: usize, length: i64, slice_length: usize) -> bool {
    (offset as i64).checked_add(length).is_none_or(|end| end > slice_length as i64)
}

/// `parseSequenceOf`
fn parse_sequence_of(bytes: &[u8], elem: &'static Ty, depth: usize) -> Result<Vec<Val>> {
    let (match_any, expected_tag, compound_type) = elem.universal_type();
    let mut num_elements = 0;
    let mut offset = 0;
    while offset < bytes.len() {
        let (mut t, o) = parse_tag_and_length(bytes, offset)?;
        offset = o;
        match t.tag {
            TAG_IA5_STRING | TAG_GENERAL_STRING | TAG_T61_STRING | TAG_UTF8_STRING
            | TAG_NUMERIC_STRING | TAG_BMP_STRING => t.tag = TAG_PRINTABLE_STRING,
            TAG_GENERALIZED_TIME | TAG_UTC_TIME => t.tag = TAG_UTC_TIME,
            _ => {}
        }
        if !match_any
            && (t.class != CLASS_UNIVERSAL || t.is_compound != compound_type || t.tag != expected_tag)
        {
            return Err(structural("sequence tag mismatch"));
        }
        if invalid_length(offset, t.length, bytes.len()) {
            return Err(syntax("truncated sequence"));
        }
        offset += t.length as usize;
        num_elements += 1;
    }
    let mut ret = Vec::with_capacity(num_elements);
    let mut offset = 0;
    for _ in 0..num_elements {
        let (v, o) = parse_field(elem, Params::NONE, bytes, offset, depth)?;
        offset = o;
        ret.push(v);
    }
    Ok(ret)
}

/// `parseField`
pub fn parse_field(
    ty: &'static Ty,
    params: Params,
    bytes: &[u8],
    init_offset: usize,
    depth: usize,
) -> Result<(Val, usize)> {
    let depth = depth + 1;
    if depth > 10000 {
        return Err(structural("nesting depth exceeded"));
    }
    let mut offset = init_offset;

    // If we have run out of data, it may be that there are optional elements at the end.
    if offset == bytes.len() {
        if !params.optional {
            return Err(syntax("sequence truncated"));
        }
        return Ok((Val::Absent, offset));
    }

    // Deal with the ANY type.
    if let Ty::Any = ty {
        let (t, o) = parse_tag_and_length(bytes, offset)?;
        offset = o;
        if invalid_length(offset, t.length, bytes.len()) {
            return Err(syntax("data truncated"));
        }
        let mut result = None;
        if !t.is_compound && t.class == CLASS_UNIVERSAL {
            let inner = &bytes[offset..offset + t.length as usize];
            result = match t.tag {
                TAG_BOOLEAN => parse_bool(inner).map(|_| Some(AnyValue::Other))?,
                TAG_PRINTABLE_STRING => Some(AnyValue::String(parse_printable_string(inner)?)),
                TAG_NUMERIC_STRING => Some(AnyValue::String(parse_numeric_string(inner)?)),
                TAG_IA5_STRING => Some(AnyValue::String(parse_ia5_string(inner)?)),
                TAG_T61_STRING => Some(AnyValue::String(parse_t61_string(inner))),
                TAG_UTF8_STRING => Some(AnyValue::String(parse_utf8_string(inner)?)),
                TAG_INTEGER => parse_int64(inner).map(|_| Some(AnyValue::Other))?,
                TAG_BIT_STRING => parse_bit_string(inner).map(|_| Some(AnyValue::Other))?,
                TAG_OID => parse_object_identifier(inner).map(|_| Some(AnyValue::Other))?,
                TAG_UTC_TIME => parse_utc_time(inner).map(|_| Some(AnyValue::Other))?,
                TAG_GENERALIZED_TIME => {
                    parse_generalized_time(inner).map(|_| Some(AnyValue::Other))?
                }
                TAG_OCTET_STRING => Some(AnyValue::Other),
                TAG_BMP_STRING => Some(AnyValue::String(parse_bmp_string(inner)?)),
                _ => None,
            };
        }
        offset += t.length as usize;
        return Ok((Val::Any(result), offset));
    }

    let (t, o) = parse_tag_and_length(bytes, offset)?;
    offset = o;

    let (match_any, mut universal_tag, compound_type) = ty.universal_type();
    if params.set {
        universal_tag = TAG_SET;
    }

    let mut match_any_class_and_tag = match_any;
    let mut expected_class = CLASS_UNIVERSAL;
    let mut expected_tag = universal_tag;
    if let Some(tag) = params.tag {
        expected_class = CLASS_CONTEXT_SPECIFIC;
        expected_tag = tag;
        match_any_class_and_tag = false;
    }

    if (!match_any_class_and_tag && (t.class != expected_class || t.tag != expected_tag))
        || (!match_any && t.is_compound != compound_type)
    {
        // Tags don't match. Again, it could be an optional element.
        if params.optional {
            return Ok((Val::Absent, init_offset));
        }
        return Err(structural(format!(
            "tags don't match ({} vs {}) {} {} @{}",
            expected_tag,
            t,
            params,
            ty.go_name(),
            offset
        )));
    }
    if invalid_length(offset, t.length, bytes.len()) {
        return Err(syntax("data truncated"));
    }
    let inner = &bytes[offset..offset + t.length as usize];
    offset += t.length as usize;

    let val = match ty {
        Ty::RawValue => Val::Raw(RawValue {
            class: t.class,
            tag: t.tag,
            is_compound: t.is_compound,
            bytes: inner.to_vec(),
            full_bytes: bytes[init_offset..offset].to_vec(),
        }),
        Ty::Oid => Val::Oid(parse_object_identifier(inner)?),
        Ty::BitString => Val::Bits(parse_bit_string(inner)?),
        Ty::Bool => Val::Bool(parse_bool(inner)?),
        Ty::Int => Val::Int(parse_int64(inner)?),
        Ty::Bytes => Val::Bytes(inner.to_vec()),
        Ty::Struct {
            raw_content,
            fields,
            ..
        } => {
            let mut out = Vec::with_capacity(fields.len() + 1);
            if *raw_content {
                out.push(Val::Bytes(bytes[init_offset..offset].to_vec()));
            }
            let mut inner_offset = 0;
            for f in fields.iter() {
                let (v, o) = parse_field(f.ty, f.params, inner, inner_offset, depth)?;
                inner_offset = o;
                out.push(v);
            }
            // We allow extra bytes at the end of the SEQUENCE because adding elements to the
            // end has been used in X.509 as the version numbers have increased.
            Val::Struct(out)
        }
        Ty::Slice { elem, .. } => Val::Slice(parse_sequence_of(inner, elem, depth)?),
        Ty::Any => unreachable_any(),
    };
    Ok((val, offset))
}

fn unreachable_any() -> Val {
    // `Ty::Any` is handled before tag matching above.
    Val::Any(None)
}

/// `asn1.Unmarshal(b, &val)`: returns the value and the remaining bytes.
pub fn unmarshal<'a>(b: &'a [u8], ty: &'static Ty) -> Result<(Val, &'a [u8])> {
    let (v, offset) = parse_field(ty, Params::NONE, b, 0, 0)?;
    Ok((v, &b[offset..]))
}

/// `asn1.UnmarshalWithParams(b, &val, params)`
pub fn unmarshal_with_params<'a>(
    b: &'a [u8],
    ty: &'static Ty,
    params: Params,
) -> Result<(Val, &'a [u8])> {
    let (v, offset) = parse_field(ty, params, b, 0, 0)?;
    Ok((v, &b[offset..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn int64_parsing() {
        assert_eq!(parse_int64(&[0x01]).unwrap(), 1);
        assert_eq!(parse_int64(&[0xff]).unwrap(), -1);
        assert_eq!(parse_int64(&[0x00, 0x80]).unwrap(), 128);
        assert!(parse_int64(&[0x00, 0x01]).is_err());
        assert!(parse_int64(&[]).is_err());
        assert!(parse_int64(&[1; 9]).is_err());
    }

    #[test]
    fn oid_parsing() {
        assert_eq!(
            parse_object_identifier(&[0x55, 0x1d, 0x11]).unwrap(),
            vec![2, 5, 29, 17]
        );
        assert_eq!(oid_string(&[2, 5, 29, 17]), "2.5.29.17");
        assert!(parse_object_identifier(&[]).is_err());
        assert!(parse_object_identifier(&[0x80, 0x01]).is_err());
    }

    #[test]
    fn tag_and_length() {
        assert!(parse_tag_and_length(&[0x30, 0x80], 0).is_err());
        assert!(parse_tag_and_length(&[0x30, 0x81, 0x05], 0).is_err());
        let (t, o) = parse_tag_and_length(&[0x30, 0x81, 0x80], 0).unwrap();
        assert_eq!((t.tag, t.length, t.is_compound, o), (16, 128, true, 3));
    }

    #[test]
    fn times() {
        assert!(parse_utc_time(b"2401011200Z").is_ok());
        assert!(parse_utc_time(b"240101120000Z").is_ok());
        assert!(parse_utc_time(b"240101120000+0100").is_ok());
        assert!(parse_utc_time(b"240101120000+0000").is_err());
        assert!(parse_utc_time(b"240230120000Z").is_err());
        assert!(parse_generalized_time(b"20240101120000.5Z").is_ok());
        assert!(parse_generalized_time(b"20240101120000.50Z").is_err());
    }
}
