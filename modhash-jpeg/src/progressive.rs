//! Progressive (SOF2) scan decoding: spectral selection and successive
//! approximation, per T.81 Annex G.
//!
//! Scan classification follows libjpeg's `jdphuff.c`:
//!
//! | Ss | Se | Ah | Al | meaning |
//! |----|----|----|----|---------|
//! |  0 |  0 |  0 | *  | DC first (may be interleaved, MCU walk) |
//! |  0 |  0 | >0 | *  | DC refine (may be interleaved) |
//! | >0 | ≥Ss|  0 | *  | AC first (Ns must be 1) |
//! | >0 | ≥Ss| >0 | *  | AC refine (Ns must be 1) |
//!
//! AC scans cover the single component's true block grid
//! (`ceil(down_w/8)` × `ceil(down_h/8)`); the restart interval counts
//! blocks. DC scans cover the interleaved MCU grid and count MCUs.
//! EOBRUN persists across blocks and resets at every restart marker.

use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

use crate::bits::{Bits, extend};
use crate::huffman::{Table, ZIGZAG};
use crate::parser::{Component, Frame, Parser, Tables};

/// One progressive scan (SOS segment + entropy data).
pub(crate) fn decode_scan(
    p: &mut Parser<'_>,
    sos: &[u8],
    frame: &mut Frame,
    tables: &Tables,
) -> Result<()> {
    let (order, ss, se, ah, al) = parse_sos(sos, frame)?;
    let mut bits = Bits::new(p.raw(), p.offset());
    let ri = tables.restart_interval;

    if ss == 0 {
        // DC scan: interleaved walk even when Ns == 1 (single block per
        // MCU then), matching libjpeg.
        if ah == 0 {
            dc_first(&mut bits, &order, frame, tables, al, ri)?;
        } else {
            dc_refine(&mut bits, &order, frame, al, ri)?;
        }
    } else {
        if order.len() != 1 {
            return Err(Error::BadValue("progressive AC scan with Ns > 1"));
        }
        if se < ss || se > 63 {
            return Err(Error::BadValue("AC scan spectral band invalid"));
        }
        let ci = order[0];
        if ah == 0 {
            ac_first(&mut bits, ci, frame, tables, Band { ss, se, al }, ri)?;
        } else {
            if ah.saturating_sub(1) != al {
                // T.81: a refinement scan must refine the immediately
                // previous bit position. libjpeg tolerates Al > Ah-1 by
                // treating it as harmless; we accept and decode anyway,
                // since p1 = 1<<Al is still well-defined.
            }
            ac_refine(&mut bits, ci, frame, tables, Band { ss, se, al }, ri)?;
        }
    }

    p.seek(bits.pos);
    Ok(())
}

/// Parses an SOS header for a progressive frame; returns
/// `(component order, Ss, Se, Ah, Al)`.
fn parse_sos(sos: &[u8], frame: &mut Frame) -> Result<(Vec<usize>, usize, usize, usize, usize)> {
    if sos.is_empty() {
        return Err(Error::truncated("SOS header", 1, 0));
    }
    let ns = sos[0] as usize;
    if !(1..=4).contains(&ns) {
        return Err(Error::BadValue("scan component count outside 1..4"));
    }
    let need = 1 + 2 * ns + 3;
    if sos.len() < need {
        return Err(Error::truncated("SOS header", need, sos.len()));
    }
    let mut order = Vec::with_capacity(ns);
    for i in 0..ns {
        let id = sos[1 + 2 * i];
        let t = sos[2 + 2 * i];
        let ci = frame
            .comps
            .iter()
            .position(|c| c.id == id)
            .ok_or(Error::BadValue("SOS references unknown component"))?;
        frame.comps[ci].td = (t >> 4) as usize;
        frame.comps[ci].ta = (t & 15) as usize;
        if frame.comps[ci].td > 3 || frame.comps[ci].ta > 3 {
            return Err(Error::BadValue("Huffman table selector > 3"));
        }
        order.push(ci);
    }
    let ss = sos[1 + 2 * ns] as usize;
    let se = sos[2 + 2 * ns] as usize;
    let ahal = sos[3 + 2 * ns];
    let (ah, al) = ((ahal >> 4) as usize, (ahal & 15) as usize);
    if ss > 63 || se > 63 || ah > 13 || al > 13 {
        return Err(Error::BadValue("Ss/Se/Ah/Al out of range"));
    }
    if ss == 0 && se != 0 {
        return Err(Error::BadValue("DC scan with Se != 0"));
    }
    Ok((order, ss, se, ah, al))
}

/// Spectral band + successive-approximation position of a scan.
#[derive(Copy, Clone)]
struct Band {
    /// First spectral index `Ss`.
    ss: usize,
    /// Last spectral index `Se`.
    se: usize,
    /// Low bit position `Al`.
    al: usize,
}

/// Restart bookkeeping shared by every scan kind.
struct Restart {
    /// Interval in units (MCUs or blocks); 0 = no restarts.
    interval: usize,
    /// Units left before the next expected marker.
    togo: usize,
    /// Expected RST index, mod 8.
    n: u8,
}

impl Restart {
    fn new(interval: usize) -> Self {
        Restart {
            interval,
            togo: interval,
            n: 0,
        }
    }

    /// If the interval is exhausted, consumes the next RSTn marker.
    /// Returns `true` when a restart boundary was just crossed (callers
    /// reset DC predictions / EOBRUN on that edge).
    fn check(&mut self, bits: &mut Bits<'_>) -> Result<bool> {
        if self.interval == 0 || self.togo != 0 {
            return Ok(false);
        }
        if !bits.consume_rst(self.n) {
            return Err(Error::BadValue("missing restart marker"));
        }
        self.n = (self.n + 1) & 7;
        self.togo = self.interval;
        Ok(true)
    }

    fn tick(&mut self) {
        self.togo = self.togo.saturating_sub(1);
    }
}

fn dc_table<'t>(comp: &Component, tables: &'t Tables) -> Result<&'t Table> {
    tables.huff_dc[comp.td]
        .as_ref()
        .ok_or(Error::BadValue("scan uses missing DC Huffman table"))
}

fn ac_table<'t>(comp: &Component, tables: &'t Tables) -> Result<&'t Table> {
    tables.huff_ac[comp.ta]
        .as_ref()
        .ok_or(Error::BadValue("scan uses missing AC Huffman table"))
}

/// True (unpadded) block grid of a component.
fn true_grid(comp: &Component) -> (usize, usize) {
    (comp.down_w.div_ceil(8), comp.down_h.div_ceil(8))
}

/// DC first scan (Ss=0, Ah=0): Huffman size + extend, diff added to a
/// per-component prediction, stored shifted left by Al.
fn dc_first(
    bits: &mut Bits<'_>,
    order: &[usize],
    frame: &mut Frame,
    tables: &Tables,
    al: usize,
    ri: usize,
) -> Result<()> {
    let mut preds = alloc::vec![0i32; order.len()];
    let mut tabs: Vec<&Table> = Vec::new();
    for &ci in order {
        tabs.push(dc_table(&frame.comps[ci], tables)?);
    }
    let mut rst = Restart::new(ri);

    for my in 0..frame.mcus_y {
        for mx in 0..frame.mcus_x {
            if rst.check(bits)? {
                // Restart boundary: DC predictions reset to zero.
                preds.iter_mut().for_each(|p| *p = 0);
            }
            for (si, &ci) in order.iter().enumerate() {
                let comp = &frame.comps[ci];
                let (bw, h, v) = (comp.blocks_w, comp.h, comp.v);
                for by in 0..v {
                    for bx in 0..h {
                        let bi = (my * v + by) * bw + mx * h + bx;
                        let s = tabs[si].decode(bits)?;
                        if s > 15 {
                            return Err(Error::BadValue("DC coefficient size > 15"));
                        }
                        let diff = bits.extend(u32::from(s))?;
                        let v0 = preds[si].wrapping_add(diff);
                        preds[si] = v0;
                        frame.comps[ci].coefs[bi * 64] = v0 << al;
                    }
                }
            }
            rst.tick();
        }
    }
    Ok(())
}

/// DC refine scan (Ss=0, Ah>0): one appended bit per block OR'd in at
/// position Al.
fn dc_refine(
    bits: &mut Bits<'_>,
    order: &[usize],
    frame: &mut Frame,
    al: usize,
    ri: usize,
) -> Result<()> {
    let p1: i32 = 1 << al;
    let mut rst = Restart::new(ri);
    for my in 0..frame.mcus_y {
        for mx in 0..frame.mcus_x {
            rst.check(bits)?;
            for &ci in order {
                let comp = &frame.comps[ci];
                let (bw, h, v) = (comp.blocks_w, comp.h, comp.v);
                for by in 0..v {
                    for bx in 0..h {
                        let bi = (my * v + by) * bw + mx * h + bx;
                        let b = bits.bits(1)? as i32;
                        frame.comps[ci].coefs[bi * 64] |= b * p1;
                    }
                }
            }
            rst.tick();
        }
    }
    Ok(())
}

/// Block grid walker for single-component AC scans, honoring restarts
/// (which reset EOBRUN).
struct Blocks {
    bw: usize,
    bh: usize,
    bw_store: usize,
    bx: usize,
    by: usize,
    rst: Restart,
    /// Remaining EOB run length in blocks (persists across blocks).
    eobrun: u32,
    done: bool,
}

impl Blocks {
    fn new(comp: &Component, ri: usize) -> Self {
        let (bw, bh) = true_grid(comp);
        Blocks {
            bw,
            bh,
            bw_store: comp.blocks_w,
            bx: 0,
            by: 0,
            rst: Restart::new(ri),
            eobrun: 0,
            done: false,
        }
    }

    /// Advances to the next block, handling restart markers.
    /// Returns the block's base index into `coefs` or `None` when the
    /// grid is exhausted.
    fn next(&mut self, bits: &mut Bits<'_>) -> Result<Option<usize>> {
        if self.done || self.by >= self.bh {
            self.done = true;
            return Ok(None);
        }
        if self.rst.check(bits)? {
            // Restart boundary: EOBRUN resets per T.81.
            self.eobrun = 0;
        }
        let bi = self.by * self.bw_store + self.bx;
        self.bx += 1;
        if self.bx >= self.bw {
            self.bx = 0;
            self.by += 1;
        }
        self.rst.tick();
        Ok(Some(bi * 64))
    }
}

/// Appends a refinement bit to `cf` when the read bit is set and the
/// Al-position bit is not already set (T.81 §G.1.2.3 / jdphuff.c).
#[inline]
fn refine_bit(block: &mut [i32], idx: usize, p1: i32, m1: i32, set: bool) {
    if !set {
        return;
    }
    let cf = block[idx];
    if cf != 0 && (cf & p1) == 0 {
        block[idx] = cf + if cf >= 0 { p1 } else { m1 };
    }
}

/// AC first scan (Ss>0, Ah=0): run/size symbols with EOBRUN.
fn ac_first(
    bits: &mut Bits<'_>,
    ci: usize,
    frame: &mut Frame,
    tables: &Tables,
    band: Band,
    ri: usize,
) -> Result<()> {
    let (ss, se, al) = (band.ss, band.se, band.al);
    let tab = ac_table(&frame.comps[ci], tables)?.clone();
    let mut walk = Blocks::new(&frame.comps[ci], ri);
    let comp = &mut frame.comps[ci];

    while let Some(bi) = walk.next(bits)? {
        if walk.eobrun > 0 {
            walk.eobrun -= 1;
            continue;
        }
        let block = &mut comp.coefs[bi..bi + 64];
        let mut k = ss;
        while k <= se {
            let rs = tab.decode(bits)?;
            let r = (rs >> 4) as usize;
            let s = (rs & 15) as usize;
            if s != 0 {
                k += r;
                if k > 63 {
                    break; // corrupt run off the band: mirror libjpeg clamp
                }
                let v = extend(bits.receive(s as u32)?, s as u32);
                block[ZIGZAG[k] as usize] = v << al;
                k += 1;
            } else if r != 15 {
                // EOBr: run length 2^r + appended bits; this block is
                // the first of the run.
                let mut eob = 1u32 << r;
                if r > 0 {
                    eob += bits.receive(r as u32)?;
                }
                walk.eobrun = eob.saturating_sub(1);
                break;
            } else {
                k += 16; // ZRL: 16 zero positions
            }
        }
    }
    Ok(())
}

/// AC refine scan (Ss>0, Ah>0): correction bits for already-nonzero
/// coefficients plus newly-nonzero ±(1<<Al) placements.
fn ac_refine(
    bits: &mut Bits<'_>,
    ci: usize,
    frame: &mut Frame,
    tables: &Tables,
    band: Band,
    ri: usize,
) -> Result<()> {
    let (ss, se, al) = (band.ss, band.se, band.al);
    let tab = ac_table(&frame.comps[ci], tables)?.clone();
    let p1: i32 = 1 << al;
    let m1: i32 = -1 << al;
    let mut walk = Blocks::new(&frame.comps[ci], ri);
    let comp = &mut frame.comps[ci];

    while let Some(bi) = walk.next(bits)? {
        let block = &mut comp.coefs[bi..bi + 64];
        let mut k = ss;
        if walk.eobrun == 0 {
            while k <= se {
                let rs = tab.decode(bits)?;
                let mut r = (rs >> 4) as i32;
                let mut s = (rs & 15) as i32;
                if s != 0 {
                    // A new coefficient has size 1; read its sign bit.
                    // (libjpeg warns on s != 1 but proceeds identically.)
                    s = if bits.receive(1)? != 0 { p1 } else { m1 };
                } else if r != 15 {
                    // EOBr: run length 2^r + appended bits; the rest of
                    // the band is refined by the EOB loop below.
                    let mut eob = 1u32 << r;
                    if r > 0 {
                        eob += bits.receive(r as u32)?;
                    }
                    walk.eobrun = eob;
                    break;
                }
                // s == 0 with r == 15 is ZRL: skip 16 zero positions.
                // Advance k over r zero positions, appending correction
                // bits to every already-nonzero coefficient passed.
                loop {
                    if k > se {
                        break;
                    }
                    let idx = ZIGZAG[k] as usize;
                    if block[idx] != 0 {
                        let set = bits.receive(1)? != 0;
                        refine_bit(block, idx, p1, m1, set);
                    } else {
                        r -= 1;
                        if r < 0 {
                            break;
                        }
                    }
                    k += 1;
                }
                if s != 0 && k <= se {
                    block[ZIGZAG[k] as usize] = s;
                }
                k += 1;
            }
        }
        if walk.eobrun > 0 {
            // Correction bits for every nonzero coefficient in the
            // remainder of the band, then this block counts toward
            // the run (the block containing the EOB symbol already
            // decremented via this same loop).
            while k <= se {
                let idx = ZIGZAG[k] as usize;
                if block[idx] != 0 {
                    let set = bits.receive(1)? != 0;
                    refine_bit(block, idx, p1, m1, set);
                }
                k += 1;
            }
            walk.eobrun = walk.eobrun.saturating_sub(1);
        }
    }
    Ok(())
}
