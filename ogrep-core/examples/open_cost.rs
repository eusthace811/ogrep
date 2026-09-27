use std::time::Instant;
fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).unwrap());
    for _ in 0..3 {
        let t = Instant::now();
        let data = std::fs::read(dir.join("files.bin")).unwrap();
        let t_read = t.elapsed();
        let t = Instant::now();
        let h = blake3::hash(&data);
        let t_hash = t.elapsed();
        let t = Instant::now();
        let r = ogrep_core::reader::IndexReader::open(&dir).unwrap();
        let t_open = t.elapsed();
        let t = Instant::now();
        let v = ogrep_core::path_index::read_filename_index(&dir).unwrap();
        let t_fn = t.elapsed();
        let t = Instant::now();
        let m = ogrep_core::meta::IndexMeta::load(&dir).unwrap();
        let t_meta = t.elapsed();
        println!(
            "read {:?} hash {:?} open(total) {:?} fnindex {:?} meta {:?} ({} {} {})",
            t_read,
            t_hash,
            t_open,
            t_fn,
            t_meta,
            h.as_bytes()[0],
            r.num_files(),
            v.is_some() as u8 + m.complete as u8
        );
    }
}
