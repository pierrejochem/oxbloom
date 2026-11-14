//! Property metadata: a description of the configuration keys that the framework and its
//! auto-configurations read.

use std::fmt::Write;

/// One configuration key: its name, type, default and meaning.
///
/// The text is for people and tools (documentation, completion); it never carries a value, so
/// a secret key is described and nothing more.
///
/// # Examples
///
/// ```
/// use oxbloom_boot::PropertyMetadata;
///
/// let port = PropertyMetadata::new("server.port", "u16", Some("8080"), "The listening port.");
/// assert_eq!(port.key, "server.port");
/// assert_eq!(port.default, Some("8080"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PropertyMetadata {
    /// The dotted key, e.g. `server.port`.
    pub key: &'static str,
    /// The type of the value, e.g. `u16` or `String`.
    pub type_name: &'static str,
    /// The default as text, or `None` when the key has none.
    pub default: Option<&'static str>,
    /// What the key does.
    pub description: &'static str,
}

impl PropertyMetadata {
    /// Builds the metadata of one key; usable in a `static`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::PropertyMetadata;
    ///
    /// static KEYS: [PropertyMetadata; 1] =
    ///     [PropertyMetadata::new("cache.size", "usize", None, "Maximum entries.")];
    /// assert_eq!(KEYS[0].type_name, "usize");
    /// ```
    pub const fn new(
        key: &'static str,
        type_name: &'static str,
        default: Option<&'static str>,
        description: &'static str,
    ) -> Self {
        PropertyMetadata {
            key,
            type_name,
            default,
            description,
        }
    }

    /// Writes `list` as a JSON array of objects with the fields `key`, `type`, `default` and
    /// `description`, in that order and without whitespace. A missing default is `null`.
    /// Quotes, backslashes and control characters in the text are escaped.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_boot::PropertyMetadata;
    ///
    /// let list = [PropertyMetadata::new("a", "u8", None, "An \"a\".")];
    /// assert_eq!(
    ///     PropertyMetadata::to_json(&list),
    ///     r#"[{"key":"a","type":"u8","default":null,"description":"An \"a\"."}]"#
    /// );
    /// assert_eq!(PropertyMetadata::to_json(&[]), "[]");
    /// ```
    pub fn to_json(list: &[PropertyMetadata]) -> String {
        let mut out = String::from("[");
        for (i, p) in list.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            out.push_str("{\"key\":");
            push_string(&mut out, p.key);
            out.push_str(",\"type\":");
            push_string(&mut out, p.type_name);
            out.push_str(",\"default\":");
            match p.default {
                Some(d) => push_string(&mut out, d),
                None => out.push_str("null"),
            }
            out.push_str(",\"description\":");
            push_string(&mut out, p.description);
            out.push('}');
        }
        out.push(']');
        out
    }
}

/// Appends `s` as a JSON string literal.
fn push_string(out: &mut String, s: &str) {
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Writing to a `String` cannot fail.
            c if c < ' ' => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out.push('"');
}

static BOOT: [PropertyMetadata; 8] = [
    PropertyMetadata::new(
        "application.name",
        "String",
        Some("application"),
        "The application name used in the startup events; read only with the `logging` feature.",
    ),
    PropertyMetadata::new(
        "oxbloom.profiles.active",
        "String or array of String",
        None,
        "The active profiles, a comma-separated string or an array; the command line and \
         OXBLOOM_PROFILE take precedence.",
    ),
    PropertyMetadata::new(
        "oxbloom.profiles.group",
        "table of String to array of String",
        None,
        "Profile groups: activating a profile also activates the profiles listed under its name.",
    ),
    PropertyMetadata::new(
        "oxbloom.config.import",
        "array of String",
        None,
        "Files to merge over the file that lists them, in list order; each entry is a path \
         relative to the config directory, and an `optional:` prefix tolerates a missing file.",
    ),
    PropertyMetadata::new(
        "oxbloom.logging.enabled",
        "bool",
        Some("true"),
        "false skips installing the logging subscriber; read only with the `logging` feature.",
    ),
    PropertyMetadata::new(
        "oxbloom.main.banner-mode",
        "String",
        Some("console"),
        "console prints the banner, off suppresses it; unset keeps the builder's setting.",
    ),
    PropertyMetadata::new(
        "oxbloom.main.conditions-report",
        "String",
        Some("off"),
        "console prints the conditions evaluation report after the container is built, off \
         does not.",
    ),
    PropertyMetadata::new(
        "server.ssl.bundle",
        "String",
        None,
        "The SSL bundle whose certificate and private key the web server uses; the port then \
         speaks only HTTPS. Unset, the server speaks plain HTTP. Needs the `ssl` feature, and \
         the bundle must have an identity.",
    ),
];

/// The keys that the framework reads itself: `application.name` (the name in the startup
/// events), `oxbloom.config.import` and
/// `oxbloom.profiles.active` and `oxbloom.profiles.group` (read by the config loader), and
/// `oxbloom.main.banner-mode`, `oxbloom.main.conditions-report` and `oxbloom.logging.enabled`
/// (read by boot), and `server.ssl.bundle` (read by the web server). `oxbloom.logging.enabled`
/// and `application.name` are listed with or without the `logging` feature, and
/// `server.ssl.bundle` with or without the `ssl` feature, so the list does not depend on
/// features.
///
/// # Examples
///
/// ```
/// use oxbloom_boot::boot_properties;
///
/// let keys: Vec<_> = boot_properties().iter().map(|p| p.key).collect();
/// assert!(keys.contains(&"oxbloom.main.banner-mode"));
/// assert!(keys.contains(&"server.ssl.bundle"));
/// assert_eq!(keys.len(), 8);
/// ```
pub fn boot_properties() -> &'static [PropertyMetadata] {
    &BOOT
}
