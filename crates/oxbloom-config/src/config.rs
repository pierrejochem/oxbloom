use std::path::PathBuf;

use crate::error::ConfigError;
use crate::migrate::{self, ConfigWarning, Registry};
use crate::node::Node;
use crate::origin::Origin;

/// Merged application configuration.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
///
/// let config = Config::from_str("[server]\nport = 8080")?;
/// let port: u16 = config.get("server.port")?;
/// assert_eq!(port, 8080);
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Clone)]
pub struct Config {
    pub(crate) root: Node,
    warnings: Vec<ConfigWarning>,
    /// Active profiles after group expansion, in activation order.
    profiles: Vec<String>,
}

impl std::fmt::Debug for Config {
    /// Prints top-level key names only; values may be secrets.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let keys: Vec<&String> = self
            .root
            .as_table()
            .map(|t| t.iter().map(|e| &e.spelling).collect())
            .unwrap_or_default();
        f.debug_struct("Config")
            .field("keys", &keys)
            .finish_non_exhaustive()
    }
}

impl Config {
    /// Starts a builder that reads `application.toml`, a profile overlay and the environment.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// # let dir = tempfile::tempdir()?;
    ///
    /// let config = Config::builder().dir(dir.path()).env_from([]).build()?;
    /// # let _ = config;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder::default()
    }

    /// Parses `toml` directly, with no filesystem or environment access. Ideal for unit tests.
    ///
    /// Imports, profiles, environment, command line and migration are skipped; a
    /// `oxbloom.config.import` key stays an ordinary key (`build` follows and removes it).
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    ///
    /// let config = Config::from_str("name = \"demo\"")?;
    /// assert_eq!(config.get::<String>("name")?, "demo");
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(toml: &str) -> Result<Self, ConfigError> {
        let mut root = Node::parse_toml(toml, |line| Origin::Inline { line })?;
        // Empty environment: only config keys and defaults resolve.
        crate::placeholder::resolve(&mut root, &[])?;
        Ok(Self {
            root,
            warnings: Vec::new(),
            profiles: Vec::new(),
        })
    }

    /// Binds the subtree at the dotted `prefix` (empty for the whole config) to `T`.
    ///
    /// String leaves, such as environment overrides, are parsed into the integer, float or bool
    /// that a field expects. Fields typed as strings are never coerced.
    ///
    /// # Key matching
    ///
    /// Keys match relaxed: `maxConnections`, `max_connections`, `max-connections` and
    /// `MAX_CONNECTIONS` all bind the field `max_connections` (case and `-`/`_` are ignored),
    /// in `prefix` too. Map keys keep the spelling of the source that set them.
    ///
    /// - A key that equals a declared name (field name, `rename` or `alias`) exactly binds that
    ///   name; otherwise it binds the first declared name with the same canonical form.
    /// - Two declared names with one canonical form are reported as `Invalid` only when they
    ///   cannot be the sorted name run of a single field. Other cases, such as fields
    ///   `jwt_secret` and `jwtsecret` declared in that order, are not detected, so a struct must
    ///   not declare canonically equal field names. `rename` plus `alias` of one field
    ///   (`jwk-set-uri`, `jwk_set_uri`) is fine.
    /// - Fields of a struct that uses `#[serde(flatten)]`, and the fields and tag of internally
    ///   tagged enums, are matched exactly, not relaxed.
    ///
    /// Errors name the key and never contain configuration values (placeholders, resolved only
    /// in file values, are the one exception: see [`ConfigError::UnresolvedPlaceholder`]).
    /// Messages from a custom
    /// `Deserialize` impl are replaced by a fixed text for that reason.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// use serde::Deserialize;
    ///
    /// #[derive(Deserialize)]
    /// struct Server { host: String, port: u16 }
    ///
    /// let config = Config::from_str("[server]\nhost = \"localhost\"\nport = 8080")?;
    /// let server: Server = config.get("server")?;
    /// assert_eq!((server.host.as_str(), server.port), ("localhost", 8080));
    ///
    /// let port: u16 = config.get("server.port")?;   // a single value works too
    /// let whole: toml::Value = config.get("")?;     // empty prefix: the whole config
    /// # let _ = (port, whole);
    ///
    /// #[derive(Deserialize)]
    /// struct Pool { max_connections: u32 }
    ///
    /// let config = Config::from_str("[db.pool]\nmaxConnections = 10")?;
    /// assert_eq!(config.get::<Pool>("DB.pool")?.max_connections, 10);
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn get<T: serde::de::DeserializeOwned>(&self, prefix: &str) -> Result<T, ConfigError> {
        let node = self.node_at(prefix).ok_or_else(|| ConfigError::Missing {
            key: prefix.to_string(),
        })?;
        crate::lenient::deserialize(node, prefix)
    }

    /// The node at the dotted path `prefix` (empty: the root), by canonical key match.
    pub(crate) fn node_at(&self, prefix: &str) -> Option<&crate::node::Node> {
        let mut node = &self.root;
        if !prefix.is_empty() {
            for segment in prefix.split('.') {
                node = node.as_table()?.get(segment)?;
            }
        }
        Some(node)
    }

    /// Warnings about renamed or removed keys found while building, in rule registration order.
    ///
    /// Empty for [`Config::from_str`]. Warnings name keys and origins, never values.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::{Config, WarningKind};
    ///
    /// let dir = tempfile::tempdir()?;
    /// std::fs::write(dir.path().join("application.toml"), "[old]\nkey = 1")?;
    ///
    /// let config = Config::builder().dir(dir.path()).env_from([]).renamed("old.key", "new.key").build()?;
    /// assert_eq!(config.get::<u8>("new.key")?, 1);
    /// let warning = &config.warnings()[0];
    /// assert_eq!(warning.key, "old.key");
    /// assert_eq!(warning.kind, WarningKind::Renamed { to: "new.key".into() });
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn warnings(&self) -> &[ConfigWarning] {
        &self.warnings
    }

    /// The profiles this configuration activated, after group expansion, in activation order
    /// (each name once, first occurrence kept); empty when none is active and for
    /// [`Config::from_str`].
    ///
    /// The list is the one [`ConfigBuilder::profile`] describes: builder calls, else
    /// `--oxbloom.profiles.active=`, else `OXBLOOM_PROFILE`, else `oxbloom.profiles.active` in
    /// `application.toml`, each expanded through `oxbloom.profiles.group`. Pass it to the
    /// dependency container (`ContainerBuilder::active_profiles` in `oxbloom-di`) so profile-conditional
    /// beans follow the same profiles as the configuration files.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    ///
    /// let dir = tempfile::tempdir()?;
    /// std::fs::write(
    ///     dir.path().join("application.toml"),
    ///     "[oxbloom.profiles.group]\nprod = [\"db\", \"metrics\"]\n",
    /// )?;
    /// for p in ["prod", "db", "metrics"] {
    ///     std::fs::write(dir.path().join(format!("application-{p}.toml")), "")?;
    /// }
    ///
    /// let config = Config::builder().dir(dir.path()).profile("prod").env_from([]).build()?;
    /// assert_eq!(config.active_profiles(), ["prod", "db", "metrics"]);
    ///
    /// let config = Config::builder().dir(dir.path()).env_from([]).build()?;
    /// assert!(config.active_profiles().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn active_profiles(&self) -> &[String] {
        &self.profiles
    }

    /// The merged tree as a `toml::Value`, for comparisons in tests.
    #[cfg(test)]
    pub(crate) fn root(&self) -> toml::Value {
        self.root.to_toml_value()
    }
}

/// Builds a [`Config`] from files and environment variables.
///
/// Precedence, highest first: command line, environment, `application-{profile}.toml`, `application.toml`.
///
/// # Examples
///
/// ```
/// use oxbloom_config::Config;
///
/// let dir = tempfile::tempdir()?;
/// std::fs::write(dir.path().join("application.toml"), "[server]\nport = 8080\nhost = \"0.0.0.0\"")?;
/// std::fs::write(dir.path().join("application-dev.toml"), "[server]\nhost = \"127.0.0.1\"")?;
///
/// let config = Config::builder()
///     .dir(dir.path())
///     .profile("dev")
///     .env_from([("APP_SERVER__PORT".to_string(), "9000".to_string())])
///     .build()?;
/// assert_eq!(config.get::<String>("server.host")?, "127.0.0.1"); // profile beats base
/// assert_eq!(config.get::<u16>("server.port")?, 9000);           // env beats both
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub struct ConfigBuilder {
    dir: PathBuf,
    profiles: Vec<String>,
    prefix: String,
    env: Option<Vec<(String, String)>>,
    args: Vec<String>,
    secret_keys: Vec<String>,
    registry: Registry,
    deny_deprecated: bool,
    max_file_size: usize,
}

impl std::fmt::Debug for ConfigBuilder {
    /// Omits the injected environment, whose values may be secrets.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConfigBuilder")
            .field("dir", &self.dir)
            .field("profiles", &self.profiles)
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl Default for ConfigBuilder {
    fn default() -> Self {
        Self {
            dir: PathBuf::from("."),
            profiles: Vec::new(),
            prefix: "APP".into(),
            env: None,
            args: Vec::new(),
            secret_keys: Vec::new(),
            registry: Registry::default(),
            deny_deprecated: false,
            max_file_size: crate::loader::DEFAULT_MAX_FILE_SIZE,
        }
    }
}

impl ConfigBuilder {
    /// Directory holding the `application*.toml` files. Defaults to `.`.
    ///
    /// A missing `application.toml` gives an empty config.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    ///
    /// let config = Config::builder().dir("config").env_from([]).build()?;
    /// # let _ = config;
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.dir = dir.into();
        self
    }

    /// Activates a profile. Repeatable: profiles apply in call order, later ones win.
    ///
    /// Each active profile selects `application-{profile}.toml` (and its `oxbloom.config.import`
    /// files), merged over `application.toml`. A missing profile file is a `ConfigError::Io`.
    /// Names allow only letters, digits, `_` and `-`.
    ///
    /// When `profile` is never called, the active profiles come from, in this order of
    /// preference (the first non-empty list is used, nothing is merged): `--oxbloom.profiles.active=a,b`
    /// in [`args`](Self::args), the `OXBLOOM_PROFILE` variable (`a,b`), then `oxbloom.profiles.active`
    /// in `application.toml` (a string `"a,b"` or an array). `oxbloom.profiles.active` in a profile
    /// file is `ConfigError::Invalid`.
    ///
    /// Groups: `oxbloom.profiles.group.prod = ["db", "metrics"]` in `application.toml` makes activating
    /// `prod` activate `prod`, then `db`, then `metrics` (recursively, each profile once). A
    /// group that includes itself is `ConfigError::Invalid`. Groups nest at most 8 levels deep
    /// and at most 64 profiles are active after expansion; more is `ConfigError::Invalid`
    /// (key `oxbloom.profiles.group.<name>` or `oxbloom.profiles.active`), whichever way profiles are named.
    /// `oxbloom.profiles.active` and `oxbloom.profiles.group` are read from `application.toml` and its
    /// imports; `APP_OXBLOOM__PROFILES__ACTIVE` and `--oxbloom.profiles.active[0]` do not activate profiles.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    ///
    /// let dir = tempfile::tempdir()?;
    /// std::fs::write(dir.path().join("application-prod.toml"), "mode = \"prod\"\nregion = \"eu\"")?;
    /// std::fs::write(dir.path().join("application-us.toml"), "region = \"us\"")?;
    ///
    /// let config = Config::builder().dir(dir.path()).profile("prod").env_from([]).build()?;
    /// assert_eq!(config.get::<String>("mode")?, "prod");
    ///
    /// // Repeated calls: later profiles win.
    /// let config = Config::builder().dir(dir.path()).profile("prod").profile("us").env_from([]).build()?;
    /// assert_eq!(config.get::<String>("region")?, "us");
    ///
    /// // The same, from the environment.
    /// let config = Config::builder()
    ///     .dir(dir.path())
    ///     .env_from([("OXBLOOM_PROFILE".to_string(), "prod,us".to_string())])
    ///     .build()?;
    /// assert_eq!(config.get::<String>("region")?, "us");
    ///
    /// let bad = Config::builder().dir(dir.path()).profile("../etc").env_from([]).build();
    /// assert!(matches!(bad, Err(ConfigError::Invalid { .. })));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn profile(mut self, profile: impl Into<String>) -> Self {
        self.profiles.push(profile.into());
        self
    }

    /// Environment variable prefix. Defaults to `APP`.
    ///
    /// `MYAPP_SERVER__PORT=9000` sets `server.port`. `__` separates path segments, a single `_`
    /// stays in the key, and keys are lowercased.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// # let dir = tempfile::tempdir()?;
    ///
    /// let config = Config::builder()
    ///     .dir(dir.path())
    ///     .env_prefix("MYAPP")
    ///     .env_from([("MYAPP_SERVER__PORT".to_string(), "9000".to_string())])
    ///     .build()?;
    /// assert_eq!(config.get::<u16>("server.port")?, 9000);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn env_prefix(mut self, prefix: impl Into<String>) -> Self {
        self.prefix = prefix.into();
        self
    }

    /// Uses `vars` instead of the process environment. Intended for tests.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// # let dir = tempfile::tempdir()?;
    ///
    /// let config = Config::builder()
    ///     .dir(dir.path())
    ///     .env_from([("APP_DEBUG".to_string(), "true".to_string())])
    ///     .build()?;
    /// assert!(config.get::<bool>("debug")?);     // "true" is parsed into a bool
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn env_from(mut self, vars: impl IntoIterator<Item = (String, String)>) -> Self {
        self.env = Some(vars.into_iter().collect());
        self
    }

    /// Command-line arguments, applied after the environment (highest precedence).
    ///
    /// Only `--key=value` is read: `.` separates path segments, `[n]` or `.n` is a list index
    /// (`--servers[0].host=a`). Arguments without `=`, not starting with `--`, or with an empty
    /// key are ignored, and a repeated key keeps the last value. A key registered with
    /// [`secret_key`](Self::secret_key) is rejected, so secrets stay out of process listings.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// # let dir = tempfile::tempdir()?;
    ///
    /// let config = Config::builder()
    ///     .dir(dir.path())
    ///     .env_from([])
    ///     .args(["--server.port=9000", "--hosts[0]=a", "--verbose"])
    ///     .build()?;
    /// assert_eq!(config.get::<u16>("server.port")?, 9000);
    /// assert_eq!(config.get::<Vec<String>>("hosts")?, vec!["a".to_string()]);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn args(mut self, args: impl IntoIterator<Item = impl Into<String>>) -> Self {
        self.args = args.into_iter().map(Into::into).collect();
        self
    }

    /// Registers a key (dotted path, matched canonically) that must not be set on the command
    /// line. Setting it through [`args`](Self::args) fails with `ConfigError::Invalid`, whose
    /// message points to environment variables or files and never contains the value.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    /// # let dir = tempfile::tempdir()?;
    ///
    /// let result = Config::builder()
    ///     .dir(dir.path())
    ///     .env_from([])
    ///     .secret_key("security.jwt.secret")
    ///     .args(["--security.JWT.secret=hunter2"])
    ///     .build();
    /// assert!(matches!(result, Err(ConfigError::Invalid { .. })));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn secret_key(mut self, key: impl Into<String>) -> Self {
        self.secret_keys.push(key.into());
        self
    }

    /// Registers a renamed key. Both are dotted paths, matched canonically per segment.
    ///
    /// Old key set and new key unset: the value moves to `new`, keeping its origin. Both set:
    /// the new value wins and the old node is dropped. Either way a [`WarningKind::Renamed`]
    /// warning is recorded. If `new` cannot be created because a non-table value sits on its
    /// path, nothing moves, the old key stays and the warning is still recorded. Rules apply
    /// in registration order, so chains (`a` to `b`, `b` to `c`) work. Whole tables can be
    /// renamed. Tables emptied by the move are removed.
    ///
    /// A rule with an empty or malformed path, with `old` equal to `new` (canonically), or with
    /// one path inside the other is a programmer error: `build` fails with
    /// `ConfigError::Invalid` before anything is migrated.
    ///
    /// Renaming does not allow command-line values onto a [`secret_key`](Self::secret_key):
    /// each moved command-line value whose final key equals or lies below a secret key makes
    /// `build` fail like the command line itself (the error key is the new key plus the
    /// value's path inside the moved subtree).
    /// Renaming out of a secret key does not carry the secret marker to the new key.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// # let dir = tempfile::tempdir()?;
    ///
    /// let config = Config::builder().dir(dir.path()).env_from([]).renamed("a.b", "c.d").build()?;
    /// assert!(config.warnings().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn renamed(mut self, old: impl Into<String>, new: impl Into<String>) -> Self {
        self.registry.renamed(old.into(), new.into());
        self
    }

    /// Registers a removed key. If it is set, the node stays and a [`WarningKind::Removed`]
    /// warning carrying `reason` is recorded.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// # let dir = tempfile::tempdir()?;
    ///
    /// let config = Config::builder()
    ///     .dir(dir.path())
    ///     .env_from([])
    ///     .removed("legacy.mode", "no longer supported")
    ///     .build()?;
    /// assert!(config.warnings().is_empty());
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn removed(mut self, old: impl Into<String>, reason: impl Into<String>) -> Self {
        self.registry.removed(old.into(), reason.into());
        self
    }

    /// When `true`, `build` fails with `ConfigError::Deprecated` for the first migration
    /// warning instead of recording it. Defaults to `false`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    /// # let dir = tempfile::tempdir()?;
    ///
    /// let config = Config::builder().dir(dir.path()).env_from([]).deny_deprecated(true).build()?;
    /// # let _ = config;
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn deny_deprecated(mut self, deny: bool) -> Self {
        self.deny_deprecated = deny;
        self
    }

    /// Largest configuration file, in bytes, that `build` reads. Defaults to 1 MiB.
    ///
    /// Applies to `application.toml`, profile files and `oxbloom.config.import` files alike. A larger
    /// file is `ConfigError::FileTooLarge`; at most `bytes + 1` bytes are read from it. `0`
    /// makes every non-empty file too large; `usize::MAX` means no limit. Every configuration file must be a regular file
    /// (symlinks are followed): a FIFO or a device is `ConfigError::Io` and is never opened.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::{Config, ConfigError};
    ///
    /// let dir = tempfile::tempdir()?;
    /// std::fs::write(dir.path().join("application.toml"), "name = \"demo\"")?;
    ///
    /// let config = Config::builder().dir(dir.path()).max_file_size(64 * 1024).env_from([]).build()?;
    /// assert_eq!(config.get::<String>("name")?, "demo");
    ///
    /// let err = Config::builder().dir(dir.path()).max_file_size(4).env_from([]).build();
    /// assert!(matches!(err, Err(ConfigError::FileTooLarge { .. })));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn max_file_size(mut self, bytes: usize) -> Self {
        self.max_file_size = bytes;
        self
    }

    /// Reads the files, merges them and applies environment and command-line overrides.
    ///
    /// Any configuration file may list `oxbloom.config.import = ["extra.toml", "optional:local.toml"]`:
    /// paths relative to [`dir`](Self::dir), merged over the importing file in list order
    /// (imports may import, up to 8 levels). An absolute path, `..`, or a path that leaves the
    /// directory through a symlink is `ConfigError::ImportInvalid`; a missing file is
    /// `ConfigError::ImportMissing` unless prefixed `optional:`; a cycle is
    /// `ConfigError::ImportCycle`. The `oxbloom.config.import` key itself is not kept.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    ///
    /// let dir = tempfile::tempdir()?;
    /// let config = Config::builder().dir(dir.path()).env_from([]).build()?;  // no files: empty
    /// # let _ = config;
    ///
    /// std::fs::write(dir.path().join("application.toml"), "oxbloom.config.import = [\"db.toml\"]\nname = \"demo\"")?;
    /// std::fs::write(dir.path().join("db.toml"), "[db]\nurl = \"postgres://localhost/demo\"")?;
    /// let config = Config::builder().dir(dir.path()).env_from([]).build()?;
    /// assert_eq!(config.get::<String>("db.url")?, "postgres://localhost/demo");
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn build(self) -> Result<Config, ConfigError> {
        let vars: Vec<(String, String)> = self.env.unwrap_or_else(|| std::env::vars().collect());
        let crate::loader::Loaded { mut root, profiles } = crate::loader::load(
            &self.dir,
            self.profiles,
            &vars,
            &self.prefix,
            &self.args,
            &self.secret_keys,
            self.max_file_size,
        )?;
        let warnings = migrate::migrate(&mut root, &self.registry, &self.secret_keys)?;
        if self.deny_deprecated
            && let Some(first) = warnings.first()
        {
            return Err(ConfigError::Deprecated {
                key: first.key.clone(),
                origin: first.origin.clone(),
            });
        }
        crate::placeholder::resolve(&mut root, &vars)?;
        Ok(Config {
            root,
            warnings,
            profiles,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::origin::Origin;
    use std::fs;

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn val(s: &str) -> toml::Value {
        toml::from_str(s).unwrap()
    }

    fn build(
        dir: &std::path::Path,
        profile: Option<&str>,
        vars: &[(&str, &str)],
    ) -> Result<Config, ConfigError> {
        let mut b = Config::builder().dir(dir).env_from(env(vars));
        if let Some(p) = profile {
            b = b.profile(p);
        }
        b.build()
    }

    #[test]
    fn empty_file_gives_empty_config() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("application.toml"), "").unwrap();
        assert_eq!(build(d.path(), None, &[]).unwrap().root(), val(""));
    }

    #[test]
    fn comments_only_file_gives_empty_config() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("application.toml"), "# nothing\n# here\n").unwrap();
        assert_eq!(build(d.path(), None, &[]).unwrap().root(), val(""));
    }

    #[test]
    fn missing_base_file_is_empty_config() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(build(d.path(), None, &[]).unwrap().root(), val(""));
    }

    #[test]
    fn profile_overlay_overrides_base() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("application.toml"), "[s]\na = 1\nb = 1").unwrap();
        fs::write(d.path().join("application-dev.toml"), "[s]\nb = 2").unwrap();
        let c = build(d.path(), Some("dev"), &[]).unwrap();
        assert_eq!(c.root(), val("[s]\na = 1\nb = 2"));
    }

    #[test]
    fn env_overrides_profile() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("application.toml"), "[s]\nb = 1").unwrap();
        fs::write(d.path().join("application-dev.toml"), "[s]\nb = 2").unwrap();
        let c = build(d.path(), Some("dev"), &[("APP_S__B", "3")]).unwrap();
        assert_eq!(c.root(), val("[s]\nb = \"3\""));
    }

    #[test]
    fn missing_requested_profile_file_is_io_error() {
        let d = tempfile::tempdir().unwrap();
        let e = build(d.path(), Some("dev"), &[]).unwrap_err();
        assert!(matches!(e, ConfigError::Io { .. }), "{e:?}");
    }

    #[test]
    fn profile_from_env_var_when_not_set_explicitly() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("application-prod.toml"), "a = 1").unwrap();
        let c = build(d.path(), None, &[("OXBLOOM_PROFILE", "prod")]).unwrap();
        assert_eq!(c.root(), val("a = 1"));
    }

    #[test]
    fn explicit_profile_beats_env_var() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("application-dev.toml"), "a = 1").unwrap();
        let c = build(d.path(), Some("dev"), &[("OXBLOOM_PROFILE", "prod")]).unwrap();
        assert_eq!(c.root(), val("a = 1"));
    }

    #[test]
    fn profile_with_path_characters_rejected() {
        let d = tempfile::tempdir().unwrap();
        for bad in ["../x", "", "a/b", "a.b", "a b"] {
            let e = build(d.path(), Some(bad), &[]).unwrap_err();
            assert!(
                matches!(&e, ConfigError::Invalid { key, .. } if key == "profile"),
                "{bad}: {e:?}"
            );
        }
    }

    #[test]
    fn from_str_needs_no_filesystem() {
        let c = Config::from_str("[a]\nb = 1").unwrap();
        assert_eq!(c.root(), val("[a]\nb = 1"));
    }

    #[test]
    fn from_str_rejects_invalid_toml() {
        assert!(matches!(
            Config::from_str("a = = 1"),
            Err(ConfigError::Parse { .. })
        ));
    }

    #[test]
    fn config_debug_omits_values() {
        let c = Config::from_str("password = \"hunter2\"").unwrap();
        let text = format!("{c:?}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("password"), "{text}");
    }

    #[test]
    fn builder_debug_omits_env_values() {
        let b = Config::builder().env_from(env(&[("APP_PASSWORD", "hunter2")]));
        assert!(!format!("{b:?}").contains("hunter2"));
    }

    #[test]
    fn env_overrides_file_across_spellings() {
        #[derive(serde::Deserialize)]
        struct Server {
            max_connections: u8,
        }
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("application.toml"),
            "[server]\nmax-connections = 1\n",
        )
        .unwrap();
        let c = build(d.path(), None, &[("APP_SERVER__MAX_CONNECTIONS", "5")]).unwrap();
        assert_eq!(c.get::<Server>("server").unwrap().max_connections, 5);
        assert_eq!(c.get::<u8>("server.max-connections").unwrap(), 5);
        assert_eq!(c.root(), val("[server]\nmax_connections = \"5\""));
    }

    #[test]
    fn error_carries_origin_of_failing_value() {
        #[derive(Debug, serde::Deserialize)]
        struct Server {
            #[allow(dead_code)]
            host: String,
            #[allow(dead_code)]
            port: u16,
        }
        let origin_of = |c: &Config| match c.get::<Server>("server").unwrap_err() {
            ConfigError::Invalid { key, origin, .. } => {
                assert_eq!(key, "server.port");
                origin
            }
            other => panic!("{other:?}"),
        };
        // File line.
        let d = tempfile::tempdir().unwrap();
        fs::write(
            d.path().join("application.toml"),
            "[server]\nhost = \"h\"\nport = \"abc\"\n",
        )
        .unwrap();
        assert_eq!(
            origin_of(&build(d.path(), None, &[]).unwrap()),
            Some(Origin::File {
                path: "application.toml".into(),
                line: 3
            })
        );
        // Profile file line.
        fs::write(
            d.path().join("application-dev.toml"),
            "\n[server]\nport = 70000\n",
        )
        .unwrap();
        assert_eq!(
            origin_of(&build(d.path(), Some("dev"), &[]).unwrap()),
            Some(Origin::File {
                path: "application-dev.toml".into(),
                line: 3
            })
        );
        // Environment variable name.
        let c = build(d.path(), None, &[("APP_SERVER__PORT", "x")]).unwrap();
        assert_eq!(
            origin_of(&c),
            Some(Origin::Env {
                name: "APP_SERVER__PORT".into()
            })
        );
        // Inline text.
        let c = Config::from_str("[server]\nhost = \"h\"\n\nport = -1").unwrap();
        assert_eq!(origin_of(&c), Some(Origin::Inline { line: 4 }));
    }

    #[test]
    fn error_display_has_key_origin_reason_and_no_value() {
        let d = tempfile::tempdir().unwrap();
        let c = build(d.path(), None, &[("APP_SERVER__PORT", "abc-hunter2")]).unwrap();
        let e = c.get::<u16>("server.port").unwrap_err();
        assert_eq!(
            e.to_string(),
            "invalid configuration key `server.port` (from env APP_SERVER__PORT): expected unsigned integer"
        );
        assert!(!format!("{e:?}").contains("hunter2"), "{e:?}");
    }

    #[test]
    fn inline_parse_error_display_has_no_leading_colon() {
        let e = Config::from_str("a = 1\nb = = 1").unwrap_err();
        assert_eq!(e.to_string(), "line 2: invalid TOML");
    }

    #[test]
    fn config_debug_lists_top_level_keys_only() {
        let c = Config::from_str("Top-Key = 1\n[server]\nport = 8080").unwrap();
        let text = format!("{c:?}");
        assert!(
            text.contains("Top-Key") && text.contains("server"),
            "{text}"
        );
        assert!(!text.contains("port") && !text.contains("8080"), "{text}");
    }
}
