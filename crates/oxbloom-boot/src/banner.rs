use std::borrow::Cow;

/// The startup banner: text written to the output target before the
/// container is built.
///
/// The default banner is the Oxbloom flower and wordmark in ASCII art, with
/// a last line `:: Oxbloom ::` and the version `(v{version})` aligned to the
/// right edge of the art. The config property
/// `oxbloom.main.banner-mode` overrides the builder setting: `console` prints
/// (the default banner if the builder turned it off), `off` prints nothing.
///
/// ```
/// use oxbloom_boot::Banner;
///
/// assert_eq!(Banner::default(), Banner::default());
/// assert_ne!(Banner::default(), Banner::off());
/// ```
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Banner {
    content: Content,
}

/// What a banner prints. The default art is built when it is printed, so a
/// banner stays as small as a plain string.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Content {
    Default,
    Off,
    Custom(String),
}

impl Default for Banner {
    /// The ASCII art banner, ending with `:: Oxbloom ::` and the version
    /// `(v{CARGO_PKG_VERSION})`.
    ///
    /// ```
    /// use oxbloom_boot::Banner;
    ///
    /// assert_ne!(Banner::default(), Banner::off());
    /// assert_eq!(Banner::default(), Banner::default());
    /// ```
    fn default() -> Self {
        Self {
            content: Content::Default,
        }
    }
}

impl Banner {
    /// A banner that prints nothing.
    ///
    /// ```
    /// use oxbloom_boot::Banner;
    ///
    /// assert_ne!(Banner::off(), Banner::default());
    /// ```
    pub fn off() -> Self {
        Self {
            content: Content::Off,
        }
    }

    /// A banner with custom text, written as one line.
    ///
    /// ```
    /// use oxbloom_boot::Banner;
    ///
    /// let banner = Banner::text("My service 2.0");
    /// assert_eq!(banner, Banner::text(String::from("My service 2.0")));
    /// ```
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            content: Content::Custom(text.into()),
        }
    }

    /// The text to print, `None` for an off banner. `color` selects the
    /// brand colors, which only the default banner has.
    pub(crate) fn line(&self, color: bool) -> Option<Cow<'_, str>> {
        match &self.content {
            Content::Default => Some(Cow::Owned(default_art(color))),
            Content::Off => None,
            Content::Custom(text) => Some(Cow::Borrowed(text)),
        }
    }
}

/// The default banner. With `color`, the petals and the name are oxide, the
/// nut and the wordmark use the default foreground and the version is grey.
fn default_art(color: bool) -> String {
    const ART: &str = r#"       ,-.                     _     _
  .--.(   ).--.       _____  _| |__ | | ___   ___  _ __ ___
 (    `-o-'    )     / _ \ \/ / '_ \| |/ _ \ / _ \| '_ ` _ \
  `--' / \ `--'     | (_) >  <| |_) | | (_) | (_) | | | | | |
      (_/ \_)        \___/_/\_\_.__/|_|\___/ \___/|_| |_| |_|"#;
    const NAME: &str = " :: Oxbloom ::";
    const OXIDE: &str = "\x1b[38;2;181;70;42m";
    const GREY: &str = "\x1b[90m";
    const RESET: &str = "\x1b[0m";
    // The flower occupies the first 20 columns of every art line.
    const FLOWER_WIDTH: usize = 20;

    let width = ART.lines().map(str::len).max().unwrap_or(0);
    let version = format!("(v{})", env!("CARGO_PKG_VERSION"));
    let gap = " ".repeat(width.saturating_sub(NAME.len() + version.len()).max(1));
    if !color {
        return format!("{ART}\n{NAME}{gap}{version}");
    }
    let mut out = String::new();
    for line in ART.lines() {
        let (flower, word) = line.split_at(FLOWER_WIDTH.min(line.len()));
        let flower = flower.replace("-o-", &format!("{RESET}-o-{OXIDE}"));
        out.push_str(&format!("{OXIDE}{flower}{RESET}{word}\n"));
    }
    out.push_str(&format!("{OXIDE}{NAME}{RESET}{gap}{GREY}{version}{RESET}"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strip_ansi(text: &str) -> String {
        let mut out = String::new();
        let mut chars = text.chars();
        while let Some(c) = chars.next() {
            if c == '\x1b' {
                for e in chars.by_ref() {
                    if e == 'm' {
                        break;
                    }
                }
            } else {
                out.push(c);
            }
        }
        out
    }

    #[test]
    fn colored_default_banner_has_the_same_text_as_the_plain_one() {
        let banner = Banner::default();
        let plain = banner.line(false).unwrap();
        let colored = banner.line(true).unwrap();
        assert!(colored.contains("\x1b[38;2;181;70;42m"));
        assert!(!plain.contains('\x1b'));
        assert_eq!(strip_ansi(&colored), plain);
    }

    #[test]
    fn custom_banner_is_never_colored() {
        let banner = Banner::text("hello");
        assert_eq!(banner.line(true).as_deref(), Some("hello"));
    }
}
