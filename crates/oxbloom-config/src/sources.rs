use std::collections::BTreeMap;

use crate::error::ConfigError;
use crate::node::{Node, NodeValue, Table, canonical};
use crate::origin::Origin;

/// One step of a source key: a table key or a list index.
enum Seg {
    Key(String),
    Index(usize),
}

/// A source being assembled. Lists are sparse until the source is complete, so indices can
/// arrive in any order and gaps are found at the end.
enum Tree {
    Leaf(Node),
    Map {
        origin: Origin,
        entries: BTreeMap<String, (String, Tree)>,
    },
    List {
        origin: Origin,
        items: BTreeMap<usize, Tree>,
    },
}

impl Tree {
    fn empty_map(origin: &Origin) -> Tree {
        Tree::Map {
            origin: origin.clone(),
            entries: BTreeMap::new(),
        }
    }

    /// Sets `leaf` at `path`. A leaf replaces whatever is at its path, and a path through
    /// something of another kind replaces it, as the last write wins.
    ///
    /// Using an index and a table key directly under one path in the same source is a conflict:
    /// `Err(n)` carries the number of leading segments that name the conflicting path.
    fn set(&mut self, full: &[Seg], at: usize, leaf: Node) -> Result<(), usize> {
        let Some(seg) = full.get(at) else {
            *self = Tree::Leaf(leaf);
            return Ok(());
        };
        match seg {
            Seg::Key(key) => {
                if matches!(self, Tree::List { items, .. } if !items.is_empty()) {
                    return Err(at);
                }
                if !matches!(self, Tree::Map { .. }) {
                    *self = Tree::empty_map(&leaf.origin);
                }
                if let Tree::Map { origin, entries } = self {
                    *origin = leaf.origin.clone();
                    let slot = entries
                        .entry(canonical(key))
                        .or_insert_with(|| (key.clone(), Tree::empty_map(&leaf.origin)));
                    slot.0 = key.clone();
                    return slot.1.set(full, at + 1, leaf);
                }
            }
            Seg::Index(index) => {
                if matches!(self, Tree::Map { entries, .. } if !entries.is_empty()) {
                    return Err(at);
                }
                if !matches!(self, Tree::List { .. }) {
                    *self = Tree::List {
                        origin: leaf.origin.clone(),
                        items: BTreeMap::new(),
                    };
                }
                if let Tree::List { origin, items } = self {
                    *origin = leaf.origin.clone();
                    return items
                        .entry(*index)
                        .or_insert_with(|| Tree::empty_map(&leaf.origin))
                        .set(full, at + 1, leaf);
                }
            }
        }
        Ok(())
    }

    /// Converts to a `Node`, checking that every list starts at 0 without gaps. `path` is the
    /// key of `self`, for errors.
    fn into_node(self, path: &str) -> Result<Node, ConfigError> {
        match self {
            Tree::Leaf(node) => Ok(node),
            Tree::Map { origin, entries } => {
                let mut table = Table::new();
                for (_, (spelling, child)) in entries {
                    let child_path = if path.is_empty() {
                        spelling.clone()
                    } else {
                        format!("{path}.{spelling}")
                    };
                    table.insert(spelling, child.into_node(&child_path)?);
                }
                Ok(Node::table(origin, table))
            }
            Tree::List { origin, items } => {
                let mut nodes = Vec::with_capacity(items.len());
                for (expected, (index, child)) in items.into_iter().enumerate() {
                    if index != expected {
                        return Err(ConfigError::Invalid {
                            key: path.to_string(),
                            message: "list indices must start at 0 and have no gaps".into(),
                            origin: Some(origin),
                        });
                    }
                    nodes.push(child.into_node(&format!("{path}[{index}]"))?);
                }
                Ok(Node {
                    origin,
                    value: NodeValue::Array(nodes),
                })
            }
        }
    }
}

/// Builds the node for the entries, then merges it over `root`. Merging replaces arrays whole,
/// so a list from this source never mixes with a lower-precedence one.
fn finish(root: &mut Node, tree: Tree) -> Result<(), ConfigError> {
    let node = tree.into_node("")?;
    root.merge(node);
    Ok(())
}

/// A decimal index segment: `None` when not all digits, `Some(Err)` when it does not fit.
fn parse_index(text: &str) -> Option<Result<usize, ()>> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(text.parse::<usize>().map_err(|_| ()))
}

/// Longest key path (counting index segments) accepted from the environment or command line.
const MAX_SEGMENTS: usize = 64;

/// At most 128 characters of `text`, then `…`, so errors about huge keys stay small.
fn clip(text: &str) -> String {
    if text.chars().count() <= 128 {
        return text.to_string();
    }
    let mut out: String = text.chars().take(128).collect();
    out.push('…');
    out
}

fn too_deep(key: &str, origin: Origin) -> ConfigError {
    ConfigError::Invalid {
        key: clip(key),
        message: format!("key has more than {MAX_SEGMENTS} segments"),
        origin: Some(origin),
    }
}

fn conflict(path: &[Seg], origin: Origin) -> ConfigError {
    ConfigError::Invalid {
        key: clip(&path_text(path)),
        message: "a list index and a table key are mixed under the same path".into(),
        origin: Some(origin),
    }
}

fn path_text(segs: &[Seg]) -> String {
    let mut out = String::new();
    for seg in segs {
        match seg {
            Seg::Key(k) if out.is_empty() => out.push_str(k),
            Seg::Key(k) => {
                out.push('.');
                out.push_str(k);
            }
            Seg::Index(i) => out.push_str(&format!("[{i}]")),
        }
    }
    out
}

/// Applies `{prefix}_{PATH}` variables as string nodes with origin [`Origin::Env`]. `__`
/// separates path segments, segments are lowercased, and a key replaces an existing one with the
/// same canonical form (taking the variable's spelling). A numeric segment after the first is a
/// list index: indices must start at 0 and have no gap, and the list replaces a lower-precedence
/// one whole. Variables with another prefix, an empty path or an empty segment are ignored.
pub(crate) fn apply_env(
    root: &mut Node,
    prefix: &str,
    vars: impl IntoIterator<Item = (String, String)>,
) -> Result<(), ConfigError> {
    let lead = format!("{prefix}_");
    // Process order must not depend on the platform's variable order.
    let mut vars: Vec<_> = vars.into_iter().collect();
    vars.sort();
    let mut tree = Tree::empty_map(&Origin::Env { name: lead.clone() });
    let mut any = false;
    for (name, value) in vars {
        let Some(rest) = name.strip_prefix(&lead) else {
            continue;
        };
        let parts: Vec<String> = rest.split("__").map(str::to_lowercase).collect();
        if parts.iter().any(String::is_empty) {
            continue;
        }
        let origin = Origin::Env { name: name.clone() };
        if parts.len() > MAX_SEGMENTS {
            let short = Origin::Env { name: clip(&name) };
            return Err(too_deep(&parts.join("."), short));
        }
        let mut path: Vec<Seg> = Vec::with_capacity(parts.len());
        for part in parts {
            match (path.is_empty(), parse_index(&part)) {
                (false, Some(Ok(i))) => path.push(Seg::Index(i)),
                (false, Some(Err(()))) => {
                    return Err(ConfigError::Invalid {
                        key: path_text(&path),
                        message: "list index is too large".into(),
                        origin: Some(origin),
                    });
                }
                _ => path.push(Seg::Key(part)),
            }
        }
        let leaf = Node {
            origin: origin.clone(),
            value: NodeValue::String(value),
        };
        tree.set(&path, 0, leaf)
            .map_err(|n| conflict(&path[..n], origin))?;
        any = true;
    }
    if any { finish(root, tree) } else { Ok(()) }
}

/// Splits a command-line key into segments: `.` separates, `[n]` or `.n` is an index. `None`
/// when the key has an empty segment or is empty.
fn parse_cli_key(key: &str) -> Option<Vec<Seg>> {
    let mut path = Vec::new();
    for part in key.split('.') {
        if part.is_empty() {
            return None;
        }
        // `name[0][1]`: the name, then any number of bracketed indices.
        let (name, mut brackets) = match part.find('[') {
            Some(at) => (&part[..at], &part[at..]),
            None => (part, ""),
        };
        let mut indices = Vec::new();
        while let Some(inner) = brackets.strip_prefix('[') {
            let Some(end) = inner.find(']') else { break };
            match parse_index(&inner[..end]) {
                Some(Ok(i)) => indices.push(i),
                _ => break,
            }
            brackets = &inner[end + 1..];
        }
        if !brackets.is_empty() {
            // Not a well-formed index suffix: the whole part is a literal key.
            path.push(Seg::Key(part.to_string()));
            continue;
        }
        match (path.is_empty(), name.is_empty(), parse_index(name)) {
            (_, true, _) => return None,
            (false, false, Some(Ok(i))) if indices.is_empty() => path.push(Seg::Index(i)),
            _ => path.push(Seg::Key(name.to_string())),
        }
        path.extend(indices.into_iter().map(Seg::Index));
    }
    Some(path)
}

/// Canonical dotted form of a path, indices as `n`, for matching secret keys.
fn canonical_path(path: &[Seg]) -> String {
    path.iter()
        .map(|s| match s {
            Seg::Key(k) => canonical(k),
            Seg::Index(i) => i.to_string(),
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Applies `--key=value` arguments as string nodes with origin [`Origin::Cli`] (the key part
/// only). Other arguments, and an empty key, are ignored; a repeated key keeps the last value.
/// A key matching `secret_keys` (canonical match, or below it) is an `Invalid` error that
/// points to environment variables or files.
pub(crate) fn apply_args(
    root: &mut Node,
    args: impl IntoIterator<Item = String>,
    secret_keys: &[String],
) -> Result<(), ConfigError> {
    let secrets: Vec<String> = secret_keys
        .iter()
        .map(|k| k.split('.').map(canonical).collect::<Vec<_>>().join("."))
        .collect();
    let mut tree = Tree::empty_map(&Origin::Cli { arg: String::new() });
    let mut any = false;
    for arg in args {
        let Some(body) = arg.strip_prefix("--") else {
            continue;
        };
        let Some((key, value)) = body.split_once('=') else {
            continue;
        };
        let Some(path) = parse_cli_key(key) else {
            continue;
        };
        let origin = Origin::Cli {
            arg: format!("--{}", clip(key)),
        };
        if path.len() > MAX_SEGMENTS {
            return Err(too_deep(key, origin));
        }
        let canon = canonical_path(&path);
        let secret = secrets.iter().any(|s| {
            canon == *s
                || canon
                    .strip_prefix(s.as_str())
                    .is_some_and(|rest| rest.starts_with('.'))
        });
        if secret {
            return Err(ConfigError::Invalid {
                key: clip(key),
                message: "secret keys cannot be set on the command line; use an environment \
                          variable or a configuration file"
                    .into(),
                origin: Some(origin),
            });
        }
        let leaf = Node {
            origin: origin.clone(),
            value: NodeValue::String(value.to_string()),
        };
        tree.set(&path, 0, leaf)
            .map_err(|n| conflict(&path[..n], origin))?;
        any = true;
    }
    if any { finish(root, tree) } else { Ok(()) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn v(s: &str) -> Node {
        Node::parse_toml(s, |line| Origin::Inline { line }).unwrap()
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn applied(base: &str, vars: &[(&str, &str)]) -> toml::Value {
        let mut root = v(base);
        apply_env(&mut root, "APP", env(vars)).unwrap();
        root.to_toml_value()
    }

    fn t(s: &str) -> toml::Value {
        toml::from_str(s).unwrap()
    }

    #[test]
    fn tables_merge_deeply() {
        let mut base = v("[a]\nx = 1\ny = 2\n[a.b]\nk = 1");
        base.merge(v("[a]\ny = 3\n[a.b]\nz = 9"));
        assert_eq!(
            base.to_toml_value(),
            t("[a]\nx = 1\ny = 3\n[a.b]\nk = 1\nz = 9")
        );
    }

    #[test]
    fn scalars_replace_whole() {
        let mut base = v("a = 1");
        base.merge(v("a = \"two\""));
        assert_eq!(base.to_toml_value(), t("a = \"two\""));
    }

    #[test]
    fn arrays_replace_whole() {
        let mut base = v("a = [1, 2, 3]");
        base.merge(v("a = [9]"));
        assert_eq!(base.to_toml_value(), t("a = [9]"));
    }

    #[test]
    fn env_maps_double_underscore_to_path() {
        assert_eq!(
            applied("", &[("APP_SERVER__PORT", "9000")]),
            t("[server]\nport = \"9000\"")
        );
    }

    #[test]
    fn env_single_underscore_stays_in_key() {
        assert_eq!(
            applied("", &[("APP_DB__MAX_CONNECTIONS", "5")]),
            t("[db]\nmax_connections = \"5\"")
        );
    }

    #[test]
    fn env_without_prefix_ignored() {
        assert_eq!(applied("", &[("OTHER_A", "1"), ("APPX_A", "1")]), t(""));
    }

    #[test]
    fn env_with_empty_path_ignored() {
        assert_eq!(applied("", &[("APP_", "1")]), t(""));
    }

    #[test]
    fn env_with_empty_segment_ignored() {
        assert_eq!(applied("", &[("APP_A____B", "1"), ("APP___A", "1")]), t(""));
    }

    #[test]
    fn env_lowercases_keys() {
        assert_eq!(
            applied("", &[("APP_Server__HOST", "h")]),
            t("[server]\nhost = \"h\"")
        );
    }

    #[test]
    fn env_replaces_existing_scalar_and_through_scalar() {
        assert_eq!(
            applied("a = 1\nb = 2", &[("APP_A", "x"), ("APP_B__C", "y")]),
            t("a = \"x\"\n[b]\nc = \"y\"")
        );
    }

    #[test]
    fn env_application_is_order_independent() {
        let vars = [("APP_S", "x"), ("APP_S__PORT", "2"), ("APP_S__HOST", "h")];
        let reversed: Vec<_> = vars.iter().rev().copied().collect();
        assert_eq!(
            applied("[s]\nport = 1", &vars),
            applied("[s]\nport = 1", &reversed)
        );
    }

    #[test]
    fn env_replaces_other_spelling_and_records_origin() {
        let mut root = v("[server]\nmax-connections = 1\nhost = \"h\"");
        apply_env(
            &mut root,
            "APP",
            env(&[("APP_SERVER__MAX_CONNECTIONS", "5")]),
        )
        .unwrap();
        assert_eq!(
            root.to_toml_value(),
            t("[server]\nmax_connections = \"5\"\nhost = \"h\"")
        );
        let server = root.as_table().unwrap().get("server").unwrap();
        let entry = server
            .as_table()
            .unwrap()
            .get_entry("maxconnections")
            .unwrap();
        assert_eq!(
            entry.node.origin,
            Origin::Env {
                name: "APP_SERVER__MAX_CONNECTIONS".into()
            }
        );
        let host = server.as_table().unwrap().get("host").unwrap();
        assert_eq!(host.origin, Origin::Inline { line: 3 });
    }

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    fn cli(base: &str, list: &[&str]) -> toml::Value {
        let mut root = v(base);
        apply_args(&mut root, args(list), &[]).unwrap();
        root.to_toml_value()
    }

    fn lookup<'a>(root: &'a Node, path: &[&str]) -> &'a Node {
        let mut node = root;
        for key in path {
            node = node.as_table().unwrap().get(key).unwrap();
        }
        node
    }

    #[test]
    fn env_maps_double_underscore_and_keeps_single_underscore() {
        assert_eq!(
            applied(
                "",
                &[("APP_DB__MAX_CONNECTIONS", "5"), ("APP_SERVER__PORT", "9")]
            ),
            t("[db]\nmax_connections = \"5\"\n[server]\nport = \"9\"")
        );
    }

    #[test]
    fn env_indexed_list_builds_array() {
        assert_eq!(
            applied(
                "",
                &[
                    ("APP_HOSTS__1", "b"),
                    ("APP_HOSTS__0", "a"),
                    ("APP_HOSTS__2", "c")
                ]
            ),
            t("hosts = [\"a\", \"b\", \"c\"]")
        );
    }

    #[test]
    fn env_list_gap_is_invalid_naming_key() {
        let mut root = v("");
        let err = apply_env(
            &mut root,
            "APP",
            env(&[("APP_HOSTS__0", "secretvalue"), ("APP_HOSTS__2", "other")]),
        )
        .unwrap_err();
        match &err {
            ConfigError::Invalid { key, origin, .. } => {
                assert_eq!(key, "hosts");
                assert!(matches!(origin, Some(Origin::Env { .. })));
            }
            other => panic!("{other:?}"),
        }
        let text = format!("{err} {err:?}");
        assert!(
            !text.contains("secretvalue") && !text.contains("other"),
            "{text}"
        );
        let mut root = v("");
        let err = apply_env(&mut root, "APP", env(&[("APP_HOSTS__1", "x")])).unwrap_err();
        assert!(matches!(err, ConfigError::Invalid { .. }));
    }

    #[test]
    fn env_indexed_list_replaces_file_list_whole() {
        assert_eq!(
            applied("hosts = [\"a\", \"b\", \"c\"]", &[("APP_HOSTS__0", "x")]),
            t("hosts = [\"x\"]")
        );
    }

    #[test]
    fn env_list_of_structs_via_indices() {
        #[derive(serde::Deserialize, Debug, PartialEq)]
        struct Server {
            host: String,
            port: u16,
        }
        let config = Config::builder()
            .dir(tempfile::tempdir().unwrap().path())
            .env_from(env(&[
                ("APP_SERVERS__0__HOST", "a"),
                ("APP_SERVERS__0__PORT", "80"),
                ("APP_SERVERS__1__HOST", "b"),
                ("APP_SERVERS__1__PORT", "81"),
            ]))
            .build()
            .unwrap();
        let servers: Vec<Server> = config.get("servers").unwrap();
        assert_eq!(
            servers,
            vec![
                Server {
                    host: "a".into(),
                    port: 80
                },
                Server {
                    host: "b".into(),
                    port: 81
                }
            ]
        );
    }

    #[test]
    fn env_unknown_prefix_and_empty_segments_ignored() {
        assert_eq!(
            applied(
                "",
                &[
                    ("OTHER_A", "1"),
                    ("APP_", "1"),
                    ("APP_A____B", "1"),
                    ("APP___A", "1")
                ]
            ),
            t("")
        );
    }

    #[test]
    fn cli_key_value_sets_node_with_cli_origin() {
        let mut root = v("");
        apply_args(&mut root, args(&["--server.port=9000"]), &[]).unwrap();
        assert_eq!(root.to_toml_value(), t("[server]\nport = \"9000\""));
        assert_eq!(
            lookup(&root, &["server", "port"]).origin,
            Origin::Cli {
                arg: "--server.port".into()
            }
        );
    }

    #[test]
    fn cli_index_bracket_and_dot_forms() {
        assert_eq!(
            cli(
                "",
                &["--hosts[1]=b", "--hosts[0]=a", "--ports.0=1", "--ports.1=2"]
            ),
            t("hosts = [\"a\", \"b\"]\nports = [\"1\", \"2\"]")
        );
        assert_eq!(
            cli(
                "",
                &[
                    "--servers[0].host=a",
                    "--servers.0.port=1",
                    "--servers[1].host=b"
                ]
            ),
            t("[[servers]]\nhost = \"a\"\nport = \"1\"\n[[servers]]\nhost = \"b\"")
        );
        assert_eq!(
            cli("hosts = [\"z\", \"y\"]", &["--hosts[0]=a"]),
            t("hosts = [\"a\"]")
        );
        let mut root = v("");
        let err = apply_args(&mut root, args(&["--hosts[1]=b"]), &[]).unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Invalid {
                origin: Some(Origin::Cli { .. }),
                ..
            }
        ));
    }

    #[test]
    fn cli_beats_env_beats_file() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("application.toml"),
            "a = \"file\"\nb = \"file\"\nc = \"file\"",
        )
        .unwrap();
        let config = Config::builder()
            .dir(dir.path())
            .env_from(env(&[("APP_B", "env"), ("APP_C", "env")]))
            .args(["--c=cli"])
            .build()
            .unwrap();
        assert_eq!(config.get::<String>("a").unwrap(), "file");
        assert_eq!(config.get::<String>("b").unwrap(), "env");
        assert_eq!(config.get::<String>("c").unwrap(), "cli");
    }

    #[test]
    fn cli_key_without_equals_is_ignored() {
        assert_eq!(cli("a = 1", &["--verbose", "--a.b"]), t("a = 1"));
    }

    #[test]
    fn cli_empty_key_ignored() {
        assert_eq!(
            cli("a = 1", &["--=x", "--.=x", "--a..b=1", "--[0]=1"]),
            t("a = 1")
        );
    }

    #[test]
    fn cli_empty_value_is_empty_string() {
        assert_eq!(cli("", &["--k="]), t("k = \"\""));
    }

    #[test]
    fn cli_repeated_key_last_wins() {
        assert_eq!(cli("", &["--k=a", "--k=b"]), t("k = \"b\""));
    }

    #[test]
    fn cli_rejects_registered_secret_key_without_echoing_value() {
        let mut root = v("");
        let err = apply_args(
            &mut root,
            args(&["--Security.JWT_secret=hunter2"]),
            &["security.jwt-secret".to_string()],
        )
        .unwrap_err();
        assert!(matches!(&err, ConfigError::Invalid { key, .. } if key == "Security.JWT_secret"));
        let text = format!("{err} {err:?}");
        assert!(!text.contains("hunter2"), "{text}");
        assert!(text.contains("environment"), "{text}");
    }

    #[test]
    fn cli_non_double_dash_args_ignored() {
        assert_eq!(cli("a = 1", &["a=2", "-a=3", "/a=4", "", "-"]), t("a = 1"));
    }

    /// Runs `f` on a thread with a small stack, so unbounded recursion fails the test.
    fn small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap()
    }

    fn assert_invalid_short(err: &ConfigError, origin_is: impl Fn(&Origin) -> bool) {
        match err {
            ConfigError::Invalid { key, origin, .. } => {
                assert!(key.chars().count() <= 129, "key too long: {}", key.len());
                assert!(origin_is(origin.as_ref().unwrap()));
            }
            other => panic!("{other:?}"),
        }
        let text = format!("{err} {err:?}");
        assert!(text.len() < 1000, "error too large: {}", text.len());
        assert!(!text.contains("topsecret"), "{text}");
    }

    #[test]
    fn env_path_deeper_than_64_segments_is_invalid() {
        let err = small_stack(|| {
            let name = format!("APP_A{}", "__0".repeat(10_000));
            let mut root = v("");
            apply_env(&mut root, "APP", env(&[(&name, "topsecret")])).unwrap_err()
        });
        assert_invalid_short(&err, |o| matches!(o, Origin::Env { .. }));
        let err = small_stack(|| {
            let name = format!("APP_A{}", "__B".repeat(10_000));
            let mut root = v("");
            apply_env(&mut root, "APP", env(&[(&name, "topsecret")])).unwrap_err()
        });
        assert_invalid_short(&err, |o| matches!(o, Origin::Env { .. }));
    }

    #[test]
    fn cli_path_deeper_than_64_segments_is_invalid() {
        let err = small_stack(|| {
            let arg = format!("--a{}=topsecret", ".b".repeat(10_000));
            let mut root = v("");
            apply_args(&mut root, [arg], &[]).unwrap_err()
        });
        assert_invalid_short(&err, |o| matches!(o, Origin::Cli { .. }));
    }

    #[test]
    fn cli_bracket_index_nesting_deeper_than_64_is_invalid() {
        let err = small_stack(|| {
            let arg = format!("--a{}=topsecret", "[0]".repeat(10_000));
            let mut root = v("");
            apply_args(&mut root, [arg], &[]).unwrap_err()
        });
        assert_invalid_short(&err, |o| matches!(o, Origin::Cli { .. }));
    }

    #[test]
    fn exactly_64_segments_is_accepted() {
        small_stack(|| {
            let mut root = v("");
            let arg = format!("--a{}=x", ".b".repeat(63));
            apply_args(&mut root, [arg], &[]).unwrap();
            let name = format!("APP_A{}", "__0".repeat(63));
            apply_env(&mut root, "APP", env(&[(&name, "x")])).unwrap();
            let arg = format!("--c{}=x", "[0]".repeat(63));
            apply_args(&mut root, [arg], &[]).unwrap();
            let too_deep = format!("--a{}=x", ".b".repeat(64));
            assert!(apply_args(&mut root, [too_deep], &[]).is_err());
        });
    }

    #[test]
    fn error_for_overlong_key_is_truncated_and_has_no_value() {
        let err = small_stack(|| {
            let arg = format!("--{}=topsecret", "k".repeat(5_000) + &".b".repeat(100));
            let mut root = v("");
            apply_args(&mut root, [arg], &[]).unwrap_err()
        });
        assert_invalid_short(&err, |o| matches!(o, Origin::Cli { .. }));
        match &err {
            ConfigError::Invalid { key, origin, .. } => {
                assert!(key.ends_with('…'));
                let Some(Origin::Cli { arg }) = origin else {
                    panic!()
                };
                assert!(arg.chars().count() <= 131, "{}", arg.len());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn file_nesting_is_bounded_by_the_parser() {
        // The TOML parser caps nesting itself (a `Parse` error), well below what a default
        // 2 MiB thread stack handles, so file input cannot overflow the stack.
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(|| {
                for text in [
                    format!("a = {}1{}", "[".repeat(10_000), "]".repeat(10_000)),
                    format!("a = {}1{}", "{ b = ".repeat(10_000), " }".repeat(10_000)),
                ] {
                    let err = Node::parse_toml(&text, |line| Origin::Inline { line }).unwrap_err();
                    assert!(matches!(err, ConfigError::Parse { .. }), "{err:?}");
                }
                // Within the parser's limit: parse, merge and drop work.
                let text = format!("a = {}1{}", "[".repeat(40), "]".repeat(40));
                let mut node = Node::parse_toml(&text, |line| Origin::Inline { line }).unwrap();
                node.merge(v("a = 1"));
            })
            .unwrap()
            .join()
            .unwrap();
    }

    #[test]
    fn env_mixing_index_and_key_under_one_path_is_invalid() {
        let mut root = v("");
        let err = apply_env(
            &mut root,
            "APP",
            env(&[("APP_X__0", "topsecret"), ("APP_X__FOO", "topsecret")]),
        )
        .unwrap_err();
        assert!(
            matches!(&err, ConfigError::Invalid { key, .. } if key == "x"),
            "{err:?}"
        );
        assert!(!format!("{err} {err:?}").contains("topsecret"));
    }

    #[test]
    fn cli_mixing_index_and_key_under_one_path_is_invalid() {
        for order in [
            ["--x.0=topsecret", "--x.foo=topsecret"],
            ["--x.foo=topsecret", "--x[0]=topsecret"],
        ] {
            let mut root = v("");
            let err = apply_args(&mut root, args(&order), &[]).unwrap_err();
            assert!(
                matches!(&err, ConfigError::Invalid { key, origin: Some(Origin::Cli { .. }), .. } if key == "x"),
                "{err:?}"
            );
            assert!(!format!("{err} {err:?}").contains("topsecret"));
        }
    }

    #[test]
    fn higher_source_replaces_lower_source_list_with_table_and_vice_versa() {
        assert_eq!(
            applied("x = [1, 2]", &[("APP_X__FOO", "a")]),
            t("[x]\nfoo = \"a\"")
        );
        assert_eq!(
            applied("[x]\nfoo = 1", &[("APP_X__0", "a")]),
            t("x = [\"a\"]")
        );
    }
}
