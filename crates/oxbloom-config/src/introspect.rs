//! Introspection: where every configuration leaf came from. Values are never revealed.

use crate::config::Config;
use crate::node::{Node, NodeValue};
use crate::origin::Origin;

/// One configuration leaf: its key, where its value came from and the kind of the value.
///
/// The value itself is never part of an entry.
///
/// # Examples
///
/// ```
/// use oxbloom_config::{Config, Origin};
///
/// let config = Config::from_str("[server]\nport = 8080")?;
/// let entries = config.explain();
/// assert_eq!(entries[0].key, "server.port");
/// assert_eq!(entries[0].kind, "integer");
/// assert_eq!(entries[0].origin, Origin::Inline { line: 2 });
/// # Ok::<(), oxbloom_config::ConfigError>(())
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Dotted key in the spelling of the winning source; array elements as `list[0]`.
    pub key: String,
    /// Where the value came from.
    pub origin: Origin,
    /// One of `string`, `integer`, `float`, `boolean`, `datetime`.
    pub kind: &'static str,
}

fn kind(value: &NodeValue) -> Option<&'static str> {
    match value {
        NodeValue::String(_) => Some("string"),
        NodeValue::Integer(_) => Some("integer"),
        NodeValue::Float(_) => Some("float"),
        NodeValue::Boolean(_) => Some("boolean"),
        NodeValue::Datetime(_) => Some("datetime"),
        NodeValue::Table(_) | NodeValue::Array(_) => None,
    }
}

/// Appends the leaves below `node` (at `path`) to `out`.
fn collect(node: &Node, path: &mut String, out: &mut Vec<Entry>) {
    match &node.value {
        NodeValue::Table(table) => {
            for entry in table.iter() {
                let len = path.len();
                if !path.is_empty() {
                    path.push('.');
                }
                path.push_str(&entry.spelling);
                collect(&entry.node, path, out);
                path.truncate(len);
            }
        }
        NodeValue::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                let len = path.len();
                path.push_str(&format!("[{i}]"));
                collect(item, path, out);
                path.truncate(len);
            }
        }
        other => {
            if let Some(kind) = kind(other) {
                out.push(Entry {
                    key: path.clone(),
                    origin: node.origin.clone(),
                    kind,
                });
            }
        }
    }
}

/// Splits one path segment into its name and trailing `[n]` indices; `None` when malformed.
fn split_segment(segment: &str) -> Option<(&str, Vec<usize>)> {
    let (name, mut rest) = match segment.find('[') {
        Some(at) => segment.split_at(at),
        None => (segment, ""),
    };
    let mut indices = Vec::new();
    while !rest.is_empty() {
        let inner = rest.strip_prefix('[')?;
        let end = inner.find(']')?;
        indices.push(parse_index(&inner[..end])?);
        rest = &inner[end + 1..];
    }
    Some((name, indices))
}

fn parse_index(text: &str) -> Option<usize> {
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse().ok()
}

impl Config {
    /// Walks `key` (dotted, relaxed per segment, `list[0]` or `list.0` for elements) and returns
    /// the node with the key spelled as `explain` spells it.
    fn locate(&self, key: &str) -> Option<(&Node, String)> {
        let mut node = &self.root;
        let mut spelled = String::new();
        for segment in key.split('.') {
            let (name, indices) = split_segment(segment)?;
            if !name.is_empty() {
                match &node.value {
                    NodeValue::Table(table) => {
                        let entry = table.get_entry(name)?;
                        if !spelled.is_empty() {
                            spelled.push('.');
                        }
                        spelled.push_str(&entry.spelling);
                        node = &entry.node;
                    }
                    NodeValue::Array(_) if indices.is_empty() => {
                        // `list.0`: a bare numeric segment indexes the array.
                        node = element(node, parse_index(name)?)?;
                        spelled.push_str(&format!("[{name}]"));
                    }
                    _ => return None,
                }
            } else if indices.is_empty() {
                return None;
            }
            for index in indices {
                node = element(node, index)?;
                spelled.push_str(&format!("[{index}]"));
            }
        }
        Some((node, spelled))
    }

    /// Origin of the leaf at `key`, or `None` for a table, an array as a whole, an unknown key
    /// or the empty key.
    ///
    /// `key` is a dotted path matched like [`get`](Config::get) (case, `-` and `_` ignored per
    /// segment); array elements are addressed as `list[0]` or `list.0`. Only leaves have an
    /// origin: table origins would be the last source that touched the table, which says
    /// little.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::{Config, Origin};
    ///
    /// let config = Config::from_str("[server]\nport = 8080\nhosts = [\"a\", \"b\"]")?;
    /// assert_eq!(config.origin("SERVER.port"), Some(&Origin::Inline { line: 2 }));
    /// assert_eq!(config.origin("server.hosts[1]"), config.origin("server.hosts.1"));
    /// assert_eq!(config.origin("server"), None);   // a table
    /// assert_eq!(config.origin("missing"), None);
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn origin(&self, key: &str) -> Option<&Origin> {
        let (node, _) = self.locate(key)?;
        kind(&node.value).map(|_| &node.origin)
    }

    /// One [`Entry`] per leaf (scalars and array elements), sorted by key.
    ///
    /// Keys use the spelling of the winning source and `list[0]` for array elements. Values are
    /// never part of the output, so it is safe to print or log.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    ///
    /// let config = Config::from_str("name = \"demo\"\nports = [80, 443]")?;
    /// let keys: Vec<String> = config.explain().into_iter().map(|e| e.key).collect();
    /// assert_eq!(keys, ["name", "ports[0]", "ports[1]"]);
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn explain(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        collect(&self.root, &mut String::new(), &mut out);
        out.sort_by(|a, b| a.key.cmp(&b.key));
        out
    }

    /// Leaf keys under the dotted `prefix` (empty for everything), sorted, spelled as in
    /// [`explain`](Config::explain).
    ///
    /// A prefix that is itself a leaf returns just that key; an unknown prefix returns an empty
    /// list.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_config::Config;
    ///
    /// let config = Config::from_str("[db]\nurl = \"x\"\nhosts = [\"a\"]\n[web]\nport = 1")?;
    /// assert_eq!(config.keys("db"), ["db.hosts[0]", "db.url"]);
    /// assert_eq!(config.keys("web.port"), ["web.port"]);
    /// assert!(config.keys("nope").is_empty());
    /// assert_eq!(config.keys("").len(), 3);
    /// # Ok::<(), oxbloom_config::ConfigError>(())
    /// ```
    pub fn keys(&self, prefix: &str) -> Vec<String> {
        let mut out = Vec::new();
        if prefix.is_empty() {
            collect(&self.root, &mut String::new(), &mut out);
        } else if let Some((node, mut spelled)) = self.locate(prefix) {
            collect(node, &mut spelled, &mut out);
        }
        let mut keys: Vec<String> = out.into_iter().map(|e| e.key).collect();
        keys.sort();
        keys
    }
}

fn element(node: &Node, index: usize) -> Option<&Node> {
    match &node.value {
        NodeValue::Array(items) => items.get(index),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crate::{Config, Entry, Origin};
    use std::fs;

    const MARKER: &str = "MARKER_8f3a91";

    fn dir_with(base: &str) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join("application.toml"), base).unwrap();
        d
    }

    fn sample() -> Config {
        Config::from_str(
            "name = \"n\"\ncount = 3\nratio = 1.5\non = true\nwhen = 1979-05-27T07:32:00Z\n\
             [server]\nport = 80\nhosts = [\"a\", \"b\"]\n[[items]]\nid = 1\n[[items]]\nid = 2\n",
        )
        .unwrap()
    }

    #[test]
    fn origin_per_source_kind() {
        let d = dir_with("a = 1\nb = 2\nc = 3\n");
        let c = Config::builder()
            .dir(d.path())
            .env_from([("APP_B".to_string(), "x".to_string())])
            .args(["--c=9"])
            .build()
            .unwrap();
        assert_eq!(
            c.origin("a"),
            Some(&Origin::File {
                path: "application.toml".into(),
                line: 1
            })
        );
        assert_eq!(
            c.origin("b"),
            Some(&Origin::Env {
                name: "APP_B".into()
            })
        );
        assert_eq!(c.origin("c"), Some(&Origin::Cli { arg: "--c".into() }));
        let inline = Config::from_str("\n\nz = 1").unwrap();
        assert_eq!(inline.origin("z"), Some(&Origin::Inline { line: 3 }));
    }

    #[test]
    fn origin_line_numbers_for_nested_table_and_array() {
        let c = Config::from_str("a = 1\n[server]\nport = 1\nhosts = [\n  \"x\",\n  \"y\",\n]\n")
            .unwrap();
        assert_eq!(c.origin("server.port"), Some(&Origin::Inline { line: 3 }));
        assert_eq!(
            c.origin("server.hosts[0]"),
            Some(&Origin::Inline { line: 5 })
        );
        assert_eq!(
            c.origin("server.hosts[1]"),
            Some(&Origin::Inline { line: 6 })
        );
    }

    #[test]
    fn origin_of_table_and_unknown_key_is_none() {
        let c = sample();
        assert_eq!(c.origin("server"), None);
        assert_eq!(c.origin("server.hosts"), None);
        assert_eq!(c.origin("nope"), None);
        assert_eq!(c.origin("server.nope"), None);
        assert_eq!(c.origin("server.hosts[2]"), None);
        assert_eq!(c.origin("name.deeper"), None);
        assert_eq!(c.origin(""), None);
        assert_eq!(c.origin("server."), None);
        assert_eq!(c.origin("server.hosts[x]"), None);
    }

    #[test]
    fn origin_accepts_relaxed_spelling_and_array_notation() {
        let c = Config::from_str("[Db]\nmax-conn = 1\nlist = [5, 6]\n").unwrap();
        assert!(c.origin("DB.MAX_CONN").is_some());
        assert!(c.origin("db.maxconn").is_some());
        assert_eq!(c.origin("db.list[1]"), c.origin("db.list.1"));
        assert!(c.origin("db.list.1").is_some());
        assert!(c.origin("db.list[0]").is_some());
    }

    #[test]
    fn explain_lists_all_leaves_sorted_with_kinds() {
        let e = sample().explain();
        let got: Vec<(&str, &str)> = e.iter().map(|x| (x.key.as_str(), x.kind)).collect();
        assert_eq!(
            got,
            vec![
                ("count", "integer"),
                ("items[0].id", "integer"),
                ("items[1].id", "integer"),
                ("name", "string"),
                ("on", "boolean"),
                ("ratio", "float"),
                ("server.hosts[0]", "string"),
                ("server.hosts[1]", "string"),
                ("server.port", "integer"),
                ("when", "datetime"),
            ]
        );
        assert_eq!(e[0].origin, Origin::Inline { line: 2 });
    }

    #[test]
    fn explain_never_contains_values() {
        let c = Config::from_str(&format!(
            "s = \"{MARKER}\"\ni = 987123\nl = [\"{MARKER}\"]\n[t]\nk = \"{MARKER}\"\n"
        ))
        .unwrap();
        let e = c.explain();
        let text = format!("{e:?} {:?} {:?}", c.keys(""), c);
        assert!(!text.contains(MARKER) && !text.contains("987123"), "{text}");
    }

    #[test]
    fn explain_array_elements_use_bracket_notation() {
        let keys: Vec<String> = sample().explain().into_iter().map(|e| e.key).collect();
        assert!(keys.contains(&"server.hosts[0]".to_string()));
        assert!(keys.contains(&"items[1].id".to_string()));
        assert!(!keys.iter().any(|k| k.contains(".0") || k.contains(".1.")));
    }

    #[test]
    fn explain_key_uses_spelling_of_winning_source() {
        let d = dir_with("[Server]\nmax-conn = 1\n");
        let c = Config::builder()
            .dir(d.path())
            .env_from([("APP_SERVER__MAX_CONN".to_string(), "2".to_string())])
            .build()
            .unwrap();
        let e = c.explain();
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].key, "server.max_conn");
        assert_eq!(
            e[0].origin,
            Origin::Env {
                name: "APP_SERVER__MAX_CONN".into()
            }
        );
    }

    #[test]
    fn keys_under_prefix() {
        let c = sample();
        assert_eq!(
            c.keys("server"),
            vec!["server.hosts[0]", "server.hosts[1]", "server.port"]
        );
        assert_eq!(
            c.keys("SERVER.hosts"),
            vec!["server.hosts[0]", "server.hosts[1]"]
        );
        assert_eq!(c.keys("items[1]"), vec!["items[1].id"]);
    }

    #[test]
    fn keys_for_leaf_prefix_and_unknown_prefix() {
        let c = sample();
        assert_eq!(c.keys("server.port"), vec!["server.port"]);
        assert_eq!(c.keys("server.hosts.1"), vec!["server.hosts[1]"]);
        assert!(c.keys("nope").is_empty());
        assert!(c.keys("server.port.x").is_empty());
    }

    #[test]
    fn keys_empty_prefix_lists_everything() {
        let c = sample();
        assert_eq!(c.keys("").len(), 10);
        assert_eq!(c.keys("")[0], "count");
    }

    #[test]
    fn explain_and_keys_agree() {
        let c = sample();
        let a: Vec<String> = c.explain().into_iter().map(|e| e.key).collect();
        assert_eq!(a, c.keys(""));
    }

    #[test]
    fn entry_is_plain_data() {
        let e = Entry {
            key: "a".into(),
            origin: Origin::Inline { line: 1 },
            kind: "string",
        };
        assert_eq!(e.clone(), e);
    }

    #[test]
    fn explain_is_linear_for_20k_leaves() {
        let vars: Vec<(String, String)> = (0..20_000)
            .map(|i| (format!("APP_G{}__K{}", i % 200, i), "v".to_string()))
            .collect();
        let c = Config::builder()
            .dir(std::env::temp_dir().join("oxbloom-no-such-dir-introspect"))
            .env_from(vars)
            .build()
            .unwrap();
        let start = std::time::Instant::now();
        let e = c.explain();
        let k = c.keys("");
        let k2 = c.keys("g7");
        let took = start.elapsed();
        assert_eq!(e.len(), 20_000);
        assert_eq!(k.len(), 20_000);
        assert_eq!(k2.len(), 100);
        assert!(took.as_millis() < 1000, "took {took:?}");
    }
}
