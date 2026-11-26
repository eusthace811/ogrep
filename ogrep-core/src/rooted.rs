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
        // this succeeds, so `RootDir::open_file` checks the handle's metadata.
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
    // One file: its own open reports a root it may not search.
    RootDir::open_unchecked(root)?.open_file(path)
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
    /// Open `root`, refusing one this process may not search: every name
    /// below it would then fail on its own, an answer that looks complete.
    pub fn open(root: &Path) -> std::io::Result<Self> {
        let rooted = Self::open_unchecked(root)?;
        #[cfg(unix)]
        refuse_unsearchable(&rooted.dir)?;
        Ok(rooted)
    }

    fn open_unchecked(root: &Path) -> std::io::Result<Self> {
        Ok(Self {
            root: root.to_path_buf(),
            #[cfg(unix)]
            dir: open_search_only(root)?,
            #[cfg(windows)]
            anchor: std::fs::canonicalize(root)?,
        })
    }

    /// Open `path`, which must lie below the root, refusing a link anywhere
    /// on the way.
    #[cfg(unix)]
    pub fn open_file(&self, path: &Path) -> std::io::Result<std::fs::File> {
        let components = relative_components(&self.root, path)?;
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        if let Some(opened) = self.open_beneath(&components) {
            return opened;
        }
        self.open_by_component(&components)
    }

    /// The whole path in one `openat` call with `O_NOFOLLOW_ANY`, which makes
    /// the kernel refuse a link at any level: one call where the component
    /// walk makes one per level. The components are plain names (no `..`), so
    /// with no link to follow the path cannot leave the root. `None`, for the
    /// caller to fall back, before macOS 11 (whose kernels predate the flag
    /// and might ignore it) and wherever the flag is refused.
    #[cfg(target_os = "macos")]
    fn open_beneath(
        &self,
        components: &[std::ffi::OsString],
    ) -> Option<std::io::Result<std::fs::File>> {
        use std::os::fd::{AsRawFd, FromRawFd};

        static SUPPORTED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*SUPPORTED.get_or_init(|| darwin_major().is_some_and(|major| major >= 20)) {
            return None;
        }
        let name = joined_name(components)?;
        // `O_NOFOLLOW_ANY` covers the last component too; adding `O_NOFOLLOW`
        // makes the call fail with `EINVAL`.
        let flags = libc::O_RDONLY | libc::O_NOFOLLOW_ANY | libc::O_CLOEXEC | libc::O_NONBLOCK;
        // SAFETY: `dir` is a live directory descriptor and `name` is a
        // NUL-terminated path that outlives the call.
        let fd = retry_interrupted(|| unsafe {
            libc::openat(self.dir.as_raw_fd(), name.as_ptr(), flags)
        });
        if fd < 0 {
            let error = std::io::Error::last_os_error();
            return match error.raw_os_error() {
                // The walk decides: a path past `MAXPATHLEN`, which it opens a
                // name at a time, and anything refusing the flag itself (the
                // lookup applies it, so in practice nothing; and a socket
                // answers `EOPNOTSUPP` to any open, so it is not remembered).
                Some(libc::ENAMETOOLONG | libc::EINVAL | libc::ENOTSUP | libc::EOPNOTSUPP) => None,
                _ => Some(Err(error)),
            };
        }
        // SAFETY: `openat` returned a fresh descriptor owned here.
        Some(Ok(unsafe { std::fs::File::from_raw_fd(fd) }))
    }

    /// The whole path in one `openat2` call, with the kernel refusing any link
    /// and any escape from the root. `None` on any failure, for the walk to
    /// decide: the call may be missing (an old kernel) or refused (a sandbox,
    /// with whatever errno it was given), or the path too long for one lookup,
    /// and the walk also reports a path that is really gone or a link. Only a
    /// missing call is remembered: `ENOSYS`, confirmed by a call the kernel
    /// rejects before looking at any path (an unknown resolve flag, `EINVAL`
    /// where the call exists), since a file's own open handler may answer
    /// `ENOSYS` too. Any other refusal costs only itself.
    #[cfg(target_os = "linux")]
    fn open_beneath(
        &self,
        components: &[std::ffi::OsString],
    ) -> Option<std::io::Result<std::fs::File>> {
        use std::os::fd::AsRawFd;
        use std::sync::atomic::{AtomicBool, Ordering};

        static MISSING: AtomicBool = AtomicBool::new(false);
        if MISSING.load(Ordering::Relaxed) {
            return None;
        }
        let name = joined_name(components)?;
        match openat2(
            self.dir.as_raw_fd(),
            &name,
            libc::O_NOFOLLOW | libc::O_NONBLOCK,
        ) {
            Ok(file) => Some(Ok(file)),
            Err(e) => {
                if e.raw_os_error() == Some(libc::ENOSYS) && openat2_missing() {
                    MISSING.store(true, Ordering::Relaxed);
                }
                None
            }
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
            // `O_NOFOLLOW` on every component, including the last, and
            // `O_DIRECTORY` on the intermediates so a *file* in the middle of
            // the path fails here rather than at the next `openat`.
            let parent = dir.as_ref().map_or(self.dir.as_raw_fd(), |d| d.as_raw_fd());
            let opened = if i == last {
                let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
                // SAFETY: `parent` is a live directory descriptor, and `name`
                // is a NUL-terminated single component that outlives the call.
                let fd =
                    retry_interrupted(|| unsafe { libc::openat(parent, name.as_ptr(), flags) });
                if fd < 0 {
                    return Err(Error::last_os_error());
                }
                // SAFETY: `openat` returned a fresh owned descriptor.
                unsafe { OwnedFd::from_raw_fd(fd) }
            } else {
                open_dir_at(parent, &name, libc::O_NOFOLLOW)?
            };
            // Assigning drops the previous one, closing the parent we no
            // longer need.
            dir = Some(opened);
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
    ///
    /// A final link (a symlink or junction) is refused, as `O_NOFOLLOW`
    /// refuses it on unix; reading the reparse point itself would hand back
    /// an empty stream. Any other reparse point (a OneDrive placeholder, a
    /// deduplicated file) is a regular file whose data only a normal open
    /// returns, so it is opened again that way and checked like the first.
    #[cfg(windows)]
    pub fn open_file(&self, path: &Path) -> std::io::Result<std::fs::File> {
        use std::io::{Error, ErrorKind};
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;

        // Rejects escapes and non-literal components before anything is opened.
        relative_components(&self.root, path)?;
        let mut file = open_no_follow(path)?;
        // Containment is the path's own object's: a reparse point's data may
        // live elsewhere (a filter's store), by design.
        let opened_at = final_path_of(&file)?;
        if !opened_at.starts_with(&self.anchor) {
            return Err(Error::new(
                ErrorKind::InvalidInput,
                "path resolves outside the served root",
            ));
        }
        let meta = file.metadata()?;
        if meta.file_type().is_symlink() {
            return Err(Error::new(ErrorKind::InvalidInput, "path is a link"));
        }
        if meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            // A normal open follows whatever is at the path by now, so it must
            // be the file just checked and not a link swapped in since.
            // A file gone since the first open is gone; any other failure here
            // is transient, and must not be taken for ineligible (`InvalidInput`).
            // `ERROR_CANT_ACCESS_FILE` is a reparse point no open can read (an
            // app execution alias): as ineligible as a link.
            const ERROR_CANT_ACCESS_FILE: i32 = 1920;
            let transient = |e: std::io::Error| match e.kind() {
                ErrorKind::NotFound | ErrorKind::NotADirectory => e,
                _ if e.raw_os_error() == Some(ERROR_CANT_ACCESS_FILE) => {
                    Error::new(ErrorKind::InvalidInput, e)
                }
                _ => Error::other(e),
            };
            let reopened = std::fs::File::open(path).map_err(transient)?;
            // The data must come from where the path was checked to lead: a
            // link swapped in between the opens, even one put back since,
            // resolves elsewhere. A file saved over it in place resolves to
            // the same path and is read.
            if final_path_of(&reopened).map_err(transient)? != opened_at {
                // Transient, unlike the refusals above: the file is still
                // there to retry, and must not be taken for ineligible.
                return Err(Error::other("path changed while opening"));
            }
            file = reopened;
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

/// A reader of the `/`-separated paths below `root` that the index or Git
/// records, as [`RootDir::read_indexed_to_string`], opening the root once. A
/// root that cannot be opened fails every read, as each read would have.
pub fn indexed_text_reader(root: &Path) -> impl Fn(&str) -> std::io::Result<String> {
    let rooted = RootDir::open(root);
    move |rel| match &rooted {
        Ok(rooted) => rooted.read_indexed_to_string(rel),
        Err(e) => Err(std::io::Error::new(e.kind(), e.to_string())),
    }
}

/// Open the root as a directory to resolve names under, which needs only
/// permission to search it (`--x`), as reading a file by its full path did.
/// A link at the root itself is the user's to have, so it is followed.
#[cfg(unix)]
fn open_search_only(root: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::ffi::OsStrExt;

    let path = std::ffi::CString::new(root.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "root contains a NUL")
    })?;
    open_dir_at(libc::AT_FDCWD, &path, 0).map(std::fs::File::from)
}

/// `EACCES` for a directory this process may not search. A search-only open
/// needs no permission on the directory, nor does the read-only retry need
/// permission to search it, so such a root opens and then resolves nothing
/// below it. Looking up `.` in it asks the kernel, with this process's
/// effective credentials. Only that answer is the kernel's own permission
/// check, certain to fail every lookup below too; any other failure (a
/// flaky network filesystem, a sandbox refusing `fstatat` alone with
/// `EPERM`) says nothing about the reads, which report their own errors.
#[cfg(unix)]
fn refuse_unsearchable(dir: &std::fs::File) -> std::io::Result<()> {
    use std::os::fd::AsRawFd;

    // SAFETY: `dir` is a live directory descriptor, the name is a NUL-
    // terminated literal, and `stat` is written by the call.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstatat(dir.as_raw_fd(), c".".as_ptr(), &mut stat, 0) } != 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EACCES) {
            return Err(error);
        }
    }
    Ok(())
}

/// `name` below `parent` opened as a directory to look names up in, with
/// `flags` added: search-only (`O_PATH`, `O_SEARCH`), so its permission to
/// search is all it needs, as a path lookup's; read-only where the system
/// has no such mode, or refuses it. `O_NONBLOCK` so a FIFO swapped in cannot
/// stall the open.
#[cfg(unix)]
fn open_dir_at(
    parent: std::os::fd::RawFd,
    name: &std::ffi::CStr,
    flags: libc::c_int,
) -> std::io::Result<std::os::fd::OwnedFd> {
    use std::os::fd::FromRawFd;

    #[cfg(target_os = "linux")]
    const SEARCH_ONLY: libc::c_int = libc::O_PATH;
    #[cfg(target_os = "macos")]
    const SEARCH_ONLY: libc::c_int = libc::O_SEARCH;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    const SEARCH_ONLY: libc::c_int = libc::O_RDONLY;

    let open = |mode: libc::c_int| {
        let flags = mode | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NONBLOCK | flags;
        // SAFETY: `parent` is a live directory descriptor (or `AT_FDCWD`) and
        // `name` is NUL-terminated and outlives the call.
        let fd = retry_interrupted(|| unsafe { libc::openat(parent, name.as_ptr(), flags) });
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `openat` returned a fresh descriptor owned here.
        Ok(unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) })
    };
    match open(SEARCH_ONLY) {
        // An answer about the path itself, which a read-only open would give
        // too. Anything else (a kernel without the mode, or a sandbox refusing
        // it, with whatever errno) is tried again read-only. A kernel that
        // ignores the mode instead opens read-only, as before it existed.
        Err(e)
            if SEARCH_ONLY == libc::O_RDONLY
                || e.raw_os_error().is_some_and(|code| {
                    [libc::ENOENT, libc::ENOTDIR, libc::ELOOP].contains(&code)
                }) =>
        {
            Err(e)
        }
        Err(_) => open(libc::O_RDONLY),
        opened => opened,
    }
}

/// `name` opened read-only beneath `dir` with `openat2`, refusing any link
/// and any escape, with `flags` added.
#[cfg(target_os = "linux")]
fn openat2(
    dir: std::os::fd::RawFd,
    name: &std::ffi::CStr,
    flags: libc::c_int,
) -> std::io::Result<std::fs::File> {
    use std::os::fd::FromRawFd;

    // SAFETY: `open_how` is plain data, and all-zero is its documented
    // default for every field not set below.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.flags = (libc::O_RDONLY | libc::O_CLOEXEC | flags) as u64;
    how.resolve = libc::RESOLVE_BENEATH | libc::RESOLVE_NO_SYMLINKS | libc::RESOLVE_NO_MAGICLINKS;
    // SAFETY: `dir` is a live directory descriptor, `name` is NUL-terminated,
    // and `how` is passed with its own size.
    let fd = retry_interrupted(|| unsafe {
        libc::syscall(
            libc::SYS_openat2,
            dir,
            name.as_ptr(),
            &how as *const libc::open_how,
            std::mem::size_of::<libc::open_how>(),
        )
    });
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }
    // SAFETY: `openat2` returned a fresh descriptor owned here.
    Ok(unsafe { std::fs::File::from_raw_fd(fd as i32) })
}

/// Whether `openat2` itself is missing (or refused with `ENOSYS`): asked
/// with a resolve flag no kernel defines, which one with the call rejects
/// with `EINVAL` before any lookup.
#[cfg(target_os = "linux")]
fn openat2_missing() -> bool {
    // SAFETY: as in `openat2`; all-zero is `open_how`'s default.
    let mut how: libc::open_how = unsafe { std::mem::zeroed() };
    how.resolve = 1 << 63;
    // SAFETY: the name is a NUL-terminated literal and `how` is passed with
    // its own size; the call fails without opening anything.
    let fd = unsafe {
        libc::syscall(
            libc::SYS_openat2,
            libc::AT_FDCWD,
            c".".as_ptr(),
            &how as *const libc::open_how,
            std::mem::size_of::<libc::open_how>(),
        )
    };
    if fd >= 0 {
        // SAFETY: an unexpected descriptor, owned here and closed.
        unsafe { libc::close(fd as i32) };
        return false;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOSYS)
}

/// Run an open again while a signal interrupts it, as `std::fs::File::open`
/// does; the result is the call's, with `errno` set when it is negative.
#[cfg(unix)]
fn retry_interrupted<T: PartialOrd + Default>(mut open: impl FnMut() -> T) -> T {
    loop {
        let fd = open();
        if fd >= T::default() || std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR)
        {
            return fd;
        }
    }
}

/// `components` joined with `/` for a single `openat`. `None` for a NUL,
/// which the component walk reports.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn joined_name(components: &[std::ffi::OsString]) -> Option<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;

    let mut joined = Vec::new();
    for component in components {
        if !joined.is_empty() {
            joined.push(b'/');
        }
        joined.extend_from_slice(component.as_bytes());
    }
    std::ffi::CString::new(joined).ok()
}

/// The running kernel's major version (20 is macOS 11).
#[cfg(target_os = "macos")]
fn darwin_major() -> Option<u32> {
    // SAFETY: `utsname` is plain data, filled in by `uname`.
    let mut name: libc::utsname = unsafe { std::mem::zeroed() };
    // SAFETY: `name` is a valid, writable `utsname`.
    if unsafe { libc::uname(&mut name) } != 0 {
        return None;
    }
    // SAFETY: `uname` NUL-terminates `release`.
    let release = unsafe { std::ffi::CStr::from_ptr(name.release.as_ptr()) };
    release.to_str().ok()?.split('.').next()?.parse().ok()
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

impl RootDir {
    /// Read all of `path` with [`RootDir::open_file`]'s guarantee.
    pub fn read(&self, path: &Path) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut self.open_file(path)?, &mut bytes)?;
        Ok(bytes)
    }

    /// Read `rel`, a `/`-separated path below the root as the index or Git
    /// records it, refusing a link at any level just as the walker would.
    pub fn read_indexed(&self, rel: &str) -> std::io::Result<Vec<u8>> {
        self.read(&crate::hybrid::join_indexed_path(&self.root, rel))
    }

    /// [`RootDir::read_indexed`] for UTF-8 text, failing on anything else as
    /// `std::fs::read_to_string` does.
    pub fn read_indexed_to_string(&self, rel: &str) -> std::io::Result<String> {
        String::from_utf8(self.read_indexed(rel)?)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
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

    /// A path past the kernel's limit for one lookup (1024 bytes on macOS,
    /// 4096 on Linux) still opens, a name at a time, as it did before the
    /// single-call opens. Built with `*at` calls, since `std::fs` takes the
    /// whole path at once.
    #[cfg(unix)]
    #[test]
    fn a_path_longer_than_one_lookup_allows_still_opens() {
        use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};

        let root = TempDir::new().unwrap();
        let name = std::ffi::CString::new("d".repeat(100)).unwrap();
        let mut dir = std::fs::File::open(root.path()).map(OwnedFd::from).unwrap();
        for _ in 0..45 {
            // SAFETY: `dir` is a live directory descriptor and `name` a
            // NUL-terminated single component.
            unsafe {
                assert_eq!(libc::mkdirat(dir.as_raw_fd(), name.as_ptr(), 0o755), 0);
                let fd = libc::openat(
                    dir.as_raw_fd(),
                    name.as_ptr(),
                    libc::O_RDONLY | libc::O_DIRECTORY,
                );
                assert!(fd >= 0);
                dir = OwnedFd::from_raw_fd(fd);
            }
        }
        let file = std::ffi::CString::new("f.txt").unwrap();
        // SAFETY: as above; the new descriptor is owned by the `File`.
        let mut f = unsafe {
            let fd = libc::openat(
                dir.as_raw_fd(),
                file.as_ptr(),
                libc::O_WRONLY | libc::O_CREAT,
                0o644,
            );
            assert!(fd >= 0);
            std::fs::File::from_raw_fd(fd)
        };
        std::io::Write::write_all(&mut f, b"deep\n").unwrap();

        let rel = format!("{}/f.txt", vec!["d".repeat(100); 45].join("/"));
        let rooted = RootDir::open(root.path()).unwrap();
        assert_eq!(rooted.read_indexed_to_string(&rel).unwrap(), "deep\n");
    }

    /// The probe says `openat2` is missing exactly when a real call says so:
    /// not with it (an ordinary kernel), yes under a filter answering ENOSYS.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_openat2_probe_agrees_with_a_real_call() {
        let real = openat2(libc::AT_FDCWD, c".", libc::O_DIRECTORY);
        let missing = real.as_ref().err().and_then(|e| e.raw_os_error()) == Some(libc::ENOSYS);
        assert_eq!(openat2_missing(), missing, "{real:?}");
    }

    /// A root that may be listed but not searched does not open: nothing
    /// below it would resolve, and every read would fail one by one.
    #[cfg(unix)]
    #[test]
    fn a_root_that_cannot_be_searched_does_not_open() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("f.rs"), "x\n").unwrap();
        assert!(RootDir::open(&root).is_ok());
        struct Restore<'a>(&'a Path);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
            }
        }
        let _restore = Restore(&root);
        for mode in [0o644, 0o000] {
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(mode)).unwrap();
            if std::fs::metadata(root.join("f.rs")).is_ok() {
                eprintln!("skipped: permissions do not apply to this user");
                return;
            }
            let error = RootDir::open(&root)
                .err()
                .expect("opened a root it cannot search");
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied,
                "{mode:o}: {error}"
            );
            // One file, opened without the check: its own open must say so
            // too, or the server would take every file below for gone.
            let error = open_within_root(&root, &root.join("f.rs")).unwrap_err();
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied,
                "{mode:o}: {error}"
            );
            // The walk, where the unchecked root opens at all (not 000 on
            // macOS, which then refuses the root itself the same way).
            let components = relative_components(&root, &root.join("f.rs")).unwrap();
            let error = match RootDir::open_unchecked(&root) {
                Ok(unchecked) => unchecked.open_by_component(&components).unwrap_err(),
                Err(error) => error,
            };
            assert_eq!(
                error.kind(),
                std::io::ErrorKind::PermissionDenied,
                "walk {mode:o}: {error}"
            );
        }
    }

    /// The server decides from the error's kind whether a path left the
    /// index, so the walk (which decides whenever `openat2` fails) must say
    /// "gone" or "a link" as `openat2` would, also below a directory it may
    /// only search, not list.
    #[cfg(unix)]
    #[test]
    fn the_walk_reports_gone_and_links_below_a_search_only_directory() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("a/real")).unwrap();
        std::fs::write(root.join("a/real/f.rs"), "x\n").unwrap();
        std::os::unix::fs::symlink(root.join("a/real"), root.join("a/linked")).unwrap();
        std::os::unix::fs::symlink(root.join("a/real/f.rs"), root.join("a/real/g.rs")).unwrap();
        std::fs::set_permissions(root.join("a"), std::fs::Permissions::from_mode(0o111)).unwrap();
        // Back to listable whatever happens, or the temp tree cannot be removed.
        struct Restore<'a>(&'a Path);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
            }
        }
        let _restore = Restore(&root.join("a"));
        if std::fs::read_dir(root.join("a")).is_ok() {
            // Privileged: permissions do not apply, so this would prove nothing.
            eprintln!("skipped: permissions do not apply to this user");
            return;
        }
        let rooted = RootDir::open(root).unwrap();
        let components = |rel: &str| relative_components(root, &root.join(rel)).unwrap();
        let ineligible = |e: &std::io::Error| {
            matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
            ) || e.raw_os_error() == Some(libc::ELOOP)
        };
        for (rel, gone) in [
            ("a/real/f.rs", None),
            ("a/real/missing.rs", Some(std::io::ErrorKind::NotFound)),
            ("a/linked/f.rs", Some(std::io::ErrorKind::Other)),
            ("a/real/g.rs", Some(std::io::ErrorKind::Other)),
        ] {
            for (how, result) in [
                ("open_file", rooted.open_file(&root.join(rel))),
                ("walk", rooted.open_by_component(&components(rel))),
            ] {
                match (gone, result) {
                    (None, Ok(_)) => {}
                    (Some(std::io::ErrorKind::NotFound), Err(e)) => {
                        assert_eq!(e.kind(), std::io::ErrorKind::NotFound, "{how} {rel}: {e}")
                    }
                    (Some(_), Err(e)) => assert!(ineligible(&e), "{how} {rel}: {e}"),
                    (want, got) => panic!("{how} {rel}: want {want:?}, got {got:?}"),
                }
            }
        }
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

        let rooted = RootDir::open(root.path()).unwrap();
        assert_eq!(
            rooted.read_indexed_to_string("src/a.rs").unwrap(),
            "fn a() {}\n"
        );
        assert!(rooted.read_indexed("linked/secret.txt").is_err());
        assert!(rooted.read_indexed("src/b.rs").is_err());
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
