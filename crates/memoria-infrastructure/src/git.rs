//! Git subprocess adapter with argument arrays and NUL-delimited paths.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use memoria_application::ports::{AdapterError, FileKind, GitRepository};

use crate::bounded_process::Supervisor;
use crate::cat_file::{BatchError, CatFileBatch, Header, frameable};

/// Captured stdout for a supervised Git command, in bytes. Supervised Git is
/// used only for repository discovery, whose output is a few short lines.
const SUPERVISED_OUTPUT: u64 = 1024 * 1024;

pub struct GitCli {
    root: PathBuf,
    common_dir: PathBuf,
    /// The repository's empty tree, used as an attribute source so no
    /// `.gitattributes` filter driver can run during inspection.
    empty_tree: String,
    /// Present only for the native hook endpoint, which owns every process
    /// it starts and must terminate them when its deadline expires.
    supervisor: Option<Arc<Supervisor>>,
    /// The per-worktree metadata directory, resolved once. Startup asks for
    /// it and for a private path under it back to back.
    git_dir: OnceLock<String>,
    /// The shared historical object reader of this invocation. It holds a
    /// process, never object bytes.
    history: Mutex<HistoryReader>,
}

/// The state of the historical `cat-file --batch` reader.
enum HistoryReader {
    /// Not started, or discarded after a deadline or a refused object.
    Idle,
    Running(CatFileBatch),
    /// The batch reader failed outside its protocol once. Every later read
    /// in this invocation uses one `cat-file blob` process per object.
    Unavailable,
}

/// The outcome of one historical read through the batch reader.
enum BatchRead {
    Done(Result<Option<Vec<u8>>, AdapterError>),
    /// Use the per-object read instead.
    Fallback,
}

/// Entries that the raw comparison keeps requested ahead of its answers. A
/// writer thread sends them, so no pipe size matters.
const RAW_WINDOW: usize = 256;

/// Expected content, by worktree size, that the raw comparison keeps
/// requested ahead of its answers. With [`RAW_WINDOW`], it bounds the unread
/// output that a walk leaves behind when it stops at its first change.
const RAW_WINDOW_BYTES: u64 = 4 * 1024 * 1024;

fn history_limit() -> AdapterError {
    AdapterError::new("history_limit", None, "Historical read budget exhausted.")
}

/// One historical object, as the batch reader found it.
enum HistoricalObject {
    Blob(Vec<u8>),
    Missing,
    /// A tree, commit, or tag. `git cat-file blob` peels a tag, so the
    /// per-object read decides.
    Other,
    /// Larger than the byte budget that remains.
    TooLarge,
}

/// One index entry whose raw worktree bytes are compared with its blob.
struct IndexEntry {
    mode: String,
    oid: String,
    path: String,
}

/// The worktree side of one index entry that passed its local checks.
struct LocalEntry {
    full: PathBuf,
    /// The link text of a committed symlink. `None` is a regular file, read
    /// only when its size matches the blob.
    link: Option<Vec<u8>>,
    /// The worktree size: the link text length or the file length.
    size: u64,
}

/// Why a raw comparison stopped before it had an answer.
enum Interrupted {
    /// The batch reader cannot be used. The caller compares again with one
    /// process for each object.
    Fallback,
    Failed(AdapterError),
}

fn command(cwd: &Path) -> Command {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(cwd);
    // Global attribute files never contribute filter drivers to inspection.
    cmd.arg("-c").arg("core.attributesFile=/dev/null");
    // Never execute repository-configured programs during inspection:
    // fsmonitor hooks, pagers, and external diff/textconv drivers.
    cmd.arg("-c").arg("core.fsmonitor=false");
    // Submodule worktrees are opaque: never inspect their content (and never
    // their own attribute filters); only recorded gitlink commits count.
    cmd.arg("-c").arg("diff.ignoreSubmodules=dirty");
    cmd.arg("-c").arg("status.submoduleSummary=false");
    cmd.arg("-c").arg("submodule.recurse=false");
    cmd.arg("-c").arg("core.pager=cat");
    cmd.arg("-c").arg("diff.external=");
    cmd.env("GIT_OPTIONAL_LOCKS", "0");
    cmd.env("GIT_PAGER", "cat");
    // Inspection never retrieves missing objects from a promisor remote:
    // a missing historical blob is reported as unavailable instead.
    cmd.env("GIT_NO_LAZY_FETCH", "1");
    cmd.arg("-c").arg("fetch.negotiationAlgorithm=noop");
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    cmd.env_remove("GIT_DIR");
    cmd.env_remove("GIT_WORK_TREE");
    cmd.env_remove("GIT_INDEX_FILE");
    cmd.stdin(Stdio::null());
    cmd
}

fn run_in(
    cwd: &Path,
    args: &[&str],
    supervisor: Option<&Supervisor>,
) -> Result<Output, AdapterError> {
    let mut cmd = command(cwd);
    cmd.args(args);
    let Some(owner) = supervisor else {
        return cmd.output().map_err(|e| {
            AdapterError::new(
                "git",
                Some(cwd.display().to_string()),
                format!("cannot run git {}: {e}", args.join(" ")),
            )
        });
    };
    // The endpoint owns this child: it runs in its own session, under the
    // deadline that remains, and its group is terminated on expiry.
    let output = owner.run(cmd, owner.remaining(), SUPERVISED_OUTPUT, true)?;
    if output.output_truncated {
        return Err(AdapterError::new(
            "git",
            Some(cwd.display().to_string()),
            format!("git {} produced more output than allowed", args.join(" ")),
        ));
    }
    if output.timed_out {
        return Err(AdapterError::new(
            "git",
            Some(cwd.display().to_string()),
            format!("git {} did not finish within the deadline", args.join(" ")),
        ));
    }
    let code = output.exit_code.unwrap_or(1);
    Ok(Output {
        status: exit_status(code),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

/// Rebuild an exit status from a supervised child's exit code.
fn exit_status(code: i32) -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt as _;
    std::process::ExitStatus::from_raw(code << 8)
}

fn failure(args: &[&str], output: &Output) -> AdapterError {
    AdapterError::new(
        "git",
        None,
        format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        ),
    )
}

impl GitCli {
    /// Discover the worktree root that contains `start`.
    pub fn discover(start: &Path) -> Result<GitCli, AdapterError> {
        GitCli::discover_owned(start, None)
    }

    /// Discover under an owner that can terminate the Git children it
    /// starts. The native hook endpoint uses this so a stalled discovery
    /// cannot outlive its deadline.
    pub fn discover_supervised(
        start: &Path,
        supervisor: Arc<Supervisor>,
    ) -> Result<GitCli, AdapterError> {
        GitCli::discover_owned(start, Some(supervisor))
    }

    fn discover_owned(
        start: &Path,
        supervisor: Option<Arc<Supervisor>>,
    ) -> Result<GitCli, AdapterError> {
        let args = [
            "rev-parse",
            "--path-format=absolute",
            "--show-toplevel",
            "--git-common-dir",
        ];
        let output = run_in(start, &args, supervisor.as_deref())?;
        if !output.status.success() {
            return Err(AdapterError::new(
                "git",
                Some(start.display().to_string()),
                format!(
                    "not inside a Git worktree: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ),
            ));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut lines = text.lines();
        let root = lines.next().filter(|l| !l.is_empty()).ok_or_else(|| {
            AdapterError::new(
                "git",
                None,
                "git rev-parse returned no worktree root; bare repositories are not supported",
            )
        })?;
        let common = lines.next().unwrap_or(".git");
        let root = PathBuf::from(root);
        let empty = run_in(
            &root,
            &["hash-object", "-t", "tree", "/dev/null"],
            supervisor.as_deref(),
        )?;
        if !empty.status.success() {
            return Err(failure(&["hash-object", "-t", "tree", "/dev/null"], &empty));
        }
        let empty_tree = String::from_utf8_lossy(&empty.stdout).trim().to_string();
        Ok(GitCli {
            root,
            common_dir: PathBuf::from(common),
            empty_tree,
            supervisor,
            git_dir: OnceLock::new(),
            history: Mutex::new(HistoryReader::Idle),
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn run(&self, args: &[&str]) -> Result<Output, AdapterError> {
        // Read attributes from the empty tree: worktree and index
        // `.gitattributes` files, and therefore their clean/process filter
        // drivers, never apply to an inspection command.
        let source = format!("--attr-source={}", self.empty_tree);
        let mut full: Vec<&str> = vec![&source];
        full.extend_from_slice(args);
        run_in(&self.root, &full, self.supervisor.as_deref())
    }

    /// Whether `$GIT_DIR/info/attributes` (which `--attr-source` cannot
    /// override) could still attach a filter driver.
    fn info_attributes_declare_filters(&self) -> bool {
        match std::fs::read(self.common_dir.join("info").join("attributes")) {
            Ok(bytes) => String::from_utf8_lossy(&bytes)
                .lines()
                .any(|line| !line.trim_start().starts_with('#') && line.contains("filter=")),
            Err(_) => false,
        }
    }

    /// Dirty-worktree context computed without any content conversion:
    /// untracked files, staged changes, missing tracked files, and tracked
    /// files whose raw worktree bytes differ from their index blob.
    fn worktree_dirty_by_content(&self) -> Result<bool, AdapterError> {
        let others = self.run(&["ls-files", "--others", "--exclude-standard", "-z"])?;
        if others.status.success() && !others.stdout.is_empty() {
            return Ok(true);
        }
        let staged = self.run(&[
            "diff-index",
            "--quiet",
            "--cached",
            "--ignore-submodules=dirty",
            "HEAD",
            "--",
        ])?;
        if staged.status.code() != Some(0) {
            return Ok(true);
        }
        let args = ["ls-files", "--stage", "-z"];
        let index = self.run(&args)?;
        if !index.status.success() {
            return Err(failure(&args, &index));
        }
        let mut nested_cache: std::collections::BTreeMap<String, bool> =
            std::collections::BTreeMap::new();
        let mut entries = Vec::new();
        for entry in index.stdout.split(|b| *b == 0).filter(|e| !e.is_empty()) {
            let text = String::from_utf8_lossy(entry);
            let Some((meta, path)) = text.split_once('\t') else {
                continue;
            };
            let mut fields = meta.split_whitespace();
            let (Some(mode), Some(oid)) = (fields.next(), fields.next()) else {
                continue;
            };
            // Submodules are opaque; so is anything below a nested repository.
            if mode == "160000" || self.inside_nested_repository(path, &mut nested_cache) {
                continue;
            }
            entries.push(IndexEntry {
                mode: mode.to_string(),
                oid: oid.to_string(),
                path: path.to_string(),
            });
        }
        let Some(batch) = self.spawn_raw_reader(&entries) else {
            return self.index_entries_differ_one_by_one(&entries);
        };
        match self.index_entries_differ_batched(&entries, batch) {
            Ok(dirty) => Ok(dirty),
            Err(Interrupted::Failed(error)) => Err(error),
            Err(Interrupted::Fallback) => self.index_entries_differ_one_by_one(&entries),
        }
    }

    /// One `cat-file --batch` reader for the raw comparison. `None` means
    /// the comparison reads one object at a time.
    fn spawn_raw_reader(&self, entries: &[IndexEntry]) -> Option<CatFileBatch> {
        if entries.is_empty() || !entries.iter().all(|entry| frameable(&entry.oid)) {
            return None;
        }
        // The same attribute source and environment as a per-object read.
        let source = format!("--attr-source={}", self.empty_tree);
        let mut cmd = command(&self.root);
        cmd.args([source.as_str(), "cat-file", "--batch"]);
        CatFileBatch::spawn(cmd, self.supervisor.clone()).ok()
    }

    /// The checks of one entry that need no object: no symlink ancestor, the
    /// recorded type, and the executable bit or the link text. `None` means
    /// the entry changed, and no object needs to be read.
    fn local_entry(&self, entry: &IndexEntry) -> Option<LocalEntry> {
        // Never follow a symlink ancestor: the entry cannot be verified
        // without leaving the project, so it counts as changed.
        if crate::fs::has_symlink_ancestor(&self.root, &entry.path).unwrap_or(true) {
            return None;
        }
        let full = self.root.join(&entry.path);
        let kind = crate::fs::kind_of(&full).ok()?;
        match (entry.mode.as_str(), kind) {
            // A committed symlink is compared by its link text, never its target.
            ("120000", FileKind::Symlink) => {
                let target = std::fs::read_link(&full).ok()?;
                let link = target.into_os_string().into_encoded_bytes();
                Some(LocalEntry {
                    size: link.len() as u64,
                    link: Some(link),
                    full,
                })
            }
            ("100644" | "100755", FileKind::Regular) => {
                let meta = std::fs::metadata(&full).ok()?;
                let executable = {
                    use std::os::unix::fs::PermissionsExt;
                    meta.permissions().mode() & 0o111 != 0
                };
                if executable != (entry.mode == "100755") {
                    return None;
                }
                Some(LocalEntry {
                    size: meta.len(),
                    link: None,
                    full,
                })
            }
            // Type changed (missing, special file, directory, or a link where a
            // file was tracked): dirty without opening anything.
            _ => None,
        }
    }

    /// The per-object comparison: in index order, local checks first, then
    /// one `cat-file blob` process for the object. The first change ends it.
    fn index_entries_differ_one_by_one(
        &self,
        entries: &[IndexEntry],
    ) -> Result<bool, AdapterError> {
        for entry in entries {
            let Some(local) = self.local_entry(entry) else {
                return Ok(true);
            };
            let current = match local.link {
                Some(target) => target,
                None => match std::fs::read(&local.full) {
                    Ok(bytes) => bytes,
                    Err(_) => return Ok(true),
                },
            };
            let stored = self.run(&["cat-file", "blob", &entry.oid])?;
            if !stored.status.success() || stored.stdout != current {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The batched comparison. An entry is queued only after its local
    /// checks pass, so a change found without an object ends the walk with
    /// nothing requested for it. The lookahead holds at most
    /// [`RAW_WINDOW`] entries and stops growing once [`RAW_WINDOW_BYTES`] of
    /// expected content is outstanding, so an early change leaves little
    /// unread output behind.
    fn index_entries_differ_batched(
        &self,
        entries: &[IndexEntry],
        mut batch: CatFileBatch,
    ) -> Result<bool, Interrupted> {
        let failed = |error: BatchError| match error {
            BatchError::Protocol(_) => Interrupted::Fallback,
            BatchError::Cancelled => Interrupted::Failed(AdapterError::new(
                "cancelled",
                Some("git".to_string()),
                "the endpoint stopped before this command finished".to_string(),
            )),
            BatchError::Deadline => Interrupted::Failed(AdapterError::new(
                "git",
                Some(self.root.display().to_string()),
                "git cat-file --batch did not finish within the deadline",
            )),
        };
        let mut pending: std::collections::VecDeque<(&IndexEntry, LocalEntry)> =
            std::collections::VecDeque::new();
        let mut pending_bytes = 0u64;
        let mut next = entries.iter();
        let mut more = true;
        loop {
            while more
                && pending.len() < RAW_WINDOW
                && (pending.is_empty() || pending_bytes < RAW_WINDOW_BYTES)
            {
                let Some(entry) = next.next() else {
                    more = false;
                    break;
                };
                let Some(local) = self.local_entry(entry) else {
                    return Ok(true);
                };
                batch.queue(entry.oid.clone()).map_err(failed)?;
                pending_bytes += local.size;
                pending.push_back((entry, local));
            }
            let Some((entry, local)) = pending.pop_front() else {
                return Ok(false);
            };
            pending_bytes -= local.size;
            let size = match batch.header(&entry.oid, None).map_err(failed)? {
                Header::Object { kind, size, .. } if kind == "blob" => size,
                // A missing object is a change, as a failed `cat-file blob` was.
                Header::Missing => return Ok(true),
                // Another type, such as a replacement tag: `git cat-file
                // blob` peels a tag to its blob and the batch does not, so
                // the per-object comparison decides. The reader is dropped
                // with its unread content.
                Header::Object { .. } => return Err(Interrupted::Fallback),
            };
            let current = match local.link {
                Some(target) => target,
                None => {
                    // The size decides most changes without opening the file.
                    match std::fs::symlink_metadata(&local.full) {
                        Ok(meta) if meta.file_type().is_file() && meta.len() == size => {}
                        _ => return Ok(true),
                    }
                    match std::fs::read(&local.full) {
                        Ok(bytes) => bytes,
                        Err(_) => return Ok(true),
                    }
                }
            };
            if current.len() as u64 != size {
                return Ok(true);
            }
            if batch.content(size, None).map_err(failed)? != current {
                return Ok(true);
            }
        }
    }

    /// Read `spec` through the shared historical reader. The read has the
    /// same environment, limit, and deadline as the per-object read.
    fn batched_historical_blob(&self, spec: &str, limit: u64, deadline: Instant) -> BatchRead {
        let Ok(mut reader) = self.history.lock() else {
            return BatchRead::Fallback;
        };
        if Instant::now() >= deadline {
            return BatchRead::Done(Err(history_limit()));
        }
        if matches!(*reader, HistoryReader::Idle) {
            let mut cmd = command(&self.root);
            cmd.env("GIT_NO_REPLACE_OBJECTS", "1")
                .args(["cat-file", "--batch"]);
            // Its own group, as the supervised per-object read had, so a
            // deadline or the end of the command ends any descendant too.
            *reader = match CatFileBatch::spawn_in_own_group(cmd) {
                Ok(batch) => HistoryReader::Running(batch),
                Err(_) => HistoryReader::Unavailable,
            };
        }
        let HistoryReader::Running(batch) = &mut *reader else {
            return BatchRead::Fallback;
        };
        let result = batch
            .send(spec)
            .and_then(|()| match batch.header(spec, Some(deadline))? {
                Header::Missing => Ok(HistoricalObject::Missing),
                Header::Object { kind, .. } if kind != "blob" => Ok(HistoricalObject::Other),
                Header::Object { size, .. } if size > limit => Ok(HistoricalObject::TooLarge),
                Header::Object { size, .. } => batch
                    .content(size, Some(deadline))
                    .map(HistoricalObject::Blob),
            });
        match result {
            Ok(HistoricalObject::Blob(bytes)) => BatchRead::Done(Ok(Some(bytes))),
            Ok(HistoricalObject::Missing) => BatchRead::Done(Ok(None)),
            // Unread content stays in the pipe, so the process is discarded,
            // and the per-object read answers exactly as before.
            Ok(HistoricalObject::Other) => {
                *reader = HistoryReader::Idle;
                BatchRead::Fallback
            }
            Ok(HistoricalObject::TooLarge) | Err(BatchError::Deadline) => {
                *reader = HistoryReader::Idle;
                BatchRead::Done(Err(history_limit()))
            }
            Err(BatchError::Protocol(_) | BatchError::Cancelled) => {
                *reader = HistoryReader::Unavailable;
                BatchRead::Fallback
            }
        }
    }

    /// Whether any ancestor directory of `path` below the root holds a `.git`
    /// entry (a nested repository or an initialized submodule).
    fn inside_nested_repository(
        &self,
        path: &str,
        cache: &mut std::collections::BTreeMap<String, bool>,
    ) -> bool {
        let mut prefix = String::new();
        let components: Vec<&str> = path.split('/').collect();
        for component in components.iter().take(components.len().saturating_sub(1)) {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(component);
            let nested = *cache.entry(prefix.clone()).or_insert_with(|| {
                std::fs::symlink_metadata(self.root.join(&prefix).join(".git")).is_ok()
            });
            if nested {
                return true;
            }
        }
        false
    }
}

impl GitRepository for GitCli {
    fn eligible_paths(&self) -> Result<Vec<Vec<u8>>, AdapterError> {
        let args = [
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ];
        let output = self.run(&args)?;
        if !output.status.success() {
            return Err(failure(&args, &output));
        }
        Ok(output
            .stdout
            .split(|b| *b == 0)
            .filter(|p| !p.is_empty())
            .map(|p| p.to_vec())
            .collect())
    }

    fn unmerged_paths(&self) -> Result<Vec<String>, AdapterError> {
        let args = ["ls-files", "--unmerged", "-z"];
        let output = self.run(&args)?;
        if !output.status.success() {
            return Err(failure(&args, &output));
        }
        let mut paths: Vec<String> = output
            .stdout
            .split(|b| *b == 0)
            .filter(|e| !e.is_empty())
            .filter_map(|entry| {
                entry
                    .iter()
                    .position(|b| *b == b'\t')
                    .map(|i| String::from_utf8_lossy(&entry[i + 1..]).into_owned())
            })
            .collect();
        paths.sort();
        paths.dedup();
        Ok(paths)
    }

    fn sparse_checkout_enabled(&self) -> Result<bool, AdapterError> {
        let output = self.run(&["config", "--bool", "--get", "core.sparseCheckout"])?;
        Ok(output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true")
    }

    fn head_commit(&self) -> Result<Option<String>, AdapterError> {
        let output = self.run(&["rev-parse", "--verify", "--quiet", "HEAD^{commit}"])?;
        if output.status.success() {
            Ok(Some(
                String::from_utf8_lossy(&output.stdout).trim().to_string(),
            ))
        } else {
            Ok(None)
        }
    }

    fn worktree_dirty(&self) -> Result<bool, AdapterError> {
        if self.info_attributes_declare_filters() {
            // `info/attributes` still applies under `--attr-source`; avoid every
            // content-converting Git path and compare raw bytes instead.
            return self.worktree_dirty_by_content();
        }
        let args = [
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignore-submodules=dirty",
            "-z",
        ];
        let output = self.run(&args)?;
        if !output.status.success() {
            return Err(failure(&args, &output));
        }
        Ok(!output.stdout.is_empty())
    }

    fn read_blob(&self, commit: &str, path: &str) -> Result<Option<Vec<u8>>, AdapterError> {
        if commit.is_empty() || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(None);
        }
        let spec = format!("{commit}:{path}");
        let output = self.run(&["cat-file", "blob", &spec])?;
        if output.status.success() {
            Ok(Some(output.stdout))
        } else {
            Ok(None)
        }
    }

    fn historical_blob(
        &self,
        commit: &str,
        path: &str,
        limit: u64,
        deadline: std::time::Instant,
    ) -> Result<Option<Vec<u8>>, AdapterError> {
        if commit.is_empty() || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
            return Ok(None);
        }
        let spec = format!("{commit}:{path}");
        if frameable(&spec)
            && let BatchRead::Done(result) = self.batched_historical_blob(&spec, limit, deadline)
        {
            return result;
        }
        let mut cmd = command(&self.root);
        cmd.env("GIT_NO_REPLACE_OBJECTS", "1")
            .args(["cat-file", "blob", &spec]);
        let owner = Supervisor::new(deadline);
        let output = owner.run(cmd, owner.remaining(), limit, false)?;
        if output.timed_out || output.output_truncated {
            return Err(history_limit());
        }
        Ok((output.exit_code == Some(0)).then_some(output.stdout))
    }

    fn recent_commits(
        &self,
        limit: usize,
        deadline: std::time::Instant,
    ) -> Result<Vec<String>, AdapterError> {
        let mut cmd = command(&self.root);
        cmd.env("GIT_NO_REPLACE_OBJECTS", "1").args([
            "rev-list",
            &format!("--max-count={limit}"),
            "HEAD",
            "--",
        ]);
        let owner = Supervisor::new(deadline);
        let output = owner.run(cmd, owner.remaining(), (limit as u64 + 1) * 66, false)?;
        if output.timed_out || output.output_truncated {
            return Err(AdapterError::new(
                "history_limit",
                None,
                "Historical candidate budget exhausted.",
            ));
        }
        if output.exit_code != Some(0) {
            return Ok(vec![]);
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|s| matches!(s.len(), 40 | 64) && s.bytes().all(|b| b.is_ascii_hexdigit()))
            .map(str::to_string)
            .collect())
    }

    fn explain_ignore(&self, path: &str) -> Result<Option<String>, AdapterError> {
        let output = self.run(&["check-ignore", "--verbose", "--non-matching", "--", path])?;
        let text = String::from_utf8_lossy(&output.stdout);
        let line = text.lines().next().unwrap_or("");
        if !output.status.success() || line.starts_with("::") {
            return Ok(None);
        }
        // Format: <source>:<linenum>:<pattern>\t<path>
        Ok(line.split('\t').next().map(|s| s.to_string()))
    }

    fn git_dir(&self) -> Result<String, AdapterError> {
        if let Some(dir) = self.git_dir.get() {
            return Ok(dir.clone());
        }
        let args = ["rev-parse", "--path-format=absolute", "--absolute-git-dir"];
        let output = self.run(&args)?;
        if !output.status.success() {
            return Err(failure(&args, &output));
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let path = text.lines().next().unwrap_or("").trim();
        if path.is_empty() {
            return Err(AdapterError::new(
                "git",
                None,
                "git rev-parse --absolute-git-dir returned no path",
            ));
        }
        Ok(self.git_dir.get_or_init(|| path.to_string()).clone())
    }

    fn private_path(&self, relative: &str) -> Result<String, AdapterError> {
        // `git rev-parse --git-path` resolves symlinked components, so a
        // substituted directory inside the metadata location would silently
        // move the destination. The path is composed from the metadata
        // directory instead; the filesystem adapter then refuses any
        // symlink between that directory and the target.
        if relative.is_empty()
            || relative.starts_with('/')
            || relative
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(AdapterError::new(
                "git",
                Some(relative.to_string()),
                "a private path must be a normalized relative path",
            ));
        }
        let mut path = PathBuf::from(self.git_dir()?);
        for part in relative.split('/') {
            path.push(part);
        }
        Ok(path.display().to_string())
    }

    fn main_worktree(&self) -> Result<String, AdapterError> {
        // The common Git directory belongs to the main checkout. Its parent
        // is that checkout's worktree root, unless this repository is the
        // main checkout itself.
        let common = if self.common_dir.is_absolute() {
            self.common_dir.clone()
        } else {
            self.root.join(&self.common_dir)
        };
        let parent = common.parent().ok_or_else(|| {
            AdapterError::new(
                "git",
                Some(common.display().to_string()),
                "the common Git directory has no parent worktree",
            )
        })?;
        let canonical = parent.canonicalize().map_err(|err| {
            AdapterError::new("git", Some(parent.display().to_string()), err.to_string())
        })?;
        Ok(canonical.display().to_string())
    }
}
