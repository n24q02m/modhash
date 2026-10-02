//! Top-level demuxing: `ftyp` validation, `moov`/`mvhd`/`trak` assembly,
//! and the [`Mp4`]/[`Track`] facade a consumer iterates (ISO/IEC 14496-12
//! §6, §8.3, §8.4, §8.6).
//!
//! The walk is deliberately shallow: boxes we do not read (`udta`, `meta`,
//! `free`, `skip`, `dinf`, `vmhd`, `edts`, ...) are never opened — the
//! demuxer only descends into the container chain `moov → trak → mdia →
//! minf → stbl` that owns the sample tables. Fragmented files (`moof`/`mvex`)
//! are refused with [`Error::Unsupported`], which the spec reserves for
//! format variants deliberately out of scope.

use alloc::vec::Vec;
use modhash_primitives::{BitReader, Error, Result};

use crate::boxes::{Boxes, Four, Reader, full_box};
use crate::sample_table::{
    EntryKind, SampleIter, SampleTable, parse_ctts, parse_stco, parse_stsc, parse_stsd, parse_stss,
    parse_stsz, parse_stts, parse_stz2,
};

/// File brands we accept in `ftyp` (major or compatible). Anything outside
/// this list is not necessarily invalid, but it is not the ISO-BMFF family
/// this crate claims to demux, so it fails loudly instead of guessing.
const KNOWN_BRANDS: &[&[u8; 4]] = &[
    b"isom", b"iso2", b"iso3", b"iso4", b"iso5", b"iso6", b"iso7", b"iso8", b"iso9", b"mp41",
    b"mp42", b"mp71", b"avc1", b"M4V ", b"M4A ", b"M4P ", b"M4B ", b"qt  ", b"dash", b"cmfc",
    b"cmff", b"MSNV", b"mmp4",
];

/// Brands accepted by prefix: `3gp*`, `3g2*`, `3ge*`/`3gg*` 3GPP variants.
fn known_brand(brand: Four) -> bool {
    if KNOWN_BRANDS.contains(&&brand) {
        return true;
    }
    brand[..3] == *b"3gp" || brand[..3] == *b"3g2" || brand[..3] == *b"3ge" || brand[..3] == *b"3gg"
}

/// A demuxed ISO-BMFF file: validated brands plus the tracks under `moov`.
#[derive(Debug)]
pub struct Mp4 {
    /// `ftyp` major brand.
    pub major_brand: Four,
    /// `ftyp` compatible brands (major excluded), in file order.
    pub compatible_brands: Vec<Four>,
    /// Movie timescale from `mvhd` (units per second).
    pub timescale: u32,
    /// Movie duration in `timescale` units from `mvhd`.
    pub duration: u64,
    /// One [`Track`] per `trak` box, in file order.
    pub tracks: Vec<Track>,
}

impl Mp4 {
    /// The first track whose handler is `vide`, if any. Convenience for the
    /// dominant "give me the video" consumer; use [`Mp4::tracks`] when the
    /// caller needs to choose itself.
    pub fn video_track(&self) -> Option<&Track> {
        self.tracks.iter().find(|t| t.handler == *b"vide")
    }
}

/// One `trak`: its headers plus the parsed sample table.
#[derive(Debug)]
pub struct Track {
    /// `tkhd` track id.
    pub id: u32,
    /// `mdhd` media timescale (units per second); sample times are in this.
    pub timescale: u32,
    /// `mdhd` media duration in `timescale` units.
    pub duration: u64,
    /// ISO-639-2/T three-letter code from `mdhd`, e.g. `"eng"`. `None` when
    /// the packed field does not decode to letters.
    pub language: Option<[u8; 3]>,
    /// `hdlr` handler type: `vide`, `soun`, `text`, `hint`, ...
    pub handler: Four,
    /// `tkhd` display width in pixels (16.16 fixed point, integral part).
    pub width: u32,
    /// `tkhd` display height in pixels.
    pub height: u32,
    /// The parsed `stbl` sample table.
    pub table: SampleTable,
}

impl Track {
    /// Iterates this track's samples in decoding order.
    pub fn samples(&self) -> SampleIter<'_> {
        self.table.samples()
    }

    /// The `avcC` record of the first `stsd` entry carrying one, if this is
    /// an AVC track. Returned verbatim — interpreting it is the h264
    /// crate's job (ISO/IEC 14496-15 §5.3.3).
    pub fn avcc(&self) -> Option<&[u8]> {
        self.table.description.entries.iter().find_map(|e| match e {
            EntryKind::Visual { avcc: Some(b), .. } => Some(b.as_slice()),
            _ => None,
        })
    }

    /// The byte range `[offset, offset+size)` of sample `index` in the
    /// original file, if the index is in range.
    pub fn sample_range(&self, index: usize) -> Option<core::ops::Range<u64>> {
        let s = self.table.samples().nth(index)?.ok()?;
        Some(s.offset..s.offset + u64::from(s.size))
    }

    /// The payload bytes of sample `index`, sliced from `file` (the same
    /// buffer `demux` was given). Errors when the table ran out early or the
    /// recorded range overruns `file` — which parse-time validation already
    /// rejects, so this is only reachable for files fed through unchecked.
    pub fn sample_bytes<'a>(&self, file: &'a [u8], index: usize) -> Result<&'a [u8]> {
        let range = self
            .sample_range(index)
            .ok_or(Error::BadValue("sample index out of range"))?;
        let start = usize::try_from(range.start)
            .map_err(|_| Error::BadValue("sample offset exceeds file"))?;
        let end =
            usize::try_from(range.end).map_err(|_| Error::BadValue("sample end exceeds file"))?;
        file.get(start..end)
            .ok_or(Error::truncated("sample payload", end, file.len()))
    }
}

/// Demuxes a whole in-memory ISO-BMFF file.
///
/// Returns the parsed [`Mp4`] with every `trak`'s sample table resolved and
/// each sample's byte range checked against the file length, so all
/// corruption surfaces here rather than mid-iteration. Errors:
///
/// * [`Error::InvalidMagic`] — no `ftyp`, or an `ftyp` naming no brand this
///   family recognises (e.g. a JPEG file handed over by mistake).
/// * [`Error::BadValue`] — no `moov`, or a track missing a mandatory box.
/// * [`Error::Unsupported`] — fragmented MP4 (`moof`/`mvex`), which a
///   sample-table demuxer cannot answer honestly.
/// * [`Error::Truncated`] — any box, field or sample range overrun.
pub fn demux(data: &[u8]) -> Result<Mp4> {
    let mut top = Boxes::new(data, 0..data.len());
    let first = top
        .next()
        .ok_or(Error::InvalidMagic { what: "empty file" })??;
    if first.four != *b"ftyp" {
        return Err(Error::InvalidMagic {
            what: "first box is not ftyp",
        });
    }
    let (major_brand, compatible_brands) = parse_ftyp(&data[first.payload.clone()])?;
    let mut moov = None;
    let mut fragmented = false;
    for head in top {
        let head = head?;
        match &head.four {
            b"moov" => {
                if moov.is_some() {
                    return Err(Error::BadValue("more than one moov box"));
                }
                moov = Some(head.payload);
            }
            b"moof" => fragmented = true,
            _ => {}
        }
    }
    let moov = moov.ok_or(Error::BadValue("no moov box"))?;
    if fragmented {
        return Err(Error::Unsupported("fragmented mp4 (moof)"));
    }
    let mut timescale = 0u32;
    let mut duration = 0u64;
    let mut tracks = Vec::new();
    for head in Boxes::new(data, moov) {
        let head = head?;
        match &head.four {
            b"mvhd" => {
                let (ts, dur) = parse_mvhd(&data[head.payload.clone()])?;
                timescale = ts;
                duration = dur;
            }
            b"trak" => tracks.push(parse_trak(data, head.payload)?),
            b"mvex" => return Err(Error::Unsupported("fragmented mp4 (mvex)")),
            _ => {}
        }
    }
    let mp4 = Mp4 {
        major_brand,
        compatible_brands,
        timescale,
        duration,
        tracks,
    };
    // Validate every declared sample range once, up front: a corrupt stco
    // entry must not surface ten thousand samples into iteration.
    let len = data.len() as u64;
    for track in &mp4.tracks {
        for sample in track.table.samples() {
            let s = sample?;
            if s.offset
                .checked_add(u64::from(s.size))
                .is_none_or(|end| end > len)
            {
                return Err(Error::truncated("sample payload", usize::MAX, data.len()));
            }
        }
    }
    Ok(mp4)
}

/// Reads `ftyp`: major brand, minor version, compatible brands (§4.3).
/// Every listed brand must be one this demuxer recognises, because a brand
/// is a claim about which spec the file follows.
fn parse_ftyp(data: &[u8]) -> Result<(Four, Vec<Four>)> {
    let mut r = Reader::new(data);
    let major = r.four("ftyp major brand")?;
    r.u32("ftyp minor version")?;
    let mut compat = Vec::new();
    while r.remaining() >= 4 {
        compat.push(r.four("ftyp compatible brand")?);
    }
    if !known_brand(major) && !compat.iter().any(|b| known_brand(*b)) {
        return Err(Error::InvalidMagic {
            what: "ftyp lists no recognised brand",
        });
    }
    Ok((major, compat))
}

/// Reads `mvhd` timescale and duration (§8.2.2); both v0 (32-bit) and v1
/// (64-bit) headers.
fn parse_mvhd(data: &[u8]) -> Result<(u32, u64)> {
    let mut r = Reader::new(data);
    let (version, _) = full_box(&mut r, "mvhd")?;
    match version {
        0 => {
            r.u32("mvhd creation time")?;
            r.u32("mvhd modification time")?;
            let ts = r.u32("mvhd timescale")?;
            let dur = u64::from(r.u32("mvhd duration")?);
            Ok((ts, dur))
        }
        1 => {
            r.u64("mvhd creation time")?;
            r.u64("mvhd modification time")?;
            let ts = r.u32("mvhd timescale")?;
            let dur = r.u64("mvhd duration")?;
            Ok((ts, dur))
        }
        _ => Err(Error::Unsupported("mvhd version > 1")),
    }
}

/// Reads `tkhd` track id, duration and display size (§8.3.2).
fn parse_tkhd(data: &[u8]) -> Result<(u32, u64, u32, u32)> {
    let mut r = Reader::new(data);
    let (version, _) = full_box(&mut r, "tkhd")?;
    let (id, duration) = match version {
        0 => {
            r.u32("tkhd creation time")?;
            r.u32("tkhd modification time")?;
            let id = r.u32("tkhd track id")?;
            r.u32("tkhd reserved")?;
            (id, u64::from(r.u32("tkhd duration")?))
        }
        1 => {
            r.u64("tkhd creation time")?;
            r.u64("tkhd modification time")?;
            let id = r.u32("tkhd track id")?;
            r.u32("tkhd reserved")?;
            (id, r.u64("tkhd duration")?)
        }
        _ => return Err(Error::Unsupported("tkhd version > 1")),
    };
    // reserved(8) + layer(2) + alt-group(2) + volume(2) + reserved(2) +
    // matrix(36) + width(4) + height(4), all 16.16 fixed for the last two.
    r.skip(8 + 2 + 2 + 2 + 2 + 36, "tkhd geometry")?;
    let width = r.u32("tkhd width")? >> 16;
    let height = r.u32("tkhd height")? >> 16;
    Ok((id, duration, width, height))
}

/// Reads `mdhd` timescale, duration and packed ISO-639-2 language
/// (§8.4.2). The 15-bit language field is the other bit-level structure
/// the shared [`BitReader`] exists for: three 5-bit letters biased by
/// `0x60` (`'a' - 1`).
fn parse_mdhd(data: &[u8]) -> Result<(u32, u64, Option<[u8; 3]>)> {
    let mut r = Reader::new(data);
    let (version, _) = full_box(&mut r, "mdhd")?;
    let (timescale, duration) = match version {
        0 => {
            r.u32("mdhd creation time")?;
            r.u32("mdhd modification time")?;
            let ts = r.u32("mdhd timescale")?;
            let dur = u64::from(r.u32("mdhd duration")?);
            (ts, dur)
        }
        1 => {
            r.u64("mdhd creation time")?;
            r.u64("mdhd modification time")?;
            let ts = r.u32("mdhd timescale")?;
            (ts, r.u64("mdhd duration")?)
        }
        _ => return Err(Error::Unsupported("mdhd version > 1")),
    };
    let packed = r.u16("mdhd language")?;
    let lang_bytes = packed.to_be_bytes();
    let mut bits = BitReader::new(&lang_bytes);
    // The field is 15 bits: one leading pad bit, then three 5-bit letters.
    bits.bits(1)?;
    let mut lang = [0u8; 3];
    let mut ok = true;
    for letter in &mut lang {
        let v = bits.bits(5)? as u8;
        if !(1..=26).contains(&v) {
            ok = false;
            break;
        }
        *letter = v + 0x60;
    }
    Ok((timescale, duration, ok.then_some(lang)))
}

/// Reads `hdlr` and returns the handler type (§8.4.3).
fn parse_hdlr(data: &[u8]) -> Result<Four> {
    let mut r = Reader::new(data);
    full_box(&mut r, "hdlr")?;
    r.u32("hdlr pre_defined")?;
    r.four("hdlr handler type")
}

/// Assembles one `trak` subtree: headers from `tkhd`/`mdhd`/`hdlr`, sample
/// geometry from `mdia → minf → stbl`.
fn parse_trak(data: &[u8], range: core::ops::Range<usize>) -> Result<Track> {
    let mut id = 0u32;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut trak_duration = 0u64;
    let mut mdia = None;
    for head in Boxes::new(data, range) {
        let head = head?;
        match &head.four {
            b"tkhd" => {
                let (i, d, w, h) = parse_tkhd(&data[head.payload.clone()])?;
                id = i;
                trak_duration = d;
                width = w;
                height = h;
            }
            b"mdia" => mdia = Some(head.payload),
            _ => {}
        }
    }
    let mdia = mdia.ok_or(Error::BadValue("trak with no mdia"))?;
    let mut timescale = 0u32;
    let mut duration = trak_duration;
    let mut language = None;
    let mut handler = *b"    ";
    let mut minf = None;
    for head in Boxes::new(data, mdia) {
        let head = head?;
        match &head.four {
            b"mdhd" => {
                let (ts, d, lang) = parse_mdhd(&data[head.payload.clone()])?;
                timescale = ts;
                duration = d;
                language = lang;
            }
            b"hdlr" => handler = parse_hdlr(&data[head.payload.clone()])?,
            b"minf" => minf = Some(head.payload),
            _ => {}
        }
    }
    let minf = minf.ok_or(Error::BadValue("mdia with no minf"))?;
    let mut stbl = None;
    for head in Boxes::new(data, minf) {
        let head = head?;
        if head.four == *b"stbl" {
            stbl = Some(head.payload);
        }
    }
    let stbl = stbl.ok_or(Error::BadValue("minf with no stbl"))?;
    let table = parse_stbl(data, stbl)?;
    if timescale == 0 {
        return Err(Error::BadValue("mdhd timescale is zero"));
    }
    Ok(Track {
        id,
        timescale,
        duration,
        language,
        handler,
        width,
        height,
        table,
    })
}

/// Parses the `stbl` children into a [`SampleTable`], then checks the
/// tables agree with each other (§8.6): `stss` indices inside the sample
/// count, `stsc` description indices inside `stsd`, and `stts` covering at
/// least `stsz`'s declared sample count.
fn parse_stbl(data: &[u8], range: core::ops::Range<usize>) -> Result<SampleTable> {
    let mut description = None;
    let mut chunks = None;
    let mut sizes = None;
    let mut offsets = None;
    let mut times = None;
    let mut comp = Vec::new();
    let mut sync = None;
    for head in Boxes::new(data, range) {
        let head = head?;
        let body = &data[head.payload.clone()];
        match &head.four {
            b"stsd" => description = Some(parse_stsd(body)?),
            b"stsc" => chunks = Some(parse_stsc(body)?),
            b"stsz" => sizes = Some(parse_stsz(body)?),
            b"stz2" => sizes = Some(parse_stz2(body)?),
            b"stco" => offsets = Some(parse_stco(body, false)?),
            b"co64" => offsets = Some(parse_stco(body, true)?),
            b"stts" => times = Some(parse_stts(body)?),
            b"ctts" => comp = parse_ctts(body)?,
            b"stss" => sync = Some(parse_stss(body)?),
            // stsh, stdp, sdtp, padb, cslg and friends are legal but not
            // needed to enumerate samples; skipped without opening.
            _ => {}
        }
    }
    let description = description.ok_or(Error::BadValue("stbl with no stsd"))?;
    let chunks = chunks.ok_or(Error::BadValue("stbl with no stsc"))?;
    let (uniform_size, size_list, sample_count) =
        sizes.ok_or(Error::BadValue("stbl with no stsz"))?;
    let chunk_offsets = offsets.ok_or(Error::BadValue("stbl with no stco/co64"))?;
    let times = times.ok_or(Error::BadValue("stbl with no stts"))?;
    let table = SampleTable {
        description,
        chunks,
        sizes: size_list,
        sample_count,
        uniform_size,
        chunk_offsets,
        times,
        comp,
        sync,
    };
    // Cross-table agreement, checked once at parse so consumers never meet
    // them halfway through iteration.
    let entries = table.description.entries.len() as u64;
    for run in &table.chunks {
        if u64::from(run.description_index) > entries {
            return Err(Error::BadValue("stsc references missing stsd entry"));
        }
    }
    let declared: u64 = table.times.iter().map(|t| u64::from(t.count)).sum();
    if sample_count > 0 && declared < sample_count as u64 {
        return Err(Error::BadValue("stts covers fewer samples than stsz"));
    }
    if let Some(list) = &table.sync {
        if let Some(&last) = list.last() {
            if last as usize > sample_count {
                return Err(Error::BadValue("stss index past sample count"));
            }
        }
    }
    Ok(table)
}
