//! Binary fields of a DaVinci Resolve project (hex text in a `.drp`), as seen in exported files:
//!
//! - **Qt-serialised maps** (`FieldsBlob`, `Time`, `Geometry`, `TracksBA`, …): `u32 version,
//!   u32 count`, then `(u32 byte length, UTF-16BE key, QVariant)` entries; a QVariant is `u32 type
//!   id, u8 is-null, value` with the usual Qt types (bool, int, uint, longlong, double, QString,
//!   QByteArray, lists and maps). Big-endian throughout ([`qt_map`]).
//! - **Versioned payloads** (`00 00 00 02, u32 length, flag, payload`): flag `0x80` is a raw
//!   protocol-buffers message, `0x81` a zstd frame holding one ([`v2_payload`], [`zstd`]).
//! - **qCompress** byte arrays: a big-endian length and a zlib stream ([`qcompress`]).
//! - **Number pairs**: frame rates and media extents are two little-endian doubles, picture sizes
//!   two big-endian 64-bit integers ([`f64_pair_le`], [`i64_pair_be`]).
//! - **Time maps** (`MediaTimemapBA`): a version byte and big-endian doubles ([`timemap`]).
//! - A tiny protocol-buffers wire reader for the payloads ([`proto_fields`] and friends).
//!
//! Every length is hostile: reads go through a bounds-checked cursor, counts and decompressed
//! sizes are capped, and nothing here panics on any input.

use std::collections::BTreeMap;
use std::io::Read as _;

use filmcraft_media::still::exr::inflate;

/// Why a blob could not be decoded.
pub type Error = &'static str;

/// Largest decompressed blob (zstd or zlib).
pub const MAX_BLOB: usize = 64 << 20;
/// Most entries in one Qt map or list.
const MAX_MAP_ENTRIES: usize = 4096;
/// Deepest nesting of Qt containers.
const MAX_DEPTH: usize = 8;
/// Most fields in one protocol-buffers message.
const MAX_FIELDS: usize = 16_384;

/// Decode hex text (either case; whitespace ignored). Odd lengths and non-hex digits are `None`.
pub fn hex(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    let mut out = Vec::new();
    out.try_reserve_exact(digits.len() / 2).ok()?;
    for [a, b] in digits.as_chunks::<2>().0 {
        let hi = char::from(*a).to_digit(16)?;
        let lo = char::from(*b).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
    }
    Some(out)
}

/// A bounds-checked big-endian cursor.
struct Cur<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Cur<'a> {
    fn new(b: &'a [u8]) -> Self {
        Self { b, pos: 0 }
    }
    fn at_end(&self) -> bool {
        self.pos >= self.b.len()
    }
    fn remaining(&self) -> usize {
        self.b.len().saturating_sub(self.pos)
    }
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.pos.checked_add(n)?;
        let s = self.b.get(self.pos..end)?;
        self.pos = end;
        Some(s)
    }
    fn rest(&self) -> &'a [u8] {
        self.b.get(self.pos..).unwrap_or_default()
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).and_then(|s| s.first().copied())
    }
    fn array<const N: usize>(&mut self) -> Option<[u8; N]> {
        self.take(N).and_then(|s| s.try_into().ok())
    }
    fn u32(&mut self) -> Option<u32> {
        self.array().map(u32::from_be_bytes)
    }
    fn i32(&mut self) -> Option<i32> {
        self.array().map(i32::from_be_bytes)
    }
    fn u64(&mut self) -> Option<u64> {
        self.array().map(u64::from_be_bytes)
    }
    fn i64(&mut self) -> Option<i64> {
        self.array().map(i64::from_be_bytes)
    }
    fn f64(&mut self) -> Option<f64> {
        self.array().map(f64::from_be_bytes)
    }
    fn f32(&mut self) -> Option<f32> {
        self.array().map(f32::from_be_bytes)
    }
    fn u32_le(&mut self) -> Option<u32> {
        self.array().map(u32::from_le_bytes)
    }
    fn u64_le(&mut self) -> Option<u64> {
        self.array().map(u64::from_le_bytes)
    }
}

// ---------------------------------------------------------------------------------------------
// Qt-serialised maps
// ---------------------------------------------------------------------------------------------

/// A value in a Qt-serialised map.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(i64),
    UInt(u64),
    Double(f64),
    /// A QString (`None` when the string is null).
    Str(Option<String>),
    Bytes(Vec<u8>),
    List(Vec<Value>),
    Map(Map),
    StrList(Vec<Option<String>>),
    /// A QVariant type this reader does not know; nothing after it in the map could be read.
    Unknown(u32),
}

/// Keys to values. Resolve writes each key once.
pub type Map = BTreeMap<String, Value>;

impl Value {
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Value::Int(i) => Some(*i),
            Value::UInt(u) => i64::try_from(*u).ok(),
            Value::Double(d) if d.is_finite() && d.fract() == 0.0 && d.abs() < 9.0e15 => Some(*d as i64),
            _ => None,
        }
    }
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::UInt(u) => Some(*u as f64),
            Value::Double(d) if d.is_finite() => Some(*d),
            _ => None,
        }
    }
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            Value::Int(i) => Some(*i != 0),
            Value::UInt(u) => Some(*u != 0),
            _ => None,
        }
    }
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(Some(s)) => Some(s),
            _ => None,
        }
    }
    pub fn as_bytes(&self) -> Option<&[u8]> {
        match self {
            Value::Bytes(b) => Some(b),
            _ => None,
        }
    }
}

enum Stop {
    Truncated,
    Unknown(u32),
}

/// Decode a Qt-serialised map. An unknown value type ends the map early (its size cannot be
/// known), leaving the entries read so far plus a [`Value::Unknown`] marker under that key.
pub fn qt_map(bytes: &[u8]) -> Result<Map, Error> {
    let mut c = Cur::new(bytes);
    let _version = c.u32().ok_or("truncated map header")?;
    let count = c.u32().ok_or("truncated map header")?;
    read_map(&mut c, count, 0).map_err(|e| match e {
        Stop::Truncated => "truncated map",
        Stop::Unknown(_) => "unknown map value type",
    })
}

fn read_map(c: &mut Cur<'_>, count: u32, depth: usize) -> Result<Map, Stop> {
    let count = usize::try_from(count).map_err(|_| Stop::Truncated)?;
    if count > MAX_MAP_ENTRIES || count > c.remaining() / 4 {
        return Err(Stop::Truncated);
    }
    let mut map = Map::new();
    for _ in 0..count {
        let key = qstring(c).ok_or(Stop::Truncated)?.unwrap_or_default();
        match variant(c, depth) {
            Ok(v) => {
                map.insert(key, v);
            }
            Err(Stop::Unknown(t)) => {
                map.insert(key, Value::Unknown(t));
                break;
            }
            Err(Stop::Truncated) => return Err(Stop::Truncated),
        }
    }
    Ok(map)
}

fn variant(c: &mut Cur<'_>, depth: usize) -> Result<Value, Stop> {
    if depth > MAX_DEPTH {
        return Err(Stop::Truncated);
    }
    let t = c.u32().ok_or(Stop::Truncated)?;
    let _is_null = c.u8().ok_or(Stop::Truncated)?;
    let v = match t {
        0 => Value::Null,
        1 => Value::Bool(c.u8().ok_or(Stop::Truncated)? != 0),
        2 => Value::Int(i64::from(c.i32().ok_or(Stop::Truncated)?)),
        3 => Value::UInt(u64::from(c.u32().ok_or(Stop::Truncated)?)),
        4 => Value::Int(c.i64().ok_or(Stop::Truncated)?),
        5 => Value::UInt(c.u64().ok_or(Stop::Truncated)?),
        6 => Value::Double(c.f64().ok_or(Stop::Truncated)?),
        38 => Value::Double(f64::from(c.f32().ok_or(Stop::Truncated)?)),
        10 => Value::Str(qstring(c).ok_or(Stop::Truncated)?),
        12 => {
            let n = c.u32().ok_or(Stop::Truncated)?;
            if n == u32::MAX { Value::Null } else { Value::Bytes(c.take(usize::try_from(n).map_err(|_| Stop::Truncated)?).ok_or(Stop::Truncated)?.to_vec()) }
        }
        8 => {
            let n = c.u32().ok_or(Stop::Truncated)?;
            Value::Map(read_map(c, n, depth + 1)?)
        }
        9 => {
            let n = usize::try_from(c.u32().ok_or(Stop::Truncated)?).map_err(|_| Stop::Truncated)?;
            if n > MAX_MAP_ENTRIES || n > c.remaining() / 5 {
                return Err(Stop::Truncated);
            }
            let mut l = Vec::with_capacity(n);
            for _ in 0..n {
                l.push(variant(c, depth + 1)?);
            }
            Value::List(l)
        }
        11 => {
            let n = usize::try_from(c.u32().ok_or(Stop::Truncated)?).map_err(|_| Stop::Truncated)?;
            if n > MAX_MAP_ENTRIES || n > c.remaining() / 4 {
                return Err(Stop::Truncated);
            }
            let mut l = Vec::with_capacity(n);
            for _ in 0..n {
                l.push(qstring(c).ok_or(Stop::Truncated)?);
            }
            Value::StrList(l)
        }
        other => return Err(Stop::Unknown(other)),
    };
    Ok(v)
}

/// A serialised QString: byte length (`0xFFFFFFFF` = null) and UTF-16BE code units.
fn qstring(c: &mut Cur<'_>) -> Option<Option<String>> {
    let n = c.u32()?;
    if n == u32::MAX {
        return Some(None);
    }
    let raw = c.take(usize::try_from(n).ok()?)?;
    let units = raw.as_chunks::<2>().0.iter().map(|p| u16::from_be_bytes(*p));
    Some(Some(char::decode_utf16(units).map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER)).collect()))
}

// ---------------------------------------------------------------------------------------------
// Versioned payloads, zstd, qCompress
// ---------------------------------------------------------------------------------------------

/// The protocol-buffers message inside a versioned payload (`u32 version, u32 length, flag, …`):
/// flag `0x80` stores it raw, `0x81` as a zstd frame.
pub fn v2_payload(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let mut c = Cur::new(bytes);
    let version = c.u32().ok_or("truncated blob header")?;
    if !(1..=2).contains(&version) {
        return Err("unknown blob version");
    }
    let len = usize::try_from(c.u32().ok_or("truncated blob header")?).map_err(|_| "blob length out of range")?;
    let flag = c.u8().ok_or("truncated blob header")?;
    let payload_len = len.checked_sub(1).ok_or("blob length out of range")?;
    let payload = c.take(payload_len).ok_or("blob length disagrees with its data")?;
    match flag {
        0x80 => Ok(payload.to_vec()),
        0x81 => zstd(payload),
        _ => Err("unknown blob encoding"),
    }
}

/// Decompress one zstd frame (RFC 8878). The frame content size, when the header states it, is
/// checked against [`MAX_BLOB`] before anything is decoded; the output is capped either way.
pub fn zstd(frame: &[u8]) -> Result<Vec<u8>, Error> {
    let stated = frame_content_size(frame)?;
    if stated.is_some_and(|s| s > MAX_BLOB as u64) {
        return Err("zstd blob too large");
    }
    let mut dec = ruzstd::decoding::StreamingDecoder::new(frame).map_err(|_| "corrupt zstd frame header")?;
    let mut out = Vec::new();
    if let Some(s) = stated {
        out.try_reserve_exact(usize::try_from(s).map_err(|_| "zstd blob too large")?).map_err(|_| "zstd blob too large")?;
    }
    (&mut dec).take(MAX_BLOB as u64 + 1).read_to_end(&mut out).map_err(|_| "corrupt zstd data")?;
    if out.len() > MAX_BLOB {
        return Err("zstd blob too large");
    }
    if stated.is_some_and(|s| s != out.len() as u64) {
        return Err("zstd data ends early");
    }
    Ok(out)
}

/// The frame content size a zstd frame header states, if any (RFC 8878 §3.1.1.1).
fn frame_content_size(frame: &[u8]) -> Result<Option<u64>, Error> {
    let mut c = Cur::new(frame);
    if c.array::<4>() != Some([0x28, 0xb5, 0x2f, 0xfd]) {
        return Err("not a zstd frame");
    }
    let fhd = c.u8().ok_or("truncated zstd frame header")?;
    let single_segment = fhd & 0x20 != 0;
    if !single_segment {
        c.u8().ok_or("truncated zstd frame header")?; // window descriptor
    }
    let dict_bytes = match fhd & 3 {
        0 => 0,
        1 => 1,
        2 => 2,
        _ => 4,
    };
    c.take(dict_bytes).ok_or("truncated zstd frame header")?;
    let fcs_bytes = match fhd >> 6 {
        0 => usize::from(single_segment),
        1 => 2,
        2 => 4,
        _ => 8,
    };
    if fcs_bytes == 0 {
        return Ok(None);
    }
    let b = c.take(fcs_bytes).ok_or("truncated zstd frame header")?;
    let mut v = 0u64;
    for (i, x) in b.iter().enumerate() {
        v |= u64::from(*x) << (8 * i);
    }
    if fcs_bytes == 2 {
        v = v.saturating_add(256);
    }
    Ok(Some(v))
}

/// Decompress a `qCompress` byte array: big-endian uncompressed length, then a zlib stream.
pub fn qcompress(bytes: &[u8]) -> Result<Vec<u8>, Error> {
    let mut c = Cur::new(bytes);
    let len = usize::try_from(c.u32().ok_or("truncated qCompress header")?).map_err(|_| "qCompress blob too large")?;
    if len > MAX_BLOB {
        return Err("qCompress blob too large");
    }
    inflate::zlib(c.rest(), len).map_err(|_| "corrupt zlib data")
}

// ---------------------------------------------------------------------------------------------
// Numbers
// ---------------------------------------------------------------------------------------------

/// Two little-endian doubles (frame rates `(fps, 0)`, media extents `(start s, duration s)`).
pub fn f64_pair_le(b: &[u8]) -> Option<(f64, f64)> {
    let a: [u8; 16] = b.try_into().ok()?;
    let (x, y) = a.split_at(8);
    Some((f64::from_le_bytes(x.try_into().ok()?), f64::from_le_bytes(y.try_into().ok()?)))
}

/// Two big-endian 64-bit integers (picture sizes `(width, height)`).
pub fn i64_pair_be(b: &[u8]) -> Option<(i64, i64)> {
    let a: [u8; 16] = b.try_into().ok()?;
    let (x, y) = a.split_at(8);
    Some((i64::from_be_bytes(x.try_into().ok()?), i64::from_be_bytes(y.try_into().ok()?)))
}

/// What a `MediaTimemapBA` says about a clip's source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timemap {
    /// Duration of the source media, in seconds.
    pub media_seconds: f64,
    /// The map is not the plain one-to-one map every untouched clip carries: the clip may be
    /// retimed (speed change, freeze), which the importer does not reproduce.
    pub retimed: bool,
}

/// Decode a `MediaTimemapBA`: version byte 2, then one big-endian double (the media duration) or
/// five (`duration, 0, duration − ε, 0, duration` for an untouched clip).
pub fn timemap(b: &[u8]) -> Option<Timemap> {
    let (&version, rest) = b.split_first()?;
    if version != 2 || !rest.len().is_multiple_of(8) {
        return None;
    }
    let vals: Vec<f64> = rest.as_chunks::<8>().0.iter().map(|p| f64::from_be_bytes(*p)).collect();
    match vals.as_slice() {
        [d] if d.is_finite() => Some(Timemap { media_seconds: *d, retimed: false }),
        [d, a, _, c, e] if d.is_finite() => Some(Timemap { media_seconds: *d, retimed: !(*a == 0.0 && *c == 0.0 && (d - e).abs() <= 1e-6) }),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Protocol buffers wire format
// ---------------------------------------------------------------------------------------------

/// One field of a protocol-buffers message, by wire type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Wire<'a> {
    Varint(u64),
    Fixed64(u64),
    Fixed32(u32),
    Bytes(&'a [u8]),
}

fn varint(c: &mut Cur<'_>) -> Option<u64> {
    let mut v = 0u64;
    for i in 0..10u32 {
        let byte = c.u8()?;
        let shift = 7 * i;
        if shift >= 64 {
            return None;
        }
        v |= u64::from(byte & 0x7f).checked_shl(shift)?;
        if byte & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// Every `(field number, value)` of a message, in order. Groups (wire types 3 and 4) are not
/// supported; a message using them is an error.
pub fn proto_fields(b: &[u8]) -> Result<Vec<(u32, Wire<'_>)>, Error> {
    let mut c = Cur::new(b);
    let mut out = Vec::new();
    while !c.at_end() {
        if out.len() >= MAX_FIELDS {
            return Err("too many protobuf fields");
        }
        let tag = varint(&mut c).ok_or("truncated protobuf")?;
        let field = u32::try_from(tag >> 3).map_err(|_| "bad protobuf field number")?;
        if field == 0 {
            return Err("bad protobuf field number");
        }
        let wire = match tag & 7 {
            0 => Wire::Varint(varint(&mut c).ok_or("truncated protobuf")?),
            1 => Wire::Fixed64(c.u64_le().ok_or("truncated protobuf")?),
            5 => Wire::Fixed32(c.u32_le().ok_or("truncated protobuf")?),
            2 => {
                let n = usize::try_from(varint(&mut c).ok_or("truncated protobuf")?).map_err(|_| "truncated protobuf")?;
                Wire::Bytes(c.take(n).ok_or("truncated protobuf")?)
            }
            _ => return Err("unsupported protobuf wire type"),
        };
        out.push((field, wire));
    }
    Ok(out)
}

/// The first length-delimited value of `field` (a string or a nested message).
pub fn proto_bytes(b: &[u8], field: u32) -> Option<&[u8]> {
    proto_fields(b).ok()?.into_iter().find_map(|(f, w)| match w {
        Wire::Bytes(x) if f == field => Some(x),
        _ => None,
    })
}

/// The first value of `field` as UTF-8 text.
pub fn proto_string(b: &[u8], field: u32) -> Option<String> {
    std::str::from_utf8(proto_bytes(b, field)?).ok().map(str::to_string)
}

/// The first varint value of `field`.
pub fn proto_varint(b: &[u8], field: u32) -> Option<u64> {
    proto_fields(b).ok()?.into_iter().find_map(|(f, w)| match w {
        Wire::Varint(x) if f == field => Some(x),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(s: &str) -> Vec<u8> {
        hex(s).unwrap()
    }

    /// Serialise a QString (null for `None`).
    fn qs(s: Option<&str>) -> Vec<u8> {
        match s {
            None => u32::MAX.to_be_bytes().to_vec(),
            Some(s) => {
                let units: Vec<u8> = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
                let mut v = (units.len() as u32).to_be_bytes().to_vec();
                v.extend(units);
                v
            }
        }
    }

    fn var(t: u32, payload: &[u8]) -> Vec<u8> {
        let mut v = t.to_be_bytes().to_vec();
        v.push(0);
        v.extend_from_slice(payload);
        v
    }

    fn map(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
        let mut v = 1u32.to_be_bytes().to_vec();
        v.extend((entries.len() as u32).to_be_bytes());
        for (k, val) in entries {
            v.extend(qs(Some(k)));
            v.extend(val);
        }
        v
    }

    #[test]
    fn hex_text() {
        assert_eq!(hex("0a0B ff"), Some(vec![10, 11, 255]));
        assert_eq!(hex(""), Some(vec![]));
        assert_eq!(hex("abc"), None);
        assert_eq!(hex("zz"), None);
    }

    #[test]
    fn qt_map_real_track_fields() {
        // Sm2TiTrack.FieldsBlob from an exported project: {"NumLayers": 0}
        let m = qt_map(&h("000000010000000100000012004e0075006d004c00610079006500720073000000020000000000")).unwrap();
        assert_eq!(m.get("NumLayers"), Some(&Value::Int(0)));
    }

    #[test]
    fn qt_map_types() {
        let nested = map(&[("SampleRate", var(3, &48_000u32.to_be_bytes())), ("Duration", var(4, &2_426_065i64.to_be_bytes()))]);
        let mut ba = (nested.len() as u32).to_be_bytes().to_vec();
        ba.extend(&nested);
        let b = map(&[
            ("Bool", var(1, &[1])),
            ("Int", var(2, &(-7i32).to_be_bytes())),
            ("Double", var(6, &30.0f64.to_be_bytes())),
            ("Float", var(38, &1.5f32.to_be_bytes())),
            ("Name", var(10, &qs(Some("Homeless Camp_005.mov")))),
            ("NullStr", var(10, &qs(None))),
            ("NullBytes", var(12, &u32::MAX.to_be_bytes())),
            ("Tracks", var(12, &ba)),
        ]);
        let m = qt_map(&b).unwrap();
        assert_eq!(m["Bool"].as_bool(), Some(true));
        assert_eq!(m["Int"].as_i64(), Some(-7));
        assert_eq!(m["Double"].as_f64(), Some(30.0));
        assert_eq!(m["Double"].as_i64(), Some(30));
        assert_eq!(m["Float"].as_f64(), Some(1.5));
        assert_eq!(m["Name"].as_str(), Some("Homeless Camp_005.mov"));
        assert_eq!(m["NullStr"], Value::Str(None));
        assert_eq!(m["NullBytes"], Value::Null);
        let inner = qt_map(m["Tracks"].as_bytes().unwrap()).unwrap();
        assert_eq!(inner["SampleRate"].as_i64(), Some(48_000));
        assert_eq!(inner["Duration"].as_i64(), Some(2_426_065));
        // containers
        let mut list = 2u32.to_be_bytes().to_vec();
        list.extend(var(2, &1i32.to_be_bytes()));
        list.extend(var(10, &qs(Some("x"))));
        let mut sl = 1u32.to_be_bytes().to_vec();
        sl.extend(qs(Some("a")));
        let mut vm = 1u32.to_be_bytes().to_vec();
        vm.extend(qs(Some("k")));
        vm.extend(var(1, &[0]));
        let m = qt_map(&map(&[("L", var(9, &list)), ("S", var(11, &sl)), ("M", var(8, &vm))])).unwrap();
        assert_eq!(m["L"], Value::List(vec![Value::Int(1), Value::Str(Some("x".into()))]));
        assert_eq!(m["S"], Value::StrList(vec![Some("a".into())]));
        assert_eq!(m["M"], Value::Map(Map::from([("k".to_string(), Value::Bool(false))])));
    }

    #[test]
    fn qt_map_unknown_type_keeps_what_was_read() {
        let b = map(&[("A", var(2, &1i32.to_be_bytes())), ("Weird", var(77, &[1, 2, 3])), ("B", var(2, &2i32.to_be_bytes()))]);
        let m = qt_map(&b).unwrap();
        assert_eq!(m["A"].as_i64(), Some(1));
        assert_eq!(m["Weird"], Value::Unknown(77));
        assert!(!m.contains_key("B"));
    }

    #[test]
    fn qt_map_hostile() {
        let good = map(&[("A", var(2, &1i32.to_be_bytes())), ("S", var(10, &qs(Some("hello"))))]);
        for n in 0..good.len() {
            let _ = qt_map(&good[..n]);
        }
        // huge counts and lengths
        let mut huge = good.clone();
        huge[4..8].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(qt_map(&huge).is_err());
        let mut lie = good.clone();
        lie[8..12].copy_from_slice(&0x7FFF_FFF0u32.to_be_bytes());
        assert!(qt_map(&lie).is_err());
        // deep nesting
        let mut deep = var(2, &0i32.to_be_bytes());
        for _ in 0..40 {
            let mut inner = 1u32.to_be_bytes().to_vec();
            inner.extend(qs(Some("k")));
            inner.extend(&deep);
            deep = var(8, &inner);
        }
        assert!(qt_map(&map(&[("D", deep)])).is_err());
        // odd UTF-16 byte counts and unpaired surrogates do not panic
        let mut odd = 1u32.to_be_bytes().to_vec();
        odd.extend(1u32.to_be_bytes());
        odd.extend(3u32.to_be_bytes());
        odd.extend([0xd8, 0x00, 0x41]);
        odd.extend(var(1, &[1]));
        assert!(qt_map(&odd).is_ok());
    }

    #[test]
    fn v2_payload_raw_protobuf() {
        // Sm2MpFolder.FieldsBlob: field 4 varint, field 11 message {1: "f83dc669aa53", 2: ""}
        let p = v2_payload(&h("00000002000000198020deb695ad0d5a100a0c6638336463363639616135331200")).unwrap();
        assert_eq!(proto_varint(&p, 4), Some(3_584_383_838));
        let sys = proto_bytes(&p, 11).unwrap();
        assert_eq!(proto_string(sys, 1).as_deref(), Some("f83dc669aa53"));
        assert_eq!(proto_bytes(sys, 2), Some(&[][..]));
        // Sm2TiItem.FieldsBlob: {1: {4: 1}, 15: 4}
        let p = v2_payload(&h("0000000200000007800a0220017804")).unwrap();
        assert_eq!(proto_varint(proto_bytes(&p, 1).unwrap(), 4), Some(1));
        assert_eq!(proto_varint(&p, 15), Some(4));
        assert_eq!(v2_payload(&h("00000003000000018000")), Err("unknown blob version"));
        assert_eq!(v2_payload(&h("000000020000000182")), Err("unknown blob encoding"));
        assert_eq!(v2_payload(&h("00000002000000ff80")), Err("blob length disagrees with its data"));
        assert!(v2_payload(&[]).is_err());
    }

    fn raw_frame(data: &[u8]) -> Vec<u8> {
        let mut f = vec![0x28, 0xb5, 0x2f, 0xfd, 0x20, data.len() as u8];
        let hdr = ((data.len() as u32) << 3) | 1;
        f.extend_from_slice(&hdr.to_le_bytes()[..3]);
        f.extend_from_slice(data);
        f
    }

    #[test]
    fn zstd_frames() {
        // raw block
        assert_eq!(zstd(&raw_frame(b"hello")).unwrap(), b"hello");
        // RLE block: 7 × 'z'
        let mut rle = vec![0x28, 0xb5, 0x2f, 0xfd, 0x20, 7];
        rle.extend_from_slice(&((7u32 << 3) | 2 | 1).to_le_bytes()[..3]);
        rle.push(b'z');
        assert_eq!(zstd(&rle).unwrap(), b"zzzzzzz");
        // compressed blocks, through ruzstd's own encoder
        let text: Vec<u8> = (0..300).flat_map(|i| format!("<Element>{}</Element>", i % 7).into_bytes()).collect();
        let frame = ruzstd::encoding::compress_to_vec(text.as_slice(), ruzstd::encoding::CompressionLevel::Fastest);
        assert!(frame.len() < text.len());
        assert_eq!(zstd(&frame).unwrap(), text);
        // a v2 payload wrapping that frame
        let mut blob = 2u32.to_be_bytes().to_vec();
        blob.extend((frame.len() as u32 + 1).to_be_bytes());
        blob.push(0x81);
        blob.extend(&frame);
        assert_eq!(v2_payload(&blob).unwrap(), text);
        // stated size over the cap is refused before decoding
        let mut big = vec![0x28, 0xb5, 0x2f, 0xfd, 0xe0];
        big.extend((1u64 << 40).to_le_bytes());
        assert_eq!(zstd(&big), Err("zstd blob too large"));
        // header lies about the size
        let mut lie = raw_frame(b"hello");
        lie[5] = 9;
        assert!(zstd(&lie).is_err());
        assert_eq!(zstd(b"nope"), Err("not a zstd frame"));
    }

    #[test]
    fn zstd_hostile_inputs_do_not_panic() {
        let text: Vec<u8> = (0..200).flat_map(|i| format!("<N>{}</N>", i % 5).into_bytes()).collect();
        let frame = ruzstd::encoding::compress_to_vec(text.as_slice(), ruzstd::encoding::CompressionLevel::Fastest);
        for n in 0..frame.len() {
            let _ = zstd(&frame[..n]);
        }
        for i in 0..frame.len() {
            for bit in [0u8, 4, 7] {
                let mut m = frame.clone();
                m[i] ^= 1 << bit;
                let _ = zstd(&m);
            }
        }
        let mut x = 0x9E37_79B9u64;
        for _ in 0..200 {
            let bytes: Vec<u8> = (0..64)
                .map(|_| {
                    x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    (x >> 33) as u8
                })
                .collect();
            let mut f = vec![0x28, 0xb5, 0x2f, 0xfd];
            f.extend(&bytes);
            let _ = zstd(&f);
        }
    }

    #[test]
    fn qcompress_blob() {
        let out = qcompress(&h("0000002478da6366606420040000b20005")).unwrap();
        assert_eq!(out.len(), 36);
        assert_eq!(&out[..4], &[3, 0, 1, 0]);
        assert!(qcompress(&h("000000")).is_err());
        assert_eq!(qcompress(&h("ffffffff78da")), Err("qCompress blob too large"));
        assert!(qcompress(&h("0000002400ff")).is_err());
    }

    #[test]
    fn number_pairs_and_timemaps() {
        assert_eq!(f64_pair_le(&h("0000000000003e400000000000000000")), Some((30.0, 0.0)));
        assert_eq!(f64_pair_le(&h("000000000020ac4040444444444449 40")), Some((3600.0, 50.5333333333333)));
        assert_eq!(i64_pair_be(&h("00000000000007800000000000000438")), Some((1920, 1080)));
        assert_eq!(f64_pair_le(&[0; 15]), None);
        let t = timemap(&h("0240489dddddddddde")).unwrap();
        assert!((t.media_seconds - 49.2333333).abs() < 1e-6 && !t.retimed);
        let t = timemap(&h("02407123d9a2568fe80000000000000000407123734d6a161e0000000000000000407123d9a2568fe8")).unwrap();
        assert!((t.media_seconds - 274.2406).abs() < 1e-3 && !t.retimed);
        let t = timemap(&h("02407123d9a2568fe80000000000000000407123734d6a161e0000000000000000400000000000 0000")).unwrap();
        assert!(t.retimed);
        assert_eq!(timemap(&h("01")), None);
        assert_eq!(timemap(&h("0200")), None);
        assert_eq!(timemap(&[]), None);
    }

    #[test]
    fn protobuf_hostile() {
        assert!(proto_fields(&[0x80]).is_err()); // truncated varint
        assert!(proto_fields(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]).is_err()); // 11-byte varint
        assert_eq!(proto_fields(&[0x0b]), Err("unsupported protobuf wire type")); // group start
        assert_eq!(proto_fields(&[0x00, 0x01]), Err("bad protobuf field number"));
        assert!(proto_fields(&[0x0a, 0x10, 0x01]).is_err()); // length past the end
        assert_eq!(proto_fields(&[0x09, 1, 0, 0, 0, 0, 0, 0, 0]).unwrap(), vec![(1, Wire::Fixed64(1))]);
        assert_eq!(proto_fields(&[0x0d, 2, 0, 0, 0]).unwrap(), vec![(1, Wire::Fixed32(2))]);
        assert_eq!(proto_string(&[0x0a, 0x02, 0xff, 0xfe], 1), None);
    }
}
