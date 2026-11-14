use std::io::Write;
use std::sync::Mutex;

/// Line-oriented diagnostic sink; write errors are ignored.
pub(crate) struct Output {
    writer: Mutex<Box<dyn Write + Send>>,
    color: bool,
}

impl Output {
    pub(crate) fn new(writer: impl Write + Send + 'static) -> Self {
        Self {
            writer: Mutex::new(Box::new(writer)),
            color: false,
        }
    }

    /// Standard error. ANSI colors are on only when stderr is a terminal and
    /// `NO_COLOR` is unset or empty.
    pub(crate) fn stderr() -> Self {
        use std::io::IsTerminal;
        let mut out = Self::new(std::io::stderr());
        out.color = std::io::stderr().is_terminal()
            && std::env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
        out
    }

    /// Whether the target takes ANSI colors.
    pub(crate) fn color(&self) -> bool {
        self.color
    }

    pub(crate) fn line(&self, text: &str) {
        let mut w = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let _ = writeln!(w, "{text}");
        let _ = w.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    struct Failing;
    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("no"))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Err(std::io::Error::other("no"))
        }
    }

    #[derive(Clone, Default)]
    struct Buf(Arc<Mutex<Vec<u8>>>);
    impl Write for Buf {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn line_appends_newline() {
        let buf = Buf::default();
        let out = Output::new(buf.clone());
        out.line("a");
        out.line("b");
        assert_eq!(&*buf.0.lock().unwrap(), b"a\nb\n");
    }

    #[test]
    fn write_errors_are_ignored() {
        Output::new(Failing).line("x");
    }
}
