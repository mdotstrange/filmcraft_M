//! Minimal read-only ZIP reader for DaVinci Resolve `.drp` exports, written from the PKWARE
//! APPNOTE (version 6.3.x): end-of-central-directory record, central directory file headers and
//! local file headers. Only what a `.drp` needs is supported: stored (method 0) and DEFLATE
//! (method 8) entries, one disk, no ZIP64, no encryption. Entry sizes come from the central
//! directory, so entries written with a data descriptor read fine.
//!
//! Every length and offset in the archive is hostile: all reads go through `get()`, every sum is
//! checked, and the decompressed size of an entry is capped.

use filmcraft_media::still::exr::inflate;

/// Signature of a local file header (`PK\x03\x04`).
const LOCAL_SIG: [u8; 4] = [0x50, 0x4b, 0x03, 0x04];
/// Signature of a central directory file header (`PK\x01\x02`).
const CENTRAL_SIG: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];
/// Signature of the end-of-central-directory record (`PK\x05\x06`).
const EOCD_SIG: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
/// Fixed part of the end-of-central-directory record.
const EOCD_LEN: usize = 22;
/// Fixed part of a central directory file header.
const CENTRAL_LEN: usize = 46;
/// Fixed part of a local file header.
const LOCAL_LEN: usize = 30;
/// Largest archive comment (its length is a `u16`): how far back the EOCD search goes.
const MAX_COMMENT: usize = 0xFFFF;
/// Most entries an archive may list (the count is a `u16`; `0xFFFF` means ZIP64).
const MAX_ENTRIES: usize = 0xFFFE;
/// Largest entry we decompress (a `.drp` is a few megabytes of XML).
pub const MAX_ENTRY_SIZE: usize = 256 << 20;

/// Why an archive could not be read.
pub type Error = &'static str;

/// One file in the archive, as listed by the central directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Path inside the archive, with `/` separators as written.
    pub name: String,
    /// Compression method: 0 stored, 8 DEFLATE.
    pub method: u16,
    /// General purpose bit flags (bit 0 = encrypted).
    pub flags: u16,
    /// Size of the stored data.
    pub compressed_size: u32,
    /// Size after decompression.
    pub size: u32,
    /// Offset of the entry's local file header from the start of the archive.
    local_offset: u32,
}

impl Entry {
    /// Whether the entry is a directory marker (`dir/`).
    pub fn is_dir(&self) -> bool {
        self.name.ends_with('/')
    }
}

/// A parsed central directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Archive {
    pub entries: Vec<Entry>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    let s = b.get(at..at.checked_add(2)?)?;
    Some(u16::from_le_bytes([s[0], s[1]]))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    let s = b.get(at..at.checked_add(4)?)?;
    Some(u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
}

/// Whether `bytes` start like a ZIP archive (a local file header).
pub fn is_zip(bytes: &[u8]) -> bool {
    bytes.starts_with(&LOCAL_SIG)
}

impl Archive {
    /// Read the central directory of `bytes`.
    pub fn parse(bytes: &[u8]) -> Result<Archive, Error> {
        const TRUNC: Error = "truncated archive";
        const TRUNC_CD: Error = "truncated central directory";
        let eocd = find_eocd(bytes).ok_or("not a ZIP archive (no end-of-central-directory record)")?;
        let disk = u16_at(bytes, eocd + 4).ok_or(TRUNC)?;
        let cd_disk = u16_at(bytes, eocd + 6).ok_or(TRUNC)?;
        if disk != 0 || cd_disk != 0 {
            return Err("multi-disk ZIP archives are not supported");
        }
        let count = usize::from(u16_at(bytes, eocd + 10).ok_or(TRUNC)?);
        let cd_size = u32_at(bytes, eocd + 12).ok_or(TRUNC)?;
        let cd_offset = u32_at(bytes, eocd + 16).ok_or(TRUNC)?;
        if count > MAX_ENTRIES || cd_size == u32::MAX || cd_offset == u32::MAX {
            return Err("ZIP64 archives are not supported");
        }
        let cd_start = usize::try_from(cd_offset).map_err(|_| "central directory offset out of range")?;
        let cd_len = usize::try_from(cd_size).map_err(|_| "central directory size out of range")?;
        let cd_end = cd_start.checked_add(cd_len).ok_or("central directory size out of range")?;
        if cd_end > eocd {
            return Err("central directory lies outside the archive");
        }
        let mut entries = Vec::new();
        entries.try_reserve_exact(count).map_err(|_| "too many entries")?;
        let mut pos = cd_start;
        for _ in 0..count {
            if bytes.get(pos..pos.checked_add(4).ok_or(TRUNC_CD)?) != Some(&CENTRAL_SIG) {
                return Err("bad central directory file header");
            }
            let flags = u16_at(bytes, pos + 8).ok_or(TRUNC_CD)?;
            let method = u16_at(bytes, pos + 10).ok_or(TRUNC_CD)?;
            let compressed_size = u32_at(bytes, pos + 20).ok_or(TRUNC_CD)?;
            let size = u32_at(bytes, pos + 24).ok_or(TRUNC_CD)?;
            let name_len = usize::from(u16_at(bytes, pos + 28).ok_or(TRUNC_CD)?);
            let extra_len = usize::from(u16_at(bytes, pos + 30).ok_or(TRUNC_CD)?);
            let comment_len = usize::from(u16_at(bytes, pos + 32).ok_or(TRUNC_CD)?);
            let local_offset = u32_at(bytes, pos + 42).ok_or(TRUNC_CD)?;
            let name_start = pos.checked_add(CENTRAL_LEN).ok_or(TRUNC_CD)?;
            let name_end = name_start.checked_add(name_len).ok_or(TRUNC_CD)?;
            let name_bytes = bytes.get(name_start..name_end).ok_or(TRUNC_CD)?;
            if name_end > cd_end {
                return Err("central directory file header lies outside the central directory");
            }
            if flags & 0x0001 != 0 {
                return Err("encrypted ZIP archives are not supported");
            }
            if compressed_size == u32::MAX || size == u32::MAX || local_offset == u32::MAX {
                return Err("ZIP64 archives are not supported");
            }
            let name = String::from_utf8_lossy(name_bytes).into_owned();
            entries.push(Entry { name, method, flags, compressed_size, size, local_offset });
            pos = name_end.checked_add(extra_len).and_then(|p| p.checked_add(comment_len)).ok_or(TRUNC_CD)?;
        }
        Ok(Archive { entries })
    }

    /// The entry at exactly `name` (archive paths use `/`).
    pub fn find(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Decompress `entry` out of `bytes` (the same bytes the archive was parsed from).
    pub fn read(&self, bytes: &[u8], entry: &Entry) -> Result<Vec<u8>, Error> {
        const TRUNC: Error = "truncated local file header";
        let size = usize::try_from(entry.size).map_err(|_| "entry size out of range")?;
        if size > MAX_ENTRY_SIZE {
            return Err("entry too large");
        }
        let local = usize::try_from(entry.local_offset).map_err(|_| "local header offset out of range")?;
        if bytes.get(local..local.checked_add(4).ok_or(TRUNC)?) != Some(&LOCAL_SIG) {
            return Err("bad local file header");
        }
        // The local header carries its own name and extra lengths (the extra field may differ
        // from the central directory's); the sizes are the central directory's.
        let name_len = usize::from(u16_at(bytes, local + 26).ok_or(TRUNC)?);
        let extra_len = usize::from(u16_at(bytes, local + 28).ok_or(TRUNC)?);
        let data_start = local.checked_add(LOCAL_LEN).and_then(|p| p.checked_add(name_len)).and_then(|p| p.checked_add(extra_len)).ok_or(TRUNC)?;
        let compressed = usize::try_from(entry.compressed_size).map_err(|_| "entry size out of range")?;
        let data_end = data_start.checked_add(compressed).ok_or("entry data out of range")?;
        let data = bytes.get(data_start..data_end).ok_or("entry data lies outside the archive")?;
        match entry.method {
            0 => {
                if data.len() != size {
                    return Err("stored entry sizes disagree");
                }
                Ok(data.to_vec())
            }
            8 => {
                let out = inflate::inflate(data, size).map_err(|_| "corrupt DEFLATE data")?;
                if out.len() != size {
                    return Err("DEFLATE data ends early");
                }
                Ok(out)
            }
            _ => Err("unsupported compression method (only stored and DEFLATE)"),
        }
    }
}

/// Offset of the end-of-central-directory record: the last `PK\x05\x06` whose comment length
/// matches the bytes that follow it.
fn find_eocd(bytes: &[u8]) -> Option<usize> {
    let len = bytes.len();
    if len < EOCD_LEN {
        return None;
    }
    let first = len - EOCD_LEN;
    let last = first.saturating_sub(MAX_COMMENT);
    let mut pos = first;
    loop {
        if bytes.get(pos..pos + 4) == Some(&EOCD_SIG)
            && let Some(comment) = u16_at(bytes, pos + 20)
            && pos + EOCD_LEN + usize::from(comment) == len
        {
            return Some(pos);
        }
        if pos == last {
            return None;
        }
        pos -= 1;
    }
}

/// Write a ZIP archive with stored (uncompressed) entries. Tests and fixture builders use it;
/// `.drp` files are never written by FilmCraft.
pub fn write_stored(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let owned: Vec<(&str, &[u8], u16, Vec<u8>)> = entries.iter().map(|(n, d)| (*n, *d, 0u16, d.to_vec())).collect();
    write_archive(&owned)
}

/// Write a ZIP archive from `(name, uncompressed data, method, stored bytes)` tuples. Entry
/// names and sizes beyond the 16/32-bit fields are truncated; this writer exists for tests.
pub fn write_archive(entries: &[(&str, &[u8], u16, Vec<u8>)]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xFFFF_FFFFu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 { (crc >> 1) ^ 0xEDB8_8320 } else { crc >> 1 };
            }
        }
        !crc
    }
    fn u16f(n: usize) -> [u8; 2] {
        u16::try_from(n).unwrap_or(u16::MAX).to_le_bytes()
    }
    fn u32f(n: usize) -> [u8; 4] {
        u32::try_from(n).unwrap_or(u32::MAX).to_le_bytes()
    }
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data, method, stored) in entries {
        let local = out.len();
        let crc = crc32(data);
        let name = name.as_bytes();
        out.extend_from_slice(&LOCAL_SIG);
        out.extend_from_slice(&20u16.to_le_bytes()); // version needed
        out.extend_from_slice(&0x0800u16.to_le_bytes()); // flags: UTF-8 names
        out.extend_from_slice(&method.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]); // time, date
        out.extend_from_slice(&crc.to_le_bytes());
        out.extend_from_slice(&u32f(stored.len()));
        out.extend_from_slice(&u32f(data.len()));
        out.extend_from_slice(&u16f(name.len()));
        out.extend_from_slice(&0u16.to_le_bytes()); // extra
        out.extend_from_slice(name);
        out.extend_from_slice(stored);
        central.extend_from_slice(&CENTRAL_SIG);
        central.extend_from_slice(&20u16.to_le_bytes()); // version made by
        central.extend_from_slice(&20u16.to_le_bytes()); // version needed
        central.extend_from_slice(&0x0800u16.to_le_bytes());
        central.extend_from_slice(&method.to_le_bytes());
        central.extend_from_slice(&[0, 0, 0, 0]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&u32f(stored.len()));
        central.extend_from_slice(&u32f(data.len()));
        central.extend_from_slice(&u16f(name.len()));
        central.extend_from_slice(&0u16.to_le_bytes()); // extra
        central.extend_from_slice(&0u16.to_le_bytes()); // comment
        central.extend_from_slice(&0u16.to_le_bytes()); // disk
        central.extend_from_slice(&0u16.to_le_bytes()); // internal attributes
        central.extend_from_slice(&0u32.to_le_bytes()); // external attributes
        central.extend_from_slice(&u32f(local));
        central.extend_from_slice(name);
    }
    let cd_offset = out.len();
    out.extend_from_slice(&central);
    out.extend_from_slice(&EOCD_SIG);
    out.extend_from_slice(&0u16.to_le_bytes()); // this disk
    out.extend_from_slice(&0u16.to_le_bytes()); // central directory disk
    out.extend_from_slice(&u16f(entries.len()));
    out.extend_from_slice(&u16f(entries.len()));
    out.extend_from_slice(&u32f(central.len()));
    out.extend_from_slice(&u32f(cd_offset));
    out.extend_from_slice(&0u16.to_le_bytes()); // comment length
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &[u8] = b"<Sm2MpFolder DbId=\"8dca124b\"><Name>Master</Name><Name>Master</Name><Name>Master</Name></Sm2MpFolder>";

    /// `XML` as a raw DEFLATE stream (zlib level 9), produced once with Python's `zlib`.
    const XML_DEFLATED: &[u8] = &[
        0xb3, 0x09, 0xce, 0x35, 0xf2, 0x2d, 0x70, 0xcb, 0xcf, 0x49, 0x49, 0x2d, 0x52, 0x70, 0x49, 0xf2, 0x4c, 0xb1, 0x55, 0xb2, 0x48, 0x49, 0x4e, 0x34, 0x34,
        0x32, 0x49, 0x52, 0xb2, 0xb3, 0xf1, 0x4b, 0xcc, 0x4d, 0xb5, 0xf3, 0x4d, 0x2c, 0x2e, 0x49, 0x2d, 0xb2, 0xd1, 0x07, 0x73, 0x88, 0x14, 0xd2, 0x47, 0x32,
        0xd6, 0x0e, 0x00,
    ];

    fn sample() -> Vec<u8> {
        write_stored(&[("project.xml", b"<SM_Project/>"), ("MediaPool/Master/MpFolder.xml", XML), ("dir/", b"")])
    }

    #[test]
    fn stored_entries_round_trip() {
        let zip = sample();
        assert!(is_zip(&zip));
        let a = Archive::parse(&zip).unwrap();
        assert_eq!(a.entries.len(), 3);
        let p = a.find("project.xml").unwrap();
        assert_eq!(a.read(&zip, p).unwrap(), b"<SM_Project/>");
        let f = a.find("MediaPool/Master/MpFolder.xml").unwrap();
        assert_eq!(a.read(&zip, f).unwrap(), XML);
        assert!(a.find("dir/").unwrap().is_dir());
        assert!(a.find("nope.xml").is_none());
    }

    #[test]
    fn deflated_entry_inflates() {
        let zip = write_archive(&[("a.xml", XML, 8, XML_DEFLATED.to_vec())]);
        let a = Archive::parse(&zip).unwrap();
        let e = a.find("a.xml").unwrap();
        assert_eq!(e.method, 8);
        assert_eq!(a.read(&zip, e).unwrap(), XML);
        // a stored DEFLATE block (BTYPE 00) is DEFLATE too
        let n = XML.len() as u16;
        let mut stored_block = vec![0x01];
        stored_block.extend_from_slice(&n.to_le_bytes());
        stored_block.extend_from_slice(&(!n).to_le_bytes());
        stored_block.extend_from_slice(XML);
        let zip = write_archive(&[("b.xml", XML, 8, stored_block)]);
        let a = Archive::parse(&zip).unwrap();
        assert_eq!(a.read(&zip, a.find("b.xml").unwrap()).unwrap(), XML);
    }

    #[test]
    fn archive_comment_is_skipped() {
        let mut zip = sample();
        let n = zip.len();
        zip[n - 2..].copy_from_slice(&5u16.to_le_bytes());
        zip.extend_from_slice(b"hello");
        assert_eq!(Archive::parse(&zip).unwrap().entries.len(), 3);
    }

    #[test]
    fn hostile_archives_are_errors_not_panics() {
        let zip = sample();
        // truncations at every length
        for n in 0..zip.len() {
            if let Ok(a) = Archive::parse(&zip[..n]) {
                for e in &a.entries {
                    let _ = a.read(&zip[..n], e);
                }
            }
        }
        assert!(Archive::parse(b"").is_err());
        assert!(Archive::parse(b"PK\x03\x04").is_err());
        // central directory offset past the end
        let mut bad = zip.clone();
        let n = bad.len();
        bad[n - 6..n - 2].copy_from_slice(&0xFFFF_FF00u32.to_le_bytes());
        assert!(Archive::parse(&bad).is_err());
        // ZIP64 markers
        let mut z64 = zip.clone();
        z64[n - 6..n - 2].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(Archive::parse(&z64), Err("ZIP64 archives are not supported"));
        let mut z64 = zip.clone();
        z64[n - 12..n - 10].copy_from_slice(&0xFFFFu16.to_le_bytes());
        assert_eq!(Archive::parse(&z64), Err("ZIP64 archives are not supported"));
        // encryption flag and an unknown method on the first entry
        let a = Archive::parse(&zip).unwrap();
        let cd = zip.len() - EOCD_LEN - a.entries.iter().map(|e| CENTRAL_LEN + e.name.len()).sum::<usize>();
        let mut enc = zip.clone();
        enc[cd + 8] |= 1;
        assert_eq!(Archive::parse(&enc), Err("encrypted ZIP archives are not supported"));
        let mut meth = zip.clone();
        meth[cd + 10] = 99;
        let a = Archive::parse(&meth).unwrap();
        assert!(a.read(&meth, &a.entries[0]).is_err());
        // sizes that disagree with the data, and a size over the cap
        let mut big = zip.clone();
        big[cd + 24..cd + 28].copy_from_slice(&(u32::MAX - 1).to_le_bytes());
        let a = Archive::parse(&big).unwrap();
        assert_eq!(a.read(&big, &a.entries[0]), Err("entry too large"));
        let mut lie = zip.clone();
        lie[cd + 24..cd + 28].copy_from_slice(&3u32.to_le_bytes());
        let a = Archive::parse(&lie).unwrap();
        assert_eq!(a.read(&lie, &a.entries[0]), Err("stored entry sizes disagree"));
        // corrupt DEFLATE data
        let zip = write_archive(&[("a.xml", XML, 8, vec![0xFF; 8])]);
        let a = Archive::parse(&zip).unwrap();
        assert!(a.read(&zip, &a.entries[0]).is_err());
    }

    #[test]
    fn random_bytes_never_panic() {
        // a cheap LCG keeps the test deterministic and dependency-free
        let mut x = 0x2545_F491u64;
        for len in [0usize, 1, 21, 22, 23, 46, 100, 1000] {
            for _ in 0..50 {
                let bytes: Vec<u8> = (0..len)
                    .map(|_| {
                        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                        (x >> 33) as u8
                    })
                    .collect();
                if let Ok(a) = Archive::parse(&bytes) {
                    for e in &a.entries {
                        let _ = a.read(&bytes, e);
                    }
                }
            }
        }
        // bit flips of a valid archive
        let zip = sample();
        for i in 0..zip.len() {
            for bit in [0u8, 3, 7] {
                let mut m = zip.clone();
                m[i] ^= 1 << bit;
                if let Ok(a) = Archive::parse(&m) {
                    for e in &a.entries {
                        let _ = a.read(&m, e);
                    }
                }
            }
        }
    }
}
