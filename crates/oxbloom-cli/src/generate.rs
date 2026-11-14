use std::fs;
use std::path::{Path, PathBuf};

use std::fmt;

use anyhow::Context;

/// Why `write_project` failed.
#[derive(Debug)]
pub enum GenerateError {
    /// The target path is occupied (a file or a non-empty directory).
    Conflict(String),
    Io(anyhow::Error),
}

impl fmt::Display for GenerateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict(m) => f.write_str(m),
            Self::Io(e) => write!(f, "{e:#}"),
        }
    }
}

impl std::error::Error for GenerateError {}

impl From<anyhow::Error> for GenerateError {
    fn from(e: anyhow::Error) -> Self {
        Self::Io(e)
    }
}

type Result<T> = std::result::Result<T, GenerateError>;

/// Writes `files` (relative path, contents) to `<parent>/<name>` and returns that path.
///
/// Creates `parent` if missing. Refuses a target that is a file or a non-empty directory.
/// On failure, removes everything this call created.
pub fn write_project(parent: &Path, name: &str, files: &[(String, String)]) -> Result<PathBuf> {
    let target = parent.join(name);
    // `symlink_metadata` so a dangling symlink counts as occupied instead of absent.
    let created_target = match fs::symlink_metadata(&target) {
        Ok(meta) => {
            if !meta.is_dir() {
                return Err(GenerateError::Conflict(format!(
                    "{} exists and is not a directory",
                    target.display()
                )));
            }
            if fs::read_dir(&target)
                .map_err(anyhow::Error::from)?
                .next()
                .is_some()
            {
                return Err(GenerateError::Conflict(format!(
                    "{} exists and is not empty",
                    target.display()
                )));
            }
            false
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
        Err(e) => return Err(GenerateError::Io(anyhow::Error::from(e))),
    };
    fs::create_dir_all(parent)
        .with_context(|| format!("create {}", parent.display()))
        .map_err(GenerateError::Io)?;

    let result = write_all(&target, files).map_err(GenerateError::Io);
    if result.is_err() {
        rollback(&target, created_target);
    }
    result.map(|()| target)
}

fn write_all(target: &Path, files: &[(String, String)]) -> anyhow::Result<()> {
    fs::create_dir_all(target).with_context(|| format!("create {}", target.display()))?;
    for (rel, contents) in files {
        let path = target.join(rel);
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
        }
        fs::write(&path, contents).with_context(|| format!("write {}", path.display()))?;
    }
    Ok(())
}

/// The target was empty or absent before this run, so clearing it restores the prior state.
fn rollback(target: &Path, created_target: bool) {
    if created_target {
        let _ = fs::remove_dir_all(target);
    } else if let Ok(entries) = fs::read_dir(target) {
        for entry in entries.flatten() {
            let p = entry.path();
            let _ = if p.is_dir() {
                fs::remove_dir_all(&p)
            } else {
                fs::remove_file(&p)
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn files() -> Vec<(String, String)> {
        vec![
            ("Cargo.toml".into(), "a".into()),
            ("src/lib.rs".into(), "b".into()),
            ("src/main.rs".into(), "c".into()),
        ]
    }

    #[test]
    fn writes_all_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = write_project(tmp.path(), "demo", &files()).unwrap();
        assert_eq!(root, tmp.path().join("demo"));
        for (p, c) in files() {
            assert_eq!(fs::read_to_string(root.join(p)).unwrap(), c);
        }
    }

    #[test]
    fn creates_missing_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let parent = tmp.path().join("a/b");
        write_project(&parent, "demo", &files()).unwrap();
        assert!(parent.join("demo/Cargo.toml").exists());
    }

    #[test]
    fn refuses_non_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("demo");
        fs::create_dir(&target).unwrap();
        fs::write(target.join("keep.txt"), "x").unwrap();
        assert!(write_project(tmp.path(), "demo", &files()).is_err());
        assert_eq!(fs::read_to_string(target.join("keep.txt")).unwrap(), "x");
        assert!(!target.join("Cargo.toml").exists());
    }

    #[test]
    fn refuses_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("demo");
        fs::write(&target, "orig").unwrap();
        assert!(write_project(tmp.path(), "demo", &files()).is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "orig");
    }

    #[test]
    fn accepts_existing_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir(tmp.path().join("demo")).unwrap();
        write_project(tmp.path(), "demo", &files()).unwrap();
        assert!(tmp.path().join("demo/src/lib.rs").exists());
    }

    #[test]
    fn cleans_up_on_failure() {
        let tmp = tempfile::tempdir().unwrap();
        let mut f = files();
        f.push(("src/lib.rs/x".into(), "bad".into())); // file where a dir is needed
        assert!(write_project(tmp.path(), "demo", &f).is_err());
        assert!(!tmp.path().join("demo").exists());
    }

    #[test]
    fn failure_in_preexisting_empty_dir_leaves_it_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("demo");
        fs::create_dir(&target).unwrap();
        let mut f = files();
        f.push(("src/lib.rs/x".into(), "bad".into()));
        assert!(write_project(tmp.path(), "demo", &f).is_err());
        assert!(target.is_dir());
        assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
    }

    #[test]
    fn deterministic_across_runs() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        write_project(a.path(), "demo", &files()).unwrap();
        write_project(b.path(), "demo", &files()).unwrap();
        for (p, _) in files() {
            assert_eq!(
                fs::read(a.path().join("demo").join(&p)).unwrap(),
                fs::read(b.path().join("demo").join(&p)).unwrap()
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn dangling_symlink_target_is_a_conflict_and_survives() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("demo");
        std::os::unix::fs::symlink("/nonexistent/x", &target).unwrap();
        let err = write_project(tmp.path(), "demo", &files()).unwrap_err();
        assert!(matches!(err, GenerateError::Conflict(_)), "{err}");
        assert!(fs::symlink_metadata(&target).is_ok(), "symlink was deleted");
    }
}
