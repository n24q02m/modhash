use modhash_mp3::{decode, Limits};
fn main() {
    for name in ["l3_stereo", "l3_mono", "l3_48k", "l3_short", "l3_joint", "l3_vbr"] {
        let data = std::fs::read(format!("modhash-mp3/tests/fixtures/{name}.mp3")).unwrap();
        let m = decode(&data, &Limits::default()).unwrap();
        let refp = std::fs::read(format!("lab/mp3/{name}.mimp3.pcm")).unwrap();
        let r: Vec<i32> = refp.chunks_exact(4).map(|c| i32::from_le_bytes([c[0],c[1],c[2],c[3]])).collect();
        let mut best = (f64::INFINITY, 0i64);
        for off in -5000i64..5000 {
            let mut e=0.0; let mut n=0;
            for i in (0..m.samples.len()).step_by(3) {
                let j = i as i64 + off;
                if j < 0 || j as usize >= r.len() { continue }
                let d = (m.samples[i] as f64 - r[j as usize] as f64)/2f64.powi(31);
                e += d*d; n += 1;
            }
            if n>2000 { let rmse=(e/n as f64).sqrt(); if rmse<best.0 { best=(rmse,off); } }
        }
        let off=best.1;
        let mut e=0.0; let mut n=0;
        for i in 0..m.samples.len() {
            let j = i as i64 + off;
            if j < 0 || j as usize >= r.len() { continue }
            let d = (m.samples[i] as f64 - r[j as usize] as f64)/2f64.powi(31);
            e += d*d; n += 1;
        }
        println!("{name}: mine {} ref {} off {} rmse-all {:.5} (n{})", m.samples.len(), r.len(), off, (e/n.max(1) as f64).sqrt(), n);
    }
}
