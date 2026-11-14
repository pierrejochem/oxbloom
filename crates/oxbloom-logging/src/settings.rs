//! The `logging.*` keys, read from a [`Config`] into levels and console options.

use std::collections::BTreeMap;
use std::path::PathBuf;

use oxbloom_config::{Config, ConfigError};
use serde::Deserialize;

use crate::error::LoggingError;
use crate::level::Level;

/// The layout of a console line, from `logging.console.format`.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_logging::ConsoleFormat;
///
/// let config = Config::from_str("[logging.console]\nformat = \"compact\"")?;
/// assert_eq!(config.get::<ConsoleFormat>("logging.console.format")?, ConsoleFormat::Compact);
/// assert_eq!(ConsoleFormat::default(), ConsoleFormat::Full);
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleFormat {
    /// One line per event with time, level, target, fields and span context; the default.
    #[default]
    Full,
    /// One shorter line per event.
    Compact,
    /// Several lines per event, for reading during development.
    Pretty,
    /// One JSON object per line, shaped by `logging.json.*`.
    Json,
}

/// How often the log file rotates, from `logging.file.rotation`.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_logging::FileRotation;
///
/// let config = Config::from_str("[logging.file]\nrotation = \"hourly\"")?;
/// assert_eq!(config.get::<FileRotation>("logging.file.rotation")?, FileRotation::Hourly);
/// assert_eq!(FileRotation::default(), FileRotation::Daily);
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileRotation {
    /// A new file every hour.
    Hourly,
    /// A new file every day; the default.
    #[default]
    Daily,
    /// One file that is never rotated.
    Never,
}

/// The layout of a log file line, from `logging.file.format`. Files are never colored.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_logging::FileFormat;
///
/// let config = Config::from_str("[logging.file]\nformat = \"json\"")?;
/// assert_eq!(config.get::<FileFormat>("logging.file.format")?, FileFormat::Json);
/// assert_eq!(FileFormat::default(), FileFormat::Full);
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FileFormat {
    /// One line per event with time, level, target, fields and span context; the default.
    #[default]
    Full,
    /// One shorter line per event.
    Compact,
    /// One JSON object per line, shaped by `logging.json.*`.
    Json,
}

/// A field of the JSON line layout, as in `logging.json.fields.<field>` and
/// `logging.json.names.<field>`.
///
/// # Examples
///
/// ```
/// use oxbloom_logging::JsonField;
///
/// assert_eq!(JsonField::ALL.len(), 6);
/// assert_eq!(JsonField::ALL[0], JsonField::Timestamp);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum JsonField {
    /// The time of the event.
    Timestamp,
    /// The level of the event.
    Level,
    /// The target of the event.
    Target,
    /// The message of the event.
    Message,
    /// The enclosing spans.
    Span,
    /// The name of the emitting thread.
    Thread,
}

impl JsonField {
    /// Every field, in declaration order.
    pub const ALL: [JsonField; 6] = [
        JsonField::Timestamp,
        JsonField::Level,
        JsonField::Target,
        JsonField::Message,
        JsonField::Span,
        JsonField::Thread,
    ];

    /// The default output name, also the key suffix.
    fn key(self) -> &'static str {
        match self {
            JsonField::Timestamp => "timestamp",
            JsonField::Level => "level",
            JsonField::Target => "target",
            JsonField::Message => "message",
            JsonField::Span => "span",
            JsonField::Thread => "thread",
        }
    }

    fn index(self) -> usize {
        self as usize
    }

    /// The field a config key suffix names, matched the way Config matches keys.
    fn from_key(key: &str) -> Option<JsonField> {
        let canon = canonical(key);
        JsonField::ALL.into_iter().find(|f| f.key() == canon)
    }
}

/// The file output options, from `logging.file.*`.
///
/// Not a public API: it is reachable through the hidden `__private::Settings` only.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_logging::FileRotation;
/// use oxbloom_logging::__private::Settings;
///
/// let config = Config::from_str("[logging.file]\nenabled = true\nrotation = \"never\"")?;
/// let settings = Settings::from_config(&config)?;
/// assert_eq!(settings.file().unwrap().rotation, FileRotation::Never);
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileOptions {
    /// The directory for log files.
    pub dir: PathBuf,
    /// The file name prefix.
    pub name: String,
    /// The rotation period.
    pub rotation: FileRotation,
    /// The number of rotated files to keep; `None` keeps all.
    pub max_history: Option<usize>,
    /// The line layout.
    pub format: FileFormat,
}

/// The JSON layout options, from `logging.json.*`.
///
/// Not a public API: it is reachable through the hidden `__private::Settings` only.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_logging::JsonField;
/// use oxbloom_logging::__private::Settings;
///
/// let config = Config::from_str("[logging.json.names]\nmessage = \"msg\"")?;
/// let settings = Settings::from_config(&config)?;
/// assert_eq!(settings.json().name(JsonField::Message), "msg");
/// assert!(settings.json().is_on(JsonField::Message));
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonOptions {
    on: [bool; 6],
    names: [String; 6],
}

impl Default for JsonOptions {
    fn default() -> Self {
        JsonOptions {
            on: [true; 6],
            names: JsonField::ALL.map(|f| f.key().to_string()),
        }
    }
}

impl JsonOptions {
    /// Whether `field` is part of the output.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::JsonField;
    /// use oxbloom_logging::__private::Settings;
    ///
    /// let settings = Settings::from_config(&Config::from_str("")?)?;
    /// assert!(settings.json().is_on(JsonField::Span));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn is_on(&self, field: JsonField) -> bool {
        self.on[field.index()]
    }

    /// The output name of `field`: its own name unless renamed.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::JsonField;
    /// use oxbloom_logging::__private::Settings;
    ///
    /// let settings = Settings::from_config(&Config::from_str("")?)?;
    /// assert_eq!(settings.json().name(JsonField::Target), "target");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn name(&self, field: JsonField) -> &str {
        &self.names[field.index()]
    }
}

/// Whether console output carries ANSI colors, from `logging.console.color`.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_logging::ConsoleColor;
///
/// let config = Config::from_str("[logging.console]\ncolor = \"never\"")?;
/// assert_eq!(config.get::<ConsoleColor>("logging.console.color")?, ConsoleColor::Never);
/// assert_eq!(ConsoleColor::default(), ConsoleColor::Auto);
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConsoleColor {
    /// Colors only when the output is a terminal; the default.
    #[default]
    Auto,
    /// Always colors.
    Always,
    /// Never colors.
    Never,
}

/// The configuration keys that [`Settings::from_config`] reads, as
/// `(key, type name, default, description)`. `<target>` and `<name>` stand for any target or
/// group name. Boot turns these into its property metadata.
///
/// # Examples
///
/// ```
/// use oxbloom_logging::PROPERTIES;
///
/// let (key, _, default, _) = PROPERTIES[0];
/// assert_eq!((key, default), ("logging.level.root", Some("info")));
/// assert!(PROPERTIES.iter().any(|p| p.0 == "logging.group.<name>"));
/// ```
pub const PROPERTIES: &[(&str, &str, Option<&str>, &str)] = &[
    (
        "logging.level.root",
        "String",
        Some("info"),
        "The default level for every target: off, error, warn, info, debug or trace, matched \
         case-insensitively.",
    ),
    (
        "logging.level.<target>",
        "String",
        None,
        "The level for a target (a module path such as `oxbloom_web` or `oxbloom_web::router`) \
         or for a group name; a group name takes precedence over a target of the same name.",
    ),
    (
        "logging.group.<name>",
        "array of String",
        None,
        "Targets that share a level: `logging.level.<name>` applies to each of them. A target's \
         own `logging.level.<target>` takes precedence over its group's level.",
    ),
    (
        "logging.console.format",
        "String",
        Some("full"),
        "The console line layout: full, compact, pretty or json.",
    ),
    (
        "logging.console.color",
        "String",
        Some("auto"),
        "ANSI colors on the console: auto (only on a terminal), always or never.",
    ),
    (
        "logging.file.enabled",
        "bool",
        Some("false"),
        "Write the log to a file as well as the console.",
    ),
    (
        "logging.file.path",
        "String",
        Some("logs"),
        "The directory for log files; created when missing.",
    ),
    (
        "logging.file.name",
        "String",
        None,
        "The log file name prefix: `application.name` when set, else `oxbloom`.",
    ),
    (
        "logging.file.rotation",
        "String",
        Some("daily"),
        "How often the log file rotates: hourly, daily or never.",
    ),
    (
        "logging.file.max-history",
        "usize",
        None,
        "The number of rotated log files to keep, at least 1; unset keeps all of them.",
    ),
    (
        "logging.file.format",
        "String",
        Some("full"),
        "The log file line layout: full, compact or json. Files are never colored.",
    ),
    (
        "logging.json.fields.<field>",
        "bool",
        Some("true"),
        "Whether the JSON layout includes the field: timestamp, level, target, message, span \
         or thread.",
    ),
    (
        "logging.json.names.<field>",
        "String",
        None,
        "The name of the field in the JSON layout; defaults to the field's own name. Names \
         must be non-empty and differ between included fields.",
    ),
];

/// The console options, fixed when the subscriber is installed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ConsoleOptions {
    pub(crate) format: ConsoleFormat,
    pub(crate) color: ConsoleColor,
}

/// The `logging` subtree as written; every part is optional.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Raw {
    level: BTreeMap<String, Level>,
    group: BTreeMap<String, Vec<String>>,
    console: RawConsole,
    file: RawFile,
    json: RawJson,
}

#[derive(Deserialize)]
#[serde(default)]
struct RawFile {
    enabled: bool,
    path: String,
    name: Option<String>,
    rotation: FileRotation,
    #[serde(rename = "max-history")]
    max_history: Option<usize>,
    format: FileFormat,
}

impl Default for RawFile {
    fn default() -> Self {
        RawFile {
            enabled: false,
            path: "logs".to_string(),
            name: None,
            rotation: FileRotation::default(),
            max_history: None,
            format: FileFormat::default(),
        }
    }
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawJson {
    fields: BTreeMap<String, bool>,
    names: BTreeMap<String, String>,
}

#[derive(Default, Deserialize)]
#[serde(default)]
struct RawConsole {
    format: ConsoleFormat,
    color: ConsoleColor,
}

/// The logging configuration: a root level, the groups with their levels, each target's own
/// level and the console options. Groups are expanded when the filter directives are built.
///
/// Not a public API: it is exposed (hidden) for this crate's integration tests only.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
/// use oxbloom_logging::__private::Settings;
///
/// let config = Config::from_str("[logging.level]\nroot = \"warn\"\noxbloom_web = \"debug\"")?;
/// let settings = Settings::from_config(&config)?;
/// assert_eq!(settings.directives(), "warn,oxbloom_web=debug");
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    root: Level,
    /// Plain targets and their own levels. Never `root` or a group name.
    targets: BTreeMap<String, Level>,
    /// The groups in name order, so the last one by name wins for a shared member.
    groups: Vec<Group>,
    console: ConsoleOptions,
    file: Option<FileOptions>,
    json: JsonOptions,
}

/// A `logging.group.<name>` entry with the level of `logging.level.<name>`, if any.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Group {
    /// The canonical name, as Config matches keys.
    name: String,
    members: Vec<String>,
    level: Option<Level>,
}

impl Settings {
    /// Reads `logging.level.*`, `logging.group.*`, `logging.console.*`, `logging.file.*` and
    /// `logging.json.*` from `config`. An absent `logging` table gives the defaults: root
    /// `info`, format `full`, color `auto`, no file, every JSON field under its own name.
    ///
    /// Each group's level applies to its members; a group name takes precedence over a
    /// target of the same name, and a member's own level takes precedence over its group's.
    /// A target listed in several groups takes the level of the last group by name. The key
    /// `root` (matched relaxed, like every Config key) is the root level, never a target.
    ///
    /// # Errors
    ///
    /// [`LoggingError::Config`] with [`ConfigError::Invalid`] when a level, console, file or
    /// JSON value is not one of its names or is out of range, when a target or group member
    /// contains characters outside letters, digits, `_`, `-` and `:` (which would change the
    /// filter syntax), when a group is named `root`, or when two included JSON fields share a
    /// name. The error names the key, never the value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    /// use oxbloom_logging::LoggingError;
    /// use oxbloom_logging::__private::Settings;
    ///
    /// let config = Config::from_str(
    ///     "[logging.group]\nweb = [\"oxbloom_web\", \"oxbloom_security\"]\n\
    ///      [logging.level]\nweb = \"debug\"",
    /// )?;
    /// let settings = Settings::from_config(&config)?;
    /// assert_eq!(settings.directives(), "info,oxbloom_security=debug,oxbloom_web=debug");
    ///
    /// let bad = Config::from_str("[logging.level]\nroot = \"loud\"")?;
    /// assert!(matches!(
    ///     Settings::from_config(&bad),
    ///     Err(LoggingError::Config(ConfigError::Invalid { key, .. })) if key == "logging.level.root"
    /// ));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn from_config(config: &Config) -> Result<Settings, LoggingError> {
        let raw = match config.get::<Raw>("logging") {
            Ok(raw) => raw,
            Err(ConfigError::Missing { key }) if key == "logging" => Raw::default(),
            Err(e) => return Err(e.into()),
        };

        let mut root = Level::Info;
        // Levels by the canonical form of their key, so a group finds its level the way
        // Config matches keys.
        let mut named: BTreeMap<String, Level> = BTreeMap::new();
        for (key, &level) in &raw.level {
            if canonical(key) == "root" {
                root = level;
            } else {
                named.insert(canonical(key), level);
            }
        }

        let mut groups = Vec::new();
        for (name, members) in &raw.group {
            let key = format!("logging.group.{name}");
            if canonical(name) == "root" {
                return Err(ConfigError::invalid(key, "`root` cannot be a group name").into());
            }
            for member in members {
                if !is_target(member) {
                    return Err(ConfigError::invalid(key, TARGET_RULE).into());
                }
            }
            groups.push(Group {
                name: canonical(name),
                members: members.clone(),
                level: named.get(&canonical(name)).copied(),
            });
        }

        let mut targets = BTreeMap::new();
        for (target, &level) in &raw.level {
            let canon = canonical(target);
            if canon == "root" || groups.iter().any(|g| g.name == canon) {
                continue;
            }
            if !is_target(target) {
                return Err(
                    ConfigError::invalid(format!("logging.level.{target}"), TARGET_RULE).into(),
                );
            }
            targets.insert(target.clone(), level);
        }

        let file = if raw.file.enabled {
            if raw.file.max_history == Some(0) {
                return Err(
                    ConfigError::invalid("logging.file.max-history", "must be at least 1").into(),
                );
            }
            let name = raw
                .file
                .name
                .or_else(|| config.get::<String>("application.name").ok())
                .unwrap_or_else(|| "oxbloom".to_string());
            Some(FileOptions {
                dir: PathBuf::from(raw.file.path),
                name,
                rotation: raw.file.rotation,
                max_history: raw.file.max_history,
                format: raw.file.format,
            })
        } else {
            None
        };

        Ok(Settings {
            root,
            targets,
            groups,
            console: ConsoleOptions {
                format: raw.console.format,
                color: raw.console.color,
            },
            file,
            json: json_options(&raw.json)?,
        })
    }

    /// The `EnvFilter` directive string: the root level first, then `target=level` for each
    /// target in name order.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::__private::Settings;
    ///
    /// let settings = Settings::from_config(&Config::from_str("")?)?;
    /// assert_eq!(settings.directives(), "info");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn directives(&self) -> String {
        let mut out = self.root.to_string();
        for (target, level) in &self.levels() {
            out.push(',');
            out.push_str(target);
            out.push('=');
            out.push_str(&level.to_string());
        }
        out
    }

    /// The root level, `info` unless `logging.level.root` is set.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::Level;
    /// use oxbloom_logging::__private::Settings;
    ///
    /// let settings = Settings::from_config(&Config::from_str("[logging.level]\nroot = \"off\"")?)?;
    /// assert_eq!(settings.root(), Level::Off);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn root(&self) -> Level {
        self.root
    }

    /// The console line layout from `logging.console.format`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::ConsoleFormat;
    /// use oxbloom_logging::__private::Settings;
    ///
    /// let settings = Settings::from_config(&Config::from_str("")?)?;
    /// assert_eq!(settings.format(), ConsoleFormat::Full);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn format(&self) -> ConsoleFormat {
        self.console.format
    }

    /// The file output options, `None` unless `logging.file.enabled` is true.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::__private::Settings;
    ///
    /// let off = Settings::from_config(&Config::from_str("")?)?;
    /// assert!(off.file().is_none());
    /// let on = Settings::from_config(&Config::from_str("[logging.file]\nenabled = true")?)?;
    /// assert_eq!(on.file().unwrap().name, "oxbloom");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn file(&self) -> Option<&FileOptions> {
        self.file.as_ref()
    }

    /// The JSON layout options from `logging.json.*`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::JsonField;
    /// use oxbloom_logging::__private::Settings;
    ///
    /// let settings = Settings::from_config(&Config::from_str("")?)?;
    /// assert!(settings.json().is_on(JsonField::Timestamp));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn json(&self) -> &JsonOptions {
        &self.json
    }

    /// The console color mode from `logging.console.color`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use oxbloom_logging::ConsoleColor;
    /// use oxbloom_logging::__private::Settings;
    ///
    /// let settings = Settings::from_config(&Config::from_str("")?)?;
    /// assert_eq!(settings.color(), ConsoleColor::Auto);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn color(&self) -> ConsoleColor {
        self.console.color
    }
}

impl Settings {
    /// Target to level after group expansion: each group's level for its members, in name
    /// order, then the targets' own levels on top.
    fn levels(&self) -> BTreeMap<String, Level> {
        let mut levels = BTreeMap::new();
        for group in &self.groups {
            if let Some(level) = group.level {
                for member in &group.members {
                    levels.insert(member.clone(), level);
                }
            }
        }
        for (target, &level) in &self.targets {
            levels.insert(target.clone(), level);
        }
        levels
    }

    /// `root` and every configured target with its level, groups expanded as in
    /// [`levels`](Self::levels).
    pub(crate) fn configured_levels(&self) -> BTreeMap<String, Level> {
        let mut levels = self.levels();
        levels.insert("root".to_string(), self.root);
        levels
    }

    /// The groups whose name matches `name` the way Config matches keys.
    fn groups_named(&self, name: &str) -> impl Iterator<Item = usize> + '_ {
        let canon = canonical(name);
        (0..self.groups.len()).filter(move |&i| self.groups[i].name == canon)
    }

    /// Sets the level of `root`, of a group or of a target, as `logging.level.<name>` would.
    pub(crate) fn set(&mut self, name: &str, level: Level) -> Result<(), LoggingError> {
        if canonical(name) == "root" {
            self.root = level;
            return Ok(());
        }
        let groups: Vec<usize> = self.groups_named(name).collect();
        if !groups.is_empty() {
            for i in groups {
                self.groups[i].level = Some(level);
            }
            return Ok(());
        }
        check_target(name)?;
        self.targets.insert(name.to_string(), level);
        Ok(())
    }

    /// Returns `name` to its level in `baseline`. For a group that is the group's level and
    /// each member's own level, so every member is back where `baseline` had it.
    pub(crate) fn reset(&mut self, name: &str, baseline: &Settings) -> Result<(), LoggingError> {
        if canonical(name) == "root" {
            self.root = baseline.root;
            return Ok(());
        }
        let groups: Vec<usize> = self.groups_named(name).collect();
        if groups.is_empty() {
            check_target(name)?;
            self.reset_target(name, baseline);
            return Ok(());
        }
        for i in groups {
            // `reload` replaces current and baseline together, so the groups line up.
            self.groups[i].level = baseline.groups[i].level;
            for member in self.groups[i].members.clone() {
                self.reset_target(&member, baseline);
            }
        }
        Ok(())
    }

    fn reset_target(&mut self, target: &str, baseline: &Settings) {
        match baseline.targets.get(target) {
            Some(&level) => self.targets.insert(target.to_string(), level),
            None => self.targets.remove(target),
        };
    }

    /// The level of `root`, of a group (its own level, or root when it has none), or of a
    /// target: the level of the longest configured target it starts with, the way the
    /// filter matches, or root.
    pub(crate) fn level(&self, name: &str) -> Level {
        if canonical(name) == "root" {
            return self.root;
        }
        if let Some(i) = self.groups_named(name).next() {
            return self.groups[i].level.unwrap_or(self.root);
        }
        self.levels()
            .into_iter()
            .filter(|(target, _)| name.starts_with(target.as_str()))
            .max_by_key(|(target, _)| target.len())
            .map_or(self.root, |(_, level)| level)
    }
}

/// Applies `logging.json.fields.*` and `logging.json.names.*` to the defaults.
fn json_options(raw: &RawJson) -> Result<JsonOptions, LoggingError> {
    let mut json = JsonOptions::default();
    for (key, &on) in &raw.fields {
        let field = JsonField::from_key(key).ok_or_else(|| {
            ConfigError::invalid(format!("logging.json.fields.{key}"), JSON_FIELD_RULE)
        })?;
        json.on[field.index()] = on;
    }
    // Which fields the user renamed: a clash blames one of these.
    let mut renamed = [false; 6];
    for (key, name) in &raw.names {
        let field = JsonField::from_key(key).ok_or_else(|| {
            ConfigError::invalid(format!("logging.json.names.{key}"), JSON_FIELD_RULE)
        })?;
        if name.is_empty() {
            return Err(ConfigError::invalid(
                format!("logging.json.names.{}", field.key()),
                "a name must not be empty",
            )
            .into());
        }
        json.names[field.index()] = name.clone();
        renamed[field.index()] = true;
    }
    // Two included fields under one name would write a duplicate key in the object.
    for field in JsonField::ALL {
        if !json.is_on(field) {
            continue;
        }
        let clash = JsonField::ALL
            .into_iter()
            .take(field.index())
            .find(|&other| json.is_on(other) && json.name(other) == json.name(field));
        if let Some(other) = clash {
            // Blame the key the user wrote; when both were renamed, the later one.
            let blamed = if renamed[field.index()] { field } else { other };
            return Err(ConfigError::invalid(
                format!("logging.json.names.{}", blamed.key()),
                "a name must differ from the other included fields",
            )
            .into());
        }
    }
    Ok(json)
}

const JSON_FIELD_RULE: &str = "a field is one of timestamp, level, target, message, span or thread";

pub(crate) fn check_target(name: &str) -> Result<(), LoggingError> {
    if is_target(name) {
        Ok(())
    } else {
        Err(ConfigError::invalid(format!("logging.level.{name}"), TARGET_RULE).into())
    }
}

const TARGET_RULE: &str = "a target may contain only letters, digits, `_`, `-` and `:`";

/// The characters `EnvFilter` accepts in a target. Anything else (`,`, `=`, `[`, spaces)
/// would add or change directives.
fn is_target(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | ':'))
}

/// Config's relaxed key form: lowercase, `-` and `_` removed.
fn canonical(key: &str) -> String {
    key.chars()
        .filter(|c| !matches!(c, '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_ignores_case_and_separators() {
        assert_eq!(canonical("Oxbloom-Web_x"), "oxbloomwebx");
    }

    #[test]
    fn targets_follow_the_filter_syntax() {
        assert!(is_target("oxbloom_web::router"));
        assert!(is_target("my-app"));
        assert!(!is_target(""));
        assert!(!is_target("a,b"));
        assert!(!is_target("a=b"));
        assert!(!is_target("a[b]"));
        assert!(!is_target("a b"));
    }
}
