use std::sync::{Arc, Mutex};

/// A cloneable ordered event log.
///
/// All clones share one list, so callbacks that each hold a clone can record events in order.
///
/// # Examples
///
/// ```
/// use oxbloom_test::Log;
///
/// let log = Log::new();
/// log.clone().push("started");
/// assert_eq!(log.entries(), vec!["started"]);
/// ```
#[derive(Clone, Default, Debug)]
pub struct Log(Arc<Mutex<Vec<String>>>);

impl Log {
    /// Creates an empty log.
    ///
    /// # Examples
    ///
    /// ```
    /// assert!(oxbloom_test::Log::new().entries().is_empty());
    /// ```
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends an entry.
    ///
    /// # Examples
    ///
    /// ```
    /// let log = oxbloom_test::Log::new();
    /// log.push("a");
    /// log.push(String::from("b"));
    /// assert_eq!(log.entries(), vec!["a", "b"]);
    /// ```
    pub fn push(&self, entry: impl Into<String>) {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(entry.into());
    }

    /// Returns a copy of the entries in the order they were pushed.
    ///
    /// # Examples
    ///
    /// ```
    /// let log = oxbloom_test::Log::new();
    /// log.push("a");
    /// assert_eq!(log.entries(), vec!["a"]);
    /// ```
    pub fn entries(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_keeps_order() {
        let log = Log::new();
        log.clone().push("a");
        log.push(String::from("b"));
        assert_eq!(log.entries(), vec!["a", "b"]);
    }
}
