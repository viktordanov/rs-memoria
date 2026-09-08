//! Filesystem adapters: bounded project reads, durable atomic writes, the
//! advisory write lock, and the UTC clock.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use memoria_application::ports::{
    AdapterError, AtomicWriter, Clock, FileKind, LockFailure, ProjectFiles, WriteCoordinator,
    WriteGuard,
};
use memoria_domain::Timestamp;

fn adapter(operation: &str, path: &Path, err: io::Error) -> AdapterError {
    AdapterError::new(operation, Some(path.display().to_string()), err.to_string())
}

/// Reject a relative path whose ancestor component (below `root`) is a
/// symlink. Reads and writes through such a path could leave the project.
pub fn check_ancestors(root: &Path, relative: &str) -> Result<(), AdapterError> {
    let mut current = root.to_path_buf();
    let components: Vec<&str> = relative.split('/').filter(|c| !c.is_empty()).collect();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(AdapterError::new(
                    "contain",
                    Some(relative.to_string()),
                    format!(
                        "ancestor {} is a symlink; Memoria never follows symlinks inside the project",
                        current.display()
                    ),
                ));
            }
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(err) => return Err(adapter("stat", &current, err)),
        }
    }
    Ok(())
}

/// Whether any ancestor component of `relative` below `root` is a symlink.
pub fn has_symlink_ancestor(root: &Path, relative: &str) -> Result<bool, AdapterError> {
    match check_ancestors(root, relative) {
        Ok(()) => Ok(false),
        Err(err) if err.operation == "contain" => Ok(true),
        Err(err) => Err(err),
    }
}

/// Reads limited to the project root, never following symlinks anywhere
/// below the root.
pub struct FsProjectFiles {
    root: PathBuf,
}

impl FsProjectFiles {
    pub fn new(root: PathBuf) -> FsProjectFiles {
        FsProjectFiles { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn resolve(&self, relative: &str) -> PathBuf {
        if relative.is_empty() {
            self.root.clone()
        } else {
            self.root.join(relative)
        }
    }
}

pub fn kind_of(path: &Path) -> Result<FileKind, io::Error> {
    match fs::symlink_metadata(path) {
        Ok(meta) => {
            let file_type = meta.file_type();
            Ok(if file_type.is_symlink() {
                FileKind::Symlink
            } else if file_type.is_dir() {
                FileKind::Directory
            } else if file_type.is_file() {
                FileKind::Regular
            } else {
                FileKind::Other
            })
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(FileKind::Missing),
        Err(err) => Err(err),
    }
}

/// Read a regular file fully; symlinks are rejected.
pub fn read_regular(path: &Path) -> Result<Vec<u8>, AdapterError> {
    match kind_of(path).map_err(|e| adapter("stat", path, e))? {
        FileKind::Regular => fs::read(path).map_err(|e| adapter("read", path, e)),
        other => Err(AdapterError::new(
            "read",
            Some(path.display().to_string()),
            format!("expected a regular file, found {other:?}"),
        )),
    }
}

impl ProjectFiles for FsProjectFiles {
    fn kind(&self, path: &str) -> Result<FileKind, AdapterError> {
        if has_symlink_ancestor(&self.root, path)? {
            // Reported as a symlink so selected paths fail and excluded paths stay harmless.
            return Ok(FileKind::Symlink);
        }
        let full = self.resolve(path);
        kind_of(&full).map_err(|e| adapter("stat", &full, e))
    }

    fn read(&self, path: &str) -> Result<Vec<u8>, AdapterError> {
        check_ancestors(&self.root, path)?;
        read_regular(&self.resolve(path))
    }

    fn has_git_entry(&self, directory: &str) -> bool {
        fs::symlink_metadata(self.resolve(directory).join(".git")).is_ok()
    }

    fn subdirectories(&self, directory: &str) -> Result<Vec<String>, AdapterError> {
        check_ancestors(&self.root, &format!("{directory}/x"))?;
        let full = self.resolve(directory);
        let mut names = Vec::new();
        for entry in fs::read_dir(&full).map_err(|e| adapter("read_dir", &full, e))? {
            let entry = entry.map_err(|e| adapter("read_dir", &full, e))?;
            let file_type = entry
                .file_type()
                .map_err(|e| adapter("stat", &entry.path(), e))?;
            if file_type.is_dir()
                && let Ok(name) = entry.file_name().into_string()
            {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    fn root_display(&self) -> String {
        self.root.display().to_string()
    }
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temp_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if name == "memoria.lock" {
        // The committed state has one reserved temporary pattern, so a
        // snapshot can recognize it without guessing.
        return target.with_file_name(format!(".memoria.lock.tmp.{}", random_hex32()));
    }
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    target.with_file_name(format!(
        ".{name}.memoria-tmp-{}-{counter}-{nanos}",
        std::process::id()
    ))
}

/// Thirty-two lowercase hexadecimal characters from the process identity,
/// a monotonic counter, and the clock. The name only has to be unique
/// within one directory; `create_new` still refuses an existing file.
fn random_hex32() -> String {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed) as u128;
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id() as u128;
    let mixed = nanos
        .wrapping_mul(0x2545_f491_4f6c_dd1d)
        .wrapping_add(counter.wrapping_mul(0x9e37_79b9_7f4a_7c15))
        .wrapping_add(pid << 64);
    format!("{mixed:032x}")
}

/// Synchronize a directory so completed renames are durable.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

/// Injectable failure points for durability tests. Production code passes
/// [`NO_FAULTS`]; a test hook returns an error for a named step.
pub type FaultHook<'a> = &'a dyn Fn(&str) -> Option<io::Error>;

/// A hook that never injects a failure.
pub const NO_FAULTS: FaultHook<'static> = &|_| None;

/// Consult a fault hook for a named step.
pub fn fault(hook: FaultHook<'_>, step: &str) -> io::Result<()> {
    match hook(step) {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

/// Write bytes to a unique temporary file in the target directory, flush,
/// sync, rename over the target, and sync the directory.
pub fn durable_replace(target: &Path, bytes: &[u8], mode: Option<u32>) -> Result<(), AdapterError> {
    durable_replace_with(target, bytes, mode, NO_FAULTS)
}

/// [`durable_replace`] with injectable failures at `temp-create`, `write`,
/// `sync-file`, `rename`, and `sync-dir`. A failure before the rename leaves
/// the previous target untouched and removes the temporary file. A failure of
/// the directory sync after the rename leaves the complete new file in place
/// and reports the error explicitly.
pub fn durable_replace_with(
    target: &Path,
    bytes: &[u8],
    mode: Option<u32>,
    faults: FaultHook<'_>,
) -> Result<(), AdapterError> {
    let dir = target.parent().ok_or_else(|| {
        AdapterError::new(
            "replace",
            Some(target.display().to_string()),
            "target has no parent directory",
        )
    })?;
    let temp = temp_path(target);
    let staged = (|| -> io::Result<()> {
        fault(faults, "temp-create")?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        if let Some(mode) = mode {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(mode))?;
        }
        fault(faults, "write")?;
        file.write_all(bytes)?;
        file.flush()?;
        fault(faults, "sync-file")?;
        file.sync_all()?;
        drop(file);
        fault(faults, "rename")?;
        fs::rename(&temp, target)?;
        Ok(())
    })();
    if let Err(err) = staged {
        let _ = fs::remove_file(&temp);
        return Err(adapter("replace", target, err));
    }
    if let Err(err) = fault(faults, "sync-dir").and_then(|_| sync_dir(dir)) {
        return Err(AdapterError::new(
            "sync_dir",
            Some(target.display().to_string()),
            format!(
                "the new content is in place but the directory sync failed; durability is not guaranteed until the next successful sync: {err}"
            ),
        ));
    }
    Ok(())
}

/// Compare-then-replace writes inside the project.
pub struct AtomicFileWriter<'a> {
    root: PathBuf,
    faults: FaultHook<'a>,
}

impl AtomicFileWriter<'static> {
    pub fn new(root: PathBuf) -> AtomicFileWriter<'static> {
        AtomicFileWriter {
            root,
            faults: NO_FAULTS,
        }
    }
}

impl<'a> AtomicFileWriter<'a> {
    /// A writer that consults `faults` before each durable step. Steps are
    /// named `replace:<path>` (before any work) plus the
    /// [`durable_replace_with`] steps.
    pub fn with_faults(root: PathBuf, faults: FaultHook<'a>) -> AtomicFileWriter<'a> {
        AtomicFileWriter { root, faults }
    }
}

impl AtomicWriter for AtomicFileWriter<'_> {
    fn replace(&self, path: &str, expected: &[u8], new: &[u8]) -> Result<(), AdapterError> {
        check_ancestors(&self.root, path)?;
        let full = self.root.join(path);
        if let Some(err) = (self.faults)(&format!("replace:{path}")) {
            return Err(adapter("replace", &full, err));
        }
        let current = read_regular(&full)?;
        if current != expected {
            return Err(AdapterError::new(
                "replace",
                Some(full.display().to_string()),
                "file changed since it was read; rerun the command",
            ));
        }
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            fs::metadata(&full).ok().map(|m| m.permissions().mode())
        };
        durable_replace_with(&full, new, mode, self.faults)
    }

    fn create_new(&self, path: &str, bytes: &[u8]) -> Result<(), AdapterError> {
        check_ancestors(&self.root, path)?;
        let full = self.root.join(path);
        if let Some(parent) = full.parent() {
            fs::create_dir_all(parent).map_err(|e| adapter("create_dir", parent, e))?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full)
            .map_err(|e| adapter("create", &full, e))?;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| adapter("write", &full, e))?;
        if let Some(parent) = full.parent() {
            sync_dir(parent).map_err(|e| adapter("sync_dir", parent, e))?;
        }
        Ok(())
    }
}

/// Exclusive, nonblocking advisory lock on a worktree-private path.
///
/// The committed `memoria.lock` never acts as the process lock. The write
/// lock lives under Git metadata, at the path
/// `git rev-parse --git-path memoria/write.lock` resolves, so linked
/// worktrees receive separate locks and read-only commands create nothing
/// inside the worktree.
pub struct LockFileCoordinator {
    root: PathBuf,
    lock_path: PathBuf,
    /// The directory the lock must stay under, with no symlink between.
    boundary: PathBuf,
}

impl LockFileCoordinator {
    /// `lock_path` is the absolute worktree-private destination.
    /// `boundary` defaults to the lock's grandparent when the caller has no
    /// Git metadata directory to supply.
    pub fn new(root: PathBuf, lock_path: PathBuf) -> LockFileCoordinator {
        let boundary = lock_path
            .parent()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.clone());
        LockFileCoordinator {
            root,
            lock_path,
            boundary,
        }
    }

    /// The Git metadata directory is the containment boundary.
    pub fn with_boundary(
        root: PathBuf,
        lock_path: PathBuf,
        boundary: PathBuf,
    ) -> LockFileCoordinator {
        LockFileCoordinator {
            root,
            lock_path,
            boundary,
        }
    }

    pub fn lock_path(&self) -> &Path {
        &self.lock_path
    }
}

pub struct FileGuard {
    _file: File,
}

impl WriteGuard for FileGuard {}

/// Whether `path` is a symlink, following nothing.
fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|meta| meta.file_type().is_symlink())
        .unwrap_or(false)
}

/// Refuse a symlink anywhere from `boundary` down to `path`, and refuse a
/// nonregular entry at `path` itself.
///
/// The lock lives under Git metadata, outside the worktree, so the state
/// writer's containment guarantees must be enforced on this path too. A
/// substituted directory or lock file would otherwise place the advisory
/// lock outside the private metadata location.
fn refuse_substituted_lock_path(path: &Path, boundary: &Path) -> Result<(), LockFailure> {
    let refuse = |what: &Path, reason: &str| -> LockFailure {
        LockFailure::Io(AdapterError::new(
            "contain",
            Some(what.display().to_string()),
            format!("{} {reason}; Memoria refuses to use it", what.display()),
        ))
    };
    // Ancestors between the boundary and the lock, outermost first.
    let mut ancestors: Vec<&Path> = Vec::new();
    let mut current = path.parent();
    while let Some(dir) = current {
        if dir == boundary || dir.parent().is_none() {
            break;
        }
        ancestors.push(dir);
        current = dir.parent();
    }
    for dir in ancestors.into_iter().rev() {
        if is_symlink(dir) {
            return Err(refuse(dir, "is a symlink"));
        }
        match fs::symlink_metadata(dir) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => return Err(refuse(dir, "is not a directory")),
            // A missing ancestor is created below, under this same check.
            Err(_) => {}
        }
    }
    if is_symlink(path) {
        return Err(refuse(path, "is a symlink"));
    }
    match kind_of(path) {
        Ok(FileKind::Missing) | Ok(FileKind::Regular) => Ok(()),
        Ok(other) => Err(refuse(path, &format!("is a {other:?}, not a regular file"))),
        Err(err) => Err(LockFailure::Io(adapter("stat", path, err))),
    }
}

/// Acquire an exclusive nonblocking flock on `path`, creating it if needed.
///
/// `boundary` is the directory the lock must stay under, normally the Git
/// metadata directory that resolved it. Every component between the
/// boundary and the lock must be a real directory, and the lock itself must
/// be a regular file.
pub fn lock_file_within(path: &Path, boundary: &Path) -> Result<FileGuard, LockFailure> {
    refuse_substituted_lock_path(path, boundary)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| LockFailure::Io(adapter("create_dir", parent, e)))?;
    }
    // Re-check after creation: a concurrent writer could have substituted a
    // component between the check and the create.
    refuse_substituted_lock_path(path, boundary)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        // Never traverse a final symlink, even one created just now.
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    let file = options
        .open(path)
        .map_err(|e| LockFailure::Io(adapter("open", path, e)))?;
    let meta = file
        .metadata()
        .map_err(|e| LockFailure::Io(adapter("stat", path, e)))?;
    if !meta.is_file() {
        return Err(LockFailure::Io(AdapterError::new(
            "contain",
            Some(path.display().to_string()),
            format!("{} is not a regular file", path.display()),
        )));
    }
    match rustix::fs::flock(&file, rustix::fs::FlockOperation::NonBlockingLockExclusive) {
        Ok(()) => Ok(FileGuard { _file: file }),
        Err(rustix::io::Errno::WOULDBLOCK) => Err(LockFailure::Busy),
        Err(err) => Err(LockFailure::Io(AdapterError::new(
            "flock",
            Some(path.display().to_string()),
            err.to_string(),
        ))),
    }
}

/// [`lock_file_within`] with the lock's own parent as the boundary.
pub fn lock_file(path: &Path) -> Result<FileGuard, LockFailure> {
    let boundary = path
        .parent()
        .and_then(Path::parent)
        .unwrap_or_else(|| Path::new("/"));
    lock_file_within(path, boundary)
}

/// Fail when a reserved state path is a symlink or another special file.
///
/// The committed state and its reserved temporary destination must be
/// ordinary files. A symlink there would let a write leave the project.
pub fn check_state_dir(root: &Path) -> Result<(), AdapterError> {
    for relative in ["memoria.lock", ".memoria", ".memoria/state.json"] {
        let full = root.join(relative);
        if let Ok(meta) = fs::symlink_metadata(&full)
            && meta.file_type().is_symlink()
        {
            return Err(AdapterError::new(
                "contain",
                Some(relative.to_string()),
                format!("{} is a symlink; Memoria refuses to use it", full.display()),
            ));
        }
    }
    Ok(())
}

impl WriteCoordinator for LockFileCoordinator {
    fn lock(&self) -> Result<Box<dyn WriteGuard + '_>, LockFailure> {
        check_state_dir(&self.root).map_err(LockFailure::Io)?;
        // The Git metadata directory is the containment boundary: no
        // component below it may be substituted.
        let guard = lock_file_within(&self.lock_path, &self.boundary)?;
        Ok(Box::new(guard))
    }
}

/// UTC wall clock formatted as RFC 3339 seconds.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

pub fn format_rfc3339(seconds: u64) -> String {
    // Civil-from-days algorithm (Howard Hinnant).
    let days = (seconds / 86_400) as i64;
    let secs = seconds % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        (secs % 3600) / 60,
        secs % 60
    )
}

impl Clock for SystemClock {
    fn now(&self) -> Timestamp {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Timestamp(format_rfc3339(seconds))
    }
}

/// Read up to `limit` bytes. At most one byte beyond the limit is ever
/// requested from the reader; that extra byte only detects overflow.
pub fn read_bounded(reader: &mut dyn Read, limit: u64) -> io::Result<Result<Vec<u8>, u64>> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 65_536];
    loop {
        let remaining = limit.saturating_sub(buffer.len() as u64);
        let want = (remaining + 1).min(chunk.len() as u64) as usize;
        let n = reader.read(&mut chunk[..want])?;
        if n == 0 {
            return Ok(Ok(buffer));
        }
        if buffer.len() as u64 + n as u64 > limit {
            return Ok(Err(limit));
        }
        buffer.extend_from_slice(&chunk[..n]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_timestamps() {
        assert_eq!(format_rfc3339(0), "1970-01-01T00:00:00Z");
        assert_eq!(format_rfc3339(1_788_782_400), "2026-09-07T12:00:00Z");
        assert_eq!(format_rfc3339(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn durable_replace_preserves_mode_and_content() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("state.json");
        fs::write(&target, b"old").unwrap();
        durable_replace(&target, b"new", Some(0o600)).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"new");
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn replace_rejects_changed_content() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.md"), b"one").unwrap();
        let writer = AtomicFileWriter::new(dir.path().to_path_buf());
        assert!(writer.replace("a.md", b"other", b"two").is_err());
        assert_eq!(fs::read(dir.path().join("a.md")).unwrap(), b"one");
        writer.replace("a.md", b"one", b"two").unwrap();
        assert_eq!(fs::read(dir.path().join("a.md")).unwrap(), b"two");
        assert!(writer.create_new("a.md", b"x").is_err());
    }

    #[test]
    fn lock_is_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        let coordinator = LockFileCoordinator::new(
            dir.path().to_path_buf(),
            dir.path().join(".git/memoria/write.lock"),
        );
        let first = coordinator.lock().unwrap_or_else(|_| panic!("first lock"));
        assert!(matches!(coordinator.lock().err(), Some(LockFailure::Busy)));
        drop(first);
        assert!(coordinator.lock().is_ok());
    }

    #[test]
    fn bounded_read_detects_overflow() {
        let mut data: &[u8] = b"hello";
        assert_eq!(read_bounded(&mut data, 5).unwrap(), Ok(b"hello".to_vec()));
        let mut data: &[u8] = b"hello!";
        assert_eq!(read_bounded(&mut data, 5).unwrap(), Err(5));
    }

    /// Counts bytes handed out; never reaches EOF unless `total` is set.
    struct Counting {
        served: usize,
        total: Option<usize>,
    }

    impl Read for Counting {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = match self.total {
                Some(total) => buf.len().min(total - self.served),
                None => buf.len(),
            };
            buf[..n].fill(b' ');
            self.served += n;
            Ok(n)
        }
    }

    #[test]
    fn bounded_read_consumes_at_most_one_extra_byte() {
        let cap = 64 * 1024 * 1024;
        let mut endless = Counting {
            served: 0,
            total: None,
        };
        assert_eq!(read_bounded(&mut endless, cap).unwrap(), Err(cap));
        assert_eq!(
            endless.served as u64,
            cap + 1,
            "exactly one byte beyond the cap is requested"
        );
        let mut exact = Counting {
            served: 0,
            total: Some(cap as usize),
        };
        assert_eq!(
            read_bounded(&mut exact, cap)
                .unwrap()
                .map(|b| b.len() as u64),
            Ok(cap)
        );
        assert_eq!(
            exact.served as u64, cap,
            "an exact-cap stream reads no extra byte"
        );
        let mut short = Counting {
            served: 0,
            total: Some(10),
        };
        assert_eq!(
            read_bounded(&mut short, cap).unwrap().map(|b| b.len()),
            Ok(10)
        );
        assert_eq!(short.served, 10);
        let mut zero = Counting {
            served: 0,
            total: Some(1),
        };
        assert_eq!(read_bounded(&mut zero, 0).unwrap(), Err(0));
        assert_eq!(zero.served, 1);
    }

    #[test]
    fn ancestor_symlinks_are_rejected_but_leaf_symlinks_are_reported() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("a.rs"), b"OUTSIDE").unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join("child")).unwrap();
        fs::write(dir.path().join("plain.rs"), b"inside").unwrap();
        std::os::unix::fs::symlink("plain.rs", dir.path().join("leaf.rs")).unwrap();
        let files = FsProjectFiles::new(dir.path().to_path_buf());
        assert_eq!(files.kind("child/a.rs").unwrap(), FileKind::Symlink);
        assert!(
            files
                .read("child/a.rs")
                .unwrap_err()
                .message
                .contains("symlink")
        );
        assert_eq!(files.kind("leaf.rs").unwrap(), FileKind::Symlink);
        assert_eq!(files.kind("plain.rs").unwrap(), FileKind::Regular);
        assert_eq!(files.read("plain.rs").unwrap(), b"inside");
        assert_eq!(
            files.kind("missing/deeper/x.rs").unwrap(),
            FileKind::Missing
        );
        let writer = AtomicFileWriter::new(dir.path().to_path_buf());
        assert!(
            writer
                .replace("child/a.rs", b"OUTSIDE", b"changed")
                .is_err()
        );
        assert_eq!(fs::read(outside.path().join("a.rs")).unwrap(), b"OUTSIDE");
        assert!(writer.create_new("child/new.rs", b"x").is_err());
        assert!(!outside.path().join("new.rs").exists());
    }

    #[test]
    fn state_directory_symlink_blocks_the_lock() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), dir.path().join(".memoria")).unwrap();
        let coordinator = LockFileCoordinator::new(
            dir.path().to_path_buf(),
            dir.path().join(".git/memoria/write.lock"),
        );
        assert!(matches!(coordinator.lock().err(), Some(LockFailure::Io(_))));
        assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    }

    fn failing(step: &'static str) -> impl Fn(&str) -> Option<io::Error> {
        move |name| {
            if name == step {
                Some(io::Error::other(format!("injected {step}")))
            } else {
                None
            }
        }
    }

    #[test]
    fn injected_failures_before_rename_preserve_the_old_file() {
        for step in ["temp-create", "write", "sync-file", "rename"] {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("state.json");
            fs::write(&target, b"old").unwrap();
            let hook = failing(step);
            let err = durable_replace_with(&target, b"new", Some(0o600), &hook).unwrap_err();
            assert!(err.message.contains("injected"), "{step}: {err}");
            assert_eq!(
                fs::read(&target).unwrap(),
                b"old",
                "{step} must leave the previous content"
            );
            assert_eq!(
                fs::read_dir(dir.path()).unwrap().count(),
                1,
                "{step} must remove its temporary file"
            );
        }
    }

    #[test]
    fn directory_sync_failure_after_rename_reports_but_keeps_the_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("state.json");
        fs::write(&target, b"old").unwrap();
        let hook = failing("sync-dir");
        let err = durable_replace_with(&target, b"new", Some(0o600), &hook).unwrap_err();
        assert_eq!(err.operation, "sync_dir");
        assert!(err.message.contains("new content is in place"));
        assert_eq!(fs::read(&target).unwrap(), b"new");
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
    }
}
