use modhash_mp3::{decode, Limits};
fn main() {
    let data = std::fs::read("modhash-mp3/tests/fixtures/l3_short.mp3").unwrap();
    let m = decode(&data, &Limits::default()).unwrap();
    let fref = std::fs::read("modhash-mp3/tests/fixtures/l3_short.ref.pcm").unwrap();
    let fr: Vec<i32> = fref.chunks_exact(4).map(|c| i32::from_le_bytes([c[0],c[1],c[2],c[3]])).collect();
    let mref = std::fs::read("lab/mp3/l3_short.mimp3.pcm").unwrap();
    let mr: Vec<i32> = mref.chunks_exact(4).map(|c| i32::from_le_bytes([c[0],c[1],c[2],c[3]])).collect();
    // mine[i] ~ ffmpeg[i - 576 + off(-?)] ... use trim: mine[576+11] ~ fr[?]. earlier: trimmed rmse at off 11 → fr[j]=mine[i] j=i+11; and mine vs minimp3 off 1152 → mr[i+1152]=mine[i] i.e. minimp3 starts 1152 earlier.
    // So mr[k+1152]=mine[k], fr[k-11+576]=mine[k] => fr[k+565]=mr[k+1152] => mr[j]=fr[j+587].
    let mut e=0.0; let mut n=0;
    for i in 2000..10000usize {
        let mi = m.samples[i] as f64/2f64.powi(31);
        let fj = (i as i64 + 587) as usize;  // mine -> ffmpeg idx
        let mj = i + 1152;                  // mine -> minimp3 idx
        let f = fr.get(fj).map(|&x| x as f64/2f64.powi(31)).unwrap_or(0.0);
        let mm = mr.get(mj).map(|&x| x as f64/2f64.powi(31)).unwrap_or(0.0);
        if i < 2020 { println!("{i}: mine {mi:.4} f {f:.4} mm {mm:.4}"); }
        e += (mm - f)*(mm - f); n += 1;
    }
    println!("ffmpeg-vs-minimp3 rmse {:.4}", (e/n as f64).sqrt());
}
