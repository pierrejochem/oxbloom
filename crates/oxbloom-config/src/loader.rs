//! The load pipeline: `application.toml` and its imports, active profiles (expanded through
//! groups) and their imports, then the environment and the command line.

use std::collections::HashSet;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::error::ConfigError;
use crate::node::{Node, NodeValue, Table, canonical};
use crate::origin::Origin;
use crate::sources::{apply_args, apply_env};

const PROFILE_ENV: &str = "OXBLOOM_PROFILE";
/// The one reserved top-level table. Only `oxbloom.config.import`, `oxbloom.profiles.active`
/// and `oxbloom.profiles.group.*` are interpreted by the loader.
const NS: &str = "oxbloom";
const BASE_FILE: &str = "application.toml";
const OPTIONAL: &str = "optional:";
/// Import nesting allowed below a top-level file (`application.toml` or a profile file).
const MAX_IMPORT_DEPTH: usize = 8;
/// `oxbloom.config.import` entries one load may follow in total (all files, repeats included). Bounds
/// the work of import fan-out (a file reached many ways is read each time).
const MAX_IMPORT_ENTRIES: usize = 256;
/// Default for [`crate::ConfigBuilder::max_file_size`]: 1 MiB.
pub(crate) const DEFAULT_MAX_FILE_SIZE: usize = 1024 * 1024;
/// Profiles active after group expansion, at most.
const MAX_ACTIVE_PROFILES: usize = 64;
/// Profile groups nested inside each other, at most.
const MAX_GROUP_DEPTH: usize = 8;

/// The result of [`load`]: the merged tree and the profiles it activated.
pub(crate) struct Loaded {
    pub(crate) root: Node,
    /// Active profiles after group expansion, in activation order, each once.
    pub(crate) profiles: Vec<String>,
}

/// Loads the merged tree from `dir`: `application.toml` and its imports, then each active
/// profile file and its imports in activation order, then `env` (with `prefix`) and `args`.
///
/// Active profiles: builder `profile()` calls if any, else `--oxbloom.profiles.active=`, else
/// `OXBLOOM_PROFILE`, else `oxbloom.profiles.active` of the base file; the first non-empty list wins.
/// Every name is validated before it is used for a file name.
pub(crate) fn load(
    dir: &Path,
    profiles: Vec<String>,
    env: &[(String, String)],
    prefix: &str,
    args: &[String],
    secret_keys: &[String],
    max_file_size: usize,
) -> Result<Loaded, ConfigError> {
    let mut requested = profiles;
    if requested.is_empty() {
        requested = cli_profiles(args);
    }
    if requested.is_empty() {
        requested = env
            .iter()
            .find(|(k, _)| k == PROFILE_ENV)
            .map(|(_, v)| split_names(v))
            .unwrap_or_default();
    }
    for name in &requested {
        validate_name(name, None)?;
    }

    let mut loader = Loader {
        dir,
        canonical_dir: None,
        entries_followed: 0,
        max_file_size,
    };
    let mut root = match loader.read_file(Path::new(BASE_FILE))? {
        Some(node) => node,
        // No base file: an empty root, attributed to where the base file would be.
        None => Node::table(
            Origin::File {
                path: BASE_FILE.into(),
                line: 1,
            },
            Table::new(),
        ),
    };

    if requested.is_empty()
        && let Some(node) = lookup(&root, &[NS, "profiles", "active"])
    {
        requested = names_of(node, "oxbloom.profiles.active")?;
        for name in &requested {
            validate_name(name, Some(&node.origin))?;
        }
    }

    let groups = lookup(&root, &[NS, "profiles", "group"]).and_then(Node::as_table);
    let active = expand_all(&requested, groups)?;

    for name in &active {
        let file = PathBuf::from(format!("application-{name}.toml"));
        let overlay = loader.read_file(&file)?.ok_or_else(|| ConfigError::Io {
            path: dir.join(&file),
            source: std::io::Error::from(std::io::ErrorKind::NotFound),
        })?;
        if let Some(node) = lookup(&overlay, &[NS, "profiles", "active"]) {
            return Err(ConfigError::Invalid {
                key: "oxbloom.profiles.active".into(),
                message: "profiles can only be activated in application.toml".into(),
                origin: Some(node.origin.clone()),
            });
        }
        root.merge(overlay);
    }

    apply_env(&mut root, prefix, env.iter().cloned())?;
    apply_args(&mut root, args.iter().cloned(), secret_keys)?;
    Ok(Loaded {
        root,
        profiles: active,
    })
}

/// The last `--oxbloom.profiles.active=` value on the command line, split at commas.
fn cli_profiles(args: &[String]) -> Vec<String> {
    args.iter()
        .filter_map(|arg| arg.strip_prefix("--")?.split_once('='))
        .rfind(|(key, _)| {
            let segs: Vec<String> = key.split('.').map(canonical).collect();
            segs == [NS, "profiles", "active"]
        })
        .map(|(_, value)| split_names(value))
        .unwrap_or_default()
}

/// Comma-separated names, trimmed, empty items dropped.
fn split_names(text: &str) -> Vec<String> {
    text.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect()
}

/// Profile names from a string (comma-separated) or an array of strings at `key`.
fn names_of(node: &Node, key: &str) -> Result<Vec<String>, ConfigError> {
    let wrong = |origin: &Origin| ConfigError::Invalid {
        key: key.into(),
        message: "expected a string or an array of strings".into(),
        origin: Some(origin.clone()),
    };
    match &node.value {
        NodeValue::String(s) => Ok(split_names(s)),
        NodeValue::Array(items) => items
            .iter()
            .map(|item| match &item.value {
                NodeValue::String(s) => Ok(s.trim().to_string()),
                _ => Err(wrong(&item.origin)),
            })
            .collect(),
        _ => Err(wrong(&node.origin)),
    }
}

fn validate_name(name: &str, origin: Option<&Origin>) -> Result<(), ConfigError> {
    if !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        return Ok(());
    }
    Err(ConfigError::Invalid {
        key: "profile".into(),
        message: "must be non-empty and contain only letters, digits, `_` or `-`".into(),
        origin: origin.cloned(),
    })
}

/// Expands `requested` through `groups` into the active profiles, in activation order, each
/// name once (first occurrence kept). At most [`MAX_ACTIVE_PROFILES`] names and
/// [`MAX_GROUP_DEPTH`] levels of group nesting.
fn expand_all(requested: &[String], groups: Option<&Table>) -> Result<Vec<String>, ConfigError> {
    let mut active = Active::default();
    for name in requested {
        expand(name, groups, &mut Vec::new(), &mut active)?;
    }
    Ok(active.order)
}

#[derive(Default)]
struct Active {
    order: Vec<String>,
    seen: HashSet<String>,
}

/// Appends `name` and then, if it is a group, its members (recursively) to `out`, keeping the
/// first occurrence of each. `stack` holds the groups being expanded, by canonical name.
fn expand(
    name: &str,
    groups: Option<&Table>,
    stack: &mut Vec<String>,
    out: &mut Active,
) -> Result<(), ConfigError> {
    let canon = canonical(name);
    if stack.contains(&canon) {
        let entry = groups.and_then(|g| g.get_entry(name));
        return Err(ConfigError::Invalid {
            key: format!(
                "oxbloom.profiles.group.{}",
                entry.map_or(name, |e| e.spelling.as_str())
            ),
            message: "profile group includes itself".into(),
            origin: entry.map(|e| e.node.origin.clone()),
        });
    }
    if out.seen.contains(name) {
        return Ok(());
    }
    if out.order.len() == MAX_ACTIVE_PROFILES {
        return Err(ConfigError::invalid(
            "oxbloom.profiles.active",
            "more than 64 active profiles",
        ));
    }
    out.seen.insert(name.to_string());
    out.order.push(name.to_string());
    let Some(group) = groups.and_then(|g| g.get_entry(name)) else {
        return Ok(());
    };
    let key = format!("oxbloom.profiles.group.{}", group.spelling);
    if stack.len() == MAX_GROUP_DEPTH {
        return Err(ConfigError::Invalid {
            key,
            message: "profile groups are nested more than 8 levels deep".into(),
            origin: Some(group.node.origin.clone()),
        });
    }
    let members = names_of(&group.node, &key)?;
    stack.push(canon);
    for member in &members {
        validate_name(member, Some(&group.node.origin))?;
        expand(member, groups, stack, out)?;
    }
    stack.pop();
    Ok(())
}

/// The node at `path` (canonical match per segment).
fn lookup<'a>(root: &'a Node, path: &[&str]) -> Option<&'a Node> {
    path.iter()
        .try_fold(root, |node, seg| node.as_table()?.get(seg))
}

struct Loader<'a> {
    dir: &'a Path,
    /// `dir` after `std::fs::canonicalize`, computed on the first import.
    canonical_dir: Option<PathBuf>,
    /// `oxbloom.config.import` entries processed so far in this load, against [`MAX_IMPORT_ENTRIES`].
    entries_followed: usize,
    /// Largest file read, in bytes.
    max_file_size: usize,
}

/// One file of an import chain.
struct Link {
    /// Path relative to the config dir, as written.
    relative: PathBuf,
    /// Canonical path, for cycle detection.
    canonical: PathBuf,
}

impl Loader<'_> {
    /// Reads a top-level file (relative to the config dir) and its imports; `None` when the
    /// file does not exist.
    fn read_file(&mut self, relative: &Path) -> Result<Option<Node>, ConfigError> {
        let path = self.dir.join(relative);
        let Some(mut node) = read(self.dir, relative, &path, self.max_file_size)? else {
            return Ok(None);
        };
        if lookup(&node, &[NS, "config", "import"]).is_some() {
            let mut chain = vec![Link {
                relative: relative.to_path_buf(),
                canonical: self.canonical(&path)?,
            }];
            self.apply_imports(&mut node, &mut chain)?;
        }
        Ok(Some(node))
    }

    /// Removes `oxbloom.config.import` from `node` and merges each listed file (with its own imports)
    /// over it, in list order. `chain` ends with the file `node` was read from.
    fn apply_imports(&mut self, node: &mut Node, chain: &mut Vec<Link>) -> Result<(), ConfigError> {
        let Some(entries) = take_imports(node)? else {
            return Ok(());
        };
        let canonical_dir = self.canonical_dir()?;
        for entry in entries {
            self.entries_followed += 1;
            if self.entries_followed > MAX_IMPORT_ENTRIES {
                return Err(ConfigError::ImportLimit);
            }
            let (optional, written) = match entry.strip_prefix(OPTIONAL) {
                Some(rest) => (true, rest),
                None => (false, entry.as_str()),
            };
            let relative = PathBuf::from(written);
            let invalid = || ConfigError::ImportInvalid {
                path: relative.clone(),
            };
            let plain = !written.is_empty()
                && relative
                    .components()
                    .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
            if !plain {
                return Err(invalid());
            }
            let full = self.dir.join(&relative);
            let canonical = match std::fs::canonicalize(&full) {
                Ok(path) => path,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if optional {
                        continue;
                    }
                    return Err(ConfigError::ImportMissing { path: relative });
                }
                Err(source) => return Err(ConfigError::Io { path: full, source }),
            };
            if !canonical.starts_with(&canonical_dir) {
                return Err(invalid());
            }
            if let Some(at) = chain.iter().position(|l| l.canonical == canonical) {
                let mut files: Vec<PathBuf> =
                    chain[at..].iter().map(|l| l.relative.clone()).collect();
                files.push(relative);
                return Err(ConfigError::ImportCycle { files });
            }
            if chain.len() > MAX_IMPORT_DEPTH {
                return Err(ConfigError::ImportTooDeep);
            }
            // Read the resolved path: the one that was checked against the config dir.
            let Some(mut imported) = read(self.dir, &relative, &canonical, self.max_file_size)?
            else {
                if optional {
                    continue;
                }
                return Err(ConfigError::ImportMissing { path: relative });
            };
            chain.push(Link {
                relative,
                canonical,
            });
            self.apply_imports(&mut imported, chain)?;
            chain.pop();
            node.merge(imported);
        }
        Ok(())
    }

    fn canonical_dir(&mut self) -> Result<PathBuf, ConfigError> {
        if self.canonical_dir.is_none() {
            self.canonical_dir = Some(self.canonical(self.dir)?);
        }
        Ok(self.canonical_dir.clone().unwrap_or_default())
    }

    fn canonical(&self, path: &Path) -> Result<PathBuf, ConfigError> {
        std::fs::canonicalize(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })
    }
}

/// Removes `oxbloom.config.import` from `node` (and `oxbloom.config`, then `oxbloom`, when
/// that leaves them empty), returning the entries. `Invalid` when it is not an array of
/// strings.
fn take_imports(node: &mut Node) -> Result<Option<Vec<String>>, ConfigError> {
    let Some(root) = node.as_table_mut() else {
        return Ok(None);
    };
    let Some(ns) = root.get_mut(NS).and_then(Node::as_table_mut) else {
        return Ok(None);
    };
    let Some(config) = ns.get_mut("config").and_then(Node::as_table_mut) else {
        return Ok(None);
    };
    let Some(entry) = config.remove("import") else {
        return Ok(None);
    };
    if config.is_empty() {
        ns.remove("config");
    }
    if ns.is_empty() {
        root.remove(NS);
    }
    let wrong = |origin: &Origin| ConfigError::Invalid {
        key: "oxbloom.config.import".into(),
        message: "expected an array of strings".into(),
        origin: Some(origin.clone()),
    };
    match entry.node.value {
        NodeValue::Array(items) => items
            .into_iter()
            .map(|item| match item.value {
                NodeValue::String(s) => Ok(s),
                _ => Err(wrong(&item.origin)),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(Some),
        _ => Err(wrong(&entry.node.origin)),
    }
}

/// Reads and parses `path`; origins name `relative` (relative to `dir`). `None` when the file
/// does not exist. A parse error keeps the full path. `path` must be a regular file (after
/// following symlinks) of at most `cap` bytes; it is checked before it is opened, so a FIFO
/// or a device is never read.
fn read(dir: &Path, relative: &Path, path: &Path, cap: usize) -> Result<Option<Node>, ConfigError> {
    let io = |source| ConfigError::Io {
        path: dir.join(relative),
        source,
    };
    match std::fs::metadata(path) {
        Ok(meta) if !meta.is_file() => {
            return Err(io(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file",
            )));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(io(source)),
    }
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|file| {
            file.take((cap as u64).saturating_add(1))
                .read_to_end(&mut bytes)
        })
        .map_err(io)?;
    if bytes.len() > cap {
        return Err(ConfigError::FileTooLarge {
            path: dir.join(relative),
        });
    }
    let text = String::from_utf8(bytes).map_err(|_| {
        io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "stream did not contain valid UTF-8",
        ))
    })?;
    Node::parse_toml(&text, |line| Origin::File {
        path: relative.to_path_buf(),
        line,
    })
    .map(Some)
    .map_err(|e| match e {
        ConfigError::Parse { line, message, .. } => ConfigError::Parse {
            path: dir.join(relative),
            line,
            message,
        },
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::origin::Origin;
    use std::fs;
    use std::path::PathBuf;

    fn val(s: &str) -> toml::Value {
        toml::from_str(s).unwrap()
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    /// Loads with builder profiles, environment and arguments; prefix `APP`, no secret keys.
    fn run(
        dir: &Path,
        profiles: &[&str],
        env: &[(&str, &str)],
        args: &[&str],
    ) -> Result<Node, ConfigError> {
        load(
            dir,
            strings(profiles),
            &pairs(env),
            "APP",
            &strings(args),
            &[],
            DEFAULT_MAX_FILE_SIZE,
        )
        .map(|loaded| loaded.root)
    }

    fn tree(dir: &Path, profiles: &[&str], env: &[(&str, &str)], args: &[&str]) -> toml::Value {
        run(dir, profiles, env, args).unwrap().to_toml_value()
    }

    fn write(dir: &Path, name: &str, text: &str) {
        let path = dir.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, text).unwrap();
    }

    fn origin_at(root: &Node, key: &str) -> Origin {
        let mut node = root;
        for seg in key.split('.') {
            node = node.as_table().and_then(|t| t.get(seg)).unwrap();
        }
        node.origin.clone()
    }

    fn file(path: &str, line: usize) -> Origin {
        Origin::File {
            path: path.into(),
            line,
        }
    }

    // ---- imports -------------------------------------------------------------------------

    #[test]
    fn import_overrides_importing_file() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"extra.toml\"]\na = 1\nb = 1\n",
        );
        write(d.path(), "extra.toml", "b = 2\nc = 2\n");
        assert_eq!(tree(d.path(), &[], &[], &[]), val("a = 1\nb = 2\nc = 2"));
    }

    #[test]
    fn namespaced_config_import_key_is_removed_from_tree() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "[oxbloom.config]\nimport = [\"extra.toml\"]\nother = 1\n",
        );
        write(
            d.path(),
            "extra.toml",
            "oxbloom.config.import = []\nx = 1\n",
        );
        assert_eq!(
            tree(d.path(), &[], &[], &[]),
            val("x = 1\n[oxbloom.config]\nother = 1")
        );
        // An `oxbloom` table holding only the import list disappears.
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"extra.toml\"]\n",
        );
        assert_eq!(tree(d.path(), &[], &[], &[]), val("x = 1"));
    }

    #[test]
    fn non_array_config_import_is_invalid() {
        let d = tempfile::tempdir().unwrap();
        for text in [
            "oxbloom.config.import = \"extra.toml\"",
            "oxbloom.config.import = [1]",
        ] {
            write(d.path(), "application.toml", text);
            let e = run(d.path(), &[], &[], &[]).unwrap_err();
            assert!(
                matches!(&e, ConfigError::Invalid { key, origin: Some(_), .. } if key == "oxbloom.config.import"),
                "{text}: {e:?}"
            );
        }
    }

    #[test]
    fn imports_apply_in_list_order() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"one.toml\", \"sub/two.toml\"]\nv = 0\n",
        );
        write(d.path(), "one.toml", "v = 1\nonly-one = 1\n");
        write(d.path(), "sub/two.toml", "v = 2\n");
        assert_eq!(tree(d.path(), &[], &[], &[]), val("v = 2\nonly-one = 1"));
    }

    #[test]
    fn optional_import_missing_is_ok() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"optional:local.toml\"]\na = 1\n",
        );
        assert_eq!(tree(d.path(), &[], &[], &[]), val("a = 1"));
        // Present optional imports apply.
        write(d.path(), "local.toml", "a = 2\n");
        assert_eq!(tree(d.path(), &[], &[], &[]), val("a = 2"));
    }

    #[test]
    fn missing_import_is_import_missing() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"gone.toml\"]\n",
        );
        let e = run(d.path(), &[], &[], &[]).unwrap_err();
        assert!(
            matches!(&e, ConfigError::ImportMissing { path } if path == Path::new("gone.toml")),
            "{e:?}"
        );
    }

    #[test]
    fn import_cycle_is_error_naming_files() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"a.toml\"]\n",
        );
        write(d.path(), "a.toml", "oxbloom.config.import = [\"b.toml\"]\n");
        write(d.path(), "b.toml", "oxbloom.config.import = [\"a.toml\"]\n");
        let e = run(d.path(), &[], &[], &[]).unwrap_err();
        let expected: Vec<PathBuf> = vec!["a.toml".into(), "b.toml".into(), "a.toml".into()];
        assert!(
            matches!(&e, ConfigError::ImportCycle { files } if *files == expected),
            "{e:?}"
        );
        // A file importing itself.
        write(d.path(), "a.toml", "oxbloom.config.import = [\"a.toml\"]\n");
        let e = run(d.path(), &[], &[], &[]).unwrap_err();
        let expected: Vec<PathBuf> = vec!["a.toml".into(), "a.toml".into()];
        assert!(
            matches!(&e, ConfigError::ImportCycle { files } if *files == expected),
            "{e:?}"
        );
        // The base file importing itself.
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"application.toml\"]\n",
        );
        let e = run(d.path(), &[], &[], &[]).unwrap_err();
        assert!(matches!(e, ConfigError::ImportCycle { .. }), "{e:?}");
    }

    #[test]
    fn same_file_imported_twice_without_cycle_is_ok() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"a.toml\", \"b.toml\"]\n",
        );
        write(
            d.path(),
            "a.toml",
            "oxbloom.config.import = [\"common.toml\"]\na = 1\n",
        );
        write(
            d.path(),
            "b.toml",
            "oxbloom.config.import = [\"common.toml\"]\nb = 1\n",
        );
        write(d.path(), "common.toml", "c = 1\n");
        assert_eq!(tree(d.path(), &[], &[], &[]), val("a = 1\nb = 1\nc = 1"));
    }

    /// `application.toml` imports `f1.toml`, which imports `f2.toml`, ... up to `f{depth}`.
    fn import_chain(dir: &Path, depth: usize) {
        write(
            dir,
            "application.toml",
            "oxbloom.config.import = [\"f1.toml\"]\n",
        );
        for i in 1..=depth {
            let next = if i < depth {
                format!("oxbloom.config.import = [\"f{}.toml\"]\n", i + 1)
            } else {
                String::new()
            };
            write(dir, &format!("f{i}.toml"), &format!("{next}level = {i}\n"));
        }
    }

    #[test]
    fn import_depth_over_8_is_too_deep() {
        let d = tempfile::tempdir().unwrap();
        import_chain(d.path(), 8);
        assert_eq!(tree(d.path(), &[], &[], &[]), val("level = 8"));
        let d = tempfile::tempdir().unwrap();
        import_chain(d.path(), 9);
        let e = run(d.path(), &[], &[], &[]).unwrap_err();
        assert!(matches!(e, ConfigError::ImportTooDeep), "{e:?}");
    }

    #[test]
    fn absolute_and_dotdot_imports_are_invalid() {
        let outer = tempfile::tempdir().unwrap();
        let dir = outer.path().join("config");
        fs::create_dir_all(&dir).unwrap();
        write(outer.path(), "outside.toml", "leak = 1\n");
        let absolute = outer.path().join("outside.toml");
        let absolute = absolute.to_str().unwrap().replace('\\', "\\\\");
        for entry in [
            absolute.as_str(),
            "../outside.toml",
            "optional:../outside.toml",
            "sub/../../outside.toml",
            "sub/../x.toml",
            "optional:../missing.toml",
            "",
        ] {
            write(
                &dir,
                "application.toml",
                &format!("oxbloom.config.import = [\"{entry}\"]\n"),
            );
            let e = run(&dir, &[], &[], &[]).unwrap_err();
            assert!(
                matches!(e, ConfigError::ImportInvalid { .. }),
                "{entry}: {e:?}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlink_import_leaving_dir_is_invalid() {
        let outer = tempfile::tempdir().unwrap();
        let dir = outer.path().join("config");
        fs::create_dir_all(&dir).unwrap();
        write(outer.path(), "outside.toml", "leak = 1\n");
        std::os::unix::fs::symlink(outer.path().join("outside.toml"), dir.join("link.toml"))
            .unwrap();
        std::os::unix::fs::symlink(outer.path(), dir.join("up")).unwrap();
        for entry in ["link.toml", "optional:link.toml", "up/outside.toml"] {
            write(
                &dir,
                "application.toml",
                &format!("oxbloom.config.import = [\"{entry}\"]\n"),
            );
            let e = run(&dir, &[], &[], &[]).unwrap_err();
            assert!(
                matches!(&e, ConfigError::ImportInvalid { path } if !path.as_os_str().is_empty()),
                "{entry}: {e:?}"
            );
        }
        // A symlink that stays inside the directory is fine.
        write(&dir, "real.toml", "inside = 1\n");
        std::os::unix::fs::symlink(dir.join("real.toml"), dir.join("alias.toml")).unwrap();
        write(
            &dir,
            "application.toml",
            "oxbloom.config.import = [\"alias.toml\"]\n",
        );
        assert_eq!(tree(&dir, &[], &[], &[]), val("inside = 1"));
    }

    #[cfg(unix)]
    #[test]
    fn config_dir_may_be_a_symlink() {
        let outer = tempfile::tempdir().unwrap();
        let real = outer.path().join("real");
        fs::create_dir_all(&real).unwrap();
        write(
            &real,
            "application.toml",
            "oxbloom.config.import = [\"extra.toml\"]\na = 1\n",
        );
        write(&real, "extra.toml", "b = 2\n");
        let link = outer.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(tree(&link, &[], &[], &[]), val("a = 1\nb = 2"));
    }

    #[test]
    fn import_inside_profile_file_works() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "application.toml", "a = 0\nb = 0\n");
        write(
            d.path(),
            "application-dev.toml",
            "oxbloom.config.import = [\"dev-extra.toml\"]\na = 1\n",
        );
        write(d.path(), "dev-extra.toml", "b = 2\n");
        assert_eq!(tree(d.path(), &["dev"], &[], &[]), val("a = 1\nb = 2"));
    }

    #[test]
    fn imports_keep_their_own_file_origin() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"sub/extra.toml\"]\nbase = 1\n",
        );
        write(d.path(), "sub/extra.toml", "\n\n[db]\nurl = \"x\"\n");
        let root = run(d.path(), &[], &[], &[]).unwrap();
        assert_eq!(origin_at(&root, "base"), file("application.toml", 2));
        assert_eq!(origin_at(&root, "db.url"), file("sub/extra.toml", 4));
    }

    #[test]
    fn import_parse_error_names_imported_file() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"bad.toml\"]\n",
        );
        write(d.path(), "bad.toml", "a = = \"hunter2\"\n");
        let e = run(d.path(), &[], &[], &[]).unwrap_err();
        assert!(
            matches!(&e, ConfigError::Parse { path, .. } if path.ends_with("bad.toml")),
            "{e:?}"
        );
        assert!(!format!("{e} {e:?}").contains("hunter2"), "{e:?}");
    }

    // ---- profiles ------------------------------------------------------------------------

    #[test]
    fn builder_profiles_apply_in_call_order() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "application-a.toml", "v = \"a\"\nonly-a = 1\n");
        write(d.path(), "application-b.toml", "v = \"b\"\n");
        let c = Config::builder()
            .dir(d.path())
            .profile("a")
            .profile("b")
            .env_from([])
            .build()
            .unwrap();
        assert_eq!(c.root(), val("v = \"b\"\nonly-a = 1"));
    }

    #[test]
    fn comma_separated_env_profiles_apply_in_order() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "application-a.toml", "v = \"a\"\nonly-a = 1\n");
        write(d.path(), "application-b.toml", "v = \"b\"\n");
        let env = [("OXBLOOM_PROFILE", " a, ,b ,")];
        assert_eq!(tree(d.path(), &[], &env, &[]), val("v = \"b\"\nonly-a = 1"));
        let env = [("OXBLOOM_PROFILE", "b,a")];
        assert_eq!(tree(d.path(), &[], &env, &[]), val("v = \"a\"\nonly-a = 1"));
    }

    #[test]
    fn cli_profiles_active_applies_in_order() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "application-a.toml", "v = \"a\"\n");
        write(d.path(), "application-b.toml", "v = \"b\"\n");
        let args = ["--oxbloom.profiles.active=a,b"];
        assert_eq!(
            tree(d.path(), &[], &[], &args),
            val("v = \"b\"\n[oxbloom.profiles]\nactive = \"a,b\"")
        );
    }

    #[test]
    fn profiles_active_in_base_file_applies() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "application-a.toml", "v = \"a\"\n");
        write(d.path(), "application-b.toml", "v = \"b\"\n");
        write(
            d.path(),
            "application.toml",
            "oxbloom.profiles.active = \"b, a\"\n",
        );
        assert_eq!(
            tree(d.path(), &[], &[], &[]).get("v"),
            Some(&toml::Value::String("a".into()))
        );
        write(
            d.path(),
            "application.toml",
            "oxbloom.profiles.active = [\"a\", \"b\"]\n",
        );
        assert_eq!(
            tree(d.path(), &[], &[], &[]).get("v"),
            Some(&toml::Value::String("b".into()))
        );
        // Set by an import of the base file.
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"p.toml\"]\n",
        );
        write(d.path(), "p.toml", "oxbloom.profiles.active = \"a\"\n");
        assert_eq!(
            tree(d.path(), &[], &[], &[]).get("v"),
            Some(&toml::Value::String("a".into()))
        );
    }

    /// Base file activates `base`, env `env`, command line `cli`; each profile sets `v`.
    fn preference_dir() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "oxbloom.profiles.active = \"base\"\n",
        );
        for p in ["base", "env", "cli", "builder"] {
            write(
                d.path(),
                &format!("application-{p}.toml"),
                &format!("v = \"{p}\"\n"),
            );
        }
        d
    }

    fn active(d: &Path, profiles: &[&str], env: &[(&str, &str)], args: &[&str]) -> toml::Value {
        tree(d, profiles, env, args).get("v").cloned().unwrap()
    }

    #[test]
    fn profile_preference_order_builder_beats_cli() {
        let d = preference_dir();
        let v = active(
            d.path(),
            &["builder"],
            &[],
            &["--oxbloom.profiles.active=cli"],
        );
        assert_eq!(v, toml::Value::String("builder".into()));
    }

    #[test]
    fn profile_preference_order_cli_beats_env() {
        let d = preference_dir();
        let v = active(
            d.path(),
            &[],
            &[("OXBLOOM_PROFILE", "env")],
            &["--oxbloom.profiles.active=cli"],
        );
        assert_eq!(v, toml::Value::String("cli".into()));
        // An empty command-line value falls through to the environment.
        let v = active(
            d.path(),
            &[],
            &[("OXBLOOM_PROFILE", "env")],
            &["--oxbloom.profiles.active="],
        );
        assert_eq!(v, toml::Value::String("env".into()));
    }

    #[test]
    fn profile_preference_order_env_beats_base_file() {
        let d = preference_dir();
        let v = active(d.path(), &[], &[("OXBLOOM_PROFILE", "env")], &[]);
        assert_eq!(v, toml::Value::String("env".into()));
        let v = active(d.path(), &[], &[], &[]);
        assert_eq!(v, toml::Value::String("base".into()));
    }

    #[test]
    fn profiles_active_in_profile_file_is_invalid() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application-dev.toml",
            "\noxbloom.profiles.active = \"x\"\n",
        );
        let e = run(d.path(), &["dev"], &[], &[]).unwrap_err();
        assert!(
            matches!(&e, ConfigError::Invalid { key, origin: Some(o), .. }
                if key == "oxbloom.profiles.active" && *o == file("application-dev.toml", 2)),
            "{e:?}"
        );
        // Also when it arrives through an import of the profile file.
        write(
            d.path(),
            "application-dev.toml",
            "oxbloom.config.import = [\"dev-extra.toml\"]\n",
        );
        write(
            d.path(),
            "dev-extra.toml",
            "[oxbloom.profiles]\nactive = [\"x\"]\n",
        );
        let e = run(d.path(), &["dev"], &[], &[]).unwrap_err();
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "oxbloom.profiles.active"),
            "{e:?}"
        );
    }

    #[test]
    fn profile_group_expands_in_order() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "[oxbloom.profiles.group]\nprod = [\"db\", \"metrics\"]\ndb = [\"pool\", \"metrics\"]\n",
        );
        for p in ["prod", "db", "pool", "metrics", "extra"] {
            write(
                d.path(),
                &format!("application-{p}.toml"),
                &format!("v = \"{p}\"\n{p} = 1\n"),
            );
        }
        // prod, db, pool, metrics: `metrics` once (first occurrence), last wins `v`.
        let t = tree(d.path(), &["prod"], &[], &[]);
        assert_eq!(t.get("v"), Some(&toml::Value::String("metrics".into())));
        for p in ["prod", "db", "pool", "metrics"] {
            assert!(t.get(p).is_some(), "{p}");
        }
        // Order check: `extra` after the group wins; `db` repeated later is not re-applied.
        let t = tree(d.path(), &["prod", "extra", "db"], &[], &[]);
        assert_eq!(t.get("v"), Some(&toml::Value::String("extra".into())));
        // `pool` before `db` keeps its first position, so `db`'s members come after it.
        let t = tree(d.path(), &["metrics", "db"], &[], &[]);
        assert_eq!(t.get("v"), Some(&toml::Value::String("pool".into())));
    }

    #[test]
    fn profile_group_cycle_is_invalid() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "[oxbloom.profiles.group]\na = [\"b\"]\nb = [\"c\", \"a\"]\n",
        );
        for p in ["a", "b", "c"] {
            write(d.path(), &format!("application-{p}.toml"), "");
        }
        let e = run(d.path(), &["a"], &[], &[]).unwrap_err();
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "oxbloom.profiles.group.a"),
            "{e:?}"
        );
        // A group naming itself.
        write(
            d.path(),
            "application.toml",
            "oxbloom.profiles.group.a = [\"a\"]\n",
        );
        let e = run(d.path(), &["a"], &[], &[]).unwrap_err();
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "oxbloom.profiles.group.a"),
            "{e:?}"
        );
    }

    #[test]
    fn profile_group_member_names_are_validated() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "oxbloom.profiles.group.a = [\"../x\"]\n",
        );
        write(d.path(), "application-a.toml", "");
        let e = run(d.path(), &["a"], &[], &[]).unwrap_err();
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "profile"),
            "{e:?}"
        );
    }

    /// Writes `application.toml` with `base` and an empty profile file for each of `names`.
    fn profile_dir(base: &str, names: impl IntoIterator<Item = String>) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "application.toml", base);
        for name in names {
            write(d.path(), &format!("application-{name}.toml"), "");
        }
        d
    }

    /// A chain `g0 = ["g1"]`, ..., `g{n-1} = ["leaf"]` as `[oxbloom.profiles.group]` text.
    fn group_chain(n: usize) -> String {
        let mut text = String::from("[oxbloom.profiles.group]\n");
        for i in 0..n {
            let next = if i + 1 == n {
                "leaf".to_string()
            } else {
                format!("g{}", i + 1)
            };
            text.push_str(&format!("g{i} = [\"{next}\"]\n"));
        }
        text
    }

    fn is_invalid_key(e: &ConfigError, want: &str) -> bool {
        matches!(e, ConfigError::Invalid { key, .. } if key == want)
    }

    #[test]
    fn group_chain_of_50000_is_invalid_without_stack_overflow() {
        let result = std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(|| {
                let d = profile_dir(&group_chain(50_000), []);
                run(d.path(), &[], &[("OXBLOOM_PROFILE", "g0")], &[]).map(|_| ())
            })
            .unwrap()
            .join()
            .unwrap();
        let e = result.unwrap_err();
        assert!(is_invalid_key(&e, "oxbloom.profiles.group.g8"), "{e:?}");
    }

    #[test]
    fn group_nesting_depth_8_is_accepted_and_9_is_invalid() {
        let names = |n: usize| (0..n).map(|i| format!("g{i}")).chain(["leaf".to_string()]);
        let d = profile_dir(&group_chain(8), names(8));
        assert!(run(d.path(), &["g0"], &[], &[]).is_ok());
        let d = profile_dir(&group_chain(9), names(9));
        let e = run(d.path(), &["g0"], &[], &[]).unwrap_err();
        assert!(is_invalid_key(&e, "oxbloom.profiles.group.g8"), "{e:?}");
        assert!(
            e.to_string()
                .ends_with(": profile groups are nested more than 8 levels deep"),
            "{e}"
        );
    }

    #[test]
    fn group_with_100000_members_is_invalid_quickly() {
        // Short distinct names (base 36) keep the file well under the size cap.
        let name = |mut i: usize| {
            let digits = b"0123456789abcdefghijklmnopqrstuvwxyz";
            let mut s = Vec::new();
            loop {
                s.push(digits[i % 36]);
                i /= 36;
                if i == 0 {
                    break;
                }
            }
            format!("m{}", String::from_utf8(s).unwrap())
        };
        let members: Vec<String> = (0..100_000).map(|i| format!("\"{}\"", name(i))).collect();
        let d = profile_dir(
            &format!("oxbloom.profiles.group.big = [{}]\n", members.join(",")),
            [],
        );
        let start = std::time::Instant::now();
        let e = run(d.path(), &["big"], &[], &[]).unwrap_err();
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        assert!(is_invalid_key(&e, "oxbloom.profiles.active"), "{e:?}");
    }

    #[test]
    fn active_profiles_64_accepted_65_invalid() {
        let names = |n: usize| (0..n).map(|i| format!("p{i}")).collect::<Vec<_>>();
        let d = profile_dir("", names(65));
        let message =
            "invalid configuration key `oxbloom.profiles.active`: more than 64 active profiles";
        for (n, ok) in [(64, true), (65, false)] {
            let list = names(n);
            let refs: Vec<&str> = list.iter().map(String::as_str).collect();
            let joined = list.join(",");
            let arg = format!("--oxbloom.profiles.active={joined}");
            let results = [
                run(d.path(), &refs, &[], &[]),
                run(d.path(), &[], &[], &[arg.as_str()]),
                run(d.path(), &[], &[("OXBLOOM_PROFILE", joined.as_str())], &[]),
            ];
            for result in results {
                match result {
                    Ok(_) => assert!(ok, "{n} accepted"),
                    Err(e) => {
                        assert!(!ok, "{n}: {e:?}");
                        assert_eq!(e.to_string(), message);
                    }
                }
            }
            write(
                d.path(),
                "application.toml",
                &format!("oxbloom.profiles.active = \"{joined}\"\n"),
            );
            let result = run(d.path(), &[], &[], &[]);
            assert_eq!(result.is_ok(), ok, "{n} base file: {result:?}");
            // Through a group: the group itself plus n - 1 members.
            let members: Vec<String> = list[1..].iter().map(|m| format!("\"{m}\"")).collect();
            write(
                d.path(),
                "application.toml",
                &format!("oxbloom.profiles.group.p0 = [{}]\n", members.join(",")),
            );
            let result = run(d.path(), &["p0"], &[], &[]);
            assert_eq!(result.is_ok(), ok, "{n} group: {result:?}");
            write(d.path(), "application.toml", "");
        }
        // Repeats do not count.
        let many = vec!["p1"; 1000];
        assert!(run(d.path(), &many, &[], &[]).is_ok());
    }

    #[test]
    fn dedupe_keeps_first_occurrence_order() {
        let mut groups = Table::new();
        let origin = Origin::File {
            path: "application.toml".into(),
            line: 1,
        };
        let member = |s: &str| Node {
            origin: origin.clone(),
            value: NodeValue::String(s.into()),
        };
        groups.insert(
            "g",
            Node {
                origin: origin.clone(),
                value: NodeValue::Array(vec![member("c"), member("a"), member("d")]),
            },
        );
        let active = expand_all(&strings(&["b", "a", "b", "g", "c", "a"]), Some(&groups)).unwrap();
        assert_eq!(active, strings(&["b", "a", "g", "c", "d"]));
    }

    #[test]
    fn profile_name_with_path_chars_invalid() {
        let d = tempfile::tempdir().unwrap();
        // Unparsable base file: names from builder, command line and environment are checked
        // before any file is read.
        write(d.path(), "application.toml", "a = = 1\n");
        let is_profile_error =
            |e: &ConfigError| matches!(e, ConfigError::Invalid { key, .. } if key == "profile");
        for bad in ["../x", "", "a/b", "a.b", "a b"] {
            let e = run(d.path(), &[bad], &[], &[]).unwrap_err();
            assert!(is_profile_error(&e), "builder {bad:?}: {e:?}");
        }
        for bad in ["../x", "a/b", "ok,a.b"] {
            let e = run(d.path(), &[], &[("OXBLOOM_PROFILE", bad)], &[]).unwrap_err();
            assert!(is_profile_error(&e), "env {bad:?}: {e:?}");
            let arg = format!("--oxbloom.profiles.active={bad}");
            let e = run(d.path(), &[], &[], &[arg.as_str()]).unwrap_err();
            assert!(is_profile_error(&e), "cli {bad:?}: {e:?}");
        }
        // From the base file.
        write(
            d.path(),
            "application.toml",
            "oxbloom.profiles.active = \"../etc\"\n",
        );
        let e = run(d.path(), &[], &[], &[]).unwrap_err();
        assert!(is_profile_error(&e), "{e:?}");
    }

    #[test]
    fn requested_profile_without_file_is_io_error() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "application-a.toml", "");
        let e = run(d.path(), &["a", "b"], &[], &[]).unwrap_err();
        assert!(
            matches!(&e, ConfigError::Io { path, .. } if path.ends_with("application-b.toml")),
            "{e:?}"
        );
    }

    #[test]
    fn env_and_args_still_override_files() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"x.toml\"]\n",
        );
        write(d.path(), "x.toml", "a = 1\nb = 1\n");
        let t = tree(d.path(), &[], &[("APP_A", "2"), ("APP_B", "2")], &["--b=3"]);
        assert_eq!(t, val("a = \"2\"\nb = \"3\""));
    }

    #[test]
    fn import_errors_display_fixed_text_without_contents() {
        let d = tempfile::tempdir().unwrap();
        let cases: Vec<(&str, &str)> = vec![
            (
                "oxbloom.config.import = [\"hunter2.toml\"]",
                "configuration import file not found",
            ),
            (
                "oxbloom.config.import = [\"../hunter2.toml\"]",
                "configuration import path must be relative and stay inside the config directory",
            ),
        ];
        for (text, expected) in cases {
            write(d.path(), "application.toml", text);
            let e = run(d.path(), &[], &[], &[]).unwrap_err();
            assert_eq!(e.to_string(), expected);
        }
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"hunter2.toml\"]",
        );
        write(
            d.path(),
            "hunter2.toml",
            "oxbloom.config.import = [\"hunter2.toml\"]\nsecret = \"pw\"",
        );
        let e = run(d.path(), &[], &[], &[]).unwrap_err();
        assert_eq!(e.to_string(), "configuration imports form a cycle");
        assert!(!format!("{e:?}").contains("pw"), "{e:?}");
        assert_eq!(
            ConfigError::ImportTooDeep.to_string(),
            "configuration imports are nested more than 8 levels deep"
        );
    }
    // ---- import entry budget -------------------------------------------------------------

    /// Writes `application.toml` importing `file` `count` times, and an empty `file`.
    fn repeated_imports(dir: &Path, file: &str, count: usize) {
        let list = vec![format!("\"{file}\""); count].join(", ");
        write(
            dir,
            "application.toml",
            &format!("oxbloom.config.import = [{list}]\n"),
        );
        if !dir.join(file).exists() {
            write(dir, file, "");
        }
    }

    const BOMB_FAN_OUT: usize = 10;

    #[test]
    fn import_fan_out_bomb_hits_the_limit_quickly() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"f0.toml\"]\n",
        );
        for level in 0..8 {
            let text = if level < 7 {
                let list = vec![format!("\"f{}.toml\"", level + 1); BOMB_FAN_OUT].join(", ");
                format!("oxbloom.config.import = [{list}]\nlevel = {level}\n")
            } else {
                "level = 7\n".to_string()
            };
            write(d.path(), &format!("f{level}.toml"), &text);
        }
        let start = std::time::Instant::now();
        let e = run(d.path(), &[], &[], &[]).unwrap_err();
        let elapsed = start.elapsed();
        assert!(matches!(e, ConfigError::ImportLimit), "{e:?}");
        assert!(elapsed < std::time::Duration::from_secs(2), "{elapsed:?}");
    }

    #[test]
    fn exactly_256_import_entries_is_accepted() {
        let d = tempfile::tempdir().unwrap();
        repeated_imports(d.path(), "x.toml", 256);
        assert_eq!(tree(d.path(), &[], &[], &[]), val(""));
        // Optional entries for missing files count too, and 256 of them still succeed.
        let d = tempfile::tempdir().unwrap();
        repeated_imports(d.path(), "optional:none.toml", 256);
        assert!(run(d.path(), &[], &[], &[]).is_ok());
    }

    #[test]
    fn entry_257_is_import_limit() {
        let d = tempfile::tempdir().unwrap();
        repeated_imports(d.path(), "x.toml", 257);
        let e = run(d.path(), &[], &[], &[]).unwrap_err();
        assert!(matches!(e, ConfigError::ImportLimit), "{e:?}");
        // Missing optional entries count.
        let d = tempfile::tempdir().unwrap();
        repeated_imports(d.path(), "optional:none.toml", 257);
        let e = run(d.path(), &[], &[], &[]).unwrap_err();
        assert!(matches!(e, ConfigError::ImportLimit), "{e:?}");
        // The budget is shared by the base file and profile files: 200 + 57.
        let d = tempfile::tempdir().unwrap();
        repeated_imports(d.path(), "x.toml", 200);
        let list = vec!["\"x.toml\""; 56].join(", ");
        write(
            d.path(),
            "application-p.toml",
            &format!("oxbloom.config.import = [{list}]\n"),
        );
        assert!(run(d.path(), &["p"], &[], &[]).is_ok());
        let list = vec!["\"x.toml\""; 57].join(", ");
        write(
            d.path(),
            "application-p.toml",
            &format!("oxbloom.config.import = [{list}]\n"),
        );
        let e = run(d.path(), &["p"], &[], &[]).unwrap_err();
        assert!(matches!(e, ConfigError::ImportLimit), "{e:?}");
    }

    #[test]
    fn diamond_import_still_works_and_counts_each_entry() {
        let d = tempfile::tempdir().unwrap();
        // base -> a, b; a -> c; b -> c: 4 entries.
        let diamond = "\"a.toml\", \"b.toml\"";
        write(
            d.path(),
            "a.toml",
            "oxbloom.config.import = [\"c.toml\"]\na = 1\n",
        );
        write(
            d.path(),
            "b.toml",
            "oxbloom.config.import = [\"c.toml\"]\nb = 1\n",
        );
        write(d.path(), "c.toml", "c = 1\n");
        write(d.path(), "x.toml", "");
        write(
            d.path(),
            "application.toml",
            &format!("oxbloom.config.import = [{diamond}]\n"),
        );
        assert_eq!(tree(d.path(), &[], &[], &[]), val("a = 1\nb = 1\nc = 1"));
        // 4 diamond entries + 252 more = 256: accepted; one more is the 257th.
        for (extra, ok) in [(252, true), (253, false)] {
            let list = vec!["\"x.toml\""; extra].join(", ");
            write(
                d.path(),
                "application.toml",
                &format!("oxbloom.config.import = [{diamond}, {list}]\n"),
            );
            let result = run(d.path(), &[], &[], &[]);
            if ok {
                assert!(result.is_ok(), "{extra}: {result:?}");
            } else {
                assert!(
                    matches!(result, Err(ConfigError::ImportLimit)),
                    "{extra}: {result:?}"
                );
            }
        }
    }

    #[test]
    fn limit_is_per_load_not_global() {
        let d = tempfile::tempdir().unwrap();
        repeated_imports(d.path(), "x.toml", 200);
        assert!(run(d.path(), &[], &[], &[]).is_ok());
        assert!(run(d.path(), &[], &[], &[]).is_ok());
    }

    #[test]
    fn import_limit_error_has_fixed_text_and_no_path() {
        let d = tempfile::tempdir().unwrap();
        repeated_imports(d.path(), "hunter2.toml", 257);
        write(d.path(), "hunter2.toml", "secret = \"pw\"\n");
        let e = run(d.path(), &[], &[], &[]).unwrap_err();
        assert_eq!(
            e.to_string(),
            "too many configuration imports (more than 256)"
        );
        let debug = format!("{e:?}");
        assert!(
            !debug.contains("hunter2") && !debug.contains("pw"),
            "{debug}"
        );
    }

    // ---- `oxbloom.` namespace ------------------------------------------------------------

    fn is_invalid_at(e: &ConfigError, want: &str) -> bool {
        matches!(e, ConfigError::Invalid { key, .. } if key == want)
    }

    fn profile_files(d: &Path) {
        write(d, "application-a.toml", "v = \"a\"\n");
        write(d, "application-b.toml", "v = \"b\"\n");
    }

    #[test]
    fn namespaced_import_key_is_followed_and_removed_from_tree() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "[oxbloom.config]\nimport = [\"extra.toml\"]\nother = 1\n",
        );
        write(
            d.path(),
            "extra.toml",
            "oxbloom.config.import = []\nx = 1\n",
        );
        assert_eq!(
            tree(d.path(), &[], &[], &[]),
            val("x = 1\n[oxbloom.config]\nother = 1")
        );
    }

    #[test]
    fn old_config_import_key_is_ordinary_data() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "config.import = [\"extra.toml\"]\n",
        );
        write(d.path(), "extra.toml", "x = 1\n");
        assert_eq!(
            tree(d.path(), &[], &[], &[]),
            val("[config]\nimport = [\"extra.toml\"]")
        );
    }

    #[test]
    fn user_config_table_with_import_key_is_not_an_error() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "[config]\nimport = \"auto\"\nk = 1\n",
        );
        assert_eq!(
            tree(d.path(), &[], &[], &[]),
            val("[config]\nimport = \"auto\"\nk = 1")
        );
    }

    #[test]
    fn namespaced_profiles_active_in_base_file_selects_profiles() {
        let d = tempfile::tempdir().unwrap();
        profile_files(d.path());
        write(
            d.path(),
            "application.toml",
            "oxbloom.profiles.active = \"b, a\"\n",
        );
        let t = tree(d.path(), &[], &[], &[]);
        assert_eq!(t.get("v"), Some(&toml::Value::String("a".into())));
        write(
            d.path(),
            "application.toml",
            "[oxbloom.profiles]\nactive = [\"a\", \"b\"]\n",
        );
        let t = tree(d.path(), &[], &[], &[]);
        assert_eq!(t.get("v"), Some(&toml::Value::String("b".into())));
    }

    #[test]
    fn old_profiles_active_is_ordinary_data_and_does_not_select_profiles() {
        let d = tempfile::tempdir().unwrap();
        profile_files(d.path());
        write(d.path(), "application.toml", "profiles.active = \"a\"\n");
        assert_eq!(
            tree(d.path(), &[], &[], &[]),
            val("[profiles]\nactive = \"a\"")
        );
        // Also not an error inside a profile file.
        write(
            d.path(),
            "application-a.toml",
            "profiles.active = \"x\"\nv = 1\n",
        );
        let t = tree(d.path(), &["a"], &[], &[]);
        assert_eq!(t.get("v"), Some(&toml::Value::Integer(1)));
    }

    #[test]
    fn user_profiles_table_with_active_bool_loads() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "application.toml", "[profiles]\nactive = true\n");
        assert_eq!(
            tree(d.path(), &[], &[], &[]),
            val("[profiles]\nactive = true")
        );
    }

    #[test]
    fn namespaced_profiles_group_expands() {
        let d = tempfile::tempdir().unwrap();
        profile_files(d.path());
        write(d.path(), "application-both.toml", "w = 1\n");
        write(
            d.path(),
            "application.toml",
            "[oxbloom.profiles.group]\nboth = [\"a\", \"b\"]\n",
        );
        let t = tree(d.path(), &["both"], &[], &[]);
        assert_eq!(t.get("v"), Some(&toml::Value::String("b".into())));
        assert_eq!(t.get("w"), Some(&toml::Value::Integer(1)));
        // The old group spelling is plain data.
        write(
            d.path(),
            "application.toml",
            "[profiles.group]\nboth = [\"a\", \"b\"]\n",
        );
        let t = tree(d.path(), &["both"], &[], &[]);
        assert_eq!(t.get("v"), None);
        assert_eq!(t.get("w"), Some(&toml::Value::Integer(1)));
    }

    #[test]
    fn cli_oxbloom_profiles_active_selects_profiles() {
        let d = tempfile::tempdir().unwrap();
        profile_files(d.path());
        let args = ["--oxbloom.profiles.active=a,b"];
        assert_eq!(
            tree(d.path(), &[], &[], &args),
            val("v = \"b\"\n[oxbloom.profiles]\nactive = \"a,b\"")
        );
    }

    #[test]
    fn cli_old_profiles_active_is_ordinary_data() {
        let d = tempfile::tempdir().unwrap();
        profile_files(d.path());
        let args = ["--profiles.active=a,b"];
        assert_eq!(
            tree(d.path(), &[], &[], &args),
            val("[profiles]\nactive = \"a,b\"")
        );
    }

    #[test]
    fn namespaced_keys_match_relaxed_spellings() {
        let d = tempfile::tempdir().unwrap();
        profile_files(d.path());
        for text in [
            "OXBLOOM.Profiles.Active = \"b\"\n",
            "oxbloom.profiles-active = \"b\"\n",
            "[Oxbloom]\nPROFILES_ACTIVE = \"b\"\n",
        ] {
            // `profiles-active` is one segment, not `profiles.active`: plain data.
            write(d.path(), "application.toml", text);
            let t = tree(d.path(), &[], &[], &[]);
            let selected = t.get("v") == Some(&toml::Value::String("b".into()));
            assert_eq!(selected, text.starts_with("OXBLOOM"), "{text}");
        }
        write(
            d.path(),
            "application.toml",
            "[OxBloom.Profiles]\nACTIVE = \"a\"\n",
        );
        assert_eq!(
            tree(d.path(), &[], &[], &[]).get("v"),
            Some(&toml::Value::String("a".into()))
        );
        write(
            d.path(),
            "application.toml",
            "[Oxbloom.Config]\nIMPORT = [\"extra.toml\"]\n",
        );
        write(d.path(), "extra.toml", "x = 1\n");
        assert_eq!(
            tree(d.path(), &[], &[], &["--OXBLOOM.PROFILES.ACTIVE=b"]).get("v"),
            Some(&toml::Value::String("b".into()))
        );
        assert_eq!(
            tree(d.path(), &[], &[], &[]).get("x"),
            Some(&toml::Value::Integer(1))
        );
    }

    #[test]
    fn profiles_active_in_profile_file_is_invalid_with_namespaced_key() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application-dev.toml",
            "\noxbloom.profiles.active = \"x\"\n",
        );
        let e = run(d.path(), &["dev"], &[], &[]).unwrap_err();
        assert!(
            matches!(&e, ConfigError::Invalid { key, origin: Some(o), .. }
                if key == "oxbloom.profiles.active" && *o == file("application-dev.toml", 2)),
            "{e:?}"
        );
        assert!(e.to_string().contains("`oxbloom.profiles.active`"), "{e}");
    }

    #[test]
    fn group_errors_use_namespaced_keys() {
        let d = tempfile::tempdir().unwrap();
        write(
            d.path(),
            "application.toml",
            "[oxbloom.profiles.group]\na = [\"b\"]\nb = [\"a\"]\n",
        );
        let e = run(d.path(), &["a"], &[], &[]).unwrap_err();
        assert!(is_invalid_at(&e, "oxbloom.profiles.group.a"), "{e:?}");
        write(
            d.path(),
            "application.toml",
            "oxbloom.profiles.group.a = [1]\n",
        );
        let e = run(d.path(), &["a"], &[], &[]).unwrap_err();
        assert!(is_invalid_at(&e, "oxbloom.profiles.group.a"), "{e:?}");
        write(
            d.path(),
            "application.toml",
            &format!("oxbloom.profiles.active = \"{}\"\n", names_csv(65)),
        );
        let e = run(d.path(), &[], &[], &[]).unwrap_err();
        assert!(is_invalid_at(&e, "oxbloom.profiles.active"), "{e:?}");
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = \"extra.toml\"\n",
        );
        let e = run(d.path(), &[], &[], &[]).unwrap_err();
        assert!(is_invalid_at(&e, "oxbloom.config.import"), "{e:?}");
        assert!(e.to_string().contains("`oxbloom.config.import`"), "{e}");
    }

    fn names_csv(n: usize) -> String {
        (0..n)
            .map(|i| format!("p{i}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    #[test]
    fn oxbloom_table_removed_when_only_import_was_in_it() {
        let d = tempfile::tempdir().unwrap();
        write(d.path(), "extra.toml", "x = 1\n");
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"extra.toml\"]\n",
        );
        assert_eq!(tree(d.path(), &[], &[], &[]), val("x = 1"));
        // A non-empty `oxbloom` (or `oxbloom.config`) table is never removed.
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"extra.toml\"]\noxbloom.other = 2\n",
        );
        assert_eq!(
            tree(d.path(), &[], &[], &[]),
            val("x = 1\n[oxbloom]\nother = 2")
        );
    }

    #[test]
    fn oxbloom_profiles_stay_visible_to_get_and_explain() {
        let d = tempfile::tempdir().unwrap();
        profile_files(d.path());
        write(
            d.path(),
            "application.toml",
            "oxbloom.config.import = [\"extra.toml\"]\noxbloom.profiles.active = \"a\"\n\
             [oxbloom.profiles.group]\ng = [\"a\"]\n",
        );
        write(d.path(), "extra.toml", "x = 1\n");
        let c = Config::builder()
            .dir(d.path())
            .env_from([])
            .build()
            .unwrap();
        assert_eq!(c.get::<String>("oxbloom.profiles.active").unwrap(), "a");
        assert!(c.origin("oxbloom.profiles.active").is_some());
        let keys = c.keys("oxbloom");
        assert_eq!(
            keys,
            ["oxbloom.profiles.active", "oxbloom.profiles.group.g[0]"]
        );
        let explained: Vec<String> = c.explain().into_iter().map(|e| e.key).collect();
        assert!(explained.contains(&"oxbloom.profiles.active".to_string()));
        assert!(!explained.iter().any(|k| k.contains("config.import")));
    }
}
