//! Print a literal's plan keys with their posting lengths against an index.
//! Usage: cargo run --release -p ogrep-core --example plan_debug -- INDEX_DIR LITERAL
use ogrep_core::query::{self, PostingSource, QueryPlan};

fn main() {
    let mut args = std::env::args().skip(1);
    let dir = args.next().unwrap();
    let literal = args.next().unwrap();
    let reader = ogrep_core::reader::IndexReader::open(std::path::Path::new(&dir)).unwrap();
    println!("has_sparse_grams {}", reader.has_sparse_grams());
    let plan = query::build_literal_plan(&literal, false);
    let QueryPlan::And(queries) = plan else {
        panic!("not And")
    };
    for q in &queries {
        println!(
            "key {:#010x} gram {} fallback {} len {} dropped {} pos {:?}",
            q.hash,
            ogrep_core::sparse::is_gram_key(q.hash),
            q.fallback,
            reader.posting_len(q.hash),
            reader.gram_dropped(q.hash),
            q.position
        );
    }
    for (s, e, k) in ogrep_core::sparse::literal_grams(literal.as_bytes()) {
        println!("gram {:?} {:#010x}", &literal[s..e], k);
    }
}
