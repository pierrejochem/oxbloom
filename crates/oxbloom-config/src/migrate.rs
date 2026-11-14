//! Renamed and removed key migration, run after the sources are merged and before placeholders.

use std::fmt;

use crate::error::ConfigError;
use crate::node::{Node, NodeValue, Table, canonical};
use crate::origin::Origin;

/// A migration finding: a configured key that was renamed or removed.
///
/// Carries the key, where it is set and what happened, never the value.
///
/// # Examples
///
/// ```
/// use oxbloom_config::{ConfigWarning, Origin, WarningKind};
///
/// let warning = ConfigWarning {
///     key: "old.key".into(),
///     origin: Origin::Inline { line: 3 },
///     kind: WarningKind::Renamed { to: "new.key".into() },
/// };
/// assert_eq!(warning.to_string(), "key `old.key` is renamed to `new.key` (from line 3)");
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigWarning {
    /// The old key, as registered.
    pub key: String,
    /// Where the old key is set.
    pub origin: Origin,
    /// What happened to the key.
    pub kind: WarningKind,
}

/// What a [`ConfigWarning`] reports.
///
/// # Examples
///
/// ```
/// use oxbloom_config::WarningKind;
///
/// let kind = WarningKind::Removed { reason: "use `new.key`".into() };
/// assert_eq!(kind, WarningKind::Removed { reason: "use `new.key`".into() });
/// ```
///
/// The enum is `#[non_exhaustive]`: a `match` outside this crate needs a wildcard arm.
///
/// ```compile_fail
/// use oxbloom_config::WarningKind;
///
/// fn is_rename(kind: &WarningKind) -> bool {
///     match kind {
///         WarningKind::Renamed { .. } => true,
///         WarningKind::Removed { .. } => false,
///     }
/// }
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum WarningKind {
    /// The key was renamed to `to`.
    Renamed {
        /// The new key.
        to: String,
    },
    /// The key was removed; `reason` is developer-supplied text.
    Removed {
        /// Why the key was removed.
        reason: String,
    },
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            WarningKind::Renamed { to } => write!(
                f,
                "key `{}` is renamed to `{to}` (from {})",
                self.key, self.origin
            ),
            WarningKind::Removed { reason } => write!(
                f,
                "key `{}` is removed: {reason} (from {})",
                self.key, self.origin
            ),
        }
    }
}

#[derive(Clone, Debug)]
enum Rule {
    Renamed { old: String, new: String },
    Removed { old: String, reason: String },
}

/// Registered migration rules, applied in registration order.
#[derive(Clone, Debug, Default)]
pub(crate) struct Registry {
    rules: Vec<Rule>,
}

impl Registry {
    pub(crate) fn renamed(&mut self, old: String, new: String) {
        self.rules.push(Rule::Renamed { old, new });
    }

    pub(crate) fn removed(&mut self, old: String, reason: String) {
        self.rules.push(Rule::Removed { old, reason });
    }
}

/// Fixed text: rule paths are developer input, but the message stays value-free.
const INVALID_RULE: &str =
    "rename rule is invalid: empty, self-referential or overlapping key paths";
const INVALID_REMOVED: &str = "removed rule is invalid: empty or malformed key path";

fn invalid(key: &str, message: &str) -> ConfigError {
    ConfigError::Invalid {
        key: key.to_string(),
        message: message.to_string(),
        origin: None,
    }
}

fn canonical_segments(path: &str) -> Vec<String> {
    segments(path).into_iter().map(canonical).collect()
}

/// Checks every rule before any data is touched.
fn validate(registry: &Registry) -> Result<(), ConfigError> {
    for rule in &registry.rules {
        match rule {
            Rule::Removed { old, .. } => {
                if segments(old).is_empty() {
                    return Err(invalid(old, INVALID_REMOVED));
                }
            }
            Rule::Renamed { old, new } => {
                let (o, n) = (canonical_segments(old), canonical_segments(new));
                // Equal or either a prefix of the other: `starts_with` both ways.
                if o.is_empty() || n.is_empty() || o.starts_with(&n) || n.starts_with(&o) {
                    return Err(invalid(old, INVALID_RULE));
                }
            }
        }
    }
    Ok(())
}

/// Applies the rules to `root`, returning the warnings in rule order.
///
/// Invalid rules fail first, before anything moves. A moved subtree that carries a command-line
/// value onto a `secret_keys` key (or below one) is rejected like the command line itself does.
pub(crate) fn migrate(
    root: &mut Node,
    registry: &Registry,
    secret_keys: &[String],
) -> Result<Vec<ConfigWarning>, ConfigError> {
    validate(registry)?;
    let secrets: Vec<Vec<String>> = secret_keys.iter().map(|k| canonical_segments(k)).collect();
    let mut warnings = Vec::new();
    for rule in &registry.rules {
        match rule {
            Rule::Removed { old, reason } => {
                if let Some(node) = find(root, &segments(old)) {
                    warnings.push(ConfigWarning {
                        key: old.clone(),
                        origin: node.origin.clone(),
                        kind: WarningKind::Removed {
                            reason: reason.clone(),
                        },
                    });
                }
            }
            Rule::Renamed { old, new } => {
                let old_path = segments(old);
                let Some(origin) = find(root, &old_path).map(|n| n.origin.clone()) else {
                    continue;
                };
                let new_path = segments(new);
                if find(root, &new_path).is_some() {
                    // The new key wins; drop the old node so it cannot bind by accident.
                    take(root, &old_path);
                    prune(root, &old_path);
                } else if creatable(root, &new_path)
                    && let Some(node) = take(root, &old_path)
                {
                    prune(root, &old_path);
                    let mut base = canonical_segments(new);
                    if let Some((rel, cli)) = secret_cli_leaf(&node, &mut base, "", &secrets) {
                        return Err(ConfigError::Invalid {
                            key: format!("{new}{rel}"),
                            message: "secret keys cannot be set on the command line; use an \
                                      environment variable or a configuration file"
                                .into(),
                            origin: Some(cli),
                        });
                    }
                    insert(root, &new_path, node);
                }
                warnings.push(ConfigWarning {
                    key: old.clone(),
                    origin,
                    kind: WarningKind::Renamed { to: new.clone() },
                });
            }
        }
    }
    Ok(warnings)
}

/// Finds a command-line leaf of the moved subtree whose final canonical path (`path`, the
/// new key's, plus the leaf's relative path) equals or lies below a secret key. Returns the
/// relative path in the source spelling (`.a`, `[0]`) and the leaf's origin. A leaf that only
/// reaches a strict ancestor of a secret key is fine.
fn secret_cli_leaf(
    node: &Node,
    path: &mut Vec<String>,
    rel: &str,
    secrets: &[Vec<String>],
) -> Option<(String, Origin)> {
    match &node.value {
        NodeValue::Table(t) => t.iter().find_map(|e| {
            path.push(canonical(&e.spelling));
            let found = secret_cli_leaf(&e.node, path, &format!("{rel}.{}", e.spelling), secrets);
            path.pop();
            found
        }),
        NodeValue::Array(items) => items.iter().enumerate().find_map(|(i, n)| {
            path.push(i.to_string());
            let found = secret_cli_leaf(n, path, &format!("{rel}[{i}]"), secrets);
            path.pop();
            found
        }),
        _ if matches!(node.origin, Origin::Cli { .. })
            && secrets.iter().any(|s| path.starts_with(s)) =>
        {
            Some((rel.to_string(), node.origin.clone()))
        }
        _ => None,
    }
}

/// Removes the tables a removal at `path` emptied, from the parent upwards. Stops at the first
/// non-empty ancestor and never removes the root.
fn prune(root: &mut Node, path: &[&str]) {
    for depth in (1..path.len()).rev() {
        let parent = &path[..depth];
        let empty = find(root, parent)
            .and_then(Node::as_table)
            .is_some_and(Table::is_empty);
        if !empty {
            return;
        }
        take(root, parent);
    }
}

/// Splits a dotted path. An empty path (or one with an empty segment) is malformed and gives
/// no segments.
fn segments(path: &str) -> Vec<&str> {
    if path.split('.').any(str::is_empty) {
        return Vec::new();
    }
    path.split('.').collect()
}

fn find<'a>(root: &'a Node, path: &[&str]) -> Option<&'a Node> {
    if path.is_empty() {
        return None;
    }
    let mut node = root;
    for segment in path {
        node = node.as_table()?.get(segment)?;
    }
    Some(node)
}

/// Whether every existing node along `path` is a table, so the missing rest can be created.
fn creatable(root: &Node, path: &[&str]) -> bool {
    if path.is_empty() {
        return false;
    }
    let mut node = root;
    for segment in path {
        match node.as_table().and_then(|t| t.get(segment)) {
            Some(next) => node = next,
            None => return node.as_table().is_some(),
        }
    }
    true
}

fn take(root: &mut Node, path: &[&str]) -> Option<Node> {
    let (last, parents) = path.split_last()?;
    let mut node = root;
    for segment in parents {
        node = node.as_table_mut()?.get_mut(segment)?;
    }
    node.as_table_mut()?.remove(last).map(|e| e.node)
}

/// Inserts `node` at `path`, creating missing tables (with the origin of `node`). The caller
/// has checked [`creatable`].
fn insert(root: &mut Node, path: &[&str], node: Node) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let origin = node.origin.clone();
    let mut current = root;
    for segment in parents {
        let table = current.as_table_mut().expect("path checked as creatable");
        if table.get(segment).is_none() {
            table.insert(*segment, Node::table(origin.clone(), Table::new()));
        }
        current = table.get_mut(segment).expect("just ensured");
    }
    if let NodeValue::Table(table) = &mut current.value {
        table.insert(*last, node);
    }
}

#[cfg(test)]
mod tests {
    use crate::{Config, ConfigError, ConfigWarning, Origin, WarningKind};
    use std::fs;

    fn build(
        toml: &str,
        f: impl FnOnce(crate::ConfigBuilder) -> crate::ConfigBuilder,
    ) -> Result<Config, ConfigError> {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("application.toml"), toml).unwrap();
        f(Config::builder().dir(d.path()).env_from([])).build()
    }

    fn file(line: usize) -> Origin {
        Origin::File {
            path: "application.toml".into(),
            line,
        }
    }

    fn renamed(key: &str, to: &str, origin: Origin) -> ConfigWarning {
        ConfigWarning {
            key: key.into(),
            origin,
            kind: WarningKind::Renamed { to: to.into() },
        }
    }

    #[test]
    fn renamed_key_moves_value_and_warns() {
        let c = build("[old]\nkey = 7", |b| b.renamed("old.key", "new.sub.key")).unwrap();
        assert_eq!(c.get::<u8>("new.sub.key").unwrap(), 7);
        assert_eq!(c.warnings(), [renamed("old.key", "new.sub.key", file(2))]);
        assert!(c.get::<u8>("old.key").is_err());
    }

    #[test]
    fn renamed_keeps_origin_of_old_key() {
        let c = build("\n\n[old]\nkey = \"x\"", |b| b.renamed("old.key", "new")).unwrap();
        let e = c.get::<u8>("new").unwrap_err();
        assert!(
            matches!(&e, ConfigError::Invalid { origin: Some(o), .. } if *o == file(4)),
            "{e:?}"
        );
        // A whole table moves, subtree included.
        let c = build("[old]\na = 1\nb = 2", |b| b.renamed("old", "new")).unwrap();
        assert_eq!(c.get::<u8>("new.b").unwrap(), 2);
        assert_eq!(c.warnings()[0].origin, file(1));
    }

    #[test]
    fn renamed_with_both_present_keeps_new_and_warns() {
        let c = build("old = 1\nnew = 2", |b| b.renamed("old", "new")).unwrap();
        assert_eq!(c.get::<u8>("new").unwrap(), 2);
        assert!(matches!(
            c.get::<u8>("old"),
            Err(ConfigError::Missing { .. })
        ));
        assert_eq!(c.warnings(), [renamed("old", "new", file(1))]);
    }

    #[test]
    fn renamed_blocked_by_scalar_keeps_old_and_warns() {
        let c = build("old = 1\nnew = 2", |b| b.renamed("old", "new.sub")).unwrap();
        assert_eq!(c.get::<u8>("old").unwrap(), 1);
        assert_eq!(c.get::<u8>("new").unwrap(), 2);
        assert_eq!(c.warnings().len(), 1);
    }

    #[test]
    fn rename_chain_applies_in_order() {
        let c = build("a = 1", |b| b.renamed("a", "b").renamed("b", "c")).unwrap();
        assert_eq!(c.get::<u8>("c").unwrap(), 1);
        assert_eq!(c.warnings().len(), 2);
        // Reverse registration order: b is not yet set when a->b... c->? no chain: stops at b.
        let c = build("a = 1", |b| b.renamed("b", "c").renamed("a", "b")).unwrap();
        assert_eq!(c.get::<u8>("b").unwrap(), 1);
        assert_eq!(c.warnings().len(), 1);
    }

    #[test]
    fn removed_key_warns_and_stays() {
        let c = build("[x]\ny = 1", |b| b.removed("x.y", "gone")).unwrap();
        assert_eq!(c.get::<u8>("x.y").unwrap(), 1);
        assert_eq!(
            c.warnings(),
            [ConfigWarning {
                key: "x.y".into(),
                origin: file(2),
                kind: WarningKind::Removed {
                    reason: "gone".into()
                },
            }]
        );
    }

    #[test]
    fn matching_is_canonical_and_nested() {
        let c = build("[Old-Key]\nSub_Key = 3", |b| {
            b.renamed("oldkey.subkey", "a.b-c")
        })
        .unwrap();
        assert_eq!(c.get::<u8>("a.bc").unwrap(), 3);
        assert_eq!(c.warnings().len(), 1);
        let c = build("[a.b]\nc = 1", |b| b.removed("A.B.C", "r")).unwrap();
        assert_eq!(c.warnings().len(), 1);
    }

    #[test]
    fn absent_keys_produce_nothing() {
        let c = build("x = 1", |b| b.renamed("p.q", "r").removed("s", "t")).unwrap();
        assert!(c.warnings().is_empty());
    }

    #[test]
    fn deny_deprecated_fails_with_first_warning() {
        let e = build("a = 1\nb = 2", |b| {
            b.removed("b", "r").renamed("a", "c").deny_deprecated(true)
        })
        .unwrap_err();
        assert!(
            matches!(&e, ConfigError::Deprecated { key, origin } if key == "b" && *origin == file(2)),
            "{e:?}"
        );
        // No warnings: no error.
        assert!(build("z = 1", |b| b.removed("b", "r").deny_deprecated(true)).is_ok());
    }

    #[test]
    fn warnings_in_discovery_order() {
        let c = build("a = 1\nb = 2\nc = 3", |b| {
            b.removed("c", "r").renamed("a", "x").removed("b", "r")
        })
        .unwrap();
        let keys: Vec<&str> = c.warnings().iter().map(|w| w.key.as_str()).collect();
        assert_eq!(keys, ["c", "a", "b"]);
    }

    #[test]
    fn warning_display_has_no_value() {
        let c = build("[old]\nkey = \"hunter2\"\nother = \"hunter2\"", |b| {
            b.renamed("old.key", "new.key")
                .removed("old.other", "use vault")
        })
        .unwrap();
        let shown: Vec<String> = c.warnings().iter().map(|w| w.to_string()).collect();
        assert_eq!(
            shown[0],
            "key `old.key` is renamed to `new.key` (from application.toml:2)"
        );
        assert_eq!(
            shown[1],
            "key `old.other` is removed: use vault (from application.toml:3)"
        );
        assert!(!format!("{:?}{c:?}", c.warnings()).contains("hunter2"));
    }

    #[test]
    fn no_registry_no_warnings() {
        let c = build("a = 1", |b| b).unwrap();
        assert!(c.warnings().is_empty());
        assert!(Config::from_str("a = 1").unwrap().warnings().is_empty());
    }

    #[test]
    fn migration_runs_before_placeholders() {
        let c = build("old = \"v\"\nurl = \"${new}/x\"", |b| {
            b.renamed("old", "new")
        })
        .unwrap();
        assert_eq!(c.get::<String>("url").unwrap(), "v/x");
    }

    fn invalid_key(r: Result<Config, ConfigError>) -> String {
        match r {
            Err(ConfigError::Invalid {
                key, origin: None, ..
            }) => key,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rename_to_itself_is_invalid() {
        let r = build("a = 1\nz = 2", |b| b.renamed("a", "a"));
        assert_eq!(invalid_key(r), "a");
    }

    #[test]
    fn rename_canonical_equal_is_invalid() {
        for (o, n) in [("a", "A"), ("a-b", "a_b"), ("x.a-b", "X.AB")] {
            let r = build("a = 1\nab = 2", |b| b.renamed(o, n));
            assert_eq!(invalid_key(r), o);
        }
    }

    #[test]
    fn rename_into_own_subtree_is_invalid() {
        let r = build("a = 1", |b| b.renamed("a", "a.b"));
        assert_eq!(invalid_key(r), "a");
    }

    #[test]
    fn rename_child_over_parent_is_invalid() {
        let r = build("[a]\nb = 1\nc = 2", |b| b.renamed("a.b", "a"));
        assert_eq!(invalid_key(r), "a.b");
    }

    #[test]
    fn rename_with_empty_or_malformed_path_is_invalid() {
        for bad in ["", "a..b", ".a", "a."] {
            let r = build("a = 1", |b| b.renamed(bad, "x"));
            assert_eq!(invalid_key(r), bad);
            let r = build("a = 1", |b| b.renamed("a", bad));
            assert_eq!(invalid_key(r), "a");
        }
    }

    #[test]
    fn removed_with_malformed_path_is_invalid() {
        for bad in ["", "a..b", ".a", "a."] {
            let r = build("a = 1", |b| b.removed(bad, "r"));
            assert_eq!(invalid_key(r), bad);
        }
    }

    #[test]
    fn valid_rules_still_work_after_validation() {
        let c = build("[a]\nb = 1", |b| b.renamed("a.b", "c.d").removed("zz", "r")).unwrap();
        assert_eq!(c.get::<u8>("c.d").unwrap(), 1);
        assert_eq!(c.warnings().len(), 1);
    }

    #[test]
    fn emptied_parent_is_pruned_and_optional_struct_is_none() {
        #[derive(serde::Deserialize)]
        struct P {
            #[allow(dead_code)]
            only: u8,
        }
        #[derive(serde::Deserialize)]
        struct Top {
            p: Option<P>,
        }
        let c = build("[p]\nonly = 1", |b| b.renamed("p.only", "q.moved")).unwrap();
        assert_eq!(c.get::<u8>("q.moved").unwrap(), 1);
        assert!(c.get::<Top>("").unwrap().p.is_none());
        // Both-set case prunes too.
        let c = build("[p]\nonly = 1\n[q]\nmoved = 2", |b| {
            b.renamed("p.only", "q.moved")
        })
        .unwrap();
        assert!(c.get::<Top>("").unwrap().p.is_none());
        // Pruning climbs through several emptied ancestors.
        let c = build("[a.b]\nc = 1", |b| b.renamed("a.b.c", "z")).unwrap();
        assert!(matches!(
            c.get::<toml::Value>("a"),
            Err(ConfigError::Missing { .. })
        ));
        // A table that was empty before is not pruned.
        let c = build("[e]\n[p]\nonly = 1", |b| b.renamed("p.only", "q")).unwrap();
        assert!(c.get::<toml::Value>("e").is_ok());
    }

    #[test]
    fn non_empty_parent_is_kept() {
        let c = build("[p]\nonly = 1\nother = 2", |b| b.renamed("p.only", "q")).unwrap();
        assert_eq!(c.get::<u8>("p.other").unwrap(), 2);
        let c = build("[a.b]\nc = 1\n[a]\nd = 2", |b| b.renamed("a.b.c", "z")).unwrap();
        assert_eq!(c.get::<u8>("a.d").unwrap(), 2);
        assert!(c.get::<toml::Value>("a.b").is_err());
    }

    #[test]
    fn rename_onto_secret_key_rejects_cli_value_without_echoing_it() {
        let d = tempfile::tempdir().unwrap();
        let e = Config::builder()
            .dir(d.path())
            .env_from([])
            .secret_key("Pw")
            .renamed("old", "pw")
            .args(["--old=hunter2"])
            .build()
            .unwrap_err();
        match &e {
            ConfigError::Invalid { key, origin, .. } => {
                assert_eq!(key, "pw");
                assert_eq!(
                    *origin,
                    Some(Origin::Cli {
                        arg: "--old".into()
                    })
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(!format!("{e} {e:?}").contains("hunter2"));
        // A CLI leaf below a renamed table, onto a key below the secret.
        let e = Config::builder()
            .dir(d.path())
            .env_from([])
            .secret_key("sec")
            .renamed("old", "sec.inner")
            .args(["--old.x=hunter2"])
            .build()
            .unwrap_err();
        assert!(matches!(e, ConfigError::Invalid { .. }));
    }

    #[test]
    fn rename_onto_secret_key_from_file_or_env_is_fine() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("application.toml"), "old = \"a\"").unwrap();
        let c = Config::builder()
            .dir(d.path())
            .env_from([])
            .secret_key("pw")
            .renamed("old", "pw")
            .build()
            .unwrap();
        assert_eq!(c.get::<String>("pw").unwrap(), "a");
        let c = Config::builder()
            .dir(d.path())
            .env_from([("APP_OTHER".to_string(), "b".to_string())])
            .secret_key("pw")
            .renamed("other", "pw")
            .build()
            .unwrap();
        assert_eq!(c.get::<String>("pw").unwrap(), "b");
    }

    #[test]
    fn invalid_rule_leaves_no_partial_migration() {
        let r = build("a = 1", |b| b.renamed("a", "b").renamed("c", "c"));
        assert_eq!(invalid_key(r), "c");
    }

    #[test]
    fn placeholder_on_old_key_after_rename_is_unresolved() {
        let e = build("old = \"v\"\nurl = \"${old}/x\"", |b| {
            b.renamed("old", "new")
        })
        .unwrap_err();
        assert!(
            matches!(e, ConfigError::UnresolvedPlaceholder { .. }),
            "{e:?}"
        );
    }

    fn cli_build(
        secret: &str,
        old: &str,
        new: &str,
        args: &[&str],
        toml: &str,
    ) -> Result<Config, ConfigError> {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("application.toml"), toml).unwrap();
        Config::builder()
            .dir(d.path())
            .env_from([])
            .secret_key(secret)
            .renamed(old, new)
            .args(args.iter().map(|a| a.to_string()))
            .build()
    }

    #[test]
    fn rename_onto_ancestor_of_secret_key_rejects_cli_leaf_at_the_secret_path() {
        let e = cli_build("sec.k", "old", "sec", &["--old.k=hunter2"], "").unwrap_err();
        match &e {
            ConfigError::Invalid { key, origin, .. } => {
                assert_eq!(key, "sec.k");
                assert_eq!(
                    *origin,
                    Some(Origin::Cli {
                        arg: "--old.k".into()
                    })
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(!format!("{e} {e:?}").contains("hunter2"));
    }

    #[test]
    fn rename_onto_ancestor_of_deep_secret_rejects_cli_leaf() {
        let e = cli_build("sec.k.deep", "old", "sec", &["--old.k.deep=hunter2"], "").unwrap_err();
        assert!(
            matches!(&e, ConfigError::Invalid { key, .. } if key == "sec.k.deep"),
            "{e:?}"
        );
        assert!(!format!("{e} {e:?}").contains("hunter2"));
    }

    #[test]
    fn rename_onto_ancestor_allows_cli_leaf_that_does_not_reach_the_secret() {
        let c = cli_build("sec.k", "old", "sec", &["--old.other=1"], "").unwrap();
        assert_eq!(c.get::<u8>("sec.other").unwrap(), 1);
        // A scalar landing on a strict ancestor of the secret is fine too.
        let c = cli_build("sec.k", "old", "sec", &["--old=1"], "").unwrap();
        assert_eq!(c.get::<u8>("sec").unwrap(), 1);
    }

    #[test]
    fn rename_subtree_with_one_cli_leaf_reaching_the_secret_is_rejected_naming_that_leaf() {
        let e = cli_build(
            "sec.k",
            "old",
            "Sec",
            &["--old.a=1", "--old.K=hunter2", "--old.z=2"],
            "",
        )
        .unwrap_err();
        match &e {
            ConfigError::Invalid { key, origin, .. } => {
                assert_eq!(key, "Sec.K");
                assert_eq!(
                    *origin,
                    Some(Origin::Cli {
                        arg: "--old.K".into()
                    })
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(!format!("{e} {e:?}").contains("hunter2"));
    }

    #[test]
    fn rename_onto_ancestor_of_secret_with_file_leaf_at_the_secret_path_is_fine() {
        let c = cli_build("sec.k", "old", "sec", &[], "[old]\nk = \"v\"").unwrap();
        assert_eq!(c.get::<String>("sec.k").unwrap(), "v");
    }
}
