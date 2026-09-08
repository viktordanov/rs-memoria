//! Git subprocess adapter with argument arrays and NUL-delimited paths.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::Arc;

use memoria_application::ports::{AdapterError, FileKind, GitRepository};

use crate::bounded_process::Supervisor;

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
            // Never follow a symlink ancestor: the entry cannot be verified
            // without leaving the project, so it counts as changed.
            if crate::fs::has_symlink_ancestor(&self.root, path).unwrap_or(true) {
                return Ok(true);
            }
            let full = self.root.join(path);
            let kind = match crate::fs::kind_of(&full) {
                Ok(kind) => kind,
                Err(_) => return Ok(true),
            };
            match (mode, kind) {
                // A committed symlink is compared by its link text, never its target.
                ("120000", FileKind::Symlink) => {
                    let Ok(target) = std::fs::read_link(&full) else {
                        return Ok(true);
                    };
                    let blob = self.run(&["cat-file", "blob", oid])?;
                    if !blob.status.success()
                        || blob.stdout != target.as_os_str().as_encoded_bytes()
                    {
                        return Ok(true);
                    }
                }
                ("100644" | "100755", FileKind::Regular) => {
                    let executable = std::fs::metadata(&full)
                        .map(|m| {
                            use std::os::unix::fs::PermissionsExt;
                            m.permissions().mode() & 0o111 != 0
                        })
                        .unwrap_or(false);
                    if executable != (mode == "100755") {
                        return Ok(true);
                    }
                    let Ok(worktree) = std::fs::read(&full) else {
                        return Ok(true);
                    };
                    let blob = self.run(&["cat-file", "blob", oid])?;
                    if !blob.status.success() || blob.stdout != worktree {
                        return Ok(true);
                    }
                }
                // Type changed (missing, special file, directory, or a link where a
                // file was tracked): dirty without opening anything.
                _ => return Ok(true),
            }
        }
        Ok(false)
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
        Ok(path.to_string())
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
