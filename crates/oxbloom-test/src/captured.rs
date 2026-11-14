use std::io::Write;
use std::sync::{Arc, Mutex};

/// A cloneable in-memory writer whose contents can be read back.
///
/// All clones share one buffer, so a clone handed to the code under test can be inspected
/// through the original.
///
/// # Examples
///
/// ```
/// use std::io::Write;
/// use oxbloom_test::Captured;
///
/// let captured = Captured::new();
/// let mut writer = captured.clone();
/// writer.write_all(b"hello\n").unwrap();
/// assert_eq!(captured.contents(), "hello\n");
/// ```
#[derive(Clone, Default)]
pub struct Captured(Arc<Mutex<Vec<u8>>>);

impl std::fmt::Debug for Captured {
    /// Shows the number of bytes written, not the bytes themselves.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io::Write;
    /// let mut captured = oxbloom_test::Captured::new();
    /// captured.write_all(b"abc").unwrap();
    /// assert_eq!(format!("{captured:?}"), "Captured { len: 3 }");
    /// ```
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Captured")
            .field("len", &self.buffer().len())
            .finish()
    }
}

impl Captured {
    /// Creates an empty buffer.
    ///
    /// # Examples
    ///
    /// ```
    /// assert_eq!(oxbloom_test::Captured::new().contents(), "");
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    fn buffer(&self) -> std::sync::MutexGuard<'_, Vec<u8>> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Returns everything written so far, decoding invalid UTF-8 lossily.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io::Write;
    /// let mut captured = oxbloom_test::Captured::new();
    /// captured.write_all(b"abc").unwrap();
    /// assert_eq!(captured.contents(), "abc");
    /// ```
    pub fn contents(&self) -> String {
        String::from_utf8_lossy(&self.buffer()).into_owned()
    }

    /// Returns the written text split on newlines, without a trailing empty entry.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io::Write;
    /// let mut captured = oxbloom_test::Captured::new();
    /// captured.write_all(b"a\nb\n").unwrap();
    /// assert_eq!(captured.lines(), vec!["a", "b"]);
    /// ```
    pub fn lines(&self) -> Vec<String> {
        self.contents().lines().map(str::to_owned).collect()
    }

    /// Returns whether the written text contains `needle`.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io::Write;
    /// let mut captured = oxbloom_test::Captured::new();
    /// captured.write_all(b"started").unwrap();
    /// assert!(captured.contains("start"));
    /// ```
    pub fn contains(&self, needle: &str) -> bool {
        self.contents().contains(needle)
    }

    /// Discards everything written so far.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::io::Write;
    /// let mut captured = oxbloom_test::Captured::new();
    /// captured.write_all(b"abc").unwrap();
    /// captured.clear();
    /// assert_eq!(captured.contents(), "");
    /// ```
    pub fn clear(&self) {
        self.buffer().clear();
    }
}

impl Write for Captured {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.buffer().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn captured_collects_writes_from_clones() {
        let a = Captured::new();
        let mut b = a.clone();
        b.write_all(b"one\ntwo\n").unwrap();
        assert_eq!(a.lines(), vec!["one", "two"]);
        assert!(a.contains("two"));
    }

    #[test]
    fn debug_shows_the_length_not_the_bytes() {
        let mut c = Captured::new();
        c.write_all(b"secret").unwrap();
        let text = format!("{c:?}");
        assert_eq!(text, "Captured { len: 6 }");
    }

    #[test]
    fn clear_empties_the_buffer() {
        let mut c = Captured::new();
        c.write_all(b"x").unwrap();
        c.clear();
        assert_eq!(c.contents(), "");
    }
}
