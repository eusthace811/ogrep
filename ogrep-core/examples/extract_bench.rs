//! Single-threaded extraction cost per MB: trigrams only vs every index key.
//! Usage: cargo run --release -p ogrep-core --example extract_bench -- CORPUS
use std::time::Instant;

fn main() {
    let root = std::env::args().nth(1).expect("corpus path");
    let out = std::process::Command::new("git")
        .args(["ls-files", "-z"])
        .current_dir(&root)
        .output()
        .unwrap();
    let files: Vec<Vec<u8>> = out
        .stdout
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .filter_map(|n| std::fs::read(std::path::Path::new(&root).join(String::from_utf8_lossy(n).as_ref())).ok())
        .filter(|d| d.len() < 4 << 20 && !ogrep_core::trigram::is_binary(d))
        .collect();
    let mb = files.iter().map(Vec::len).sum::<usize>() as f64 / 1e6;
    for round in 0..2 {
        let t = Instant::now();
        let mut n = 0usize;
        for f in &files {
            n += ogrep_core::trigram::extract_merged_masks(f).len();
        }
        let tri = t.elapsed();
        let t = Instant::now();
        let mut m = 0usize;
        for f in &files {
            m += ogrep_core::trigram::extract_index_masks(f).len();
        }
        let all = t.elapsed();
        let t = Instant::now();
        let mut g = 0usize;
        for f in &files {
            ogrep_core::sparse::for_each_gram(f.len(), |i| f[i].to_ascii_lowercase(), |s, e| {
                g += ogrep_core::sparse::key_folding(&f[s..e]) as usize & 1;
            });
        }
        let grams_only = t.elapsed();
        println!(
            "round {round}: {mb:.0} MB  trigrams {:.0} MB/s ({n} keys)  all keys {:.0} MB/s ({m} keys)  gram enumeration+hash only {:.0} MB/s ({g})",
            mb / tri.as_secs_f64(),
            mb / all.as_secs_f64(),
            mb / grams_only.as_secs_f64()
        );
    }
}
