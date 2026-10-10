//! Opening files under a root without following a link at any level.
//!
//! The server, local indexed searches, the graph and the Git-path features
//! read files by a name the index or Git recorded. A directory or file that is
//! (or has since become) a link must not hand back content from outside the
//! tree, which the walker's `follow_links(false)` promises.

use std::path::{Path, PathBuf};

/// Open a file without following a final symlink, so the handle is the path
/// itself rather than wherever it points.
///
/// Only the last component. For a path whose ancestors are not already trusted,
/// use [`open_within_root`] — which on unix has no use for this, since it
/// resolves the final component the same way as every other one.
#[cfg(not(unix))]
fn open_no_follow(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // Opens the reparse point rather than its target. Unlike O_NOFOLLOW
        // this succeeds, so the caller's `is_file` check on the handle's
        // metadata is what rejects it — a reparse point is not a regular file.
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .open(path)
    }
    #[cfg(not(any(unix, windows)))]
    {
        std::fs::File::open(path)
    }
}

/// Open a file under `root` without traversing a symlink at *any* level.
///
/// Refusing to follow the final component is not enough. A path arrives here
/// from an event, a replay or the index as a name, and `root/a/file` reads the
/// same whether `a` is a directory or a link to one — so an intermediate link
/// is enough to hand back a file outside the tree, which is exactly the
/// containment the walker's `follow_links(false)` promises and the index's
/// contract depends on.
///
/// `root` itself is the trust anchor and is opened normally: it is the
/// directory the user asked us to search, so a link there is theirs to have.
/// For many files under one root, open a [`RootDir`] once instead.
pub fn open_within_root(root: &Path, path: &Path) -> std::io::Result<std::fs::File> {
    RootDir::open(root)?.open_file(path)
}

/// A root opened once, for opening many files below it with
/// [`open_within_root`]'s guarantee.
///
/// On unix this is race-free. Each name is resolved against the handle for the
/// root, never re-resolved, so there is no window in which a directory can be
/// swapped for a link between a check and the use.
pub struct RootDir {
    root: PathBuf,
    #[cfg(unix)]
    dir: std::fs::File,
    /// The root's own resolved path, since it may itself sit under a junction
    /// or a substituted drive — comparing against the path as given would
    /// then reject every file in the tree. `canonicalize` is the same
    /// `GetFinalPathNameByHandleW` query underneath, so the two agree on
    /// verbatim prefix and casing.
    #[cfg(windows)]
    anchor: PathBuf,
}

impl RootDir {
    pub fn open(root: &Path) -> std::io::Result<Self> {
        Ok(Self {
            root: root.to_path_buf(),
            #[cfg(unix)]
            dir: std::fs::File::open(root)?,
            #[cfg(windows)]
            anchor: std::fs::canonicalize(root)?,
        })
    }

    /// Open `path`, which must lie below the root, refusing a link anywhere
    /// on the way.
    #[cfg(unix)]
    pub fn open_file(&self, path: &Path) -> std::io::Result<std::fs::File> {
        let components = relative_components(&self.root, path)?;
        #[cfg(target_os = "linux")]
        if let Some(opened) = self.open_beneath(&components) {
            return opened;
        }
        self.open_by_component(&components)
    }

    /// The whole path in one `openat2` call, with the kernel refusing any link
    /// and any escape from the root. `None` when the call is unavailable (an
    /// old kernel, or a sandbox that filters it), for the caller to fall back.
    #[cfg(target_os = "linux")]
    fn open_beneath(
        &self,
        components: &[std::ffi::OsString],
    ) -> Option<std::io::Result<std::fs::File>> {
        use std::os::fd::{AsRawFd, FromRawFd};
        use std::os::unix::ffi::OsStrExt;
        use std::sync::atomic::{AtomicBool, Ordering};

        static UNSUPPORTED: AtomicBool = AtomicBool::new(false);
        if UNSUPPORTED.load(Ordering::Relaxed) {
            return None;
        }
        let mut joined = Vec::new();
        for component in components {
            if !joined.is_empty() {
                joined.push(b'/');
            }
            joined.extend_from_slice(component.as_bytes());
        }
        // A NUL is reported by the component walk.
        let name = std::ffi::CString::new(joined).ok()?;
        // SAFETY: `open_how` is plain data, and all-zero is its documented
        // default for every field not set below.
        let mut how: libc::open_how = unsafe { std::mem::zeroed() };
        how.flags = (libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK) as u64;
        how.resolve =
            libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_MAGICLINKS;
        // SAFETY: `dir` is a live directory descriptor, `name` is
        // NUL-terminated, and `how` is passed with its own size.
        let fd = unsafe {
            libc::syscall(
                libc::SYS_openat2,
                self.dir.as_raw_fd(),
                name.as_ptr(),
                &how as *const libc::open_how,
                std::mem::size_of::<libc::open_how>(),
            )
        };
        if fd >= 0 {
            // SAFETY: `openat2` returned a fresh descriptor owned here.
            return Some(Ok(unsafe { std::fs::File::from_raw_fd(fd as i32) }));
        }
        let error = std::io::Error::last_os_error();
        match error.raw_os_error() {
            Some(libc::ENOSYS) => {
                UNSUPPORTED.store(true, Ordering::Relaxed);
                None
            }
            // What seccomp filters answer for a call they do not allow.
            Some(libc::EPERM) => None,
            _ => Some(Err(error)),
        }
    }

    /// Each component with `openat` against the handle for its parent.
    #[cfg(unix)]
    fn open_by_component(
        &self,
        components: &[std::ffi::OsString],
    ) -> std::io::Result<std::fs::File> {
        use std::io::{Error, ErrorKind};
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
        use std::os::unix::ffi::OsStrExt;

        let mut dir: Option<OwnedFd> = None;
        let last = components.len() - 1;
        for (i, component) in components.iter().enumerate() {
            let name = std::ffi::CString::new(component.as_bytes()).map_err(|_| {
                Error::new(ErrorKind::InvalidInput, "path component contains a NUL")
            })?;
            // `O_DIRECTORY` on the intermediates so a *file* in the middle of
            // the path fails here rather than at the next `openat`, and
            // `O_NOFOLLOW` on every one of them, including the last.
            // `O_NONBLOCK` so a FIFO swapped in cannot stall the open.
            let mut flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
            if i != last {
                flags |= libc::O_DIRECTORY;
            }
            let parent = dir.as_ref().map_or(self.dir.as_raw_fd(), |d| d.as_raw_fd());
            // SAFETY: `parent` is a live directory descriptor, and `name` is a
            // NUL-terminated single path component that outlives the call.
            let fd = unsafe { libc::openat(parent, name.as_ptr(), flags) };
            if fd < 0 {
                return Err(Error::last_os_error());
            }
            // SAFETY: `openat` returned a fresh owned descriptor. Assigning it
            // drops the previous one, closing the parent we no longer need.
            dir = Some(unsafe { OwnedFd::from_raw_fd(fd) });
        }
        Ok(std::fs::File::from(dir.expect("at least one component")))
    }

    /// Windows has no `openat`, so containment is established after the fact
    /// instead of during resolution: the file is opened without following a
    /// final reparse point, and the *handle* is then asked where it actually
    /// ended up. Anything that is not under the root's own resolved path is
    /// refused.
    ///
    /// This is race-free in the way that matters. Checking each ancestor by
    /// path first would only reject a junction that happened to be there at
    /// the time of the check — one substituted between the check and the open
    /// would still be followed. Asking the handle removes the second lookup
    /// entirely: whatever the open resolved through, the answer describes the
    /// object we are actually holding.
    #[cfg(windows)]
    pub fn open_file(&self, path: &Path) -> std::io::Result<std::fs::File> {
        use std::io::{Error, ErrorKind};

        // Rejects escapes and non-literal components before anything is opened.
        relative_components(&self.root, path)?;
        let file = open_no_follow(path)?;
        let opened = final_path_of(&file)?;
        if !opened.starts_with(&self.anchor) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "path resolves outside the served root",
            ));
        }
        Ok(file)
    }

    /// A fallback for platforms that are neither unix nor Windows, where there
    /// is no way to do better than refusing a link that is actually there.
    #[cfg(not(any(unix, windows)))]
    pub fn open_file(&self, path: &Path) -> std::io::Result<std::fs::File> {
        use std::io::{Error, ErrorKind};

        let components = relative_components(&self.root, path)?;
        let mut walked = self.root.clone();
        for component in &components[..components.len() - 1] {
            walked.push(component);
            let meta = std::fs::symlink_metadata(&walked)?;
            if meta.file_type().is_symlink() {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "path traverses a symlink",
                ));
            }
            if !meta.is_dir() {
                return Err(Error::new(ErrorKind::NotADirectory, "not a directory"));
            }
        }
        open_no_follow(path)
    }
}

/// Where an open handle actually is, with every reparse point on the way
/// resolved.
#[cfg(windows)]
pub fn final_path_of(file: &std::fs::File) -> std::io::Result<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_NAME_NORMALIZED, GetFinalPathNameByHandleW, VOLUME_NAME_DOS,
    };

    let handle = file.as_raw_handle() as isize;
    let mut buf = vec![0u16; 512];
    loop {
        // SAFETY: `handle` is a live file handle borrowed from `file`, and the
        // buffer's length is passed as its capacity in `u16`s.
        let needed = unsafe {
            GetFinalPathNameByHandleW(
                handle as _,
                buf.as_mut_ptr(),
                buf.len() as u32,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            )
        };
        if needed == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // The return value excludes the NUL when it fits and includes it when
        // it does not, so a value at or past the capacity means "too small".
        if (needed as usize) < buf.len() {
            buf.truncate(needed as usize);
            return Ok(PathBuf::from(std::ffi::OsString::from_wide(&buf)));
        }
        buf.resize(needed as usize + 1, 0);
    }
}

/// `path` split into the literal components below `root`.
///
/// Anything that is not a plain name — `..`, a root, a prefix — is refused
/// rather than interpreted, since resolving those is the whole business
/// [`open_within_root`] is avoiding.
fn relative_components(root: &Path, path: &Path) -> std::io::Result<Vec<std::ffi::OsString>> {
    use std::io::{Error, ErrorKind};

    let rel = path
        .strip_prefix(root)
        .map_err(|_| Error::new(ErrorKind::InvalidInput, "path is outside the served root"))?;
    let mut components = Vec::new();
    for component in rel.components() {
        match component {
            std::path::Component::Normal(name) => components.push(name.to_os_string()),
            _ => {
                return Err(Error::new(
                    ErrorKind::InvalidInput,
                    "path has a non-literal component",
                ));
            }
        }
    }
    if components.is_empty() {
        return Err(Error::new(ErrorKind::InvalidInput, "path is the root"));
    }
    Ok(components)
}

/// Read `rel`, a `/`-separated path below `root` as the index or Git records
/// it, refusing a link at any level just as the walker would.
pub fn read_indexed(root: &Path, rel: &str) -> std::io::Result<Vec<u8>> {
    use std::io::Read;

    let mut file = open_within_root(root, &crate::hybrid::join_indexed_path(root, rel))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// [`read_indexed`] for UTF-8 text, failing on anything else as
/// `std::fs::read_to_string` does.
pub fn read_indexed_to_string(root: &Path, rel: &str) -> std::io::Result<String> {
    String::from_utf8(read_indexed(root, rel)?)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// The metadata the eligibility check uses and the bytes that get indexed
    /// have to describe the same object, which means one handle.
    #[test]
    fn open_within_root_reads_a_regular_file() {
        use std::io::Read;

        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        let path = tmp.path().join("src").join("plain.rs");
        std::fs::write(&path, "fn main() {}\n").unwrap();

        let file = open_within_root(tmp.path(), &path).expect("a regular file opens");
        let meta = file.metadata().expect("metadata off the handle");
        assert!(meta.is_file());
        assert_eq!(meta.len(), 13);

        let mut data = String::new();
        (&file).read_to_string(&mut data).unwrap();
        assert_eq!(data, "fn main() {}\n");
    }

    /// A symlink must not open as its target, or a link committed to a branch
    /// would pull a file from outside the served root into the index.
    #[cfg(unix)]
    #[test]
    fn open_within_root_refuses_a_symlinked_file() {
        let outside = TempDir::new().unwrap();
        let target = outside.path().join("secret.txt");
        std::fs::write(&target, "sensitive\n").unwrap();

        let root = TempDir::new().unwrap();
        let link = root.path().join("link.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(
            open_within_root(root.path(), &link).is_err(),
            "the link must not open as its target"
        );
    }

    /// And neither must a symlink anywhere *above* the file: `root/a/file`
    /// reads the same whether `a` is a directory or a link to one, so guarding
    /// only the last component still lets a whole tree in from outside.
    #[cfg(unix)]
    #[test]
    fn open_within_root_refuses_a_symlinked_ancestor() {
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "sensitive\n").unwrap();

        let root = TempDir::new().unwrap();
        let link = root.path().join("a");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        let through_link = link.join("secret.txt");

        // The file at the end of that path is a perfectly ordinary file, and
        // opening it by name works — which is the point.
        assert!(std::fs::File::open(&through_link).is_ok());
        assert!(
            open_within_root(root.path(), &through_link).is_err(),
            "an intermediate symlink must not be traversed"
        );
    }

    /// Nothing may be resolved that could climb back out of the root.
    #[test]
    fn open_within_root_refuses_paths_that_escape_or_are_not_literal() {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        std::fs::write(tmp.path().join("src").join("a.rs"), "x\n").unwrap();

        assert!(
            open_within_root(tmp.path(), tmp.path()).is_err(),
            "the root"
        );
        assert!(
            open_within_root(tmp.path(), &tmp.path().join("..").join("a.rs")).is_err(),
            "a parent component"
        );
        assert!(
            open_within_root(&tmp.path().join("src"), &tmp.path().join("src")).is_err(),
            "outside the given root"
        );
    }

    /// The helper the index, graph and Git-path readers share: a plain read
    /// below the root works, and a link anywhere on the way is refused.
    #[cfg(unix)]
    #[test]
    fn read_indexed_reads_plain_files_and_refuses_links() {
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "sensitive\n").unwrap();
        let root = TempDir::new().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/a.rs"), "fn a() {}\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            root.path().join("src/b.rs"),
        )
        .unwrap();

        assert_eq!(
            read_indexed_to_string(root.path(), "src/a.rs").unwrap(),
            "fn a() {}\n"
        );
        assert!(read_indexed(root.path(), "linked/secret.txt").is_err());
        assert!(read_indexed(root.path(), "src/b.rs").is_err());
    }

    /// Linux resolves with `openat2`, so the per-component walk that older
    /// kernels and other unixes use has to be checked on its own.
    #[cfg(unix)]
    #[test]
    fn component_walk_reads_plain_files_and_refuses_links() {
        let outside = TempDir::new().unwrap();
        std::fs::write(outside.path().join("secret.txt"), "sensitive\n").unwrap();
        let root = TempDir::new().unwrap();
        std::fs::create_dir(root.path().join("src")).unwrap();
        std::fs::write(root.path().join("src/a.rs"), "fn a() {}\n").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("secret.txt"),
            root.path().join("src/b.rs"),
        )
        .unwrap();

        let rooted = RootDir::open(root.path()).unwrap();
        let walk = |rel: &str| {
            let components: Vec<_> = rel.split('/').map(std::ffi::OsString::from).collect();
            rooted.open_by_component(&components)
        };
        assert!(walk("src/a.rs").is_ok());
        assert!(walk("linked/secret.txt").is_err());
        assert!(walk("src/b.rs").is_err());
        assert!(
            walk("src/a.rs/x").is_err(),
            "a file in the middle of the path"
        );
    }
}
