//! Annex-B NAL unit splitting and emulation-prevention removal.
//!
//! A H.264 byte stream is a run of start-code-prefixed NAL units
//! (`00 00 01` or `00 00 00 01`). Inside each NAL's payload the encoder
//! inserts `03` after `00 00` whenever the next byte would be `00`,
//! `01`, `02` or `03`, so the RBSP is recovered by stripping every
//! `00 00 03` triple (spec 7.4.1.1).

use alloc::vec::Vec;

use modhash_primitives::{Error, Result};

/// NAL unit type codes (spec Table 7-1). Only the codes the decoder
/// acts on get named constants.
pub(crate) const NAL_SLICE: u8 = 1;
/// DPA — partitioning is a baseline-legal feature this crate declines.
pub(crate) const NAL_DPA: u8 = 2;
/// DPB.
pub(crate) const NAL_DPB: u8 = 3;
/// DPC.
pub(crate) const NAL_DPC: u8 = 4;
/// IDR slice.
pub(crate) const NAL_IDR: u8 = 5;
/// SEI — skipped.
pub(crate) const NAL_SEI: u8 = 6;
/// Sequence parameter set.
pub(crate) const NAL_SPS: u8 = 7;
/// Picture parameter set.
pub(crate) const NAL_PPS: u8 = 8;
/// Access unit delimiter — skipped.
pub(crate) const NAL_AUD: u8 = 9;
/// End of sequence — flushes the last picture.
pub(crate) const NAL_EOSEQ: u8 = 10;
/// End of stream — flushes the last picture.
pub(crate) const NAL_EOSTREAM: u8 = 11;
/// Filler data — skipped.
pub(crate) const NAL_FILLER: u8 = 12;
/// SPS extension.
pub(crate) const NAL_SPS_EXT: u8 = 13;
/// Auxiliary / extension slices and reserved codes this crate declines.
pub(crate) const NAL_CODED_SLICE_AUX: u8 = 19;
/// Slice without partitioning is legal but beyond baseline scope here
/// (extension/slice-aux and scalable types 14/15/20/21 are all routed
/// to [`Error::Unsupported`]).

/// One Annex-B NAL unit: header fields plus the payload span in the
/// source buffer (emulation prevention NOT yet removed).
#[derive(Copy, Clone, Debug)]
pub(crate) struct Nal<'a> {
    /// `nal_ref_idc` (0..=3).
    pub ref_idc: u8,
    /// `nal_unit_type` (0..=31).
    pub unit_type: u8,
    /// Raw NAL payload including emulation-prevention bytes.
    pub payload: &'a [u8],
}

/// Splits an Annex-B stream into NAL units.
///
/// `Err` when the stream contains no start code at all — a raw RBSP
/// without Annex-B framing is not a byte stream. Trailing zero bytes
/// after the last start code (or before the next one) are dropped from
/// each payload, matching how decoders delimit the RBSP.
pub(crate) fn split_annex_b(data: &[u8]) -> Result<Vec<Nal<'_>>> {
    // Collect start-code positions.
    let mut starts: Vec<usize> = Vec::new();
    let mut i = 0usize;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i);
            i += 3;
        } else {
            i += 1;
        }
    }
    if starts.is_empty() {
        return Err(Error::InvalidMagic {
            what: "h264 Annex-B start code",
        });
    }
    let mut out = Vec::with_capacity(starts.len());
    for (s, &sc) in starts.iter().enumerate() {
        let mut begin = sc + 3;
        // zero_byte: a single 0x00 is legal between start_code_prefix
        // and the NAL header (a NAL header is never 0x00 itself).
        if begin < data.len() && data[begin] == 0 {
            begin += 1;
        }
        let end = match starts.get(s + 1) {
            Some(&n) => n,
            None => data.len(),
        };
        // Drop trailing_zero_8bits run ahead of the next start code.
        let mut payload_end = end;
        while payload_end > begin && data[payload_end - 1] == 0 {
            payload_end -= 1;
        }
        let payload = &data[begin..payload_end.max(begin)];
        if payload.is_empty() {
            continue;
        }
        let ref_idc = (payload[0] >> 5) & 0x03;
        let unit_type = payload[0] & 0x1f;
        out.push(Nal {
            ref_idc,
            unit_type,
            payload: &payload[1..],
        });
    }
    Ok(out)
}

/// Removes emulation-prevention bytes from a NAL payload, producing the
/// RBSP. The scan runs from `start` so slice headers can hand their
/// `SODB` view in directly.
pub(crate) fn rbsp(payload: &[u8]) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(payload.len());
    let mut zeros = 0usize;
    for (i, &b) in payload.iter().enumerate() {
        if zeros >= 2 && b == 0x03 {
            // emulation_prevention_three_byte: consume and reset the
            // run so 00 00 03 00 00 03 strips both.
            zeros = 0;
            continue;
        }
        // A start-code prefix inside a payload is a stream error, not
        // silently lost data.
        if zeros >= 2 && b <= 0x02 {
            return Err(Error::BadValue("start code prefix inside NAL payload"));
        }
        zeros = if b == 0 { zeros + 1 } else { 0 };
        out.push(b);
        let _ = i;
    }
    Ok(out)
}
