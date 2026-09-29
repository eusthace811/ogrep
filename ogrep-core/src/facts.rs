//! A cache of facts derived from file contents, keyed by content.
//!
//! Parsing a file for its structure takes milliseconds; looking the result up
//! takes microseconds. Facts are keyed by [`ContentId`], a hash of the exact
//! text they were derived from, so an entry can never go stale: an edited file
//! has a new id and simply misses. The store sits beside the index, under
//! `facts/<name>/`, and a new version of an extractor uses a new name.
//!
//! Layout: `data.bin` holds payloads appended one after another; `index.bin`
//! is a sorted array of `(content id: 16 bytes, offset: u64, length: u32)`
//! entries, searched in place through a memory map. Adding entries appends to
//! `data.bin` and rewrites `index.bin` through a temporary file and a rename,
//! under an exclusive lock, so readers always see a complete index whose
//! entries point at bytes already written.

use std::io::Write;
use std::path::{Path, PathBuf};

use memmap2::Mmap;

use crate::Result;
use crate::meta::ContentId;

const ENTRY: usize = 16 + 8 + 4;

/// An open fact store.
pub struct FactStore {
    index: Option<Mmap>,
    data: Option<Mmap>,
}

fn store_dir(index_dir: &Path, name: &str) -> PathBuf {
    index_dir.join("facts").join(name)
}

fn map(path: &Path) -> Option<Mmap> {
    let file = std::fs::File::open(path).ok()?;
    if file.metadata().ok()?.len() == 0 {
        return None;
    }
    // SAFETY: read-only map. `index.bin` is only ever replaced by rename, and
    // `data.bin` only appended to, so mapped bytes never change underneath.
    unsafe { Mmap::map(&file).ok() }
}

impl FactStore {
    /// Open the store named `name` beside `index_dir`. A missing store is an
    /// empty one.
    pub fn open(index_dir: &Path, name: &str) -> Self {
        let dir = store_dir(index_dir, name);
        Self {
            index: map(&dir.join("index.bin")),
            data: map(&dir.join("data.bin")),
        }
    }

    fn entries(&self) -> usize {
        self.index.as_ref().map_or(0, |m| m.len() / ENTRY)
    }

    fn entry(&self, i: usize) -> (&[u8], u64, u32) {
        let index = self.index.as_ref().expect("entries() > 0");
        let e = &index[i * ENTRY..(i + 1) * ENTRY];
        (
            &e[..16],
            u64::from_le_bytes(e[16..24].try_into().unwrap()),
            u32::from_le_bytes(e[24..28].try_into().unwrap()),
        )
    }

    /// The payload stored for `id`.
    pub fn get(&self, id: &ContentId) -> Option<&[u8]> {
        let key = id.as_bytes();
        let (mut lo, mut hi) = (0, self.entries());
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let (k, offset, len) = self.entry(mid);
            match k.cmp(key.as_slice()) {
                std::cmp::Ordering::Less => lo = mid + 1,
                std::cmp::Ordering::Greater => hi = mid,
                std::cmp::Ordering::Equal => {
                    let data = self.data.as_ref()?;
                    let start = usize::try_from(offset).ok()?;
                    return data.get(start..start.checked_add(len as usize)?);
                }
            }
        }
        None
    }

    /// Add payloads to the store named `name` beside `index_dir`. Ids already
    /// present keep their first payload.
    pub fn put(index_dir: &Path, name: &str, mut new: Vec<(ContentId, Vec<u8>)>) -> Result<()> {
        if new.is_empty() {
            return Ok(());
        }
        let dir = store_dir(index_dir, name);
        std::fs::create_dir_all(&dir)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join("lock"))?;
        lock_exclusive(&lock)?;

        // Re-read under the lock: another process may have added entries.
        let current = Self::open(index_dir, name);
        new.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        new.dedup_by(|b, a| a.0 == b.0);
        new.retain(|(id, _)| current.get(id).is_none());
        if new.is_empty() {
            return Ok(());
        }
        let mut data = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("data.bin"))?;
        let mut offset = data.metadata()?.len();
        let mut added: Vec<([u8; 16], u64, u32)> = Vec::with_capacity(new.len());
        let mut buf = Vec::new();
        for (id, payload) in &new {
            buf.extend_from_slice(payload);
            added.push((*id.as_bytes(), offset, payload.len() as u32));
            offset += payload.len() as u64;
        }
        data.write_all(&buf)?;
        data.sync_data()?;

        // Merge the sorted old entries with the sorted new ones.
        let tmp = dir.join("index.bin.tmp");
        let mut out = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        let (mut i, mut j) = (0, 0);
        while i < current.entries() || j < added.len() {
            let take_old = j == added.len()
                || (i < current.entries() && current.entry(i).0 < added[j].0.as_slice());
            let (key, off, len) = if take_old {
                let (k, o, l) = current.entry(i);
                i += 1;
                (<[u8; 16]>::try_from(k).unwrap(), o, l)
            } else {
                j += 1;
                added[j - 1]
            };
            out.write_all(&key)?;
            out.write_all(&off.to_le_bytes())?;
            out.write_all(&len.to_le_bytes())?;
        }
        out.flush()?;
        drop(out);
        drop(current);
        std::fs::rename(&tmp, dir.join("index.bin"))?;
        Ok(())
    }
}

fn lock_exclusive(file: &std::fs::File) -> std::io::Result<()> {
    fs2::FileExt::lock_exclusive(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stores_and_finds_payloads_across_updates() {
        let dir = tempfile::tempdir().unwrap();
        let id = |s: &str| ContentId::from_indexed_bytes(s.as_bytes());
        assert!(FactStore::open(dir.path(), "t").get(&id("a")).is_none());
        FactStore::put(
            dir.path(),
            "t",
            vec![(id("a"), b"AAA".to_vec()), (id("b"), b"B".to_vec())],
        )
        .unwrap();
        FactStore::put(
            dir.path(),
            "t",
            vec![(id("c"), b"CC".to_vec()), (id("a"), b"ignored".to_vec())],
        )
        .unwrap();
        let store = FactStore::open(dir.path(), "t");
        assert_eq!(store.get(&id("a")), Some(&b"AAA"[..]));
        assert_eq!(store.get(&id("b")), Some(&b"B"[..]));
        assert_eq!(store.get(&id("c")), Some(&b"CC"[..]));
        assert!(store.get(&id("d")).is_none());
    }
}
