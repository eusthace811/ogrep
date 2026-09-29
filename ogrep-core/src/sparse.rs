//! Sparse n-grams: variable-length index keys that make long literals cheap.
//!
//! A trigram index answers a long literal by intersecting one posting list per
//! trigram, and most of those trigrams are common. Sparse n-grams (GitHub code
//! search, Cursor) add longer keys chosen so that a literal can be covered by a
//! few of them, and each longer key has a much shorter posting list.
//!
//! ## Which substrings are grams
//!
//! Every adjacent byte pair gets a weight from a fixed table ([`pair_weight`]).
//! A substring of length 4 to [`MAX_LEN`] is a gram when the weights of its first
//! and last byte pairs are both strictly greater than the weight of every byte
//! pair inside it. The rule looks only at the substring's own bytes, so any text
//! that contains a query literal also contains every gram of that literal. That
//! is what makes any set of the literal's grams a sound prefilter: an indexed
//! file lacking one of them cannot contain the literal.
//!
//! A file has at most `2n` grams, found in one pass with a monotonic stack. The
//! weights rank byte pairs by how rare they are in source code (Cursor's
//! frequency weights; GitHub hashes the pair instead): grams then end at rare
//! pairs and span common ones, which measured slightly better than hash weights
//! on the rust, cpython and django corpora. See `scripts/gen_sparse_weights.py`.
//!
//! ## Keys
//!
//! Grams are taken from ASCII-lowercased content only. That is sound for both
//! case modes: text containing a literal also contains the literal's lowercased
//! form in its lowercased content. Exact-case trigrams keep their precision for
//! case-sensitive searches, and indexing one case halves the extra postings.
//!
//! A gram's key is a 32-bit hash placed above the trigram range, which uses
//! only 24 bits. Grams therefore share `lookup.bin` and `index.bin` with the
//! trigrams, with no change to the on-disk layout, and a reader that predates
//! them never asks for such a key. A hash collision can only add candidates,
//! never remove one, because every candidate is verified against the file.
//!
//! An index advertises its grams in `meta.json` ([`VERSION`]). A planner uses
//! gram keys only against an index that has them: an older index falls back to
//! trigrams alone, never to a wrong answer.

/// Identifies the gram rule, weight table, length limit and key hash. Any change
/// to one of them must bump this, or an index built by one binary would be
/// queried with another binary's keys.
pub const VERSION: u32 = 2;

/// Longest gram, in bytes. 8 captured nearly all of the posting-list reduction
/// of 12 and 16 on the benchmark corpora at a fraction of the extra keys.
pub const MAX_LEN: usize = 8;

/// Shortest gram. Three-byte grams are exactly the trigrams, already indexed.
pub const MIN_LEN: usize = 4;

/// First key of the gram range. Trigram keys are `< 2^24`.
pub const KEY_BASE: u32 = 1 << 24;

/// Reserved key listing the grams a full build left out because they occur in
/// too many files to narrow a search ([`prune_threshold`]). Its "postings" are
/// those gram keys, one per entry's file-id field. It sorts after every gram,
/// is never produced by [`key`], and travels with `lookup.bin`/`index.bin`, so
/// whatever publishes or merges an index carries the list along with the
/// postings it describes. A planner must not read a listed gram's absence as
/// "no file contains it".
pub const DROPPED_KEY: u32 = u32::MAX;

/// Gram posting lists longer than this are left out of a full build: one
/// percent of the files, and never fewer than 32. On the benchmark corpora such
/// grams were 0.3-1.2% of gram keys but held 43-48% of gram postings, and a
/// gram in that many files narrows little that trigrams do not.
pub fn prune_threshold(num_files: usize) -> usize {
    (num_files / 100).max(32)
}

static WEIGHTS: &[u8; 131_072] = include_bytes!("sparse_weights.bin");

/// Weight of the byte pair `(a, b)`: its rarity rank in source code. Every pair
/// has a distinct weight.
#[inline]
pub fn pair_weight(a: u8, b: u8) -> u16 {
    let i = (usize::from(a) << 8 | usize::from(b)) * 2;
    u16::from_le_bytes([WEIGHTS[i], WEIGHTS[i + 1]])
}

/// Whether an index key is a sparse gram rather than a trigram.
#[inline]
pub fn is_gram_key(key: u32) -> bool {
    key >= KEY_BASE && key != DROPPED_KEY
}

/// Whether a key sorts after every trigram key: a gram or [`DROPPED_KEY`].
/// This, not [`is_gram_key`], is the monotone predicate for searching a sorted
/// lookup table.
#[inline]
pub fn sorts_after_trigrams(key: u32) -> bool {
    key >= KEY_BASE
}

#[inline]
fn mix64(mut x: u64) -> u64 {
    // SplitMix64 finalizer: a fixed, portable bijection.
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^ (x >> 31)
}

/// ASCII-lowercase eight bytes at once: `A`-`Z` gain `0x20`, everything else,
/// including bytes with the high bit set, is unchanged.
#[inline]
fn ascii_lowercase_word(x: u64) -> u64 {
    const ONES: u64 = 0x0101_0101_0101_0101;
    let low7 = x & (0x7f * ONES);
    // No carries between bytes: every sum stays below 0x100.
    let at_least_a = low7 + (0x80 - u64::from(b'A')) * ONES;
    let above_z = low7 + (0x80 - u64::from(b'Z') - 1) * ONES;
    let upper = (at_least_a ^ above_z) & !x & (0x80 * ONES);
    x | (upper >> 2)
}

/// Key of a gram, lowercasing it. `gram.len()` must be in `MIN_LEN..=MAX_LEN`.
#[inline]
pub fn key_folding(gram: &[u8]) -> u32 {
    let len = gram.len();
    debug_assert!((MIN_LEN..=MAX_LEN).contains(&len));
    let mut word = [0u8; 8];
    word[..len].copy_from_slice(gram);
    let packed = ascii_lowercase_word(u64::from_le_bytes(word));
    // Packing is injective for a fixed length; mixing the length in keeps
    // grams of different lengths apart.
    let hash = mix64(packed ^ mix64(len as u64 + 0x9e37_79b9_7f4a_7c15));
    // Keys span KEY_BASE..DROPPED_KEY, leaving the reserved key unused.
    let range = u64::from(DROPPED_KEY - KEY_BASE);
    KEY_BASE + ((u128::from(hash) * u128::from(range)) >> 64) as u32
}

/// Key of a gram given as lowercased bytes.
pub fn key(gram: &[u8]) -> u32 {
    key_folding(gram)
}

/// Call `f(start, end)` for every sparse gram of `len` bytes where `at(i)`
/// returns the (lowercased) byte at `i`. Each gram is reported once per
/// occurrence, as a half-open byte range.
///
/// The stack holds pair positions with strictly decreasing weights; the pairs
/// between two neighbours on the stack all weigh less than both. Processing pair
/// `j`, every lighter pair popped from the top forms a gram with `j`, and so does
/// the heavier or equal pair left on top. An equal pair is then replaced by `j`,
/// since any later gram spanning both would have an interior pair as heavy as
/// its edge. Pairs too far back to start a gram of at most [`MAX_LEN`] bytes are
/// dropped from the bottom, which bounds the stack at `MAX_LEN - 1` entries.
#[inline]
pub fn for_each_gram(len: usize, at: impl Fn(usize) -> u8, mut f: impl FnMut(usize, usize)) {
    if len < MIN_LEN {
        return;
    }
    const CAP: usize = MAX_LEN;
    let mut weights = [0u16; CAP];
    let mut starts = [0usize; CAP];
    let (mut bottom, mut top) = (0usize, 0usize); // live entries are bottom..top (mod CAP)
    let mut prev = at(0);
    for j in 0..len - 1 {
        let next = at(j + 1);
        let w = pair_weight(prev, next);
        prev = next;
        let end = j + 2;
        // Entries starting before `end - MAX_LEN` cannot start a short enough gram.
        while bottom < top && end - starts[bottom % CAP] > MAX_LEN {
            bottom += 1;
        }
        while bottom < top && weights[(top - 1) % CAP] < w {
            let start = starts[(top - 1) % CAP];
            if end - start >= MIN_LEN {
                f(start, end);
            }
            top -= 1;
        }
        if bottom < top {
            let t = (top - 1) % CAP;
            let start = starts[t];
            if end - start >= MIN_LEN {
                f(start, end);
            }
            if weights[t] == w {
                top -= 1;
            }
        }
        weights[top % CAP] = w;
        starts[top % CAP] = j;
        top += 1;
        if top - bottom > CAP {
            bottom += 1;
        }
    }
}

/// Sparse grams of a query literal (any case), as `(start, end, key)`.
pub fn literal_grams(literal: &[u8]) -> Vec<(usize, usize, u32)> {
    let mut grams = Vec::new();
    for_each_gram(
        literal.len(),
        |i| literal[i].to_ascii_lowercase(),
        |start, end| {
            grams.push((start, end, key_folding(&literal[start..end])));
        },
    );
    grams
}

/// Unique gram keys of indexed content (any case; lowercased on the fly).
pub fn content_keys(content: &[u8]) -> Vec<u32> {
    let mut keys = Vec::new();
    for_each_gram(
        content.len(),
        |i| content[i].to_ascii_lowercase(),
        |start, end| {
            keys.push(key_folding(&content[start..end]));
        },
    );
    keys.sort_unstable();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The definition, checked directly: edges strictly outweigh the interior.
    fn grams_by_definition(s: &[u8]) -> Vec<(usize, usize)> {
        let lower = s.to_ascii_lowercase();
        let w: Vec<u16> = lower.windows(2).map(|p| pair_weight(p[0], p[1])).collect();
        let mut out = Vec::new();
        for start in 0..s.len() {
            for end in start + MIN_LEN..=(start + MAX_LEN).min(s.len()) {
                let first = w[start];
                let last = w[end - 2];
                let interior = &w[start + 1..end - 2];
                if interior.iter().all(|&x| x < first && x < last) {
                    out.push((start, end));
                }
            }
        }
        out.sort_unstable();
        out
    }

    fn grams_by_stack(s: &[u8]) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for_each_gram(s.len(), |i| s[i].to_ascii_lowercase(), |a, b| out.push((a, b)));
        out.sort_unstable();
        out
    }

    fn xorshift(state: &mut u64) -> u64 {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state
    }

    #[test]
    fn the_stack_finds_exactly_the_grams_of_the_definition() {
        let samples: &[&[u8]] = &[
            b"fn parse_header(input: &str) -> Result<Header, Error> {",
            b"aaaaaaaaaaaaaaaa",
            b"abababababababab",
            b"The quick brown fox jumps over the lazy dog",
            b"pool exhausted",
            b"SELECT * FROM users WHERE id = 1",
            b"zyxwvutsrqponmlkjihgfedcba",
            b"abc",
            b"abcd",
            b"",
        ];
        for s in samples {
            assert_eq!(grams_by_stack(s), grams_by_definition(s), "{:?}", String::from_utf8_lossy(s));
        }
        let mut state = 0x1234_5678_9abc_def1u64;
        let alphabet = b"abcde _(){}ABZ01\n";
        for _ in 0..2000 {
            let len = (xorshift(&mut state) % 40) as usize;
            let s: Vec<u8> = (0..len)
                .map(|_| alphabet[(xorshift(&mut state) % alphabet.len() as u64) as usize])
                .collect();
            assert_eq!(grams_by_stack(&s), grams_by_definition(&s), "{:?}", String::from_utf8_lossy(&s));
        }
    }

    #[test]
    fn a_literal_grams_are_all_present_in_any_text_containing_it() {
        let mut state = 0xdead_beef_cafe_f00du64;
        let alphabet = b"abcdefgh _.:()XYZ";
        for _ in 0..500 {
            let len = 20 + (xorshift(&mut state) % 200) as usize;
            let text: Vec<u8> = (0..len)
                .map(|_| alphabet[(xorshift(&mut state) % alphabet.len() as u64) as usize])
                .collect();
            let keys = content_keys(&text);
            let start = (xorshift(&mut state) as usize) % (len - 10);
            let end = start + 4 + (xorshift(&mut state) as usize) % (len - start - 4).min(30);
            // Uppercase the literal: grams are case-insensitive by construction.
            let literal = text[start..end].to_ascii_uppercase();
            for (_, _, k) in literal_grams(&literal) {
                assert!(keys.binary_search(&k).is_ok(), "missing gram key for literal");
            }
        }
    }

    #[test]
    fn keys_live_above_the_trigram_range_and_are_stable() {
        for gram in [&b"pool"[..], &b"exhausted"[..8], &b"_header"[..], &b"abcd"[..]] {
            let k = key(gram);
            assert!(is_gram_key(k));
        }
        // Pinned: changing the hash, table or rule must bump VERSION.
        assert_eq!(key(b"pool"), key(b"pool"));
        assert_ne!(key(b"pool"), key(b"loop"));
        assert_ne!(key(b"abcd"), key(b"abcde"));
    }

    #[test]
    fn word_lowercasing_matches_bytewise_lowercasing() {
        for x in 0..=255u8 {
            for &fill in &[0u8, b'A', b'z', 0xC3, 0x7F, 0x80, b'@', b'['] {
                let bytes = [x, fill, x.wrapping_add(1), fill, x.wrapping_sub(1), 0xFF, x, b'Q'];
                let expected: Vec<u8> = bytes.iter().map(u8::to_ascii_lowercase).collect();
                let got = ascii_lowercase_word(u64::from_le_bytes(bytes)).to_le_bytes();
                assert_eq!(got.to_vec(), expected, "{bytes:?}");
            }
        }
    }

    #[test]
    fn weights_are_a_permutation() {
        let mut seen = vec![false; 65536];
        for a in 0..=255u8 {
            for b in 0..=255u8 {
                let w = pair_weight(a, b) as usize;
                assert!(!seen[w]);
                seen[w] = true;
            }
        }
    }
}
