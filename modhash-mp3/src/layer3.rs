//! Layer III: the hybrid filterbank — side information, the bit
//! reservoir, scalefactors, Huffman requantization, stereo processing,
//! antialiasing, the IMDCT with its four window shapes, and frequency
//! inversion.
//!
//! The spec text (ISO/IEC 11172-3 §2.4.3) leaves several ambiguities in
//! its OCR'd form; the settled semantics below are taken from two
//! independent reference decoders in `lab/mp3/ref/` where they agree:
//! minimp3 (`minimp3.h`) for the sfbtab-per-window scalefactor layout,
//! `ist_pos` handling, `region_count` defaults (7 for switched, 8 for
//! pure short), the `subblock_gain << (2 - scalefac_scale)` fold and the
//! −2 quarter-unit MS-stereo gain offset; mpg123 (`mpg123_layer3.c`)
//! for region boundaries in pairs; and libmad (`mad_layer3.c`) for the
//! uniform 18-sample overlap buffer (`out = z + ovl; ovl = z[18..36]`)
//! shared by long and short blocks.

extern crate alloc;
use alloc::vec::Vec;
use core::cmp::min;

use crate::crc::Crc16;
use crate::header::{Header, Mode};
use crate::hufftab::{HUFF_QUAD_A, HUFF_QUAD_B, HUFF_TABLES, HuffEntry, HuffTable};
use crate::tables::{
    AA_CA, AA_CS, IMDCT_COS12, IMDCT_COS36, IS_PAN, POW43, PRETAB, SFB_LONG, SFB_SHORT, SLEN,
    W_LONG, W_SHORT, W_START, W_STOP,
};
use modhash_primitives::{BitReader, Error, Result};

/// Maximum `main_data_begin` value: the reservoir holds 2^9-1 bytes
/// (§2.4.3.5) so a 9-bit field can always name its furthest history byte.
const RESERVOIR_BYTES: usize = 511;

/// One granule's decoded side information plus its scalefactors.
#[derive(Clone)]
struct Granule {
    part2_3_length: u16,
    big_values: u16,
    global_gain: u8,
    scalefac_compress: u8,
    window_switching: bool,
    block_type: u8,
    mixed: bool,
    table_select: [u8; 3],
    subblock_gain: [u8; 3],
    /// `region0_count`/`region1_count` as the number of sfbtab entries
    /// the region spans MINUS ONE (minimp3's `region_count` semantics).
    region0: u8,
    region1: u8,
    preflag: bool,
    scalefac_scale: u8,
    count1table: u8,
    scfsi: u8,
    /// Transmitted scalefactor index per sfbtab entry; entries beyond the
    /// transmitted set stay 0 (`iscf[36..39]` for short blocks is the
    /// un-transmitted band 12).
    iscf: [u8; 40],
    /// `ist_pos` per sfbtab entry: right channel scalefactors double as
    /// intensity positions for bands it no longer codes. Maintained in
    /// parallel with `iscf`; the top-entry default (3 for MPEG-1) is
    /// written by [`stereo`] per granule.
    ist_pos: [u8; 44],
}

impl Granule {
    const EMPTY: Granule = Granule {
        part2_3_length: 0,
        big_values: 0,
        global_gain: 0,
        scalefac_compress: 0,
        window_switching: false,
        block_type: 0,
        mixed: false,
        table_select: [0; 3],
        subblock_gain: [0; 3],
        region0: 0,
        region1: 0,
        preflag: false,
        scalefac_scale: 0,
        count1table: 0,
        scfsi: 0,
        iscf: [0; 40],
        ist_pos: [0; 44],
    };
}

/// Per-sfbtab-entry line widths in scalefactor/decode order: long blocks
/// list their 21 band widths; pure short blocks interleave window 0/1/2
/// per band (12 bands → 36 real entries + 3 zero-width for the
/// un-transmitted band 12); mixed blocks are 8 long widths then short
/// bands 3..12 (9 bands → 27 entries + 3 zero-width).
struct SfbTab {
    /// `width[i]` is the coded line count of sfbtab entry `i`;
    /// `width[n]` and beyond are 0, which both terminates minimp3-style
    /// `while sfb[i]` walks and yields zero gain bandwidth.
    width: [u16; 45],
    /// Number of real entries (21 / 39 / 38).
    n: usize,
}

/// Build the sfbtab for this granule — the layout that determines the
/// scalefactor read order, the gain boundaries, the reorder walk and the
/// intensity-stereo band scan.
fn sfb_tab(g: &Granule, sri: usize) -> SfbTab {
    let mut w = [0u16; 45];
    let mut n = 0usize;
    if g.block_type == 2 {
        if g.mixed {
            // MPEG-1: 8 long bands, then short bands 3..=12 (10 bands,
            // 3 windows each — minimp3's `n_short_sfb = 30`, so band 12
            // entries carry real widths with zeroed scalefactors).
            for b in 1..=8 {
                w[n] = SFB_LONG[sri][b] - SFB_LONG[sri][b - 1];
                n += 1;
            }
            for b in 3..13 {
                for _ in 0..3 {
                    w[n] = SFB_SHORT[sri][b + 1] - SFB_SHORT[sri][b];
                    n += 1;
                }
            }
        } else {
            for b in 0..13 {
                for _ in 0..3 {
                    w[n] = SFB_SHORT[sri][b + 1] - SFB_SHORT[sri][b];
                    n += 1;
                }
            }
        }
    } else {
        for b in 1..=21 {
            w[n] = SFB_LONG[sri][b] - SFB_LONG[sri][b - 1];
            n += 1;
        }
    }
    SfbTab { width: w, n }
}

/// The bit reservoir: a rolling window over the assembled main_data
/// stream — the last [`RESERVOIR_BYTES`] bytes of every frame's
/// `history + main_data` concatenation.
pub(crate) struct Reservoir {
    buf: Vec<u8>,
}

impl Reservoir {
    /// A fresh, empty reservoir (decoder startup or `Decoder::reset`).
    pub(crate) fn new() -> Self {
        Reservoir {
            buf: Vec::with_capacity(RESERVOIR_BYTES),
        }
    }

    /// Append a frame's main-data bytes to the rolling stream and keep
    /// only the last 511 bytes — the bytes this frame contributes are
    /// `main_data`, NOT the assembled `history + main_data` (the history
    /// is already inside `buf`; pushing the assembly would duplicate it).
    /// Frames that decode to nothing (a Xing/Info tag) still contribute
    /// their bytes: they are part of the reservoir stream.
    pub(crate) fn push(&mut self, main_data: &[u8]) {
        self.buf.extend_from_slice(main_data);
        if self.buf.len() > RESERVOIR_BYTES {
            let drop = self.buf.len() - RESERVOIR_BYTES;
            self.buf.drain(..drop);
        }
    }

    /// Build this frame's granule bit stream: the last `main_data_begin`
    /// bytes of history followed by the frame's own `main_data`.
    fn assemble(&self, main_data_begin: usize, main_data: &[u8]) -> Result<Vec<u8>> {
        if main_data_begin > self.buf.len() {
            return Err(Error::Truncated {
                what: "bit reservoir",
                needed: main_data_begin,
                found: self.buf.len(),
            });
        }
        let mut v = Vec::with_capacity(main_data_begin + main_data.len());
        v.extend_from_slice(&self.buf[self.buf.len() - main_data_begin..]);
        v.extend_from_slice(main_data);
        Ok(v)
    }
}

/// Per-channel IMDCT overlap state: 18 samples per subband, carried
/// across granules AND frames (the hybrid filterbank is persistent).
pub(crate) struct Overlap {
    ovl: [[f32; 18 * 32]; 2],
}

impl Overlap {
    /// Zeroed overlap — every band's previous block is silent.
    pub(crate) fn new() -> Self {
        Overlap {
            ovl: [[0.0; 18 * 32]; 2],
        }
    }
}

/// The Layer III half of `Decoder::decode_frame`: turns `frame` (from
/// the sync word) into `sb[ch][subband][36]` — 36 hybrid-domain samples
/// per subband per channel for the polyphase synthesis filterbank.
pub(crate) fn decode_frame(
    h: &Header,
    frame: &[u8],
    reservoir: &mut Reservoir,
    overlap: &mut Overlap,
) -> Result<[[[f32; 36]; 32]; 2]> {
    let nch = h.channels();
    let body = &frame[4..];
    let mut r = BitReader::new(body);

    // Optional CRC: two bytes at the head of the body covering header
    // bytes 2..4 plus the (byte-aligned) side info — 17 or 32 bytes.
    if !h.unprotected {
        let crc_target = r.bits(16)? as u16;
        let side_bytes = if nch == 1 { 17 } else { 32 };
        if body.len() < 2 + side_bytes {
            return Err(Error::Truncated {
                what: "side information",
                needed: 2 + side_bytes,
                found: body.len(),
            });
        }
        let mut crc = Crc16::new();
        crc.bytes(&frame[2..4]);
        crc.bytes(&body[2..2 + side_bytes]);
        if crc.finish() != crc_target {
            return Err(Error::BadValue("crc"));
        }
    }

    let main_data_begin = r.bits(9)? as usize;
    r.bits(if nch == 1 { 5 } else { 3 })?; // private bits
    let mut scfsi = [0u8; 2];
    for s in scfsi.iter_mut().take(nch) {
        *s = r.bits(4)? as u8;
    }

    let mut grs = [
        Granule::EMPTY,
        Granule::EMPTY,
        Granule::EMPTY,
        Granule::EMPTY,
    ];
    for gr in 0..2 {
        for ch in 0..nch {
            let g = &mut grs[ch * 2 + gr];
            g.part2_3_length = r.bits(12)? as u16;
            g.big_values = r.bits(9)? as u16;
            if g.big_values > 288 {
                return Err(Error::BadValue("big_values"));
            }
            g.global_gain = r.bits(8)? as u8;
            g.scalefac_compress = r.bits(4)? as u8;
            g.window_switching = r.bits(1)? != 0;
            if g.window_switching {
                g.block_type = r.bits(2)? as u8;
                if g.block_type == 0 {
                    return Err(Error::BadValue("block_type"));
                }
                g.mixed = r.bits(1)? != 0;
                g.table_select[0] = r.bits(5)? as u8;
                g.table_select[1] = r.bits(5)? as u8;
                for s in &mut g.subblock_gain {
                    *s = r.bits(3)? as u8;
                }
                // Region counts are implicit for switched blocks
                // (minimp3/mpg123): 7 for start/stop and mixed, 8 for
                // pure short — both name the sfbtab entry ending at
                // line 36. region1 spans to the end.
                g.region0 = if g.block_type == 2 && !g.mixed { 8 } else { 7 };
                g.region1 = 255;
            } else {
                for t in &mut g.table_select {
                    *t = r.bits(5)? as u8;
                }
                g.region0 = r.bits(4)? as u8;
                g.region1 = r.bits(3)? as u8;
            }
            g.preflag = r.bits(1)? != 0;
            g.scalefac_scale = r.bits(1)? as u8;
            g.count1table = r.bits(1)? as u8;
            // scfsi applies to granule 1 of a channel only when that
            // granule is a plain long block (mpg123; switched granules
            // never gate partitions).
            g.scfsi = if gr == 1 && !g.window_switching {
                scfsi[ch]
            } else {
                0
            };
        }
    }

    // Side info must fit the frame; main data follows byte-aligned.
    let side_end = r.bit_position().div_ceil(8);
    if side_end > body.len() {
        return Err(Error::Truncated {
            what: "side information",
            needed: side_end,
            found: body.len(),
        });
    }
    let main_data = &body[side_end..];

    let stream = reservoir.assemble(main_data_begin, main_data)?;

    // Sanity: the declared granule bits must fit the assembled stream.
    // (minimp3's part_23_sum check.)
    let part23_bits: usize = grs[..nch * 2]
        .iter()
        .map(|g| usize::from(g.part2_3_length))
        .sum();
    if part23_bits > stream.len() * 8 {
        return Err(Error::Truncated {
            what: "main_data",
            needed: part23_bits,
            found: stream.len() * 8,
        });
    }

    let mut lines = [[0f32; 576]; 4]; // [ch*2+gr]
    // `lines` is long-lived but each granule's spectrum starts as zeros:
    // `huffman` only writes the coded prefix, so granule N+1's buffer
    // must not inherit granule N's tail (ISO 11172-3 treats uncoded
    // lines as exactly 0).
    {
        let mut br = BitReader::new(&stream);
        for gr_i in 0..2 {
            for ch in 0..nch {
                let limit = br.bit_position() + usize::from(grs[ch * 2 + gr_i].part2_3_length);
                if limit > stream.len() * 8 {
                    return Err(Error::Truncated {
                        what: "granule",
                        needed: limit,
                        found: stream.len() * 8,
                    });
                }
                let idx = ch * 2 + gr_i;
                // `prev` is granule 0 of this channel — clone it so `g`
                // can borrow `grs` mutably without aliasing.
                let prev = grs[ch * 2].clone();
                let g = &mut grs[idx];
                lines[idx].fill(0.0);
                read_scalefactors(&mut br, g, &prev, limit)?;
                huffman(
                    &mut br,
                    g,
                    &mut lines[idx],
                    h.sample_rate,
                    limit,
                    h.ms_stereo(),
                )?;
                // A granule may end before its declared length — stuffing —
                // but must never pass it.
                if br.bit_position() > limit {
                    return Err(Error::BadValue("part2_3_length"));
                }
                let mut left = limit - br.bit_position();
                while left > 0 {
                    let n = min(left, 32);
                    br.bits(n)?;
                    left -= n;
                }
            }
        }
        // Record this frame's contribution to the main-data stream.
        reservoir.push(main_data);
    }

    // Stereo + per-granule reconstruction pipeline.
    let mut out = [[[0f32; 36]; 32]; 2];
    for gr_i in 0..2 {
        if h.mode == Mode::JointStereo && nch == 2 {
            let (lo, hi) = lines.split_at_mut(2);
            let (glo, ghi) = grs.split_at_mut(2);
            stereo(
                &mut lo[gr_i], // ch0 = left
                &mut hi[gr_i], // ch1 = right
                &mut glo[gr_i],
                &mut ghi[gr_i],
                h.sample_rate,
                h.ms_stereo(),
                h.intensity_stereo(),
            )?;
        }
        for ch in 0..nch {
            post_process(
                &mut lines[ch * 2 + gr_i],
                &mut out[ch],
                &mut overlap.ovl[ch],
                &grs[ch * 2 + gr_i],
                h.sample_rate,
                gr_i,
            );
        }
    }
    Ok(out)
}

/// Read the granule's scalefactors into `g.iscf`/`g.ist_pos`, honouring
/// `scfsi` (which copies granule-0's ist_pos range for gated
/// partitions). Layout per mpg123's `III_get_scale_factors_1`: long
/// {6,5,5,5} of sizes {s1,s1,s2,s2}; mixed {8,9,6,12} of {s1,s1,s2,s2};
/// short {9,9,6,12} of {s1,s1,s2,s2}. `limit` bounds the granule.
fn read_scalefactors(
    br: &mut BitReader<'_>,
    g: &mut Granule,
    prev: &Granule,
    limit: usize,
) -> Result<()> {
    let slen = SLEN[usize::from(g.scalefac_compress)];
    let (s1, s2) = (usize::from(slen[0]), usize::from(slen[1]));
    let parts: [(usize, usize, bool); 4] = if g.block_type == 2 {
        if g.mixed {
            [
                (8, s1, false),
                (9, s1, false),
                (6, s2, false),
                (12, s2, false),
            ]
        } else {
            [
                (9, s1, false),
                (9, s1, false),
                (6, s2, false),
                (12, s2, false),
            ]
        }
    } else {
        [(6, s1, true), (5, s1, true), (5, s2, true), (5, s2, true)]
    };

    let mut i = 0usize;
    for (pi, &(cnt, len, gated)) in parts.iter().enumerate() {
        if gated && (g.scfsi >> (3 - pi)) & 1 == 1 {
            for j in i..i + cnt {
                g.iscf[j] = prev.iscf[j];
                g.ist_pos[j] = prev.ist_pos[j];
            }
        } else {
            for j in i..i + cnt {
                if br.bit_position() + len > limit {
                    return Err(Error::Truncated {
                        what: "scalefactors",
                        needed: limit,
                        found: br.bit_position() + len,
                    });
                }
                g.iscf[j] = if len > 0 { br.bits(len)? as u8 } else { 0 };
                g.ist_pos[j] = g.iscf[j];
            }
        }
        i += cnt;
    }
    Ok(())
}

/// `2^(q/4)` for integer `q` via exact exponent arithmetic — the MP3
/// scalefactor basis (all gains live in quarter-dB units).
fn pow2_quarter(q: i32) -> f32 {
    const FRAC: [f32; 4] = [
        1.0,
        1.189_207_1_f32,           // 2^0.25
        core::f32::consts::SQRT_2, // 2^0.5
        1.681_792_8_f32,           // 2^0.75
    ];
    let e = q.div_euclid(4);
    let f = q.rem_euclid(4) as usize;
    let b = (e + 127).clamp(-25, 255); // allow subnormals at the bottom
    if b <= 0 {
        return 0.0;
    }
    FRAC[f] * f32::from_bits((b as u32) << 23)
}

/// Per-entry dequantizer gain — minimp3's `scf[]` computation:
/// `one = 2^(0.25·(global_gain − 2·ms − 210 − ((iscf_eff) << scf_shift)))`
/// where `ms` folds the joint-stereo 1/√2 normalization into the gain
/// (ffmpeg `ff_mpegaudiodec_template.c` does the same via
/// `global_gain -= 2` for ms-only streams), `scf_shift =
/// scalefac_scale + 1`, and `iscf_eff` folds
/// `subblock_gain << (3 − scf_shift)` (short entries) or the pretab
/// (long + preflag) into the transmitted index.
fn granule_gain(g: &Granule, entry: usize, ms_stereo: bool) -> f32 {
    let short_base = if g.block_type == 2 && g.mixed { 8 } else { 0 };
    let scf_shift = i32::from(g.scalefac_scale) + 1;
    let mut iscf = i32::from(g.iscf[min(entry, 39)]);
    if g.block_type == 2 && entry >= short_base {
        let w = (entry - short_base) % 3;
        iscf += i32::from(g.subblock_gain[w]) << (3 - scf_shift);
    } else if g.block_type != 2 && g.preflag {
        // pretab is indexed by scalefactor band; entry == band here.
        iscf += i32::from(PRETAB[min(entry, 20)]);
    }
    // Quarter-dB exponent: 2^(0.25 · q), minimp3's scf computation.
    let q = i32::from(g.global_gain) - if ms_stereo { 2 } else { 0 } - 210 - (iscf << scf_shift);
    pow2_quarter(q)
}

/// `|v|^(4/3) * gain` with the sign bit applied — POW43 covers
/// 0..=8206, the max from a 13-linbit escape (15+8191).
fn apply_req(v: u32, s: bool, gain: f32) -> f32 {
    let mag = POW43[min(v as usize, POW43.len() - 1)] * gain;
    if s { -mag } else { mag }
}

/// Huffman-decode `2·big_values` pair-coded values then the count1 quads
/// into `out[576]`, bounded by `limit` bits from the granule start.
///
/// Region boundaries count sfbtab entries (`region0`+1 entries, then
/// `region0`+`region1`+2); gains are per-entry, so short blocks get
/// per-window scalefactors on the coded (band,window,freq) layout
/// before reordering — a pure permutation of `out`.
#[allow(clippy::needless_range_loop)]
fn huffman(
    br: &mut BitReader<'_>,
    g: &Granule,
    out: &mut [f32; 576],
    sample_rate: u32,
    limit: usize,
    ms_stereo: bool,
) -> Result<()> {
    let sri = match sample_rate {
        44100 => 0,
        48000 => 1,
        32000 => 2,
        _ => return Err(Error::BadValue("sample rate")),
    };
    let tab = sfb_tab(g, sri);

    // Cumulative line edges per entry (entry i covers [prev, edge[i])).
    let mut edge = [0usize; 45];
    let mut acc = 0usize;
    for i in 0..tab.n {
        acc += usize::from(tab.width[i]);
        edge[i] = acc;
    }
    let line_of_entry = |e: usize| -> usize {
        if e == 0 {
            0
        } else if e <= tab.n {
            edge[e - 1]
        } else {
            acc
        }
    };

    // Region boundaries in LINES. Non-switched granules transmit
    // region0/region1 as band counts; for MPEG-1 switched blocks the
    // split is fixed at line 36 / line 576 (mpg123: `!lsf` branch sets
    // `region1start = 36>>1`, `region2start = 576>>1` — the 54>>1 branch
    // is MPEG-2 only). table_select[2] is never used because
    // big_values <= 288 pairs keeps decode inside regions 0/1.
    let (r0_lines, r1_lines) = if g.window_switching {
        (36usize, 576usize)
    } else {
        (
            line_of_entry(min(usize::from(g.region0) + 1, tab.n)),
            line_of_entry(min(
                usize::from(g.region0) + usize::from(g.region1) + 2,
                tab.n,
            )),
        )
    };

    let tables = [
        HUFF_TABLES[usize::from(g.table_select[0])],
        HUFF_TABLES[usize::from(g.table_select[1])],
        HUFF_TABLES[usize::from(g.table_select[2])],
    ];

    let pair_groups = [
        len_groups(tables[0].entries),
        len_groups(tables[1].entries),
        len_groups(tables[2].entries),
    ];
    let mut i = 0usize; // spectral line index
    let mut ent = 0usize; // sfbtab entry covering i
    let mut pairs = 0usize;
    let big = usize::from(g.big_values);
    while pairs < big && i + 1 < 577 {
        let region = if 2 * pairs < r0_lines {
            0
        } else if 2 * pairs < r1_lines {
            1
        } else {
            2
        };
        let t: HuffTable = tables[region];
        if t.entries.len() <= 1 {
            // table_select 0: no codes in this region — the coded lines
            // are zero, and the granule's bit budget moves on.
            return Err(Error::BadValue("huffman table select"));
        }
        // The granule budget bounds every field read; a field that would
        // cross `limit` means the granule's data ran out mid-pair —
        // mpg123 clamps the position to the limit and remaining lines
        // stay zero. `take` never consumes past the limit, so the
        // post-granule skip realigns the reader exactly.
        let Some((x, y)) = decode_pair(br, t, limit, &pair_groups[region])? else {
            break;
        };
        let vx = if x == 15 && t.linbits > 0 {
            let Some(lin) = take(br, usize::from(t.linbits), limit)? else {
                break;
            };
            x + lin as u32
        } else {
            x
        };
        let Some(sx) = take(br, usize::from(vx != 0), limit)? else {
            break;
        };
        let vy = if y == 15 && t.linbits > 0 {
            let Some(lin) = take(br, usize::from(t.linbits), limit)? else {
                break;
            };
            y + lin as u32
        } else {
            y
        };
        let Some(sy) = take(br, usize::from(vy != 0), limit)? else {
            break;
        };
        while ent < tab.n && i >= edge[ent] {
            ent += 1;
        }
        let gain = granule_gain(g, ent, ms_stereo);
        if i < 576 {
            out[i] = apply_req(vx, sx != 0, gain);
        }
        if i + 1 < 576 {
            out[i + 1] = apply_req(vy, sy != 0, gain);
        }
        i += 2;
        pairs += 1;
    }

    // count1 quads until 576 lines or the granule's bit budget; the
    // scalefactor band may turn between a quad's pairs (minimp3's
    // RELOAD_SCALEFACTOR per two values).
    let quad: &[HuffEntry] = if g.count1table == 0 {
        HUFF_QUAD_A
    } else {
        HUFF_QUAD_B
    };
    let quad_groups = len_groups(quad);
    while i + 3 < 576 {
        if br.bit_position() >= limit {
            break;
        }
        let Some((vals, _)) = decode_quad(br, quad, limit, &quad_groups)? else {
            break;
        };
        let mut tmp = [0f32; 4];
        let mut ok = true;
        for (k, &v) in vals.iter().enumerate() {
            if (i + k) % 2 == 0 {
                while ent < tab.n && i + k >= edge[ent] {
                    ent += 1;
                }
            }
            let gain = granule_gain(g, min(ent, 43), ms_stereo);
            let s = if v != 0 {
                match take(br, 1, limit)? {
                    Some(b) => b != 0,
                    None => {
                        ok = false;
                        break;
                    }
                }
            } else {
                false
            };
            tmp[k] = apply_req(v, s, gain);
        }
        if !ok {
            break; // sign bit overran the granule: discard the quad
        }
        out[i..i + 4].copy_from_slice(&tmp);
        i += 4;
    }
    Ok(())
}

/// Read `n` bits unless doing so would pass the granule's bit budget —
/// `None` means "budget exhausted", never consumes. `Err` is real
/// truncation (the stream itself ended).
fn take(br: &mut BitReader<'_>, n: usize, limit: usize) -> Result<Option<u64>> {
    if n == 0 {
        return Ok(Some(0));
    }
    if br.bit_position() + n > limit {
        return Ok(None);
    }
    Ok(Some(br.bits(n)?))
}

/// Bit-at-a-time Huffman pair decode. `groups` is a per-length (start,
/// count) index into `t.entries` built once per granule — the tables are
/// sorted by length then code, so each candidate length scans only its
/// own group.
fn decode_pair(
    br: &mut BitReader<'_>,
    t: HuffTable,
    limit: usize,
    groups: &[(usize, usize); 20],
) -> Result<Option<(u32, u32)>> {
    let mut code = 0u32;
    for len in 1..=19u8 {
        let Some(bit) = take(br, 1, limit)? else {
            return Ok(None);
        };
        code = (code << 1) | bit as u32;
        let (start, count) = groups[usize::from(len)];
        for e in &t.entries[start..start + count] {
            if e.code == code {
                return Ok(Some((u32::from(e.x), u32::from(e.y))));
            }
        }
    }
    Err(Error::BadValue("huffman code"))
}

/// (start, count) per code length for a sorted entry list.
fn len_groups(entries: &[HuffEntry]) -> [(usize, usize); 20] {
    let mut g = [(0usize, 0usize); 20];
    for (i, e) in entries.iter().enumerate() {
        let l = usize::from(e.len);
        if g[l].1 == 0 {
            g[l].0 = i;
        }
        g[l].1 += 1;
    }
    g
}

/// Quad-table decode: `x` packs (v,w,x,y) as v<<3|w<<2|x<<1|y
/// (generator convention); returns the four values.
fn decode_quad(
    br: &mut BitReader<'_>,
    t: &[HuffEntry],
    limit: usize,
    groups: &[(usize, usize); 20],
) -> Result<Option<([u32; 4], u8)>> {
    let mut code = 0u32;
    for len in 1..=7u8 {
        let Some(bit) = take(br, 1, limit)? else {
            return Ok(None);
        };
        code = (code << 1) | bit as u32;
        let (start, count) = groups[usize::from(len)];
        for e in &t[start..start + count] {
            if e.code == code {
                let p = u32::from(e.x);
                return Ok(Some((
                    [(p >> 3) & 1, (p >> 2) & 1, (p >> 1) & 1, p & 1],
                    len,
                )));
            }
        }
    }
    Err(Error::BadValue("huffman quad code"))
}

/// Joint-stereo processing for one granule — minimp3's semantics:
/// the right channel's raw scalefactors (`ist_pos`) double as intensity
/// positions; its last *coded* sfbtab entry per window position bounds
/// where intensity applies; the top entries get default pos 3
/// (inherited from the previous entry when the coded data reaches them).
/// `left`/`right` are the two granule line buffers.
#[allow(clippy::needless_range_loop)]
fn stereo(
    left: &mut [f32; 576],
    right: &mut [f32; 576],
    _gl: &mut Granule,
    gr: &mut Granule,
    sample_rate: u32,
    ms: bool,
    is: bool,
) -> Result<()> {
    if !ms && !is {
        return Ok(());
    }
    let sri = match sample_rate {
        48000 => 1,
        32000 => 2,
        _ => 0,
    };
    let tab = sfb_tab(gr, sri);
    let n_sfb = tab.n;

    // Highest sfbtab entry whose right channel carries data, per window
    // position (entry index mod 3 for short blocks; collapsed to one
    // value for long blocks).
    let mut max_band = [-1i32; 3];
    let mut pos = 0usize;
    for i in 0..n_sfb {
        let end = pos + usize::from(tab.width[i]);
        let mut k = pos;
        while k + 1 < end && k + 1 < 576 {
            if right[k] != 0.0 || right[k + 1] != 0.0 {
                max_band[i % 3] = i as i32;
                break;
            }
            k += 2;
        }
        pos = end;
    }
    let max_blocks = if gr.block_type == 2 { 3usize } else { 1usize };
    if gr.block_type != 2 {
        let m = max_band.iter().copied().max().unwrap_or(-1);
        max_band = [m, m, m];
    }

    // Top-entry ist_pos defaults (minimp3): for each window position,
    // entry n_sfb−max_blocks+w takes 3 if coded data doesn't reach it,
    // else the previous entry's value.
    for w in 0..max_blocks {
        let itop = n_sfb - max_blocks + w;
        let prev = itop.saturating_sub(max_blocks);
        gr.ist_pos[itop] = if max_band[w] >= prev as i32 {
            3 // MPEG-1 default
        } else {
            gr.ist_pos[prev]
        };
    }

    let rt2 = core::f32::consts::SQRT_2;
    let mut pos = 0usize;
    for i in 0..n_sfb {
        let w = usize::from(tab.width[i]);
        if w == 0 {
            break;
        }
        let end = (pos + w).min(576);
        let ipos = gr.ist_pos[i];
        if is && (i as i32) > max_band[i % 3] && ipos < 7 {
            let kl = IS_PAN[usize::from(ipos) * 2];
            let kr = IS_PAN[usize::from(ipos) * 2 + 1];
            let s = if ms { rt2 } else { 1.0 };
            for k in pos..end {
                let v = left[k];
                left[k] = v * kl * s;
                right[k] = v * kr * s;
            }
        } else if ms {
            for k in pos..end {
                let (a, b) = (left[k], right[k]);
                left[k] = a + b;
                right[k] = a - b;
            }
        }
        pos += w;
    }
    Ok(())
}

/// Reorder (short), antialias, IMDCT + windowing + overlap-add, and
/// frequency inversion — the per-channel tail of one granule.
fn post_process(
    xr: &mut [f32; 576],
    sb_out: &mut [[f32; 36]; 32],
    ovl: &mut [f32; 18 * 32],
    g: &Granule,
    sample_rate: u32,
    granule: usize,
) {
    let sri = match sample_rate {
        48000 => 1,
        32000 => 2,
        _ => 0,
    };
    // In subbands: 2 for a mixed MPEG-1 block (the first 36 lines are
    // long-window data in subbands 0-1).
    let n_long_bands = if g.block_type == 2 && g.mixed { 2 } else { 0 };
    if g.block_type == 2 {
        reorder(&mut xr[n_long_bands * 18..], g, sri);
    }
    // Antialias butterflies sit between subbands: 31 boundaries for
    // long blocks; for a mixed block the boundary into the first short
    // subband is skipped, and pure short blocks skip all of them.
    let aa_bands = match (g.block_type == 2, g.mixed) {
        (true, true) => n_long_bands - 1, // between the long subbands only
        (true, false) => 0,
        _ => 31,
    };
    antialias(xr, aa_bands);
    imdct_gr(xr, sb_out, ovl, g, n_long_bands, granule);
    freq_invert(sb_out, granule);
}

/// Short-block reordering: `(sfb, win, freq)` → `(freq, win)` per
/// subband row — minimp3's `L3_reorder` walked on sfbtab widths. `buf`
/// starts at the first short line (0 for pure short, 36 for mixed).
fn reorder(buf: &mut [f32], g: &Granule, sri: usize) {
    let tab = sfb_tab(g, sri);
    let first = if g.mixed { 8 } else { 0 };
    let mut scratch = [0f32; 576];
    let (mut src, mut dst) = (0usize, 0usize);
    // `sfb_tab` expands each scalefactor band into three window
    // entries; the coded layout is per-band (window-major), so the
    // source advances one band per step while the entry index walks
    // windows in threes (minimp3's `sfb += 3`).
    let mut i = first;
    while i < tab.n {
        let len = usize::from(tab.width[i]);
        if len == 0 {
            break;
        }
        for f in 0..len {
            for w in 0..3 {
                if dst < scratch.len() && src + w * len + f < buf.len() {
                    scratch[dst] = buf[src + w * len + f];
                }
                dst += 1;
            }
        }
        src += len * 3;
        i += 3;
    }
    let n = min(dst, buf.len());
    buf[..n].copy_from_slice(&scratch[..n]);
}

/// Alias-reduction butterflies (§2.4.3.4.10, Table B.9) on `bands`
/// consecutive subband boundaries starting at subband 0/1. libmad's
/// convention with the signed `ca` table:
/// `low' = low·cs − high·ca`, `high' = low·ca + high·cs`.
fn antialias(xr: &mut [f32], bands: usize) {
    for b in 0..bands.min(31) {
        for i in 0..8 {
            let lo = xr[18 * b + 17 - i];
            let hi = xr[18 * b + 18 + i];
            xr[18 * b + 17 - i] = lo * AA_CS[i] - hi * AA_CA[i];
            xr[18 * b + 18 + i] = lo * AA_CA[i] + hi * AA_CS[i];
        }
    }
}

/// 36-point IMDCT (§2.4.3.4): `y[i] = Σ_k x[k]·cos((2i+19)(2k+1)π/72)`,
/// direct form via the generated `IMDCT_COS36` table — the workspace has
/// no shared DCT to reuse and the direct sum keeps indexing honest.
fn imdct36(x: &[f32; 18], w: &[f32; 36], z: &mut [f32; 36]) {
    for i in 0..36 {
        let mut acc = 0f32;
        for (k, &x_k) in x.iter().enumerate() {
            acc += x_k * IMDCT_COS36[i * 18 + k];
        }
        z[i] = acc * w[i];
    }
}

/// One subband's three 12-point IMDCTs + short window, packed as
/// libmad's `z[36]` concat: `[0⁶, y0h, y0t+y1h, y1t+y2h, y2t, 0⁶]` —
/// then the uniform overlap step consumes it like a long block.
fn imdct12_block(x: &[f32; 18], z: &mut [f32; 36]) {
    let mut y = [[0f32; 12]; 3];
    for (w, yw) in y.iter_mut().enumerate() {
        for i in 0..12 {
            let mut acc = 0f32;
            for k in 0..6 {
                acc += x[3 * k + w] * IMDCT_COS12[i * 6 + k];
            }
            yw[i] = acc * W_SHORT[i];
        }
    }
    z[..6].fill(0.0);
    z[30..].fill(0.0);
    for i in 0..6 {
        z[6 + i] = y[0][i];
        z[12 + i] = y[0][6 + i] + y[1][i];
        z[18 + i] = y[1][6 + i] + y[2][i];
        z[24 + i] = y[2][6 + i];
    }
}

/// IMDCT + windowing + overlap-add of one granule for all 32 subbands,
/// libmad's uniform scheme: `z` is the 36-sample windowed IMDCT
/// (short blocks concatenate three windowed halves into it), output is
/// `z[0..18] + ovl`, next overlap is `z[18..36]`.
fn imdct_gr(
    xr: &mut [f32; 576],
    sb_out: &mut [[f32; 36]; 32],
    ovl: &mut [f32; 18 * 32],
    g: &Granule,
    n_long_bands: usize,
    granule: usize,
) {
    let base = granule * 18;
    let w36: &[f32; 36] = match g.block_type {
        1 => &W_START,
        3 => &W_STOP,
        _ => &W_LONG,
    };
    for sb in 0..32 {
        let o = &mut ovl[sb * 18..sb * 18 + 18];
        let mut z = [0f32; 36];
        if sb < n_long_bands {
            // Mixed blocks: long-window subbands use the normal window.
            let mut x = [0f32; 18];
            x.copy_from_slice(&xr[sb * 18..sb * 18 + 18]);
            imdct36(&x, &W_LONG, &mut z);
        } else if g.block_type == 2 {
            let mut x = [0f32; 18];
            x.copy_from_slice(&xr[sb * 18..sb * 18 + 18]);
            imdct12_block(&x, &mut z);
        } else {
            let mut x = [0f32; 18];
            x.copy_from_slice(&xr[sb * 18..sb * 18 + 18]);
            imdct36(&x, w36, &mut z);
        }
        for i in 0..18 {
            sb_out[sb][base + i] = z[i] + o[i];
            o[i] = z[18 + i];
        }
    }
}

/// Frequency inversion of the polyphase input (§2.4.3.4): every second
/// sample of every odd subband is negated — subbands 1,3,5..31, time
/// indices 1,3,5..17 within this granule's half.
fn freq_invert(sb_out: &mut [[f32; 36]; 32], granule: usize) {
    let base = granule * 18;
    for b in (1..32).step_by(2) {
        for t in (1..18).step_by(2) {
            sb_out[b][base + t] = -sb_out[b][base + t];
        }
    }
}
