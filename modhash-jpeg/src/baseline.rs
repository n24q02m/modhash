//! Sequential (baseline SOF0 / extended SOF1) scan decoding.
//!
//! The scan is a single MCU walk: for an interleaved scan (`Ns > 1`)
//! each MCU contributes `h_i × v_i` blocks per component in raster MCU
//! order; for a single-component scan the MCU is one block and the walk
//! covers the component's true block grid `ceil(down_w/8) ×
//! ceil(down_h/8)`. Coefficients are stored in natural order in
//! [`Component::coefs`]; dequantization happens at IDCT time.
//!
//! Padded MCU-grid blocks (columns/rows past `ceil(down_w/8)`) are
//! decoded — the entropy stream contains them — but their coefficients
//! land in the padded region of the plane which upsampling never reads.

use alloc::vec::Vec;
use modhash_primitives::{Error, Result};

use crate::bits::Bits;
use crate::huffman::{Table, ZIGZAG};
use crate::parser::{Component, Frame, Parser, Tables};

/// Decodes the single sequential scan starting after the SOS header
/// segment `sos`.
pub(crate) fn decode_scan(
    p: &mut Parser<'_>,
    sos: &[u8],
    frame: &mut Frame,
    tables: &Tables,
) -> Result<()> {
    let scan = parse_sos(sos, frame, false)?;
    let mut bits = Bits::new(p.raw(), p.offset());
    let ri = tables.restart_interval;

    if scan.len() == 1 {
        // Non-interleaved: MCU = one block over the true grid.
        let ci = scan[0];
        let (bw, bh, store_w) = (
            frame.comps[ci].down_w.div_ceil(8),
            frame.comps[ci].down_h.div_ceil(8),
            frame.comps[ci].blocks_w,
        );
        let dctab = dc_table(&frame.comps[ci], tables)?;
        let actab = ac_table(&frame.comps[ci], tables)?;
        let mut pred = 0i32;
        let mut togo = ri;
        let mut rst = 0u8;
        for by in 0..bh {
            for bx in 0..bw {
                if ri > 0 && togo == 0 {
                    if !bits.consume_rst(rst) {
                        return Err(Error::BadValue("missing restart marker"));
                    }
                    rst = (rst + 1) & 7;
                    togo = ri;
                    pred = 0;
                }
                let bi = by * store_w + bx;
                decode_block(
                    &mut bits,
                    dctab,
                    actab,
                    &mut pred,
                    &mut frame.comps[ci].coefs[bi * 64..bi * 64 + 64],
                )?;
                togo = togo.saturating_sub(1);
            }
        }
    } else {
        // Interleaved MCU walk.
        let mut preds = alloc::vec![0i32; scan.len()];
        let mut tables_v: Vec<(&Table, &Table)> = Vec::new();
        for &ci in &scan {
            tables_v.push((
                dc_table(&frame.comps[ci], tables)?,
                ac_table(&frame.comps[ci], tables)?,
            ));
        }
        let mut togo = ri;
        let mut rst = 0u8;
        for my in 0..frame.mcus_y {
            for mx in 0..frame.mcus_x {
                if ri > 0 && togo == 0 {
                    if !bits.consume_rst(rst) {
                        return Err(Error::BadValue("missing restart marker"));
                    }
                    rst = (rst + 1) & 7;
                    togo = ri;
                    preds.iter_mut().for_each(|p| *p = 0);
                }
                for (si, &ci) in scan.iter().enumerate() {
                    let comp = &frame.comps[ci];
                    let (bw, h, v) = (comp.blocks_w, comp.h, comp.v);
                    for by in 0..v {
                        for bx in 0..h {
                            let bi = (my * v + by) * bw + mx * h + bx;
                            decode_block(
                                &mut bits,
                                tables_v[si].0,
                                tables_v[si].1,
                                &mut preds[si],
                                &mut frame.comps[ci].coefs[bi * 64..bi * 64 + 64],
                            )?;
                        }
                    }
                }
                togo = togo.saturating_sub(1);
            }
        }
    }

    p.seek(bits.pos);
    Ok(())
}

/// One sequential block: DC difference + run/size AC symbols.
fn decode_block(
    bits: &mut Bits<'_>,
    dctab: &Table,
    actab: &Table,
    pred: &mut i32,
    block: &mut [i32],
) -> Result<()> {
    // DC: Huffman size, then `extend`ed difference added to the
    // prediction. `wrapping_add` mirrors libjpeg's accumulator on
    // hostile streams (documented divergence-free for legal inputs).
    let s = dctab.decode(bits)?;
    if s > 15 {
        return Err(Error::BadValue("DC coefficient size > 15"));
    }
    let diff = bits.extend(u32::from(s))?;
    *pred = pred.wrapping_add(diff);
    block[0] = *pred;

    // AC.
    let mut k = 1usize;
    while k < 64 {
        let rs = actab.decode(bits)?;
        let r = (rs >> 4) as usize;
        let s = rs & 15;
        if s != 0 {
            k += r;
            let v = bits.extend(u32::from(s))?;
            // k ≤ 63 + 15 = 78 past the guard; libjpeg's extended
            // natural-order table maps the overflow tail to slot 63.
            block[ZIGZAG[k.min(63)] as usize] = v;
            k += 1;
        } else if r != 15 {
            break; // EOB
        } else {
            k += 16; // ZRL: 16 zeros
        }
    }
    Ok(())
}

/// Component indices of this scan in scan order.
fn parse_sos(sos: &[u8], frame: &mut Frame, progressive: bool) -> Result<Vec<usize>> {
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
    let ss = sos[1 + 2 * ns];
    let se = sos[2 + 2 * ns];
    let ahal = sos[3 + 2 * ns];

    if !progressive {
        if ss != 0 || se != 63 || ahal != 0 {
            return Err(Error::BadValue(
                "sequential scan with nonstandard Ss/Se/Ah/Al",
            ));
        }
        if ns > 1 && frame.comps.len() == 1 {
            return Err(Error::BadValue("interleaved scan in 1-component frame"));
        }
    }
    Ok(order)
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
