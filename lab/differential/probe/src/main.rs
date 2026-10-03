//! diff-probe — lab-only dumper for the differential lane.
//!
//! Prints one JSON object per input file to stdout with the decoded
//! intermediates the public CLI does not expose. When `--dump-dir` is
//! given, raw decoded planes/PCM/text are also written as files so the
//! Python side can byte-compare decoded content:
//!
//!   <name>.rgb      canonical RGB u8 plane (images)
//!   <name>.pcm      interleaved i32 LE PCM (audio)
//!   <name>.mono     mono i32 LE + `<name>.rate` header (audio)
//!   <name>.canon    canonical UTF-8 text (text/pdf)
//!   <name>.fNNN.y   luma plane of decoded frame N (video)
//!   <name>.fNNN.u   cb plane, <name>.fNNN.v  cr plane
//!   <name>.zip/…    extracted member payloads (zip/docx)

#![forbid(unsafe_code)]

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use modhash_raster::{Image, Layout, Sample};

// ------------------------------------------------------------------ utils

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn hex32(d: &modhash_primitives::Digest<32>) -> String {
    let mut s = String::with_capacity(64);
    for b in d.as_bytes() {
        let _ = write!(s, "{b:02x}");
    }
    s
}

fn sha(data: &[u8]) -> modhash_primitives::Digest<32> {
    modhash_primitives::sha256(data).expect("sha256 never fails on slices")
}

fn dump(dir: &Option<PathBuf>, name: &str, data: &[u8]) {
    if let Some(d) = dir {
        let p = d.join(name);
        if let Some(parent) = p.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(&p, data);
    }
}

// ----------------------------------------------- facade helper replicas

/// `modhash::image_to_rgb8` verbatim: gray replicates, alpha dropped,
/// `u16` halves by `>> 8`.
fn to_rgb8<L: Layout, T: Sample>(img: &Image<L, T>) -> (u32, u32, Vec<u8>) {
    let c = L::CHANNELS;
    let n = img.width() as usize * img.height() as usize;
    let src = img.as_slice();
    let hi_byte = T::MAX_U64 != 255;
    let u8of = |v: &T| -> u8 {
        if hi_byte {
            (v.to_u64() >> 8) as u8
        } else {
            v.to_u64() as u8
        }
    };
    let mut px = Vec::with_capacity(n * 3);
    for i in 0..n {
        let p = &src[i * c..i * c + c];
        if c == 1 {
            let g = u8of(&p[0]);
            px.extend_from_slice(&[g, g, g]);
        } else {
            px.extend_from_slice(&[u8of(&p[0]), u8of(&p[1]), u8of(&p[2])]);
        }
    }
    (img.width(), img.height(), px)
}

/// `modhash::to_mono` verbatim: `(Σ_c s) / ch` in i64, trunc toward zero.
fn to_mono(samples: &[i32], channels: u16) -> Vec<i32> {
    let ch = usize::from(channels.max(1));
    samples
        .chunks_exact(ch)
        .map(|f| (f.iter().map(|&s| i64::from(s)).sum::<i64>() / ch as i64) as i32)
        .collect()
}

fn i32_le_bytes(samples: &[i32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 4);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

// --------------------------------------------------------------- images

fn image_rgb8(bytes: &[u8], format: modhash::Format) -> Result<(u32, u32, Vec<u8>), String> {
    let e = |x: modhash_primitives::Error| format!("{x}");
    match format {
        modhash::Format::Png => {
            let png = modhash_png::decode(bytes, &modhash_png::Limits::default()).map_err(e)?;
            Ok(match png.pixels() {
                modhash_png::Pixels::Gray8(i) => to_rgb8(i),
                modhash_png::Pixels::Rgb8(i) => to_rgb8(i),
                modhash_png::Pixels::Rgba8(i) => to_rgb8(i),
                modhash_png::Pixels::Gray16(i) => to_rgb8(i),
                modhash_png::Pixels::Rgb16(i) => to_rgb8(i),
                modhash_png::Pixels::Rgba16(i) => to_rgb8(i),
            })
        }
        modhash::Format::Jpeg => match modhash_jpeg::decode(bytes).map_err(e)? {
            modhash_jpeg::Jpeg::Gray(i) => Ok(to_rgb8(&i)),
            modhash_jpeg::Jpeg::Rgb(i) => Ok(to_rgb8(&i)),
        },
        modhash::Format::Bmp => Ok(to_rgb8(&modhash_bmp::decode(bytes).map_err(e)?)),
        other => Err(format!("not an image container: {}", other.as_str())),
    }
}

// --------------------------------------------------------------- audio

struct Pcm {
    samples: Vec<i32>,
    channels: u16,
    rate: u32,
    bits: u32,
}

fn audio_pcm(bytes: &[u8], format: modhash::Format) -> Result<Pcm, String> {
    let e = |x: modhash_primitives::Error| format!("{x}");
    match format {
        modhash::Format::Wav => {
            let w = modhash_wav::decode(bytes).map_err(e)?;
            Ok(Pcm {
                samples: w.samples().to_vec(),
                channels: w.channels(),
                rate: w.sample_rate(),
                bits: w.bits_per_sample() as u32,
            })
        }
        modhash::Format::Flac => {
            let f = modhash_flac::decode(bytes, &modhash_flac::Limits::default()).map_err(e)?;
            Ok(Pcm {
                samples: f.samples().to_vec(),
                channels: f.channels(),
                rate: f.sample_rate(),
                bits: f.bits_per_sample() as u32,
            })
        }
        modhash::Format::Mp3 => {
            let m = modhash_mp3::decode(bytes, &modhash_mp3::Limits::default()).map_err(e)?;
            Ok(Pcm {
                samples: m.samples,
                channels: m.channels,
                rate: m.sample_rate,
                bits: 0,
            })
        }
        other => Err(format!("not an audio container: {}", other.as_str())),
    }
}

// --------------------------------------------------------------- video

/// Minimal avcC reader: SPS/PPS NALs + length-prefix size. Mirrors
/// `modhash-video`'s private `avcc::parse_avcc` output shape.
fn parse_avcc(rec: &[u8]) -> Result<(Vec<Vec<u8>>, Vec<Vec<u8>>, usize), String> {
    if rec.len() < 7 {
        return Err("avcC too short".into());
    }
    let nal_size = usize::from(rec[4] & 0x03) + 1;
    let mut pos = 5usize;
    let n_sps = usize::from(rec[pos] & 0x1f);
    pos += 1;
    let mut sps = Vec::new();
    for _ in 0..n_sps {
        if pos + 2 > rec.len() {
            return Err("avcC truncated sps".into());
        }
        let n = usize::from(u16::from_be_bytes([rec[pos], rec[pos + 1]]));
        pos += 2;
        if pos + n > rec.len() {
            return Err("avcC truncated sps data".into());
        }
        sps.push(rec[pos..pos + n].to_vec());
        pos += n;
    }
    if pos >= rec.len() {
        return Err("avcC truncated pps count".into());
    }
    let n_pps = usize::from(rec[pos]);
    pos += 1;
    let mut pps = Vec::new();
    for _ in 0..n_pps {
        if pos + 2 > rec.len() {
            return Err("avcC truncated pps".into());
        }
        let n = usize::from(u16::from_be_bytes([rec[pos], rec[pos + 1]]));
        pos += 2;
        if pos + n > rec.len() {
            return Err("avcC truncated pps data".into());
        }
        pps.push(rec[pos..pos + n].to_vec());
        pos += n;
    }
    Ok((sps, pps, nal_size))
}

struct Decoded {
    frames: Vec<(i64, modhash_h264::Frame)>,
    coding: String,
}

/// Decodes the video track's samples to Annex-B once, mirroring
/// `modhash_video::decode`'s plumbing (sample_to_annexb + lead-in).
fn decode_video_track(input: &[u8]) -> Result<Decoded, String> {
    let e = |x: modhash_primitives::Error| format!("{x}");
    let mp4 = modhash_mp4::demux(input).map_err(e)?;
    let track = mp4
        .video_track()
        .ok_or_else(|| "mp4 has no video track".to_string())?;
    let mut coding = "none".to_string();
    for entry in &track.table.description.entries {
        if let modhash_mp4::EntryKind::Visual { coding: c, .. } = entry {
            coding = String::from_utf8_lossy(c).to_string();
            break;
        }
    }
    let rec = track
        .avcc()
        .ok_or_else(|| "avc sample entry without avcC".to_string())?
        .to_vec();
    let (sps_nals, pps_nals, nal_size) = parse_avcc(&rec)?;

    let mut lead = Vec::new();
    for nal in sps_nals.iter().chain(pps_nals.iter()) {
        lead.extend_from_slice(&[0, 0, 0, 1]);
        lead.extend_from_slice(nal);
    }

    let mut samples = Vec::new();
    for s in track.samples() {
        samples.push(s.map_err(e)?);
    }
    let mut stream = lead;
    let mut dts: Vec<i64> = Vec::new();
    for (i, s) in samples.iter().enumerate() {
        let payload = track.sample_bytes(input, i).map_err(e)?;
        if payload.is_empty() {
            continue;
        }
        let mut pos = 0usize;
        while pos < payload.len() {
            if pos + nal_size > payload.len() {
                return Err("avc NAL length prefix truncated".into());
            }
            let n = match nal_size {
                1 => usize::from(payload[pos]),
                2 => usize::from(u16::from_be_bytes([payload[pos], payload[pos + 1]])),
                4 => u32::from_be_bytes([
                    payload[pos],
                    payload[pos + 1],
                    payload[pos + 2],
                    payload[pos + 3],
                ]) as usize,
                _ => return Err("avcC NAL length size".into()),
            };
            pos += nal_size;
            if pos + n > payload.len() {
                return Err("avc NAL payload truncated".into());
            }
            stream.extend_from_slice(&[0, 0, 0, 1]);
            stream.extend_from_slice(&payload[pos..pos + n]);
            pos += n;
        }
        dts.push(s.presentation);
    }
    let frames = modhash_h264::decode(&stream).map_err(e)?;
    Ok(Decoded {
        frames: frames
            .into_iter()
            .enumerate()
            .map(|(i, f)| (*dts.get(i).unwrap_or(&(i as i64)), f))
            .collect(),
        coding,
    })
}

// ---------------------------------------------------------------- main

fn main() {
    let mut args = std::env::args().skip(1);
    let mut dump_dir: Option<PathBuf> = None;
    let mut path: Option<String> = None;
    while let Some(a) = args.next() {
        if a == "--dump-dir" {
            dump_dir = args.next().map(PathBuf::from);
        } else {
            path = Some(a);
        }
    }
    let Some(path) = path else {
        eprintln!("usage: diff-probe <file> [--dump-dir DIR]");
        std::process::exit(2);
    };
    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            println!("{{\"file\": {}, \"error\": {}}}", json_escape(&path), json_escape(&format!("read: {e}")));
            std::process::exit(2);
        }
    };
    let det = modhash::detect(&bytes);
    let stem = Path::new(&path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("out")
        .to_string();

    let mut j = String::from("{");
    let _ = write!(j, "\"file\": {}, \"format\": \"{}\", \"modality\": \"{}\"",
                   json_escape(&path), det.format.as_str(), det.modality.as_str());

    // tier-1 + signature via the facade (its refusals are part of the run).
    match modhash::content_hash(&bytes) {
        Ok(d) => {
            let _ = write!(j, ", \"tier1\": \"{}\"", hex32(&d));
        }
        Err(e) => {
            let _ = write!(j, ", \"tier1_error\": {}", json_escape(&format!("{e}")));
        }
    }
    match modhash::signature(&bytes) {
        Ok(sig) => match &sig {
            modhash::Signature::Image(p) => {
                let _ = write!(j, ", \"phash\": \"{p:016x}\"");
            }
            modhash::Signature::Text(t) => {
                let head: Vec<String> = t.iter().map(|w| format!("{w:016x}")).collect();
                let _ = write!(j, ", \"minhash\": [{}]",
                               head.iter().map(|h| format!("\"{h}\"")).collect::<Vec<_>>().join(","));
            }
            modhash::Signature::Binary(_) => {}
            modhash::Signature::Audio(a) => {
                let _ = write!(j, ", \"audio_sig\": {{\"frames\": {}, \"peaks\": {}}}",
                               a.frames(), a.peaks().len());
            }
            modhash::Signature::Video(v) => {
                let hs: Vec<String> =
                    v.frame_hashes.iter().map(|h| format!("\"{h:016x}\"")).collect();
                let _ = write!(j, ", \"video\": {{\"frames_sampled\": {}, \"duration\": {:.6}, \"fps_sampled\": {:.4}, \"width\": {}, \"height\": {}, \"sampled_phashes\": [{}], \"digest\": \"{}\"}}",
                               v.frame_hashes.len(), v.duration, v.fps_sampled,
                               v.width, v.height, hs.join(","), hex32(&v.content_digest));
            }
        },
        Err(e) => {
            let _ = write!(j, ", \"signature_error\": {}", json_escape(&format!("{e}")));
        }
    }

    // Decoded-content dumps + hashes.
    match det.modality {
        modhash::Modality::Image => match image_rgb8(&bytes, det.format) {
            Ok((w, h, rgb)) => {
                let _ = write!(j, ", \"rgb8\": {{\"width\": {w}, \"height\": {h}, \"sha256\": \"{}\"}}",
                               hex32(&sha(&rgb)));
                dump(&dump_dir, &format!("{stem}.rgb"), &rgb);
            }
            Err(e) => {
                let _ = write!(j, ", \"rgb8_error\": {}", json_escape(&e));
            }
        },
        modhash::Modality::Audio => match audio_pcm(&bytes, det.format) {
            Ok(p) => {
                let mono = to_mono(&p.samples, p.channels);
                let payload: Vec<u8> = p
                    .rate
                    .to_le_bytes()
                    .iter()
                    .chain((mono.len() as u64).to_le_bytes().iter())
                    .chain(i32_le_bytes(&mono).iter())
                    .copied()
                    .collect();
                let _ = write!(j, ", \"pcm\": {{\"rate\": {}, \"channels\": {}, \"bits\": {}, \"frames\": {}, \"i32_sha256\": \"{}\", \"mono_sha256\": \"{}\", \"tier1_payload_sha256\": \"{}\"}}",
                               p.rate, p.channels, p.bits,
                               p.samples.len() / usize::from(p.channels.max(1)),
                               hex32(&sha(&i32_le_bytes(&p.samples))),
                               hex32(&sha(&i32_le_bytes(&mono))),
                               hex32(&sha(&payload)));
                dump(&dump_dir, &format!("{stem}.pcm"), &i32_le_bytes(&p.samples));
                dump(&dump_dir, &format!("{stem}.mono"), &i32_le_bytes(&mono));
            }
            Err(e) => {
                let _ = write!(j, ", \"pcm_error\": {}", json_escape(&e));
            }
        },
        modhash::Modality::Text => {
            if det.format == modhash::Format::Pdf {
                match modhash_pdf::extract_text(&bytes) {
                    Ok(text) => {
                        let canon = modhash_text::canonicalize(&text);
                        let _ = write!(j, ", \"pdf_text\": {{\"bytes\": {}, \"sha256\": \"{}\", \"canon_sha256\": \"{}\"}}",
                                       text.len(), hex32(&sha(text.as_bytes())),
                                       hex32(&sha(canon.as_bytes())));
                        dump(&dump_dir, &format!("{stem}.pdftext"), text.as_bytes());
                        dump(&dump_dir, &format!("{stem}.canon"), canon.as_bytes());
                    }
                    Err(e) => {
                        let _ = write!(j, ", \"pdf_text_error\": {}", json_escape(&format!("{e}")));
                    }
                }
            } else if let Ok(s) = core::str::from_utf8(&bytes) {
                let canon = modhash_text::canonicalize(s);
                let _ = write!(j, ", \"canon\": {{\"bytes\": {}, \"sha256\": \"{}\", \"words\": {}}}",
                               canon.len(), hex32(&sha(canon.as_bytes())),
                               canon.split_whitespace().count());
                dump(&dump_dir, &format!("{stem}.canon"), canon.as_bytes());
            }
        }
        modhash::Modality::Binary => {
            // FastCDC bounds + per-chunk sha256 (the real signature set).
            match modhash_fastcdc::FastCdc::new(&bytes, 2048, 8192, 32768) {
                Ok(cdc) => {
                    let mut bounds = Vec::new();
                    let mut digests = Vec::new();
                    for c in cdc {
                        bounds.push(format!("[{},{}]", c.offset, c.length));
                        let d = sha(&bytes[c.offset..c.offset + c.length]);
                        digests.push(format!("\"{}\"", hex32(&d)));
                    }
                    let _ = write!(j, ", \"cdc\": {{\"chunks\": [{}], \"digests\": [{}]}}",
                                   bounds.join(","), digests.join(","));
                }
                Err(e) => {
                    let _ = write!(j, ", \"cdc_error\": {}", json_escape(&format!("{e}")));
                }
            }
            if det.format == modhash::Format::Zip {
                match modhash_zip::ZipArchive::new(&bytes) {
                    Ok(z) => {
                        let mut entries = Vec::new();
                        for ent in z.entries() {
                            match z.extract(ent) {
                                Ok(data) => {
                                    entries.push(format!(
                                        "{{\"name\": {}, \"method\": {}, \"len\": {}, \"crc32\": \"{:08x}\", \"sha256\": \"{}\"}}",
                                        json_escape(ent.name()), ent.method(),
                                        data.len(), ent.crc32(), hex32(&sha(&data))));
                                    let safe = ent.name().replace(['/', '\\'], "_");
                                    dump(&dump_dir, &format!("{stem}.zip/{safe}"), &data);
                                }
                                Err(e) => entries.push(format!(
                                    "{{\"name\": {}, \"extract_error\": {}}}",
                                    json_escape(ent.name()),
                                    json_escape(&format!("{e}")))),
                            }
                        }
                        let _ = write!(j, ", \"zip\": {{\"entries\": [{}]}}", entries.join(","));
                    }
                    Err(e) => {
                        let _ = write!(j, ", \"zip_error\": {}", json_escape(&format!("{e}")));
                    }
                }
            }
        }
        modhash::Modality::Video => match decode_video_track(&bytes) {
            Ok(d) => {
                let mut items = Vec::new();
                for (i, (pts, f)) in d.frames.iter().enumerate() {
                    let mut planes = Vec::with_capacity(f.y.len() + f.cb.len() + f.cr.len());
                    planes.extend_from_slice(&f.y);
                    planes.extend_from_slice(&f.cb);
                    planes.extend_from_slice(&f.cr);
                    let ph = modhash_video::frame_phash(f.width, f.height, &f.y)
                        .map(|p| format!("\"{p:016x}\""))
                        .unwrap_or_else(|_| "null".into());
                    items.push(format!(
                        "{{\"i\": {i}, \"pts\": {pts}, \"w\": {}, \"h\": {}, \"y_sha256\": \"{}\", \"yuv_sha256\": \"{}\", \"phash\": {}}}",
                        f.width, f.height, hex32(&sha(&f.y)), hex32(&sha(&planes)), ph));
                    dump(&dump_dir, &format!("{stem}.f{i:03}.y"), &f.y);
                    dump(&dump_dir, &format!("{stem}.f{i:03}.u"), &f.cb);
                    dump(&dump_dir, &format!("{stem}.f{i:03}.v"), &f.cr);
                }
                let _ = write!(j, ", \"decoded\": {{\"coding\": {}, \"frames\": [{}]}}",
                               json_escape(&d.coding), items.join(","));
            }
            Err(e) => {
                let _ = write!(j, ", \"decoded_error\": {}", json_escape(&e));
            }
        },
    }

    // tier-3 facts for images: FAST-9 keypoint count.
    if det.modality == modhash::Modality::Image {
        if let Ok((w, h, rgb)) = image_rgb8(&bytes, det.format) {
            let gray: Vec<u8> = rgb
                .chunks_exact(3)
                .map(|p| {
                    ((299 * u64::from(p[0]) + 587 * u64::from(p[1]) + 114 * u64::from(p[2]) + 500)
                        / 1000)
                        .min(255) as u8
                })
                .collect();
            if let Ok(g) = Image::<modhash_raster::Gray, u8>::from_vec(w, h, gray) {
                let kps = modhash_tier3::orb::fast9(&g).len();
                let _ = write!(j, ", \"orb_keypoints\": {kps}");
            }
        }
    }

    j.push('}');
    println!("{j}");
}
