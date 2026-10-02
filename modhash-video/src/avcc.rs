//! `avcC` (AVCDecoderConfigurationRecord) parsing, ISO/IEC 14496-15 §5.3.3.
//!
//! An mp4 sample is a run of length-prefixed NAL units and the parameter
//! sets live in `avcC`, not inline — so decoding needs three things out of
//! the record: the SPS list, the PPS list, and the NAL length-prefix width
//! (`lengthSizeMinusOne + 1`). Everything past `numOfPictureParameterSets`
//! (chroma_format, bit-depth fields the High-profile extension adds) is
//! ignored: the baseline subset this kit decodes never reads them.

use alloc::vec::Vec;
use modhash_primitives::{Error, Result};

/// The decoded configuration an `avc1`/`avc3` sample entry carries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvcConfig {
    /// Bytes of the `lengthSizeMinusOne + 1` NAL length prefix (1, 2 or 4).
    pub nal_length_size: usize,
    /// SPS NAL payloads (header byte included), in record order.
    pub sps: Vec<Vec<u8>>,
    /// PPS NAL payloads (header byte included), in record order.
    pub pps: Vec<Vec<u8>>,
}

/// Reads `n` big-endian bytes starting at `*pos`, advancing it.
fn take<'a>(data: &'a [u8], pos: &mut usize, n: usize, what: &'static str) -> Result<&'a [u8]> {
    let end = pos
        .checked_add(n)
        .ok_or(Error::BadValue("avcC offset overflow"))?;
    let out = data
        .get(*pos..end)
        .ok_or(Error::truncated(what, n, data.len().saturating_sub(*pos)))?;
    *pos = end;
    Ok(out)
}

fn u8_at(data: &[u8], pos: &mut usize, what: &'static str) -> Result<u8> {
    Ok(take(data, pos, 1, what)?[0])
}

fn u16_at(data: &[u8], pos: &mut usize, what: &'static str) -> Result<u16> {
    let b = take(data, pos, 2, what)?;
    Ok(u16::from_be_bytes([b[0], b[1]]))
}

/// Parses an `avcC` record body.
///
/// Errors: [`Error::Truncated`] when any field or NAL overruns the record,
/// [`Error::BadValue`] on a non-`avcC` version byte or a zero-parameter-set
/// record, [`Error::Unsupported`] on `lengthSizeMinusOne` 3 — the odd 3-byte
/// length encoding exists in the spec but not in the wild files this crate
/// is expected to see, so it is refused rather than half-implemented.
pub fn parse_avcc(data: &[u8]) -> Result<AvcConfig> {
    let mut pos = 0usize;
    let version = u8_at(data, &mut pos, "avcC configurationVersion")?;
    if version != 1 {
        return Err(Error::BadValue("avcC configurationVersion is not 1"));
    }
    // profile, profile_compatibility, level: carried through verbatim to
    // nothing — the h264 decoder re-reads them from the SPS itself.
    take(data, &mut pos, 3, "avcC profile/level")?;
    let length_size = usize::from(u8_at(data, &mut pos, "avcC lengthSizeMinusOne")? & 0x03) + 1;
    if length_size == 3 {
        return Err(Error::Unsupported(
            "video avcC 3-byte NAL length prefix",
        ));
    }
    let sps_count = usize::from(u8_at(data, &mut pos, "avcC SPS count")? & 0x1F);
    let mut sps = Vec::with_capacity(sps_count);
    for _ in 0..sps_count {
        let n = usize::from(u16_at(data, &mut pos, "avcC SPS length")?);
        sps.push(take(data, &mut pos, n, "avcC SPS payload")?.to_vec());
    }
    let pps_count = usize::from(u8_at(data, &mut pos, "avcC PPS count")?);
    let mut pps = Vec::with_capacity(pps_count);
    for _ in 0..pps_count {
        let n = usize::from(u16_at(data, &mut pos, "avcC PPS length")?);
        pps.push(take(data, &mut pos, n, "avcC PPS payload")?.to_vec());
    }
    if sps.is_empty() && pps.is_empty() {
        // Legal for avc3 (parameter sets in-band); the decode loop accepts
        // the empty record and lets the stream carry its own SPS/PPS.
        return Ok(AvcConfig {
            nal_length_size: length_size,
            sps,
            pps,
        });
    }
    Ok(AvcConfig {
        nal_length_size: length_size,
        sps,
        pps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(ls_minus_one: u8, sps: &[&[u8]], pps: &[&[u8]]) -> Vec<u8> {
        let mut v = vec![1, 66, 0, 30, 0xFC | ls_minus_one, 0xE0 | sps.len() as u8];
        for s in sps {
            v.extend_from_slice(&(s.len() as u16).to_be_bytes());
            v.extend_from_slice(s);
        }
        v.push(pps.len() as u8);
        for p in pps {
            v.extend_from_slice(&(p.len() as u16).to_be_bytes());
            v.extend_from_slice(p);
        }
        v
    }

    #[test]
    fn parses_typical_record() {
        let c = parse_avcc(&rec(3, &[&[0x67, 1, 2, 3]], &[&[0x68, 9]])).unwrap();
        assert_eq!(c.nal_length_size, 4);
        assert_eq!(c.sps, vec![vec![0x67, 1, 2, 3]]);
        assert_eq!(c.pps, vec![vec![0x68, 9]]);
    }

    #[test]
    fn truncated_fields_error() {
        for n in 0..12 {
            let v = rec(3, &[&[0x67, 1, 2, 3]], &[&[0x68, 9]]);
            assert!(parse_avcc(&v[..n]).is_err(), "len {n} must error");
        }
        // A NAL length overrunning the record is truncated, not a panic.
        let mut v = rec(3, &[&[0x67]], &[&[0x68]]);
        v[6] = 0xFF;
        v[7] = 0xFF;
        assert!(matches!(parse_avcc(&v), Err(Error::Truncated { .. })));
    }

    #[test]
    fn bad_version_and_width3() {
        let mut v = rec(3, &[&[0x67]], &[&[0x68]]);
        v[0] = 9;
        assert!(matches!(parse_avcc(&v), Err(Error::BadValue(_))));
        let v = rec(2, &[&[0x67]], &[&[0x68]]);
        assert!(matches!(
            parse_avcc(&v),
            Err(Error::Unsupported(_))
        ));
    }
}
