//! Saved review artifacts, written only outside the project.

use std::fs;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};

use memoria_application::ports::{AdapterError, ArtifactStore, SaveFailure};

/// Writes saved review artifacts with exclusive creation and mode 0600.
#[derive(Debug, Clone)]
pub struct FsArtifactStore {
    invocation: PathBuf,
    worktree: PathBuf,
}

impl FsArtifactStore {
    /// `invocation` resolves relative destinations; `worktree` is the Git
    /// worktree root that a destination must stay outside of.
    pub fn new(invocation: PathBuf, worktree: PathBuf) -> FsArtifactStore {
        FsArtifactStore {
            invocation,
            worktree,
        }
    }
}

fn adapter(operation: &str, path: &Path, err: io::Error) -> AdapterError {
    AdapterError::new(operation, Some(path.display().to_string()), err.to_string())
}

fn open_exclusive(path: &Path) -> io::Result<fs::File> {
    let mut options = fs::OpenOptions::new();
    // `create_new` refuses an existing name, including a symlink, dangling
    // or not, so the final name is never followed.
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

impl ArtifactStore for FsArtifactStore {
    fn canonical_directory(&self, path: &str) -> Result<Option<String>, AdapterError> {
        let requested = self.invocation.join(path);
        match fs::canonicalize(&requested) {
            Ok(canonical) => {
                if canonical.is_dir() {
                    Ok(Some(canonical.display().to_string()))
                } else {
                    Ok(None)
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(err) => Err(adapter("canonicalize", &requested, err)),
        }
    }

    fn worktree_root(&self) -> Result<String, AdapterError> {
        fs::canonicalize(&self.worktree)
            .map(|path| path.display().to_string())
            .map_err(|err| adapter("canonicalize", &self.worktree, err))
    }

    fn create_new(&self, directory: &str, name: &str, bytes: &[u8]) -> Result<String, SaveFailure> {
        let path = Path::new(directory).join(name);
        let mut file = match open_exclusive(&path) {
            Ok(file) => file,
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {
                return Err(SaveFailure::Exists);
            }
            Err(err) => {
                return Err(SaveFailure::Io {
                    error: adapter("create", &path, err),
                    leftover: None,
                });
            }
        };
        let written = file.write_all(bytes).and_then(|()| file.sync_all());
        drop(file);
        match written {
            Ok(()) => Ok(path.display().to_string()),
            Err(err) => {
                let leftover = fs::remove_file(&path)
                    .err()
                    .map(|_| path.display().to_string());
                Err(SaveFailure::Io {
                    error: adapter("write", &path, err),
                    leftover,
                })
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclusive_creation_mode_and_symlinks() {
        let outside = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let store =
            FsArtifactStore::new(outside.path().to_path_buf(), project.path().to_path_buf());
        let dir = store.canonical_directory(".").unwrap().unwrap();
        let path = store.create_new(&dir, "a.json", b"{}\n").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"{}\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(
            store.create_new(&dir, "a.json", b"x"),
            Err(SaveFailure::Exists)
        );
        assert_eq!(fs::read(&path).unwrap(), b"{}\n");
        #[cfg(unix)]
        {
            let target = outside.path().join("target.txt");
            std::os::unix::fs::symlink(&target, outside.path().join("link.json")).unwrap();
            assert_eq!(
                store.create_new(&dir, "link.json", b"x"),
                Err(SaveFailure::Exists)
            );
            assert!(
                !target.exists(),
                "a symlink at the final name is never followed"
            );
        }
        assert_eq!(store.canonical_directory("missing").unwrap(), None);
        assert_eq!(store.canonical_directory("a.json").unwrap(), None);
    }
}
