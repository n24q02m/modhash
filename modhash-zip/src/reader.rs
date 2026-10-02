//! The ZIP half of the crate: end-of-central-directory discovery, central
//! directory parsing, and payload extraction for stored and deflated
//! entries.
//!
//! Layout being parsed (PKZIP APPNOTE.TXT §4.3):
//!
//! ```text
//! [local file header + name + extra + payload (+ data descriptor)] ...
//! [central directory entry] ...
//! [end of central directory (+ comment)]
//! ```
//!
//! All multi-byte integers are little-endian. The reader is deliberately
//! trust-the-directory: the central directory carries the authoritative
//! CRC, sizes and local-header offset for every entry, and the local
//! header only has to agree with it (method and flags) and say how far the
//! payload starts.

use modhash_inflate::{Limits, inflate_raw};
use modhash_primitives::{Error, Result, crc32};

/// `0x04034b50` — signature of a local file header (APPNOTE §4.3.7).
const SIG_LOCAL: u32 = 0x0403_4b50;
/// `0x02014b50` — signature of a central directory entry (APPNOTE §4.3.12).
const SIG_CENTRAL: u32 = 0x0201_4b50;
/// `0x06054b50` — signature of the end-of-central-directory record
/// (APPNOTE §4.3.16).
const SIG_EOCD: u32 = 0x0605_4b50;
/// `0x08074b50` — optional signature prefix of a data descriptor
/// (APPNOTE §4.3.9.3).
const SIG_DESCRIPTOR: u32 = 0x0807_4b50;

/// Compression method 0: bytes stored verbatim.
const METHOD_STORED: u16 = 0;
/// Compression method 8: raw RFC 1951 DEFLATE.
const METHOD_DEFLATE: u16 = 8;

/// Fixed length of the end-of-central-directory record, comment excluded.
const EOCD_LEN: usize = 22;
/// Fixed length of a central directory entry, variable fields excluded.
const CENTRAL_LEN: usize = 46;
/// Fixed length of a local file header, name and extra excluded.
const LOCAL_LEN: usize = 30;
/// Longest legal EOCD comment: the `u16` comment-length field tops out at
/// 65 535, so the record can sit up to this many bytes before EOF.
const EOCD_MAX_COMMENT: usize = 65_535;

/// Flags bit 0: traditional PKWARE encryption. Bit 6 (strong encryption)
/// and bit 13 (encrypted central directory, local header fields masked)
/// are checked alongside it in [`ZipArchive::extract`].
const FLAG_ENCRYPTED: u16 = 1 << 0;
/// Flags bit 3: CRC-32 and both sizes are zero in the local header and
/// live in a data descriptor after the payload (APPNOTE §4.3.9.1).
const FLAG_DESCRIPTOR: u16 = 1 << 3;
/// Strong encryption (APPNOTE §4.4.4): refused, we cannot and will not
/// decrypt.
const FLAG_STRONG_ENCRYPTED: u16 = 1 << 6;
/// Central-directory-encrypted marker: the local header fields are masked
/// and meaningless (APPNOTE §4.4.6.2).
const FLAG_MASKED_LOCAL: u16 = 1 << 13;

/// Little-endian `u16` at `off`, or `None` when it runs past the data.
fn le16(data: &[u8], off: usize) -> Option<u16> {
    let b = data.get(off..off + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

/// Little-endian `u32` at `off`, or `None` when it runs past the data.
fn le32(data: &[u8], off: usize) -> Option<u32> {
    let b = data.get(off..off + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The `len` bytes starting at `off`, or a named [`Error::Truncated`].
fn take<'a>(data: &'a [u8], off: usize, len: usize, what: &'static str) -> Result<&'a [u8]> {
    let end = off
        .checked_add(len)
        .ok_or_else(|| Error::truncated(what, usize::MAX, data.len()))?;
    data.get(off..end)
        .ok_or_else(|| Error::truncated(what, end, data.len()))
}

/// The parsed end-of-central-directory record and where it sits.
struct Eocd {
    /// Entries in the central directory on this disk.
    entries: u16,
    /// Size of the central directory in bytes.
    cd_size: u32,
    /// Absolute offset where the central directory starts.
    cd_offset: u32,
    /// Absolute offset of the record itself (the directory must end here).
    offset: usize,
}

/// Scans backwards from EOF for the EOCD signature and parses the record.
///
/// APPNOTE §4.3.16: the record's last field is a comment of up to 65 535
/// bytes whose length is stored in the record, so the signature can appear
/// anywhere in the last `EOCD_LEN + EOCD_MAX_COMMENT` bytes. A signature
/// is only accepted when its comment length matches the bytes actually
/// left — that check is what lets us keep scanning past a `PK\x05\x06`
/// that appears inside a comment or a payload.
fn find_eocd(data: &[u8]) -> Result<Eocd> {
    let start = data.len().saturating_sub(EOCD_LEN + EOCD_MAX_COMMENT);
    // A candidate is a *candidate* EOCD: wrong comment length, or a
    // comment length that does not reach EOF, and we move to the next
    // signature backwards.
    let mut off = data.len().saturating_sub(EOCD_LEN);
    loop {
        if le32(data, off) == Some(SIG_EOCD) {
            let comment_len = le16(data, off + 20).ok_or_else(|| {
                Error::truncated("end of central directory", off + EOCD_LEN, data.len())
            })? as usize;
            if off + EOCD_LEN + comment_len == data.len() {
                return parse_eocd(data, off);
            }
        }
        if off == start {
            return Err(Error::InvalidMagic {
                what: "zip end of central directory",
            });
        }
        off -= 1;
    }
}

/// Validates the disk fields and ZIP64 sentinels of an EOCD whose comment
/// length already checks out.
fn parse_eocd(data: &[u8], off: usize) -> Result<Eocd> {
    let fields = take(data, off + 4, EOCD_LEN - 4, "end of central directory")?;
    // fields[0..2]  disk number of this disk
    // fields[2..4]  disk where the central directory starts
    // fields[4..6]  entries on this disk
    // fields[6..8]  total entries
    // fields[8..12] central directory size
    // fields[12..16] central directory offset
    // fields[16..18] comment length (already cross-checked)
    let disk = u16::from_le_bytes([fields[0], fields[1]]);
    let cd_disk = u16::from_le_bytes([fields[2], fields[3]]);
    let entries_disk = u16::from_le_bytes([fields[4], fields[5]]);
    let entries = u16::from_le_bytes([fields[6], fields[7]]);
    let cd_size = u32::from_le_bytes([fields[8], fields[9], fields[10], fields[11]]);
    let cd_offset = u32::from_le_bytes([fields[12], fields[13], fields[14], fields[15]]);

    if disk != 0 || cd_disk != 0 || entries_disk != entries {
        return Err(Error::Unsupported("multi-disk zip archive"));
    }
    if entries == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_offset == 0xFFFF_FFFF {
        return Err(Error::Unsupported("zip64 archive"));
    }
    Ok(Eocd {
        entries,
        cd_size,
        cd_offset,
        offset: off,
    })
}

/// One central directory entry: the authoritative record of a member of
/// the archive.
#[derive(Copy, Clone, Debug)]
pub struct ZipEntry<'a> {
    name: &'a str,
    flags: u16,
    method: u16,
    /// CRC-32 of the uncompressed payload (APPNOTE §4.4.7).
    crc32: u32,
    compressed_size: u32,
    uncompressed_size: u32,
    /// Absolute offset of this entry's local file header.
    local_offset: u32,
}

impl<'a> ZipEntry<'a> {
    /// The member name as stored in the central directory. Archives whose
    /// names are not valid UTF-8 are rejected at
    /// [`ZipArchive::new`]; the kit only feeds it names it typed.
    pub fn name(&self) -> &'a str {
        self.name
    }

    /// The compression method: `0` (stored) or `8` (DEFLATE). Other
    /// methods parse fine and refuse at extraction with
    /// [`Error::Unsupported`].
    pub fn method(&self) -> u16 {
        self.method
    }

    /// Payload length on disk, in bytes.
    pub fn compressed_size(&self) -> u64 {
        u64::from(self.compressed_size)
    }

    /// Expected decompressed length, in bytes. Verification compares the
    /// produced byte count against this before the CRC is checked.
    pub fn uncompressed_size(&self) -> u64 {
        u64::from(self.uncompressed_size)
    }

    /// CRC-32 of the uncompressed payload, as recorded by the archiver.
    pub fn crc32(&self) -> u32 {
        self.crc32
    }

    /// Whether this entry carries a data descriptor (flags bit 3): the
    /// local header's CRC and size fields are zero and the real values
    /// trail the payload.
    pub fn uses_data_descriptor(&self) -> bool {
        self.flags & FLAG_DESCRIPTOR != 0
    }
}

/// A parsed ZIP archive. Construction finds the end-of-central-directory
/// record and parses the whole central directory; payload bytes are only
/// touched by [`ZipArchive::extract`].
#[derive(Clone, Debug)]
pub struct ZipArchive<'a> {
    data: &'a [u8],
    entries: Vec<ZipEntry<'a>>,
}

impl<'a> ZipArchive<'a> {
    /// Parses `data` as a ZIP archive: finds the EOCD (searching backwards
    /// over a comment of up to 64 KiB), checks the single-disk and
    /// non-ZIP64 invariants, and parses every central directory entry.
    ///
    /// The directory must sit exactly where the EOCD says and end exactly
    /// where the EOCD starts; archives that smuggle extra bytes between
    /// directory entries are malformed by definition (APPNOTE §4.3.12).
    pub fn new(data: &'a [u8]) -> Result<Self> {
        let eocd = find_eocd(data)?;

        let cd_offset = usize::try_from(eocd.cd_offset).unwrap_or(usize::MAX);
        let cd_size = usize::try_from(eocd.cd_size).unwrap_or(usize::MAX);
        let cd_end = cd_offset
            .checked_add(cd_size)
            .ok_or_else(|| Error::truncated("zip central directory", usize::MAX, data.len()))?;
        // The directory must end at the EOCD itself: the record's offset
        // field is absolute, so a directory that would overlap its own
        // terminator is corrupt, not extended.
        if cd_end > eocd.offset {
            return Err(Error::truncated(
                "zip central directory",
                cd_end,
                eocd.offset,
            ));
        }
        let dir = take(data, cd_offset, cd_size, "zip central directory")?;

        let mut entries = Vec::with_capacity(usize::from(eocd.entries));
        let mut pos = 0usize;
        for _ in 0..eocd.entries {
            let fixed = take(dir, pos, CENTRAL_LEN, "zip central directory entry")?;
            let magic = u32::from_le_bytes([fixed[0], fixed[1], fixed[2], fixed[3]]);
            if magic != SIG_CENTRAL {
                return Err(Error::InvalidMagic {
                    what: "zip central directory entry",
                });
            }
            let flags = u16::from_le_bytes([fixed[8], fixed[9]]);
            let method = u16::from_le_bytes([fixed[10], fixed[11]]);
            let crc32 = u32::from_le_bytes([fixed[16], fixed[17], fixed[18], fixed[19]]);
            let compressed_size = u32::from_le_bytes([fixed[20], fixed[21], fixed[22], fixed[23]]);
            let uncompressed_size =
                u32::from_le_bytes([fixed[24], fixed[25], fixed[26], fixed[27]]);
            let name_len = usize::from(u16::from_le_bytes([fixed[28], fixed[29]]));
            let extra_len = usize::from(u16::from_le_bytes([fixed[30], fixed[31]]));
            let comment_len = usize::from(u16::from_le_bytes([fixed[32], fixed[33]]));
            let local_offset = u32::from_le_bytes([fixed[42], fixed[43], fixed[44], fixed[45]]);

            if compressed_size == 0xFFFF_FFFF
                || uncompressed_size == 0xFFFF_FFFF
                || local_offset == 0xFFFF_FFFF
            {
                return Err(Error::Unsupported("zip64 central directory entry"));
            }

            let name_bytes = take(
                dir,
                pos + CENTRAL_LEN,
                name_len,
                "zip entry name in central directory",
            )?;
            let name = core::str::from_utf8(name_bytes)
                .map_err(|_| Error::BadValue("zip entry name is not UTF-8"))?;

            let entry_len = CENTRAL_LEN
                .checked_add(name_len)
                .and_then(|n| n.checked_add(extra_len))
                .and_then(|n| n.checked_add(comment_len))
                .ok_or(Error::BadValue("zip central directory entry length"))?;
            if entry_len > dir.len() - pos {
                return Err(Error::truncated(
                    "zip central directory entry",
                    pos + entry_len,
                    dir.len(),
                ));
            }
            entries.push(ZipEntry {
                name,
                flags,
                method,
                crc32,
                compressed_size,
                uncompressed_size,
                local_offset,
            });
            pos += entry_len;
        }
        // Leftover bytes between the last entry and the EOCD mean the
        // directory's declared size disagrees with its contents.
        if pos != dir.len() {
            return Err(Error::BadValue("zip central directory size"));
        }
        Ok(ZipArchive { data, entries })
    }

    /// The central directory entries, in archive order.
    pub fn entries(&self) -> &[ZipEntry<'a>] {
        &self.entries
    }

    /// How many members the archive holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the archive holds no members.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The entry with this exact member name (byte-exact match — the
    /// archive's own convention), or `None`.
    pub fn by_name(&self, name: &str) -> Option<&ZipEntry<'a>> {
        self.entries.iter().find(|e| e.name == name)
    }

    /// Extracts `entry` and verifies it: sizes first, CRC-32 last. The
    /// entry must come from this archive's [`entries`](Self::entries) or
    /// [`by_name`](Self::by_name) — it carries offsets into `self.data`.
    ///
    /// Extraction is refused with [`Error::Unsupported`] for encrypted
    /// entries and compression methods other than stored (0) and DEFLATE
    /// (8), and [`Error::BadValue`] when the local header disagrees with
    /// the central directory, a data descriptor's numbers do not match
    /// the directory's, the decompressed length is wrong, or the CRC-32
    /// does not match.
    pub fn extract(&self, entry: &ZipEntry<'a>) -> Result<Vec<u8>> {
        self.extract_with(entry, &Limits::default())
    }

    /// Extracts the named entry: [`Error::BadValue`] (`"no such zip
    /// entry"`) when the name is absent, otherwise exactly
    /// [`extract`](Self::extract).
    pub fn extract_by_name(&self, name: &str) -> Result<Vec<u8>> {
        let entry = self
            .by_name(name)
            .ok_or(Error::BadValue("no such zip entry"))?;
        self.extract(entry)
    }

    /// [`extract`](Self::extract) with an explicit decompression ceiling.
    /// `limits.max_output` also bounds stored entries: an archive can
    /// claim any uncompressed size it likes and the ceiling keeps the
    /// reader's promise for free.
    pub fn extract_with(&self, entry: &ZipEntry<'a>, limits: &Limits) -> Result<Vec<u8>> {
        let base = usize::try_from(entry.local_offset)
            .map_err(|_| Error::BadValue("zip local header offset"))?;
        let hdr = take(self.data, base, LOCAL_LEN, "zip local file header")?;
        let magic = u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]);
        if magic != SIG_LOCAL {
            return Err(Error::InvalidMagic {
                what: "zip local file header",
            });
        }

        // The local header has to agree with the directory on the fields
        // it cannot legitimately differ on; APPNOTE §4.3.7 makes method
        // and flags the same record's echo.
        let local_flags = u16::from_le_bytes([hdr[6], hdr[7]]);
        let local_method = u16::from_le_bytes([hdr[8], hdr[9]]);
        if local_flags != entry.flags {
            return Err(Error::BadValue("zip flags differ local vs central"));
        }
        if local_method != entry.method {
            return Err(Error::BadValue("zip method differs local vs central"));
        }

        let name_len = usize::from(u16::from_le_bytes([hdr[26], hdr[27]]));
        let extra_len = usize::from(u16::from_le_bytes([hdr[28], hdr[29]]));
        let data_start = base
            .checked_add(LOCAL_LEN)
            .and_then(|n| n.checked_add(name_len))
            .and_then(|n| n.checked_add(extra_len))
            .ok_or(Error::BadValue("zip local header offset"))?;

        if entry.flags & (FLAG_ENCRYPTED | FLAG_STRONG_ENCRYPTED | FLAG_MASKED_LOCAL) != 0 {
            return Err(Error::Unsupported("encrypted zip entry"));
        }

        let cs = usize::try_from(entry.compressed_size).unwrap_or(usize::MAX);
        let payload = take(self.data, data_start, cs, "zip entry payload")?;

        if entry.flags & FLAG_DESCRIPTOR != 0 {
            let dd_at = data_start
                .checked_add(cs)
                .ok_or(Error::BadValue("zip data descriptor offset"))?;
            self.verify_descriptor(entry, dd_at)?;
        }

        let out = match entry.method {
            METHOD_STORED => {
                if entry.uncompressed_size != entry.compressed_size {
                    return Err(Error::BadValue("zip stored entry size"));
                }
                if cs > limits.max_output {
                    return Err(Error::too_large("zip entry", limits.max_output));
                }
                payload.to_vec()
            }
            METHOD_DEFLATE => inflate_raw(payload, limits)?,
            _ => return Err(Error::Unsupported("zip compression method")),
        };

        if out.len() as u64 != u64::from(entry.uncompressed_size) {
            return Err(Error::BadValue("zip entry size mismatch"));
        }
        if crc32(&out) != entry.crc32 {
            return Err(Error::BadValue("zip entry crc32 mismatch"));
        }
        Ok(out)
    }

    /// Verifies the data descriptor trailing a bit-3 entry's payload.
    /// APPNOTE §4.3.9.3 makes the 0x08074b50 signature optional, so both
    /// 12- and 16-byte forms are accepted; the numbers must still equal
    /// the directory's.
    fn verify_descriptor(&self, entry: &ZipEntry<'a>, at: usize) -> Result<()> {
        let base = if le32(self.data, at) == Some(SIG_DESCRIPTOR) {
            at + 4
        } else {
            at
        };
        let dd = take(self.data, base, 12, "zip data descriptor")?;
        let dd_crc = u32::from_le_bytes([dd[0], dd[1], dd[2], dd[3]]);
        let dd_cs = u32::from_le_bytes([dd[4], dd[5], dd[6], dd[7]]);
        let dd_us = u32::from_le_bytes([dd[8], dd[9], dd[10], dd[11]]);
        if dd_crc == entry.crc32
            && dd_cs == entry.compressed_size
            && dd_us == entry.uncompressed_size
        {
            Ok(())
        } else {
            Err(Error::BadValue("zip data descriptor mismatch"))
        }
    }
}
