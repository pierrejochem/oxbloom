use std::path::Path;

/// A temporary directory of configuration files, removed when dropped.
///
/// Removal ignores errors, so a directory that vanished earlier does not cause a panic.
///
/// # Examples
///
/// ```
/// use oxbloom_test::TempConfigDir;
///
/// let dir = TempConfigDir::new().file("application.toml", "name = \"demo\"");
/// assert!(dir.path().join("application.toml").is_file());
/// ```
#[derive(Debug)]
pub struct TempConfigDir(tempfile::TempDir);

impl TempConfigDir {
    /// Creates an empty temporary directory.
    ///
    /// # Panics
    ///
    /// Panics if the directory cannot be created.
    ///
    /// # Examples
    ///
    /// ```
    /// let dir = oxbloom_test::TempConfigDir::new();
    /// assert!(dir.path().is_dir());
    /// ```
    #[allow(clippy::new_without_default)]
    pub fn new() -> TempConfigDir {
        TempConfigDir(tempfile::tempdir().expect("a temporary directory can be created"))
    }

    /// Writes a file with `text` at `name`, creating parent directories such as `a` for `a/b.toml`.
    ///
    /// # Panics
    ///
    /// Panics if `name` is absolute or contains a `..` component, because the file would then
    /// land outside the directory, and if the file cannot be written.
    ///
    /// # Examples
    ///
    /// ```
    /// let dir = oxbloom_test::TempConfigDir::new().file("a/b.toml", "x = 1");
    /// assert_eq!(std::fs::read_to_string(dir.path().join("a/b.toml")).unwrap(), "x = 1");
    /// ```
    ///
    /// A name that leaves the directory is refused:
    ///
    /// ```should_panic
    /// // Panics: the name must be a relative path inside the directory.
    /// let _ = oxbloom_test::TempConfigDir::new().file("../escape.toml", "x = 1");
    /// ```
    pub fn file(self, name: &str, text: &str) -> Self {
        let inside = std::path::Path::new(name).components().all(|c| {
            matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        });
        assert!(
            inside,
            "the file name `{name}` must be a relative path inside the directory, without `..`"
        );
        let target = self.0.path().join(name);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).expect("parent directories can be created");
        }
        std::fs::write(&target, text).expect("the file can be written");
        self
    }

    /// Returns the directory path.
    ///
    /// # Examples
    ///
    /// ```
    /// let dir = oxbloom_test::TempConfigDir::new();
    /// assert!(dir.path().exists());
    /// ```
    pub fn path(&self) -> &Path {
        self.0.path()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_temp_config_dir_holds_its_files_and_is_removed_on_drop() {
        let dir = TempConfigDir::new().file("application.toml", "a = 1");
        let path = dir.path().to_path_buf();
        assert!(path.join("application.toml").exists());
        drop(dir);
        assert!(!path.exists());
    }

    #[test]
    fn a_temp_config_dir_creates_parent_directories() {
        let dir = TempConfigDir::new().file("a/b.toml", "x");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a/b.toml")).unwrap(),
            "x"
        );
    }

    #[test]
    #[should_panic(expected = "must be a relative path inside the directory")]
    fn an_absolute_file_name_is_rejected() {
        let _ = TempConfigDir::new().file("/tmp/oxbloom-escape.toml", "x");
    }

    #[test]
    #[should_panic(expected = "must be a relative path inside the directory")]
    fn a_parent_component_in_a_file_name_is_rejected() {
        let _ = TempConfigDir::new().file("a/../../escape.toml", "x");
    }

    #[test]
    fn dropping_a_temp_config_dir_whose_directory_vanished_does_not_panic() {
        let dir = TempConfigDir::new();
        std::fs::remove_dir_all(dir.path()).unwrap();
        drop(dir);
    }
}
