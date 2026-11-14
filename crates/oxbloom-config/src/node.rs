//! The configuration tree: every node carries its [`Origin`]; keys keep their spelling next to
//! their canonical form.

use std::collections::BTreeMap;
use std::fmt;
use std::path::PathBuf;

use toml::de::{DeTable, DeValue};

use crate::error::ConfigError;
use crate::origin::Origin;

/// Canonical form of a key: lowercase with `-` and `_` removed.
pub(crate) fn canonical(key: &str) -> String {
    key.chars()
        .filter(|c| *c != '-' && *c != '_')
        .flat_map(char::to_lowercase)
        .collect()
}

#[derive(Clone)]
pub(crate) struct Node {
    pub origin: Origin,
    pub value: NodeValue,
}

#[derive(Clone)]
pub(crate) enum NodeValue {
    Table(Table),
    Array(Vec<Node>),
    String(String),
    Integer(i64),
    Float(f64),
    Boolean(bool),
    Datetime(toml::value::Datetime),
}

#[derive(Clone)]
pub(crate) struct Entry {
    pub spelling: String,
    pub node: Node,
}

/// Table ordered by canonical key.
#[derive(Clone, Default)]
pub(crate) struct Table {
    entries: BTreeMap<String, Entry>,
}

impl Table {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Looks up `key` by canonical match.
    pub(crate) fn get(&self, key: &str) -> Option<&Node> {
        self.entries.get(&canonical(key)).map(|e| &e.node)
    }

    pub(crate) fn get_entry(&self, key: &str) -> Option<&Entry> {
        self.entries.get(&canonical(key))
    }

    /// Inserts `node` under `spelling`, returning the entry it replaced, if any.
    pub(crate) fn insert(&mut self, spelling: impl Into<String>, node: Node) -> Option<Entry> {
        let spelling = spelling.into();
        self.entries
            .insert(canonical(&spelling), Entry { spelling, node })
    }

    /// Mutable lookup of `key` by canonical match.
    pub(crate) fn get_mut(&mut self, key: &str) -> Option<&mut Node> {
        self.entries.get_mut(&canonical(key)).map(|e| &mut e.node)
    }

    /// Removes `key` (canonical match), returning its entry.
    pub(crate) fn remove(&mut self, key: &str) -> Option<Entry> {
        self.entries.remove(&canonical(key))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Entries in canonical-key order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.entries.values()
    }
}

// Values may be secrets: `Debug` shows origins, kinds and key spellings only.
impl fmt::Debug for Node {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Node")
            .field("origin", &self.origin)
            .field("value", &self.value)
            .finish()
    }
}

impl fmt::Debug for NodeValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Table(t) => f.debug_tuple("Table").field(t).finish(),
            Self::Array(items) => f
                .debug_tuple("Array")
                .field(&items.iter().map(|n| &n.value).collect::<Vec<_>>())
                .finish(),
            Self::String(_) => f.write_str("String"),
            Self::Integer(_) => f.write_str("Integer"),
            Self::Float(_) => f.write_str("Float"),
            Self::Boolean(_) => f.write_str("Boolean"),
            Self::Datetime(_) => f.write_str("Datetime"),
        }
    }
}

impl fmt::Debug for Table {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_map()
            .entries(self.iter().map(|e| (&e.spelling, &e.node.value)))
            .finish()
    }
}

impl fmt::Debug for Entry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Entry")
            .field("spelling", &self.spelling)
            .field("node", &self.node)
            .finish()
    }
}

impl Node {
    /// A table node.
    pub(crate) fn table(origin: Origin, table: Table) -> Node {
        Node {
            origin,
            value: NodeValue::Table(table),
        }
    }

    /// The table, when this node is one.
    pub(crate) fn as_table(&self) -> Option<&Table> {
        match &self.value {
            NodeValue::Table(t) => Some(t),
            _ => None,
        }
    }

    /// The table, mutably, when this node is one.
    pub(crate) fn as_table_mut(&mut self) -> Option<&mut Table> {
        match &mut self.value {
            NodeValue::Table(t) => Some(t),
            _ => None,
        }
    }

    /// Parses TOML text; `origin_for(line)` maps a 1-based line to the origin of nodes there.
    pub(crate) fn parse_toml(
        text: &str,
        origin_for: impl Fn(usize) -> Origin,
    ) -> Result<Node, ConfigError> {
        // Line starts are computed once; each lookup is a binary search.
        let starts: Vec<usize> = std::iter::once(0)
            .chain(text.match_indices('\n').map(|(i, _)| i + 1))
            .collect();
        let line_of = |offset: usize| {
            let offset = offset.min(text.len());
            starts.partition_point(|&s| s <= offset)
        };
        let parsed = DeTable::parse(text).map_err(|e| ConfigError::Parse {
            path: match origin_for(1) {
                Origin::File { path, .. } => path,
                _ => PathBuf::new(),
            },
            // Only the position is kept: the parser's snippet may contain a secret.
            line: e.span().map(|s| line_of(s.start)),
            message: "invalid TOML".into(),
        })?;
        let bad = |offset: usize| ConfigError::Parse {
            path: match origin_for(1) {
                Origin::File { path, .. } => path,
                _ => PathBuf::new(),
            },
            line: Some(line_of(offset)),
            message: "invalid TOML".into(),
        };
        let conv = Conv {
            line_of: &line_of,
            origin_for: &origin_for,
            bad: &bad,
        };
        let span = parsed.span();
        let table = conv.table(parsed.get_ref(), &mut Vec::new())?;
        Ok(Node {
            origin: origin_for(line_of(span.start)),
            value: NodeValue::Table(table),
        })
    }

    /// Merges `over` into `self`: tables merge by canonical key (the entry takes the spelling
    /// of `over`), every other kind, arrays included, is replaced whole.
    pub(crate) fn merge(&mut self, over: Node) {
        match (&mut self.value, over.value) {
            (NodeValue::Table(base), NodeValue::Table(over_table)) => {
                self.origin = over.origin;
                for (canon, entry) in over_table.entries {
                    match base.entries.get_mut(&canon) {
                        Some(existing) => {
                            existing.spelling = entry.spelling;
                            existing.node.merge(entry.node);
                        }
                        None => {
                            base.entries.insert(canon, entry);
                        }
                    }
                }
            }
            (slot, value) => {
                *slot = value;
                self.origin = over.origin;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn to_toml_value(&self) -> toml::Value {
        match &self.value {
            NodeValue::Table(t) => toml::Value::Table(
                t.iter()
                    .map(|e| (e.spelling.clone(), e.node.to_toml_value()))
                    .collect(),
            ),
            NodeValue::Array(a) => toml::Value::Array(a.iter().map(Node::to_toml_value).collect()),
            NodeValue::String(s) => toml::Value::String(s.clone()),
            NodeValue::Integer(i) => toml::Value::Integer(*i),
            NodeValue::Float(f) => toml::Value::Float(*f),
            NodeValue::Boolean(b) => toml::Value::Boolean(*b),
            NodeValue::Datetime(d) => toml::Value::Datetime(*d),
        }
    }
}

struct Conv<'a> {
    line_of: &'a dyn Fn(usize) -> usize,
    origin_for: &'a dyn Fn(usize) -> Origin,
    bad: &'a dyn Fn(usize) -> ConfigError,
}

impl Conv<'_> {
    fn table(&self, t: &DeTable<'_>, path: &mut Vec<String>) -> Result<Table, ConfigError> {
        let mut out = Table::new();
        // DeTable iterates sorted by key; restore source order so `first` is the earlier one.
        let mut ordered: Vec<_> = t.iter().collect();
        ordered.sort_by_key(|(key, _)| key.span().start);
        for (key, value) in ordered {
            let spelling = key.get_ref().as_ref();
            path.push(spelling.to_string());
            let node = self.value(value.get_ref(), value.span().start, path)?;
            path.pop();
            if let Some(first) = out.get_entry(spelling) {
                let mut dotted = path.clone();
                dotted.push(first.spelling.clone());
                return Err(ConfigError::DuplicateKey {
                    key: dotted.join("."),
                    first: first.node.origin.clone(),
                    second: node.origin,
                });
            }
            out.insert(spelling, node);
        }
        Ok(out)
    }

    fn value(
        &self,
        v: &DeValue<'_>,
        start: usize,
        path: &mut Vec<String>,
    ) -> Result<Node, ConfigError> {
        let origin = (self.origin_for)((self.line_of)(start));
        let value = match v {
            DeValue::String(s) => NodeValue::String(s.to_string()),
            DeValue::Integer(i) => NodeValue::Integer(
                i64::from_str_radix(i.as_str(), i.radix()).map_err(|_| (self.bad)(start))?,
            ),
            DeValue::Float(f) => {
                let parsed: f64 = f.as_str().parse().map_err(|_| (self.bad)(start))?;
                NodeValue::Float(parsed)
            }
            DeValue::Boolean(b) => NodeValue::Boolean(*b),
            DeValue::Datetime(d) => NodeValue::Datetime(*d),
            DeValue::Array(items) => NodeValue::Array(
                items
                    .iter()
                    .map(|item| self.value(item.get_ref(), item.span().start, path))
                    .collect::<Result<_, _>>()?,
            ),
            DeValue::Table(t) => NodeValue::Table(self.table(t, path)?),
        };
        Ok(Node { origin, value })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(line: usize) -> Origin {
        Origin::File {
            path: "app.toml".into(),
            line,
        }
    }

    fn parse(text: &str) -> Node {
        Node::parse_toml(text, file).unwrap()
    }

    fn table(n: &Node) -> &Table {
        match &n.value {
            NodeValue::Table(t) => t,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn canonical_lowercases_and_drops_dash_and_underscore() {
        assert_eq!(canonical("Max-Conn_ections"), "maxconnections");
    }

    #[test]
    fn canonical_is_idempotent() {
        let once = canonical("Max-Conn_ections");
        assert_eq!(canonical(&once), once);
    }

    #[test]
    fn parse_toml_records_lines() {
        let n = parse("a = 1\nb = 2\n\nx = { y = 1 }\n");
        let x = table(&n).get("x").unwrap();
        assert_eq!(x.origin, file(4));
        assert_eq!(table(x).get("y").unwrap().origin, file(4));
        assert_eq!(table(&n).get("b").unwrap().origin, file(2));
    }

    #[test]
    fn parse_toml_array_elements_have_own_lines() {
        let n = parse("list = [\n  1,\n  2,\n]\n");
        let NodeValue::Array(items) = &table(&n).get("list").unwrap().value else {
            panic!("not an array");
        };
        assert_eq!(items[0].origin, file(2));
        assert_eq!(items[1].origin, file(3));
    }

    #[test]
    fn duplicate_spellings_in_one_table_are_duplicate_key() {
        let (key, first, second) = dup("max-conn = 1\nmax_conn = 2\n");
        assert_eq!(key, "max-conn");
        assert_eq!((first, second), (file(1), file(2)));
    }

    fn dup(text: &str) -> (String, Origin, Origin) {
        match Node::parse_toml(text, file).unwrap_err() {
            ConfigError::DuplicateKey { key, first, second } => (key, first, second),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn duplicate_reports_source_order_underscore_first() {
        let (key, first, second) = dup("max_conn = 1\nmax-conn = 2\n");
        assert_eq!(key, "max_conn");
        assert_eq!((first, second), (file(1), file(2)));
    }

    #[test]
    fn duplicate_reports_source_order_uppercase_first() {
        let (key, first, second) = dup("[s]\nMax_Conn = 1\nmax-conn = 2\n");
        assert_eq!(key, "s.Max_Conn");
        assert_eq!((first, second), (file(2), file(3)));
    }

    #[test]
    fn duplicate_reports_source_order_for_headers() {
        let (key, first, second) = dup("[S]\na = 1\n[s]\nb = 2\n");
        assert_eq!(key, "S");
        assert_eq!((first, second), (file(1), file(3)));
        // Lowercase header first: sorted order would put `S` first, source order must win.
        let (key, first, second) = dup("[s]\na = 1\n[S]\nb = 2\n");
        assert_eq!(key, "s");
        assert_eq!((first, second), (file(1), file(3)));
    }

    #[test]
    fn merge_tables_deeply_and_takes_overriding_spelling() {
        let mut base = parse("[server]\nmax-conn = 1\nhost = \"a\"\n");
        let over =
            Node::parse_toml("[server]\nmax_conn = 2\n", |l| Origin::Inline { line: l }).unwrap();
        base.merge(over);
        let server = table(&base).get("server").unwrap();
        let entry = table(server).get_entry("maxconn").unwrap();
        assert_eq!(entry.spelling, "max_conn");
        assert!(matches!(entry.node.value, NodeValue::Integer(2)));
        assert_eq!(entry.node.origin, Origin::Inline { line: 2 });
        assert!(table(server).get("host").is_some());
    }

    #[test]
    fn merge_replaces_arrays_and_scalars_whole() {
        let mut base = parse("l = [1, 2, 3]\ns = 1\nt = { a = 1 }\n");
        base.merge(parse("l = [9]\ns = \"x\"\nt = 5\n"));
        let v = base.to_toml_value();
        assert_eq!(v["l"].as_array().unwrap().len(), 1);
        assert_eq!(v["s"].as_str(), Some("x"));
        assert_eq!(v["t"].as_integer(), Some(5));
    }

    #[test]
    fn merge_is_associative_for_tables() {
        let a = || parse("[s]\nx = 1\ny = 1\n[t]\nk = 1\n");
        let b = || parse("[s]\nx = 2\nz = [1]\n");
        let c = || parse("[s]\ny = 3\nz = [2, 3]\n[u]\nq = true\n");

        let mut left = a();
        left.merge(b());
        left.merge(c());

        let mut bc = b();
        bc.merge(c());
        let mut right = a();
        right.merge(bc);

        assert_eq!(left.to_toml_value(), right.to_toml_value());
    }

    #[test]
    fn origin_display_forms() {
        assert_eq!(
            Origin::File {
                path: "application-dev.toml".into(),
                line: 4
            }
            .to_string(),
            "application-dev.toml:4"
        );
        assert_eq!(Origin::Inline { line: 3 }.to_string(), "line 3");
        assert_eq!(
            Origin::Env {
                name: "APP_SERVER__PORT".into()
            }
            .to_string(),
            "env APP_SERVER__PORT"
        );
        assert_eq!(
            Origin::Cli {
                arg: "--server.port".into()
            }
            .to_string(),
            "--server.port"
        );
    }

    #[test]
    fn invalid_toml_is_parse_error_without_snippet() {
        let err = Node::parse_toml("ok = 1\npassword = = \"hunter2\"\n", file).unwrap_err();
        match &err {
            ConfigError::Parse {
                path,
                line,
                message,
            } => {
                assert_eq!(path, &PathBuf::from("app.toml"));
                assert_eq!(*line, Some(2));
                assert_eq!(message, "invalid TOML");
            }
            other => panic!("{other:?}"),
        }
        let text = format!("{err} {err:?}");
        assert!(!text.contains("hunter2"), "{text}");
    }

    #[test]
    fn node_debug_prints_kinds_and_keys_never_values() {
        let n = parse(
            "password = \"hunter2\"\nport = 31337\nratio = 0.125\nflag = true\nlist = [\"s3cr3t\"]\nwhen = 1979-05-27\n[db]\nuser = \"admin-user\"\n",
        );
        let text = format!("{n:?} {:?}", n.value);
        for secret in [
            "hunter2",
            "31337",
            "0.125",
            "true",
            "s3cr3t",
            "1979",
            "admin-user",
        ] {
            assert!(!text.contains(secret), "{secret} in {text}");
        }
        for key in ["password", "port", "db", "list"] {
            assert!(text.contains(key), "{key} missing in {text}");
        }
    }
}
