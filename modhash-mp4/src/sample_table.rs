//! Sample-table boxes and the chunk-to-sample mapping (ISO/IEC 14496-12
//! §8.6).
//!
//! ISO-BMFF stores per-sample metadata as run-length tables inside `stbl`:
//!
//! * `stts` / `ctts` — decoding-time deltas and composition offsets,
//! * `stsc` — samples per chunk, run-length coded by first chunk,
//! * `stsz` — per-sample sizes (or one shared size),
//! * `stco` / `co64` — 32/64-bit chunk offsets,
//! * `stss` — the sync-sample list; absent means every sample is sync.
//!
//! [`SampleTable`] keeps the tables in their compact on-disk form and
//! [`SampleIter`] expands them lazily into one [`Sample`] record per sample,
//! so demuxing a file costs O(table size), not O(samples) memory.

use alloc::vec::Vec;
use modhash_primitives::{BitReader, Error, Result};

use crate::boxes::{Reader, full_box};

/// One `stsc` run: starting at `first_chunk` (1-based), each chunk holds
/// `samples_per_chunk` samples described by `description_index`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct ChunkRun {
    /// First chunk (1-based) the run applies to.
    pub first_chunk: u32,
    /// Samples in each chunk of the run.
    pub samples_per_chunk: u32,
    /// `stsd` entry (1-based) describing the run's samples.
    pub description_index: u32,
}

/// One `stts` run: `sample_count` consecutive samples of `delta` ticks each.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct TimeRun {
    /// Samples this run covers.
    pub count: u32,
    /// Ticks each sample lasts, in media timescale units.
    pub delta: u32,
}

/// One `ctts` run: `count` samples each shifted by `offset` composition
/// ticks. Version-0 `ctts` is unsigned; version-1 is signed.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CompRun {
    /// Samples this run covers.
    pub count: u32,
    /// Signed composition offset in media timescale units.
    pub offset: i32,
}

/// One sample's address and timing, expanded from the sample tables.
///
/// All times are in the track's media timescale units (see
/// [`Track::timescale`](crate::Track::timescale)); `presentation` is
/// `decoding` plus the `ctts` offset.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Sample {
    /// Absolute byte offset of the sample payload in the file.
    pub offset: u64,
    /// Sample payload size in bytes.
    pub size: u32,
    /// Decoding timestamp (DTS), media timescale units.
    pub decoding: u64,
    /// Presentation timestamp (PTS), media timescale units.
    pub presentation: i64,
    /// Sample duration, media timescale units.
    pub duration: u32,
    /// True when the sample is a sync point. With no `stss` box every sample
    /// is sync, per ISO/IEC 14496-12 §8.6.2.
    pub keyframe: bool,
}

/// What an `stsd` entry declared about the track's samples.
///
/// Parsing stops at the codec fourcc plus the parts the demuxer needs:
/// dimensions for visual entries, `avcC` payload for AVC, `esds` payload for
/// MPEG-4 descriptors. Other sample-entry types are reported by fourcc only.
#[derive(Clone, Debug)]
pub enum EntryKind {
    /// A visual sample entry (`avc1`, `hev1`, `mp4v`, ...).
    Visual {
        /// The sample-entry type, e.g. `b"avc1"`.
        coding: crate::boxes::Four,
        /// `width` field of the entry.
        width: u16,
        /// `height` field of the entry.
        height: u16,
        /// Full payload of the `avcC` record inside this entry, if present.
        /// Carried verbatim: decoding it is the h264 crate's job.
        avcc: Option<Vec<u8>>,
        /// Full payload of `esds`, when the entry uses MPEG-4 descriptors.
        esds: Option<Vec<u8>>,
    },
    /// An audio sample entry (`mp4a`, `ac-3`, ...).
    Audio {
        /// The sample-entry type.
        coding: crate::boxes::Four,
        /// `channelcount` field of the entry.
        channels: u16,
        /// `samplerate` field, already converted from 16.16 fixed point.
        rate: u32,
        /// Full payload of `esds`, when present.
        esds: Option<Vec<u8>>,
    },
    /// A sample entry that is neither visual nor audio (`text`, `stpp`, ...).
    Other {
        /// The sample-entry type.
        coding: crate::boxes::Four,
    },
}

/// The parsed `stsd` box: one entry list, read front to back.
#[derive(Clone, Debug)]
pub struct SampleDescription {
    /// One kind per `stsd` entry; `stsc` indexes into this 1-based.
    pub entries: Vec<EntryKind>,
}

/// Everything under one `stbl` that the demuxer keeps: the sample
/// description plus the timing/geometry tables.
#[derive(Clone, Debug)]
pub struct SampleTable {
    /// The `stsd` entries.
    pub description: SampleDescription,
    /// `stsc` runs.
    pub chunks: Vec<ChunkRun>,
    /// `stsz` per-sample sizes; `uniform` holds `stsz`'s `sample_size`
    /// shortcut when nonzero.
    pub sizes: Vec<u32>,
    /// `stsz`/`stz2` sample count, kept so `sizes` may stay empty when every
    /// sample shares `uniform`.
    pub sample_count: usize,
    /// `stsz` uniform size; zero means per-sample `sizes` are authoritative.
    pub uniform_size: u32,
    /// `stco`/`co64` chunk offsets.
    pub chunk_offsets: Vec<u64>,
    /// `stts` decoding-time runs.
    pub times: Vec<TimeRun>,
    /// `ctts` composition-offset runs, empty when the box is absent.
    pub comp: Vec<CompRun>,
    /// `stss` sync-sample indices (1-based); `None` when the box is absent,
    /// which is how a file says "every sample is sync".
    pub sync: Option<Vec<u32>>,
}

impl SampleTable {
    /// True when `index` (0-based) is a sync sample. Without an `stss` box
    /// every sample is sync; with one, membership decides. `stss` contents
    /// are validated in `parse_stbl` to be sorted and in range, so a
    /// plain binary search is correct.
    pub fn is_keyframe(&self, index: usize) -> bool {
        match &self.sync {
            None => true,
            Some(list) => {
                let one_based = index as u64 + 1;
                list.binary_search(&(one_based as u32)).is_ok()
            }
        }
    }

    /// Size of sample `index` (0-based) in bytes.
    pub fn sample_size(&self, index: usize) -> u32 {
        if self.uniform_size != 0 {
            self.uniform_size
        } else {
            self.sizes[index]
        }
    }

    /// Iterates samples in decoding order, expanding `stsc` run by run.
    ///
    /// The returned iterator borrows this table and yields [`Sample`] records
    /// until `sample_count` is reached or the tables run out — a table that
    /// stops early yields [`Error::BadValue`] once, because it means the file
    /// declared more samples than its geometry can place.
    pub fn samples(&self) -> SampleIter<'_> {
        SampleIter {
            table: self,
            chunk: 0,
            sample: 0,
            run: 0,
            run_end_chunk: 0,
            active_per: 0,
            active_start: 0,
            chunk_cursor: 0,
            chunk_samples_left: 0,
            time_run: 0,
            time_run_left: 0,
            comp_run: 0,
            comp_run_left: 0,
            dts: 0,
        }
    }

    /// Number of samples the table declares.
    pub fn len(&self) -> usize {
        self.sample_count
    }

    /// True when the table declares zero samples.
    pub fn is_empty(&self) -> bool {
        self.sample_count == 0
    }
}

/// Lazy expansion of a [`SampleTable`] into [`Sample`] records.
///
/// Implements `Iterator<Item = Result<Sample>>`; after yielding one `Err`
/// the iterator is exhausted (fused). The expansion walks `stsc` runs left to
/// right exactly once — the run whose `first_chunk` exceeds the current chunk
/// ends the previous run — which is the piece of the demuxer a hand-rolled
/// reader most often gets wrong by one.
pub struct SampleIter<'a> {
    table: &'a SampleTable,
    /// Current chunk (0-based) into `chunk_offsets`.
    chunk: usize,
    /// Samples emitted so far (0-based sample index).
    sample: usize,
    /// Index of the next `stsc` run to consult. Runs are consumed lazily:
    /// `run_end_chunk` is the chunk index the following run starts at, so
    /// `chunk == run_end_chunk` means the cursor crossed a run boundary.
    run: usize,
    run_end_chunk: usize,
    /// `samples_per_chunk` and first covered chunk (0-based) of the run
    /// currently consulted, so a chunk in a gap before `active_start` gets
    /// zero samples while later chunks re-evaluate correctly.
    active_per: u32,
    active_start: usize,
    chunk_cursor: u64,
    /// Samples left to emit inside the current chunk.
    chunk_samples_left: u32,
    /// `stts` expansion state: current run, samples left in it, running DTS.
    time_run: usize,
    time_run_left: u32,
    /// `ctts` expansion state.
    comp_run: usize,
    comp_run_left: u32,
    dts: u64,
}

impl SampleIter<'_> {
    /// Emits `e` once and exhausts the iterator: callers see the table error
    /// exactly once instead of a tail of repeating failures.
    fn fail(&mut self, e: Error) -> Option<Result<Sample>> {
        self.sample = self.table.sample_count;
        Some(Err(e))
    }
}

impl Iterator for SampleIter<'_> {
    type Item = Result<Sample>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.sample >= self.table.sample_count {
            return None;
        }
        // Move to a chunk that still has samples to place.
        while self.chunk_samples_left == 0 {
            if self.chunk >= self.table.chunk_offsets.len() {
                return self.fail(Error::BadValue("stsc/stco ran out of chunks before stsz"));
            }
            // Advance `stsc` runs until one covers `chunk`. A run covers
            // [first_chunk, next_first_chunk); the last run covers to
            // +inf, which usize::MAX stands in for. Runs are consumed
            // lazily: `run` points at the run covering (or next after)
            // the current chunk.
            while self.chunk >= self.run_end_chunk {
                if self.run >= self.table.chunks.len() {
                    return self.fail(Error::BadValue("stsc has no run covering every chunk"));
                }
                let run = self.table.chunks[self.run];
                self.active_start = run.first_chunk as usize - 1;
                self.active_per = run.samples_per_chunk;
                self.run_end_chunk = self
                    .table
                    .chunks
                    .get(self.run + 1)
                    .map_or(usize::MAX, |r| r.first_chunk as usize - 1);
                self.run += 1;
            }
            self.chunk_cursor = self.table.chunk_offsets[self.chunk];
            self.chunk_samples_left = if self.chunk >= self.active_start {
                self.active_per
            } else {
                0
            };
            if self.chunk_samples_left == 0 {
                // A run that declared zero samples per chunk, or a chunk in
                // a gap before the first run: skip ahead one chunk.
                self.chunk += 1;
            }
        }
        self.chunk_samples_left -= 1;
        let offset = self.chunk_cursor;
        let size = self.table.sample_size(self.sample);
        self.chunk_cursor = self.chunk_cursor.saturating_add(u64::from(size));
        if self.chunk_samples_left == 0 {
            self.chunk += 1;
        }
        // Decoding timestamp from stts. A file with samples but a short or
        // missing stts is corrupt, not "done early".
        while self.time_run_left == 0 {
            let Some(run) = self.table.times.get(self.time_run) else {
                return self.fail(Error::BadValue("stts ran out of samples before stsz"));
            };
            self.time_run += 1;
            self.time_run_left = run.count;
        }
        self.time_run_left -= 1;
        let decoding = self.dts;
        let duration = self.table.times[self.time_run - 1].delta;
        self.dts = self.dts.saturating_add(u64::from(duration));
        // Composition offset from ctts; absent means PTS == DTS.
        let mut comp = 0i64;
        if !self.table.comp.is_empty() {
            while self.comp_run_left == 0 {
                let Some(run) = self.table.comp.get(self.comp_run) else {
                    return self.fail(Error::BadValue("ctts ran out of samples before stsz"));
                };
                self.comp_run += 1;
                self.comp_run_left = run.count;
            }
            self.comp_run_left -= 1;
            comp = i64::from(self.table.comp[self.comp_run - 1].offset);
        }
        let index = self.sample;
        self.sample += 1;
        Some(Ok(Sample {
            offset,
            size,
            decoding,
            presentation: decoding as i64 + comp,
            duration,
            keyframe: self.table.is_keyframe(index),
        }))
    }
}

/// Parses `stsd` (§8.5.2): version/flags, entry count, then entries sized by
/// their own `size:u32, format:u32` headers.
pub fn parse_stsd(data: &[u8]) -> Result<SampleDescription> {
    let mut r = Reader::new(data);
    full_box(&mut r, "stsd")?;
    let count = r.u32("stsd entry count")? as usize;
    let mut entries = Vec::new();
    for _ in 0..count {
        let size = r.u32("stsd entry size")? as usize;
        let coding = r.four("stsd entry type")?;
        if size < 8 {
            return Err(Error::BadValue("stsd entry size smaller than header"));
        }
        let body = size - 8;
        let bytes = r.take(body, "stsd entry")?;
        entries.push(parse_entry(bytes, coding)?);
    }
    Ok(SampleDescription { entries })
}

/// Reads one sample entry body (the bytes after its `size,format` header).
fn parse_entry(data: &[u8], coding: crate::boxes::Four) -> Result<EntryKind> {
    // VisualSampleEntry (§8.5.2/14496-15 §5.3.4): 6 reserved + 2 data ref +
    // 16 pre-defined + 2 width + 2 height + 4 horizres + 4 vertres +
    // 4 reserved + 2 frame count + 32 compressorname + 2 depth +
    // 2 pre-defined = 78 bytes of fixed header.
    const VISUAL_HEADER: usize = 78;
    // AudioSampleEntry: 6 reserved + 2 data ref + 8 reserved +
    // 2 channelcount + 2 samplesize + 2 pre-defined + 2 reserved +
    // 4 samplerate(16.16) = 28 bytes of fixed header.
    const AUDIO_HEADER: usize = 28;
    let mut r = Reader::new(data);
    if is_visual(coding) {
        let header = r.take(VISUAL_HEADER, "visual sample entry")?;
        let width = u16::from_be_bytes([header[24], header[25]]);
        let height = u16::from_be_bytes([header[26], header[27]]);
        let mut avcc = None;
        let mut esds = None;
        for head in crate::boxes::Boxes::new(data, r.pos()..data.len()) {
            let head = head?;
            match &head.four {
                b"avcC" => avcc = Some(data[head.payload].to_vec()),
                b"esds" => esds = Some(data[head.payload].to_vec()),
                _ => {}
            }
        }
        return Ok(EntryKind::Visual {
            coding,
            width,
            height,
            avcc,
            esds,
        });
    }
    if is_audio(coding) {
        let header = r.take(AUDIO_HEADER, "audio sample entry")?;
        let channels = u16::from_be_bytes([header[16], header[17]]);
        let rate = u32::from_be_bytes([header[24], header[25], header[26], header[27]]) >> 16;
        let mut esds = None;
        for head in crate::boxes::Boxes::new(data, r.pos()..data.len()) {
            let head = head?;
            if head.four == *b"esds" {
                esds = Some(data[head.payload].to_vec());
            }
        }
        return Ok(EntryKind::Audio {
            coding,
            channels,
            rate,
            esds,
        });
    }
    Ok(EntryKind::Other { coding })
}

/// Sample-entry types that carry a VisualSampleEntry body (14496-15 §5.3.4
/// plus the common MPEG-4 part 2 and HEVC codings).
fn is_visual(coding: crate::boxes::Four) -> bool {
    matches!(
        &coding,
        b"avc1" | b"avc3" | b"hev1" | b"hvc1" | b"mp4v" | b"s263" | b"vp08" | b"vp09" | b"av01"
    )
}

/// Sample-entry types that carry an AudioSampleEntry body.
fn is_audio(coding: crate::boxes::Four) -> bool {
    matches!(
        &coding,
        b"mp4a" | b"ac-3" | b"ec-3" | b"alac" | b"Opus" | b"fLaC" | b"mp3 "
    )
}

/// Reads `stts`: version/flags then `count` `(sample_count, sample_delta)`
/// pairs (§8.6.1.2).
pub fn parse_stts(data: &[u8]) -> Result<Vec<TimeRun>> {
    let mut r = Reader::new(data);
    full_box(&mut r, "stts")?;
    let count = r.u32("stts entry count")? as usize;
    let mut out = Vec::new();
    for _ in 0..count {
        out.push(TimeRun {
            count: r.u32("stts sample count")?,
            delta: r.u32("stts sample delta")?,
        });
    }
    Ok(out)
}

/// Reads `ctts`: version decides whether composition offsets are signed
/// (§8.6.1.3). Version 0 stores unsigned offsets; we widen to `i64` range via
/// `i32` by treating values above `i32::MAX` as an error rather than
/// silently wrapping a v0 file.
pub fn parse_ctts(data: &[u8]) -> Result<Vec<CompRun>> {
    let mut r = Reader::new(data);
    let (version, _) = full_box(&mut r, "ctts")?;
    let count = r.u32("ctts entry count")? as usize;
    let mut out = Vec::new();
    for _ in 0..count {
        let n = r.u32("ctts sample count")?;
        let off = if version == 0 {
            let v = r.u32("ctts offset")?;
            i32::try_from(v).map_err(|_| Error::BadValue("ctts v0 offset exceeds i32"))?
        } else {
            r.i32("ctts offset")?
        };
        out.push(CompRun {
            count: n,
            offset: off,
        });
    }
    Ok(out)
}

/// Reads `stsc`: `count` `(first_chunk, samples_per_chunk, description)`
/// triples (§8.7.2). Runs must be strictly increasing in `first_chunk` and
/// 1-based.
pub fn parse_stsc(data: &[u8]) -> Result<Vec<ChunkRun>> {
    let mut r = Reader::new(data);
    full_box(&mut r, "stsc")?;
    let count = r.u32("stsc entry count")? as usize;
    let mut out = Vec::new();
    let mut prev_first = 0u32;
    for _ in 0..count {
        let run = ChunkRun {
            first_chunk: r.u32("stsc first chunk")?,
            samples_per_chunk: r.u32("stsc samples per chunk")?,
            description_index: r.u32("stsc description index")?,
        };
        if run.first_chunk == 0 || run.first_chunk <= prev_first {
            return Err(Error::BadValue("stsc first_chunk not increasing"));
        }
        prev_first = run.first_chunk;
        out.push(run);
    }
    Ok(out)
}

/// Reads `stsz`: a uniform size plus either nothing or a size per sample
/// (§8.7.3.2). Returns `(uniform, sizes, sample_count)`.
pub fn parse_stsz(data: &[u8]) -> Result<(u32, Vec<u32>, usize)> {
    let mut r = Reader::new(data);
    full_box(&mut r, "stsz")?;
    let uniform = r.u32("stsz sample size")?;
    let count = r.u32("stsz sample count")? as usize;
    let mut sizes = Vec::new();
    if uniform == 0 {
        for _ in 0..count {
            sizes.push(r.u32("stsz entry")?);
        }
    } else {
        // With a uniform size the entries are absent on disk; anything left
        // is padding we ignore.
    }
    Ok((uniform, sizes, count))
}

/// Reads `stz2` compact sizes (§8.7.3.3): fields of 4, 8 or 16 bits packed
/// into bytes. The packed fields are the one bit-level structure in the
/// sample table, so this goes through the shared [`BitReader`] rather than
/// splitting nibbles by hand. Only sizes representable in 32 bits are
/// produced.
pub fn parse_stz2(data: &[u8]) -> Result<(u32, Vec<u32>, usize)> {
    let mut r = Reader::new(data);
    full_box(&mut r, "stz2")?;
    r.u24("stz2 reserved")?;
    let field_size = r.u8("stz2 field size")?;
    let count = r.u32("stz2 sample count")? as usize;
    let width = match field_size {
        4 | 8 | 16 => usize::from(field_size),
        _ => return Err(Error::BadValue("stz2 field size not 4/8/16")),
    };
    let mut sizes = Vec::new();
    let mut bits = BitReader::new(&data[r.pos()..]);
    for _ in 0..count {
        sizes.push(
            u32::try_from(bits.bits(width)?)
                .map_err(|_| Error::BadValue("stz2 entry wider than u32"))?,
        );
    }
    Ok((0, sizes, count))
}

/// Reads `stco` (32-bit) or `co64` (64-bit) chunk offsets (§8.7.5).
pub fn parse_stco(data: &[u8], wide: bool) -> Result<Vec<u64>> {
    let mut r = Reader::new(data);
    full_box(&mut r, if wide { "co64" } else { "stco" })?;
    let count = r.u32("chunk offset count")? as usize;
    let mut out = Vec::new();
    for _ in 0..count {
        out.push(if wide {
            r.u64("co64 entry")?
        } else {
            u64::from(r.u32("stco entry")?)
        });
    }
    Ok(out)
}

/// Reads `stss`: the sorted, 1-based list of sync samples (§8.6.2).
pub fn parse_stss(data: &[u8]) -> Result<Vec<u32>> {
    let mut r = Reader::new(data);
    full_box(&mut r, "stss")?;
    let count = r.u32("stss entry count")? as usize;
    let mut out = Vec::new();
    let mut prev = 0u32;
    for _ in 0..count {
        let n = r.u32("stss entry")?;
        if n <= prev {
            return Err(Error::BadValue("stss entries not increasing"));
        }
        prev = n;
        out.push(n);
    }
    Ok(out)
}
