//! Tests for the ZIP reader: archives built byte-exactly in this file,
//! corrupt variants for every refusal path, and a prefix-fuzz sweep that
//! requires every truncation of a valid archive to fail without a panic.
//!
//! Deflated payloads are hand-built stored-block DEFLATE streams (BTYPE
//! 00): legal DEFLATE that needs no compressor dependency, and the same
//! format the archive spec names for `method 8`.

use modhash_inflate::{Limits, inflate_raw};
use modhash_primitives::Error;
use modhash_primitives::crc32;
use modhash_zip::ZipArchive;

// ------------------------------------------------------------ builders

/// Writes `v` little-endian into `out`.
fn le16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// Writes `v` little-endian into `out`.
fn le32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// A raw RFC 1951 DEFLATE stream made of stored (uncompressed) blocks:
/// one `0x01` block for inputs up to 65 535 bytes, a leading non-final
/// `0x00` block plus a final `0x01` block for the two-block case. This is
/// legal DEFLATE, which is all `method 8` requires.
fn stored_deflate(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() + 10);
    if data.len() <= 65_535 {
        out.push(0x01);
        le16(&mut out, data.len() as u16);
        le16(&mut out, !(data.len() as u16));
        out.extend_from_slice(data);
    } else {
        // Two blocks: first `split`, then the rest.
        let split = 65_535usize;
        out.push(0x00);
        le16(&mut out, split as u16);
        le16(&mut out, !(split as u16));
        out.extend_from_slice(&data[..split]);
        out.push(0x01);
        let rest = (data.len() - split) as u16;
        le16(&mut out, rest);
        le16(&mut out, !rest);
        out.extend_from_slice(&data[split..]);
    }
    out
}

/// One member of an archive under construction.
struct Spec<'a> {
    name: &'a str,
    /// Uncompressed payload.
    body: &'a [u8],
    /// `0` stored, `8` deflate, anything else written verbatim.
    method: u16,
    /// Write a data descriptor and zero the local header's CRC/sizes.
    descriptor: bool,
    /// Whether the descriptor record carries its optional signature.
    descriptor_sig: bool,
    /// Flags OR-ed into both headers on top of the descriptor bit.
    extra_flags: u16,
    /// When `Some`, this CRC is written to the directory instead of the
    /// real one — the crc-mismatch fixture.
    crc_override: Option<u32>,
}

/// A stored entry.
fn stored<'a>(name: &'a str, body: &'a [u8]) -> Spec<'a> {
    Spec {
        name,
        body,
        method: 0,
        descriptor: false,
        descriptor_sig: false,
        extra_flags: 0,
        crc_override: None,
    }
}

/// A deflate entry (stored-block DEFLATE payload).
fn deflated<'a>(name: &'a str, body: &'a [u8]) -> Spec<'a> {
    Spec {
        name,
        body,
        method: 8,
        descriptor: false,
        descriptor_sig: false,
        extra_flags: 0,
        crc_override: None,
    }
}

/// A deflate entry whose local header is followed by a data descriptor.
fn descriptor<'a>(name: &'a str, body: &'a [u8], sig: bool) -> Spec<'a> {
    Spec {
        name,
        body,
        method: 8,
        descriptor: true,
        descriptor_sig: sig,
        extra_flags: 0,
        crc_override: None,
    }
}

/// Builds a ZIP archive byte by byte from `specs`, with `comment` bytes
/// appended after the EOCD's declared comment.
fn build_zip(specs: &[Spec], comment: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    // (name, flags, method, crc, compressed, uncompressed, local offset)
    let mut directory = Vec::new();

    for spec in specs {
        let payload = if spec.method == 8 {
            stored_deflate(spec.body)
        } else {
            spec.body.to_vec()
        };
        let crc = spec.crc_override.unwrap_or_else(|| crc32(spec.body));
        let flags = spec.extra_flags | if spec.descriptor { 1 << 3 } else { 0 };
        let cs = payload.len() as u32;
        let us = spec.body.len() as u32;
        let name = spec.name.as_bytes();

        let local_offset = out.len() as u32;
        le32(&mut out, 0x0403_4b50);
        le16(&mut out, 20); // version needed to extract
        le16(&mut out, flags);
        le16(&mut out, spec.method);
        le16(&mut out, 0x7e21); // a legal MS-DOS time
        le16(&mut out, 0x0121); // a legal MS-DOS date
        if spec.descriptor {
            le32(&mut out, 0);
            le32(&mut out, 0);
            le32(&mut out, 0);
        } else {
            le32(&mut out, crc);
            le32(&mut out, cs);
            le32(&mut out, us);
        }
        le16(&mut out, name.len() as u16);
        le16(&mut out, 0);
        out.extend_from_slice(name);
        out.extend_from_slice(&payload);
        if spec.descriptor {
            if spec.descriptor_sig {
                le32(&mut out, 0x0807_4b50);
            }
            le32(&mut out, crc);
            le32(&mut out, cs);
            le32(&mut out, us);
        }
        directory.push((flags, spec.method, crc, cs, us, name, local_offset));
    }

    let cd_offset = out.len() as u32;
    for (flags, method, crc, cs, us, name, local_offset) in &directory {
        le32(&mut out, 0x0201_4b50);
        le16(&mut out, 20); // version made by
        le16(&mut out, 20); // version needed
        le16(&mut out, *flags);
        le16(&mut out, *method);
        le16(&mut out, 0x7e21);
        le16(&mut out, 0x0121);
        le32(&mut out, *crc);
        le32(&mut out, *cs);
        le32(&mut out, *us);
        le16(&mut out, name.len() as u16);
        le16(&mut out, 0); // extra
        le16(&mut out, 0); // comment
        le16(&mut out, 0); // disk start
        le16(&mut out, 0); // internal attrs
        le32(&mut out, 0); // external attrs
        le32(&mut out, *local_offset);
        out.extend_from_slice(name);
    }
    let cd_size = out.len() as u32 - cd_offset;

    le32(&mut out, 0x0605_4b50);
    le16(&mut out, 0);
    le16(&mut out, 0);
    le16(&mut out, specs.len() as u16);
    le16(&mut out, specs.len() as u16);
    le32(&mut out, cd_size);
    le32(&mut out, cd_offset);
    le16(&mut out, comment.len() as u16);
    out.extend_from_slice(comment);
    out
}

/// The offset of a field inside the first central directory entry of a
/// freshly built archive: `cd` is where the directory starts, so
/// `cd + 16` is the entry's CRC-32 field, `cd + 20` its compressed size.
fn first_entry_field(bytes: &[u8], field: usize) -> usize {
    // EOCD is the last 22 + comment bytes; cd offset is at eocd + 16.
    let eocd = bytes
        .windows(4)
        .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
        .expect("test archive has an eocd");
    let cd =
        u32::from_le_bytes(bytes[eocd + 16..eocd + 20].try_into().expect("cd offset")) as usize;
    cd + field
}

// --------------------------------------------------------------- tests

/// Stored and deflated entries round-trip with intact bytes.
#[test]
fn stored_and_deflated_entries_round_trip() {
    let a = b"stored payload, no compression at all";
    let b: Vec<u8> = (0..3000).map(|i| (i % 251) as u8).collect();
    let zip = build_zip(&[stored("a.txt", a), deflated("b.bin", &b)], b"");

    let zip = ZipArchive::new(&zip).expect("parse");
    assert_eq!(zip.len(), 2);
    assert_eq!(zip.entries()[0].name(), "a.txt");
    assert_eq!(zip.entries()[0].method(), 0);
    assert_eq!(zip.entries()[1].name(), "b.bin");
    assert_eq!(zip.entries()[1].method(), 8);
    assert_eq!(zip.extract_by_name("a.txt").unwrap(), a);
    assert_eq!(zip.extract_by_name("b.bin").unwrap(), b);
}

/// The stored-block stream the tests emit really is DEFLATE: inflate_raw
/// must reproduce the input, which is what makes the method-8 entries
/// honest fixtures rather than circular ones.
#[test]
fn stored_block_deflate_is_legal_deflate() {
    let data: Vec<u8> = (0..70_000).map(|i| (i % 253) as u8).collect();
    assert_eq!(
        inflate_raw(&stored_deflate(&data), &Limits::default()).unwrap(),
        data
    );
}

/// A deflated entry larger than one stored block still extracts.
#[test]
fn multi_block_deflated_entry_extracts() {
    let data: Vec<u8> = (0..70_000).map(|i| (i % 253) as u8).collect();
    let zip = build_zip(&[deflated("big.bin", &data)], b"");
    let zip = ZipArchive::new(&zip).unwrap();
    assert_eq!(zip.extract_by_name("big.bin").unwrap(), data);
}

/// Flags bit 3 archives: the local header's CRC/sizes are zero and the
/// real record trails the payload — with and without its signature.
#[test]
fn data_descriptor_entries_extract() {
    let body = b"descriptor-backed deflate entry";
    for sig in [true, false] {
        let zip = build_zip(&[descriptor("d.bin", body, sig)], b"");
        let zip = ZipArchive::new(&zip).unwrap();
        let entry = zip.by_name("d.bin").unwrap();
        assert!(entry.uses_data_descriptor());
        assert_eq!(zip.extract(entry).unwrap(), body);
    }
}

/// A data descriptor whose numbers disagree with the directory is a
/// named error, not silently trusted.
#[test]
fn data_descriptor_mismatch_is_bad_value() {
    let body = b"descriptor-backed deflate entry";
    let mut zip = build_zip(&[descriptor("d.bin", body, true)], b"");
    // The descriptor's uncompressed size is the last u32 of the record;
    // the directory follows it. Corrupt that field.
    let eocd = zip
        .windows(4)
        .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
        .unwrap();
    let cd = u32::from_le_bytes(zip[eocd + 16..eocd + 20].try_into().unwrap()) as usize;
    let dd_us = cd - 4;
    zip[dd_us] ^= 0xFF;
    let zip = ZipArchive::new(&zip).unwrap();
    let entry = zip.by_name("d.bin").unwrap();
    assert_eq!(
        zip.extract(entry),
        Err(Error::BadValue("zip data descriptor mismatch"))
    );
}

/// The EOCD is found past a comment — 100 bytes, well inside the 64 KiB
/// window — and the archive still parses end to end.
#[test]
fn eocd_is_found_past_a_comment() {
    let comment: Vec<u8> = (0..100).map(|i| b' ' + (i % 90) as u8).collect();
    let zip = build_zip(&[stored("a.txt", b"with comment")], &comment);
    let zip = ZipArchive::new(&zip).unwrap();
    assert_eq!(zip.extract_by_name("a.txt").unwrap(), b"with comment");
}

/// A `PK\x05\x06` lookalike inside the comment must not win over the real
/// EOCD: the comment-length cross-check keeps scanning.
#[test]
fn eocd_signature_inside_comment_is_ignored() {
    let mut comment = vec![0u8; 64];
    comment[10..14].copy_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
    let zip = build_zip(&[stored("a.txt", b"pk in comment")], &comment);
    let zip = ZipArchive::new(&zip).unwrap();
    assert_eq!(zip.extract_by_name("a.txt").unwrap(), b"pk in comment");
}

/// An empty archive (no entries) parses and reports empty.
#[test]
fn empty_archive_parses() {
    let zip = build_zip(&[], b"");
    let zip = ZipArchive::new(&zip).unwrap();
    assert!(zip.is_empty());
    assert!(zip.by_name("anything").is_none());
    assert_eq!(
        zip.extract_by_name("anything"),
        Err(Error::BadValue("no such zip entry"))
    );
}

/// A corrupt CRC in the directory is a named BadValue at extraction.
#[test]
fn crc_mismatch_is_a_named_error() {
    let mut zip = build_zip(&[stored("a.txt", b"check me")], b"");
    let at = first_entry_field(&zip, 16);
    zip[at] ^= 0xFF;
    let zip = ZipArchive::new(&zip).unwrap();
    assert_eq!(
        zip.extract_by_name("a.txt"),
        Err(Error::BadValue("zip entry crc32 mismatch"))
    );
}

/// A directory size that does not match the real payloads lands as a
/// named size error before the CRC is even reached.
#[test]
fn size_mismatch_is_a_named_error() {
    let mut zip = build_zip(&[stored("a.txt", b"check me")], b"");
    let at = first_entry_field(&zip, 24); // uncompressed size
    zip[at] ^= 0xFF;
    let zip = ZipArchive::new(&zip).unwrap();
    assert_eq!(
        zip.extract_by_name("a.txt"),
        Err(Error::BadValue("zip stored entry size"))
    );
}

/// On a *deflated* entry a lied uncompressed-size field is still caught
/// by the shared size check even though the bytes and CRC are intact —
/// this is the seam a stored entry cannot reach.
#[test]
fn deflated_size_mismatch_is_a_named_error() {
    let mut zip = build_zip(&[deflated("b.bin", b"check me")], b"");
    let at = first_entry_field(&zip, 24); // uncompressed size
    zip[at] ^= 0xFF;
    let zip = ZipArchive::new(&zip).unwrap();
    assert_eq!(
        zip.extract_by_name("b.bin"),
        Err(Error::BadValue("zip entry size mismatch"))
    );
}
/// ZIP64 EOCD sentinels are refused by name.
#[test]
fn zip64_eocd_is_refused() {
    let mut zip = build_zip(&[stored("a.txt", b"x")], b"");
    let eocd = zip
        .windows(4)
        .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
        .unwrap();
    // entries-on-disk AND total entries both at the 0xFFFF sentinel.
    for i in 0..4 {
        zip[eocd + 8 + i] = 0xFF;
    }
    assert_eq!(
        ZipArchive::new(&zip).unwrap_err(),
        Error::Unsupported("zip64 archive")
    );
}

/// A local header whose flags disagree with the directory is a named
/// error — the two halves of the record must echo each other.
#[test]
fn local_flags_mismatch_is_a_named_error() {
    let mut zip = build_zip(&[stored("a.txt", b"flags")], b"");
    zip[6] = 0xFF; // flags field of the first local header
    let zip = ZipArchive::new(&zip).unwrap();
    assert_eq!(
        zip.extract_by_name("a.txt"),
        Err(Error::BadValue("zip flags differ local vs central"))
    );
}

/// A local header whose method disagrees with the directory is a named
/// error.
#[test]
fn local_method_mismatch_is_a_named_error() {
    let mut zip = build_zip(&[stored("a.txt", b"method")], b"");
    zip[8] = 0x08; // method field of the first local header: 0 -> 8
    let zip = ZipArchive::new(&zip).unwrap();
    assert_eq!(
        zip.extract_by_name("a.txt"),
        Err(Error::BadValue("zip method differs local vs central"))
    );
}

/// A multi-disk archive is refused by name: `entries on this disk` not
/// equal to `total entries` is the tell.
#[test]
fn multi_disk_is_refused() {
    let mut zip = build_zip(&[stored("a.txt", b"x")], b"");
    let eocd = zip
        .windows(4)
        .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
        .unwrap();
    zip[eocd + 8] = 0; // entries on this disk: 0 of 1 total
    zip[eocd + 9] = 0;
    assert_eq!(
        ZipArchive::new(&zip).unwrap_err(),
        Error::Unsupported("multi-disk zip archive")
    );
}

/// An encrypted entry (flags bit 0) parses but refuses at extraction.
#[test]
fn encrypted_entry_is_refused() {
    let mut spec = stored("a.txt", b"secret");
    spec.extra_flags = 1;
    let zip = build_zip(&[spec], b"");
    let zip = ZipArchive::new(&zip).unwrap();
    assert_eq!(
        zip.extract_by_name("a.txt"),
        Err(Error::Unsupported("encrypted zip entry"))
    );
}

/// A compression method the crate does not implement refuses by name.
#[test]
fn unsupported_method_is_refused() {
    let mut spec = stored("a.txt", b"shrunk");
    spec.method = 9; // DCL implode-era leftover: parse, refuse on read.
    let zip = build_zip(&[spec], b"");
    let zip = ZipArchive::new(&zip).unwrap();
    assert_eq!(
        zip.extract_by_name("a.txt"),
        Err(Error::Unsupported("zip compression method"))
    );
}

/// A local header that does not carry the local signature is a magic
/// error, not a silent fallback.
#[test]
fn bad_local_signature_is_a_magic_error() {
    let mut zip = build_zip(&[stored("a.txt", b"payload")], b"");
    zip[0] = 0x00; // first byte of the first local header
    let zip = ZipArchive::new(&zip).unwrap();
    assert_eq!(
        zip.extract_by_name("a.txt"),
        Err(Error::InvalidMagic {
            what: "zip local file header"
        })
    );
}

/// Every prefix of a valid archive — including the empty prefix — must
/// return `Err`, and none may panic. The builder emits no `PK\x05\x06`
/// sequence inside any member's content, so no prefix can spoof a tail
/// EOCD.
#[test]
fn every_prefix_errors_and_never_panics() {
    let content = b"prefix fuzz payload without PK markers";
    let zip = build_zip(
        &[
            stored("a.txt", content),
            deflated("b.bin", b"deflated prefix payload"),
            descriptor("d.bin", b"descriptor payload", true),
        ],
        b"trailing comment",
    );
    assert!(!content.windows(4).any(|w| w == [0x50, 0x4b, 0x05, 0x06]));
    for i in 0..zip.len() {
        let result = std::panic::catch_unwind(|| ZipArchive::new(&zip[..i]).map(|_| ()));
        match result {
            Ok(Ok(())) => panic!("prefix of {i} bytes parsed as a zip"),
            Ok(Err(_)) => {}
            Err(_) => panic!("prefix of {i} bytes panicked"),
        }
    }
}

/// Bit corruption past the parse step must also fail closed: flip one
/// byte in each interesting position of a small archive and require the
/// reader to either reject the archive or reject extraction — never to
/// return wrong bytes.
#[test]
fn corrupted_bytes_never_yield_wrong_data() {
    let zip = build_zip(
        &[stored("a.txt", b"corrupt me"), deflated("b.bin", b"or me")],
        b"",
    );
    let want = b"corrupt me".to_vec();
    for i in 0..zip.len() {
        let mut bad = zip.clone();
        bad[i] ^= 0xA5;
        match ZipArchive::new(&bad) {
            Err(_) => {}
            Ok(arc) => {
                for entry in arc.entries() {
                    if let Ok(bytes) = arc.extract(entry) {
                        // Surviving mutation may only alter names or
                        // entry set, never return corrupted content.
                        if entry.name() == "a.txt" {
                            assert_eq!(bytes, want, "mutation at byte {i} corrupted output");
                        }
                    }
                }
            }
        }
    }
}
