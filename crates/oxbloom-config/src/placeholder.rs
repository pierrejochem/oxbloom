//! `${name}` / `${name:default}` resolution over the merged tree.
//!
//! Syntax: `${name}`, `${name:default}` (the default is everything after the first `:` up to the
//! matching `}`, nested `${..}` pairs balanced, and may contain placeholders itself), `$${` is a
//! literal `${`. A name is looked up in the merged config (dotted path, canonical match per
//! segment, a numeric segment indexes an array) and then in the injected environment (exact,
//! case-sensitive). Values are never put in an error: only keys and placeholder names (at most
//! 32 characters of a name).
//!
//! Only leaves from files (and `Config::from_str` text) are resolved. Leaves set by the
//! environment or the command line are final: taken literally, `$${` included, and a
//! placeholder that refers to one gets its text as is.
//!
//! Security: configuration files are trusted. A placeholder can read any variable of the
//! environment it is given; it never reads the process environment on its own.

use std::collections::HashMap;

use crate::error::ConfigError;
use crate::node::{Node, NodeValue, canonical};
use crate::origin::Origin;

/// Shared limit for the nesting of leaves resolved through each other plus defaults inside
/// defaults; beyond it the placeholders count as a cycle.
const MAX_DEPTH: usize = 32;
/// Longest placeholder name (or unterminated placeholder text) that may appear in an error.
const MAX_NAME_ECHO: usize = 32;
/// Largest resolved string.
const MAX_LEAF_BYTES: usize = 1 << 20;
/// Largest total of resolved strings in one `resolve` call.
const MAX_TOTAL_BYTES: usize = 16 << 20;

/// Resolves placeholders in every string leaf of `root`; `env` is the injected environment.
/// The result stays a string and keeps the origin of the leaf that contained it.
pub(crate) fn resolve(root: &mut Node, env: &[(String, String)]) -> Result<(), ConfigError> {
    let mut paths = Vec::new();
    collect(root, &mut Vec::new(), &mut paths);
    if paths.is_empty() {
        return Ok(());
    }
    let mut resolver = Resolver {
        root,
        env,
        cache: HashMap::new(),
        stack: Vec::new(),
        total: 0,
    };
    for path in &paths {
        resolver.leaf(path, 0)?;
    }
    // Each resolved string is moved out of the cache into the tree: stored once.
    let mut cache = resolver.cache;
    for path in paths {
        if let Some(value) = cache.remove(&id_of(&path))
            && let Some(node) = node_at_mut(root, &path)
            && let NodeValue::String(slot) = &mut node.value
        {
            *slot = value;
        }
    }
    Ok(())
}

fn id_of(path: &[String]) -> Vec<String> {
    path.iter().map(|s| canonical(s)).collect()
}

/// Whether the leaf's text is final: environment and command-line values are not resolved.
fn literal(node: &Node) -> bool {
    matches!(node.origin, Origin::Env { .. } | Origin::Cli { .. })
}

/// Paths (key spellings, array indices as numbers) of the string leaves containing `${`,
/// except literal ones.
fn collect(node: &Node, path: &mut Vec<String>, out: &mut Vec<Vec<String>>) {
    match &node.value {
        NodeValue::String(s) if s.contains("${") && !literal(node) => out.push(path.clone()),
        NodeValue::Table(t) => {
            for entry in t.iter() {
                path.push(entry.spelling.clone());
                collect(&entry.node, path, out);
                path.pop();
            }
        }
        NodeValue::Array(items) => {
            for (i, item) in items.iter().enumerate() {
                path.push(i.to_string());
                collect(item, path, out);
                path.pop();
            }
        }
        _ => {}
    }
}

fn node_at_mut<'a>(root: &'a mut Node, path: &[String]) -> Option<&'a mut Node> {
    let mut node = root;
    for seg in path {
        node = match &mut node.value {
            NodeValue::Table(t) => t.get_mut(seg)?,
            NodeValue::Array(items) => items.get_mut(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(node)
}

/// Walks `name` (dotted) from `root`; returns the node and the path with the spellings used.
fn lookup<'a>(root: &'a Node, name: &str) -> Option<(&'a Node, Vec<String>)> {
    if name.is_empty() {
        return None;
    }
    let mut node = root;
    let mut path = Vec::new();
    for seg in name.split('.') {
        match &node.value {
            NodeValue::Table(t) => {
                let entry = t.get_entry(seg)?;
                path.push(entry.spelling.clone());
                node = &entry.node;
            }
            NodeValue::Array(items) => {
                let index = seg.parse::<usize>().ok()?;
                path.push(index.to_string());
                node = items.get(index)?;
            }
            _ => return None,
        }
    }
    Some((node, path))
}

struct Frame {
    id: Vec<String>,
    dotted: String,
}

struct Resolver<'a> {
    root: &'a Node,
    env: &'a [(String, String)],
    /// Resolved text per leaf (canonical segments): a leaf resolves the same for everyone.
    cache: HashMap<Vec<String>, String>,
    /// Leaves being resolved, outermost first.
    stack: Vec<Frame>,
    /// Bytes of the finished strings in `cache`.
    total: usize,
}

impl Resolver<'_> {
    /// Resolves the string leaf at `path` into the cache.
    fn leaf(&mut self, path: &[String], depth: usize) -> Result<(), ConfigError> {
        let id = id_of(path);
        if self.cache.contains_key(&id) {
            return Ok(());
        }
        if let Some(pos) = self.stack.iter().position(|f| f.id == id) {
            return Err(self.cycle(pos));
        }
        self.stack.push(Frame {
            id: id.clone(),
            dotted: path.join("."),
        });
        // One limit for leaves resolved through each other and defaults inside defaults.
        if depth > MAX_DEPTH {
            return Err(self.cycle(0));
        }
        let root = self.root;
        let text = match lookup_path(root, path) {
            Some(Node {
                value: NodeValue::String(s),
                ..
            }) => s.as_str(),
            _ => "",
        };
        let mut out = String::new();
        self.expand(&mut out, text, depth)?;
        self.stack.pop();
        self.total += out.len();
        self.cache.insert(id, out);
        Ok(())
    }

    fn cycle(&self, from: usize) -> ConfigError {
        ConfigError::PlaceholderCycle {
            keys: self.stack[from..]
                .iter()
                .map(|f| f.dotted.clone())
                .collect(),
        }
    }

    fn current_key(&self) -> String {
        self.stack
            .last()
            .map(|f| f.dotted.clone())
            .unwrap_or_default()
    }

    /// The name echoes at most 32 characters (plus `…` when cut).
    fn unresolved(&self, name: &str) -> ConfigError {
        let mut shown: String = name.chars().take(MAX_NAME_ECHO).collect();
        if name.chars().nth(MAX_NAME_ECHO).is_some() {
            shown.push('…');
        }
        ConfigError::UnresolvedPlaceholder {
            key: self.current_key(),
            name: shown,
        }
    }

    /// Checks that `add` more bytes fit after `have` bytes of the current string.
    fn room(&self, have: usize, add: usize) -> Result<(), ConfigError> {
        let size = have.saturating_add(add);
        if size > MAX_LEAF_BYTES || self.total.saturating_add(size) > MAX_TOTAL_BYTES {
            return Err(ConfigError::PlaceholderTooLarge {
                key: self.current_key(),
            });
        }
        Ok(())
    }

    fn push(&self, out: &mut String, add: &str) -> Result<(), ConfigError> {
        self.room(out.len(), add.len())?;
        out.push_str(add);
        Ok(())
    }

    /// Appends `text` with its placeholders replaced, left to right. `depth` is the shared nesting of
    /// referenced leaves and defaults. Offsets only ever sit on ASCII bytes, so slicing is on char boundaries.
    fn expand(&mut self, out: &mut String, text: &str, depth: usize) -> Result<(), ConfigError> {
        let bytes = text.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'$' {
                let run = text[i..].find('$').map_or(text.len(), |n| i + n);
                self.push(out, &text[i..run])?;
                i = run;
            } else if bytes[i..].starts_with(b"$${") {
                self.push(out, "${")?;
                i += 3;
            } else if bytes[i..].starts_with(b"${") {
                let (name, default, end) = self.parse(text, i + 2)?;
                self.value_of(out, name, default, depth)?;
                i = end;
            } else {
                self.push(out, "$")?;
                i += 1;
            }
        }
        Ok(())
    }

    /// Parses a placeholder whose name starts at `start`; returns the name, the raw default and
    /// the offset after the closing `}`.
    fn parse<'t>(
        &self,
        text: &'t str,
        start: usize,
    ) -> Result<(&'t str, Option<&'t str>, usize), ConfigError> {
        let bytes = text.as_bytes();
        let mut j = start;
        while j < bytes.len() {
            match bytes[j] {
                b'}' => return Ok((&text[start..j], None, j + 1)),
                b':' => {
                    let name = &text[start..j];
                    let mut depth = 0usize;
                    let mut k = j + 1;
                    while k < bytes.len() {
                        if bytes[k..].starts_with(b"$${") {
                            k += 3;
                        } else if bytes[k..].starts_with(b"${") {
                            depth += 1;
                            k += 2;
                        } else if bytes[k] == b'}' {
                            if depth == 0 {
                                return Ok((name, Some(&text[j + 1..k]), k + 1));
                            }
                            depth -= 1;
                            k += 1;
                        } else {
                            k += 1;
                        }
                    }
                    return Err(self.unterminated(&text[start..]));
                }
                b'$' if bytes[j..].starts_with(b"${") => {
                    // Nested names are not supported; nothing of the text is echoed.
                    return Err(self.unresolved(""));
                }
                _ => j += 1,
            }
        }
        Err(self.unterminated(&text[start..]))
    }

    /// Error for a `${` without `}`: the name echoes at most 32 characters and nothing from a
    /// `:` on (a default may be a secret).
    fn unterminated(&self, after: &str) -> ConfigError {
        self.unresolved(after.split(':').next().unwrap_or_default())
    }

    fn value_of(
        &mut self,
        out: &mut String,
        name: &str,
        default: Option<&str>,
        depth: usize,
    ) -> Result<(), ConfigError> {
        let root = self.root;
        if let Some((node, path)) = lookup(root, name) {
            return match &node.value {
                // An environment or command-line leaf is used as is.
                NodeValue::String(s) if literal(node) => self.push(out, s),
                NodeValue::String(_) => {
                    self.leaf(&path, depth + 1)?;
                    let value = self.cache.get(&id_of(&path)).map_or("", String::as_str);
                    self.room(out.len(), value.len())?;
                    out.push_str(value);
                    Ok(())
                }
                NodeValue::Integer(i) => self.push(out, &i.to_string()),
                NodeValue::Float(f) => self.push(out, &f.to_string()),
                NodeValue::Boolean(b) => self.push(out, &b.to_string()),
                NodeValue::Datetime(d) => self.push(out, &d.to_string()),
                NodeValue::Table(_) | NodeValue::Array(_) => Err(self.unresolved(name)),
            };
        }
        if let Some((_, v)) = self.env.iter().find(|(k, _)| k == name) {
            return self.push(out, v);
        }
        match default {
            Some(d) => {
                // Checked before recursing: nesting of defaults is bounded like leaf nesting.
                if depth >= MAX_DEPTH {
                    return Err(self.cycle(0));
                }
                self.expand(out, d, depth + 1)
            }
            None => Err(self.unresolved(name)),
        }
    }
}

/// The node at `path` (key spellings and array indices), without splitting on dots.
fn lookup_path<'a>(root: &'a Node, path: &[String]) -> Option<&'a Node> {
    let mut node = root;
    for seg in path {
        node = match &node.value {
            NodeValue::Table(t) => t.get(seg)?,
            NodeValue::Array(items) => items.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(node)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use crate::loader::load;
    use crate::node::NodeValue;
    use crate::origin::Origin;

    fn parse(text: &str) -> Node {
        Node::parse_toml(text, |line| Origin::Inline { line }).unwrap()
    }

    fn env(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn run(text: &str, vars: &[(&str, &str)]) -> Result<Node, ConfigError> {
        let mut n = parse(text);
        resolve(&mut n, &env(vars))?;
        Ok(n)
    }

    fn leaf<'a>(root: &'a Node, key: &str) -> &'a Node {
        let mut node = root;
        for seg in key.split('.') {
            node = match &node.value {
                NodeValue::Array(items) => &items[seg.parse::<usize>().unwrap()],
                _ => node.as_table().and_then(|t| t.get(seg)).unwrap(),
            };
        }
        node
    }

    fn text(root: &Node, key: &str) -> String {
        match &leaf(root, key).value {
            NodeValue::String(s) => s.clone(),
            other => panic!("{other:?}"),
        }
    }

    fn unresolved(text: &str, vars: &[(&str, &str)]) -> (String, String) {
        match run(text, vars).unwrap_err() {
            ConfigError::UnresolvedPlaceholder { key, name } => (key, name),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn placeholder_resolves_config_key_canonically() {
        let n = run(
            "[server]\nMax-Conn = 5\nhost = \"h\"\n[a]\nurl = \"${SERVER.maxconn}/${server.host}\"\n",
            &[],
        )
        .unwrap();
        assert_eq!(text(&n, "a.url"), "5/h");
    }

    #[test]
    fn placeholder_resolves_array_element_by_dot_index() {
        let n = run("l = [\"x\", \"y\"]\nv = \"${l.1}\"\n", &[]).unwrap();
        assert_eq!(text(&n, "v"), "y");
    }

    #[test]
    fn placeholder_falls_back_to_environment() {
        let n = run("v = \"a-${DB_HOST}\"\n", &[("DB_HOST", "db1")]).unwrap();
        assert_eq!(text(&n, "v"), "a-db1");
        // Exact, case-sensitive match.
        assert_eq!(
            unresolved("v = \"${db_host}\"", &[("DB_HOST", "x")]).1,
            "db_host"
        );
    }

    #[test]
    fn config_key_wins_over_environment() {
        let n = run("home = \"cfg\"\nv = \"${home}\"\n", &[("home", "env")]).unwrap();
        assert_eq!(text(&n, "v"), "cfg");
    }

    #[test]
    fn env_lookup_uses_injected_env_only() {
        // SAFETY-free: the process variable exists, the injected list does not have it.
        assert!(std::env::var("PATH").is_ok());
        assert_eq!(unresolved("v = \"${PATH}\"", &[]).1, "PATH");
        let n = run("v = \"${PATH:none}\"", &[]).unwrap();
        assert_eq!(text(&n, "v"), "none");
    }

    #[test]
    fn placeholder_default_used_when_unset() {
        let n = run(
            "v = \"${nope:fallback}\"\ne = \"${nope:}\"\nc = \"${nope:a:b}\"\n",
            &[],
        )
        .unwrap();
        assert_eq!(text(&n, "v"), "fallback");
        assert_eq!(text(&n, "e"), "");
        assert_eq!(text(&n, "c"), "a:b");
    }

    #[test]
    fn nested_default_placeholder_resolves() {
        let n = run("base = \"B\"\nv = \"${nope:x-${base}-${ALSO:z}}!\"\n", &[]).unwrap();
        assert_eq!(text(&n, "v"), "x-B-z!");
    }

    #[test]
    fn escape_yields_literal() {
        let n = run("v = \"a $${b} c\"\nw = \"cost $5 $\"\n", &[]).unwrap();
        assert_eq!(text(&n, "v"), "a ${b} c");
        assert_eq!(text(&n, "w"), "cost $5 $");
    }

    #[test]
    fn unresolved_without_default_names_leaf_and_name() {
        let (key, name) = unresolved("[db]\nurl = \"x${HOST}y\"\n", &[]);
        assert_eq!((key.as_str(), name.as_str()), ("db.url", "HOST"));
    }

    #[test]
    fn referenced_leaf_is_the_key_when_its_placeholder_fails() {
        let (key, name) = unresolved("a = \"${b}\"\nb = \"${MISSING}\"\n", &[]);
        assert_eq!((key.as_str(), name.as_str()), ("b", "MISSING"));
    }

    #[test]
    fn cycle_names_keys() {
        match run("a = \"${b}\"\nb = \"${a}\"\n", &[]).unwrap_err() {
            ConfigError::PlaceholderCycle { keys } => assert_eq!(keys, ["a", "b"]),
            other => panic!("{other:?}"),
        }
        match run("a = \"${a}\"\n", &[]).unwrap_err() {
            ConfigError::PlaceholderCycle { keys } => assert_eq!(keys, ["a"]),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn depth_over_32_is_placeholder_cycle() {
        let chain = |n: usize| {
            let mut s = String::new();
            for i in 0..n {
                s.push_str(&format!("k{i:02} = \"${{k{:02}}}\"\n", i + 1));
            }
            s.push_str(&format!("k{n:02} = \"end\"\n"));
            s
        };
        let n = run(&chain(30), &[]).unwrap();
        assert_eq!(text(&n, "k00"), "end");
        assert!(matches!(
            run(&chain(40), &[]).unwrap_err(),
            ConfigError::PlaceholderCycle { .. }
        ));
    }

    #[test]
    fn wide_fanout_does_not_explode() {
        let mut s = String::from("k00 = \"x\"\n");
        for i in 1..30 {
            s.push_str(&format!(
                "k{i:02} = \"${{k{:02}}}${{k{:02}}}\"\n",
                i - 1,
                i - 1
            ));
        }
        // Would be 2^29 expansions without memoization (and the output would be huge); the
        // lengths double, so keep the depth small enough to hold the result.
        let s: String = s.lines().take(18).collect::<Vec<_>>().join("\n");
        let n = run(&s, &[]).unwrap();
        assert_eq!(text(&n, "k17").len(), 1 << 17);
    }

    #[test]
    fn resolved_value_coerces_at_bind() {
        #[derive(serde::Deserialize)]
        struct S {
            port: u16,
        }
        let c = Config::from_str("base = 9000\n[s]\nport = \"${base}\"\n").unwrap();
        assert_eq!(c.get::<S>("s").unwrap().port, 9000);
    }

    #[test]
    fn scalars_use_textual_form_and_tables_fail() {
        let n = run("f = 1.5\nb = true\nv = \"${f}/${b}\"\n", &[]).unwrap();
        assert_eq!(text(&n, "v"), "1.5/true");
        assert_eq!(unresolved("t = { a = 1 }\nv = \"${t}\"\n", &[]).1, "t");
        assert_eq!(unresolved("l = [1]\nv = \"${l}\"\n", &[]).1, "l");
    }

    #[test]
    fn result_keeps_leaf_origin() {
        let n = run("base = \"b\"\n\nv = \"${base}\"\n", &[]).unwrap();
        assert_eq!(leaf(&n, "v").origin, Origin::Inline { line: 3 });
    }

    #[test]
    fn placeholders_resolve_in_array_elements() {
        let n = run("b = \"x\"\nl = [\"${b}\", \"y\"]\n", &[]).unwrap();
        assert_eq!(text(&n, "l.0"), "x");
    }

    #[test]
    fn placeholder_in_imported_file() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("application.toml"),
            "oxbloom.config.import = [\"db.toml\"]\nhost = \"h1\"\n",
        )
        .unwrap();
        std::fs::write(
            d.path().join("db.toml"),
            "[db]\nurl = \"pg://${host}/${NAME}\"\n",
        )
        .unwrap();
        let vars = env(&[("NAME", "app")]);
        let mut root = load(
            d.path(),
            vec![],
            &vars,
            "APP",
            &[],
            &[],
            crate::loader::DEFAULT_MAX_FILE_SIZE,
        )
        .unwrap()
        .root;
        resolve(&mut root, &vars).unwrap();
        assert_eq!(text(&root, "db.url"), "pg://h1/app");
    }

    #[test]
    fn placeholder_in_env_and_cli_values() {
        let d = tempfile::tempdir().unwrap();
        let vars = env(&[("APP_A", "${base}"), ("X", "ex")]);
        let args = vec!["--b=${X}".to_string()];
        let mut root = load(
            d.path(),
            vec![],
            &vars,
            "APP",
            &args,
            &[],
            crate::loader::DEFAULT_MAX_FILE_SIZE,
        )
        .unwrap()
        .root;
        let mut base = parse("base = \"B\"\n");
        base.merge(root);
        root = base;
        resolve(&mut root, &vars).unwrap();
        // Environment and command-line values are taken literally.
        assert_eq!(text(&root, "a"), "${base}");
        assert_eq!(text(&root, "b"), "${X}");
    }

    #[test]
    fn literal_dollar_brace_without_escape_is_unresolved_error() {
        let (key, name) = unresolved("[s]\ntpl = \"echo ${HOME}\"\n", &[]);
        assert_eq!((key.as_str(), name.as_str()), ("s.tpl", "HOME"));
        let err = run("[s]\ntpl = \"echo ${HOME\"\n", &[]).unwrap_err();
        let shown = format!("{err} {err:?}");
        assert!(shown.contains("s.tpl"), "{shown}");
    }

    #[test]
    fn unterminated_never_echoes_text_after_colon_or_beyond_32() {
        let (key, name) = unresolved("v = \"${pw:hunter2-secret\"", &[]);
        assert_eq!((key.as_str(), name.as_str()), ("v", "pw"));
        let long = "n".repeat(100);
        let (_, name) = unresolved(&format!("v = \"${{{long}\""), &[]);
        assert_eq!(name, format!("{}…", "n".repeat(32)));
        let (_, name) = unresolved("v = \"${a${b}}\"", &[("b", "x")]);
        assert_eq!(name, "");
        let (_, name) = unresolved("v = \"${}\"", &[]);
        assert_eq!(name, "");
    }

    #[test]
    fn secret_value_never_in_error() {
        const MARK: &str = "S3CR3T-MARK";
        let cases = [
            // Marker in the referenced key; the failing placeholder is elsewhere.
            format!("pw = \"{MARK}\"\nv = \"${{pw}}${{MISSING}}\"\n"),
            // Marker in the default of an unterminated placeholder.
            format!("v = \"${{MISSING:{MARK}\"\n"),
            // Marker in a referenced value that is part of a cycle.
            format!("a = \"{MARK}${{b}}\"\nb = \"${{a}}\"\n"),
            // Marker in a table target.
            format!("t = {{ k = \"{MARK}\" }}\nv = \"${{t}}\"\n"),
            format!("v = \"${{a${{MISSING:{MARK}}}}}\"\n"),
        ];
        for case in cases {
            let err = run(&case, &[]).unwrap_err();
            let shown = format!("{err} {err:?}");
            assert!(!shown.contains(MARK), "{case}: {shown}");
        }
        // A default that is used is not an error; an unused marker default stays out of errors.
        let err = run(&format!("v = \"${{X:{MARK}}}${{Y}}\""), &[]).unwrap_err();
        assert!(!format!("{err} {err:?}").contains(MARK));
    }

    // ---- fix round 1 ----------------------------------------------------------------------

    fn in_small_stack<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
        std::thread::Builder::new()
            .stack_size(256 * 1024)
            .spawn(f)
            .unwrap()
            .join()
            .unwrap()
    }

    fn nested_defaults(n: usize) -> String {
        format!("v = \"{}{}\"\n", "${x:".repeat(n), "}".repeat(n))
    }

    #[test]
    fn non_ascii_text_names_and_defaults_do_not_panic() {
        let n = run(
            "v = \"${a:é}\"\nw = \"é${a:ü}ß\"\nx = \"${DB_PW:pässword}\"\n",
            &[],
        )
        .unwrap();
        assert_eq!(text(&n, "v"), "é");
        assert_eq!(text(&n, "w"), "éüß");
        assert_eq!(text(&n, "x"), "pässword");
        let n = run("v = \"${é:d}ü${ñ}\"\n", &[("ñ", "n")]).unwrap();
        assert_eq!(text(&n, "v"), "dün");
        let n = run("v = \"${a:é$${ü}}\"\n", &[]).unwrap();
        assert_eq!(text(&n, "v"), "é${ü}");
        assert_eq!(unresolved("v = \"${a:é\"", &[]).1, "a");
        assert_eq!(unresolved("v = \"${pässword\"", &[]).1, "pässword");
        assert_eq!(unresolved("v = \"${pässword}\"", &[]).1, "pässword");
        let (_, name) = unresolved(&format!("v = \"${{{}\"", "é".repeat(40)), &[]);
        assert_eq!(name, format!("{}…", "é".repeat(32)));
    }

    #[test]
    fn doubling_chain_27_levels_is_placeholder_too_large() {
        let mut s = String::from("a00 = \"xxxxxxxx\"\n");
        for i in 1..=27 {
            s.push_str(&format!(
                "a{i:02} = \"${{a{:02}}}${{a{:02}}}\"\n",
                i - 1,
                i - 1
            ));
        }
        let start = std::time::Instant::now();
        let err = run(&s, &[]).unwrap_err();
        assert!(
            matches!(err, ConfigError::PlaceholderTooLarge { .. }),
            "{err:?}"
        );
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn fan_out_2000_references_to_1mb_value_is_too_large() {
        let s = format!(
            "b = \"{}\"\na = \"{}\"\n",
            "x".repeat(1 << 20),
            "${b}".repeat(2000)
        );
        let start = std::time::Instant::now();
        match run(&s, &[]).unwrap_err() {
            ConfigError::PlaceholderTooLarge { key } => assert_eq!(key, "a"),
            other => panic!("{other:?}"),
        }
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn exactly_at_leaf_limit_is_accepted() {
        let ok = format!(
            "b = \"{}\"\nv = \"${{b}}abcd\"\n",
            "x".repeat((1 << 20) - 4)
        );
        assert_eq!(text(&run(&ok, &[]).unwrap(), "v").len(), 1 << 20);
        let over = format!(
            "b = \"{}\"\nv = \"${{b}}abcde\"\n",
            "x".repeat((1 << 20) - 4)
        );
        assert!(matches!(
            run(&over, &[]).unwrap_err(),
            ConfigError::PlaceholderTooLarge { .. }
        ));
    }

    #[test]
    fn total_budget_is_per_resolve_call() {
        let doc = |leaves: usize| {
            let mut s = format!("big = \"{}\"\n", "x".repeat(1 << 20));
            for i in 0..leaves {
                s.push_str(&format!("l{i:02} = \"${{big}}\"\n"));
            }
            s
        };
        // 12 MiB per call is within the 16 MiB budget, twice in a row.
        assert!(run(&doc(12), &[]).is_ok());
        assert!(run(&doc(12), &[]).is_ok());
        assert!(matches!(
            run(&doc(17), &[]).unwrap_err(),
            ConfigError::PlaceholderTooLarge { .. }
        ));
    }

    #[test]
    fn placeholder_too_large_has_fixed_text_and_no_value() {
        let err = ConfigError::PlaceholderTooLarge {
            key: "db.url".into(),
        };
        assert_eq!(
            err.to_string(),
            "placeholder expansion in configuration key `db.url` is too large"
        );
        let secret = format!(
            "pw = \"S3CR3T-MARK{}\"\nv = \"${{pw}}${{pw}}\"\n",
            "x".repeat(600_000)
        );
        let err = run(&secret, &[]).unwrap_err();
        assert!(matches!(err, ConfigError::PlaceholderTooLarge { .. }));
        assert!(!format!("{err} {err:?}").contains("S3CR3T"));
    }

    #[test]
    fn nested_defaults_100000_deep_is_placeholder_cycle() {
        in_small_stack(|| match run(&nested_defaults(100_000), &[]).unwrap_err() {
            ConfigError::PlaceholderCycle { keys } => assert_eq!(keys, ["v"]),
            other => panic!("{other:?}"),
        });
    }

    #[test]
    fn nested_defaults_32_deep_is_accepted() {
        in_small_stack(|| {
            let n = run(&nested_defaults(32), &[]).unwrap();
            assert_eq!(text(&n, "v"), "");
        });
    }

    #[test]
    fn nested_defaults_33_deep_is_placeholder_cycle() {
        in_small_stack(|| {
            assert!(matches!(
                run(&nested_defaults(33), &[]).unwrap_err(),
                ConfigError::PlaceholderCycle { .. }
            ));
        });
    }

    #[test]
    fn quoted_dotted_key_and_nested_table_do_not_share_a_cache_entry() {
        let n = run("\"a.b\" = \"${X:one}\"\n[a]\nb = \"${Y:two}\"\n", &[]).unwrap();
        let root = n.as_table().unwrap();
        let get = |node: &Node| match &node.value {
            NodeValue::String(s) => s.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(get(root.get("a.b").unwrap()), "one");
        assert_eq!(
            get(root.get("a").unwrap().as_table().unwrap().get("b").unwrap()),
            "two"
        );
    }

    // ---- fix round 2: one shared depth limit ----------------------------------------------

    /// `leaves` leaves; each wraps a reference to the next in `defaults` nested defaults.
    fn mixed(leaves: usize, defaults: usize) -> String {
        let mut s = String::new();
        for i in 0..leaves - 1 {
            s.push_str(&format!(
                "k{i:02} = \"{}${{k{:02}}}{}\"\n",
                "${x:".repeat(defaults),
                i + 1,
                "}".repeat(defaults)
            ));
        }
        s.push_str(&format!("k{:02} = \"end\"\nv = \"${{k00}}\"\n", leaves - 1));
        s
    }

    fn chain_of(leaves: usize) -> String {
        let mut s = String::new();
        for i in 0..leaves - 1 {
            s.push_str(&format!("k{i:02} = \"${{k{:02}}}\"\n", i + 1));
        }
        s.push_str(&format!("k{:02} = \"end\"\n", leaves - 1));
        s
    }

    fn outcome(text: String) -> Result<String, ConfigError> {
        in_small_stack(move || run(&text, &[]).map(|n| self::text(&n, "k00")))
    }

    #[test]
    fn mixed_leaf_and_default_nesting_31x32_is_placeholder_cycle() {
        for (leaves, defaults) in [(31, 32), (2, 32), (8, 32)] {
            let err = outcome(mixed(leaves, defaults)).unwrap_err();
            assert!(
                matches!(err, ConfigError::PlaceholderCycle { .. }),
                "{err:?}"
            );
        }
    }

    #[test]
    fn mixed_nesting_exactly_at_the_shared_limit_is_accepted() {
        // Shared depth at the innermost leaf: (leaves - 1) * (defaults + 1).
        assert_eq!(outcome(mixed(4, 7)).unwrap(), "end");
        assert_eq!(outcome(mixed(5, 7)).unwrap(), "end"); // 4 * 8 = 32
        assert_eq!(outcome(mixed(2, 31)).unwrap(), "end"); // 32
        assert!(matches!(
            outcome(mixed(2, 32)).unwrap_err(), // 33
            ConfigError::PlaceholderCycle { .. }
        ));
        assert!(matches!(
            outcome(mixed(3, 16)).unwrap_err(), // 2 * 17 = 34
            ConfigError::PlaceholderCycle { .. }
        ));
    }

    #[test]
    fn leaf_chain_32_deep_still_works() {
        assert_eq!(outcome(chain_of(33)).unwrap(), "end");
        assert!(matches!(
            outcome(chain_of(34)).unwrap_err(),
            ConfigError::PlaceholderCycle { .. }
        ));
    }

    #[test]
    fn default_nesting_32_deep_still_works() {
        in_small_stack(|| {
            let n = run(&nested_defaults(32), &[]).unwrap();
            assert_eq!(text(&n, "v"), "");
        });
    }

    // ---- final review: env and command-line values are literal ---------------------------

    fn build_with(
        files: &[(&str, &str)],
        vars: &[(&str, &str)],
        args: &[&str],
    ) -> Result<Config, ConfigError> {
        let d = tempfile::tempdir().unwrap();
        for (name, text) in files {
            std::fs::write(d.path().join(name), text).unwrap();
        }
        Config::builder()
            .dir(d.path())
            .env_from(env(vars))
            .args(args.iter().copied())
            .build()
    }

    #[test]
    fn env_value_with_placeholder_syntax_is_taken_literally() {
        let c = build_with(&[], &[("APP_DB__PASSWORD", "ab${S3CRETPART}cd")], &[]).unwrap();
        assert_eq!(c.get::<String>("db.password").unwrap(), "ab${S3CRETPART}cd");
        let c = build_with(
            &[("application.toml", "base = \"B\"\n")],
            &[("APP_A", "${base}")],
            &[],
        )
        .unwrap();
        assert_eq!(c.get::<String>("a").unwrap(), "${base}");
    }

    #[test]
    fn cli_value_with_placeholder_syntax_is_taken_literally() {
        let c = build_with(
            &[("application.toml", "base = \"B\"\n")],
            &[("HOME_X", "home")],
            &["--a=${base}", "--b=ab${S3CRETPART}cd", "--c=${HOME_X}"],
        )
        .unwrap();
        assert_eq!(c.get::<String>("a").unwrap(), "${base}");
        assert_eq!(c.get::<String>("b").unwrap(), "ab${S3CRETPART}cd");
        assert_eq!(c.get::<String>("c").unwrap(), "${HOME_X}");
    }

    #[test]
    fn env_value_dollar_dollar_brace_is_not_unescaped() {
        let c = build_with(&[], &[("APP_A", "$${x}")], &["--b=$${y}"]).unwrap();
        assert_eq!(c.get::<String>("a").unwrap(), "$${x}");
        assert_eq!(c.get::<String>("b").unwrap(), "$${y}");
    }

    #[test]
    fn file_value_still_resolves_but_env_overrides_do_not_double_resolve() {
        let c = build_with(
            &[("application.toml", "a = \"${b}\"\nb = \"file\"\n")],
            &[("APP_B", "x")],
            &[],
        )
        .unwrap();
        assert_eq!(c.get::<String>("a").unwrap(), "x");
        // The referenced env leaf is used as is: its own text is not scanned.
        let c = build_with(
            &[("application.toml", "a = \"<${b}>\"\nb = \"file\"\n")],
            &[("APP_B", "${MISSING}"), ("MISSING", "no")],
            &["--c=${a}"],
        )
        .unwrap();
        assert_eq!(c.get::<String>("a").unwrap(), "<${MISSING}>");
        assert_eq!(c.get::<String>("c").unwrap(), "${a}");
    }

    #[test]
    fn terminated_placeholder_name_is_capped_in_errors() {
        let long = "n".repeat(100);
        let (key, name) = unresolved(&format!("v = \"${{{long}}}\""), &[]);
        assert_eq!(key, "v");
        assert_eq!(name, format!("{}…", "n".repeat(32)));
        // Exactly 32 characters: not cut.
        let exact = "m".repeat(32);
        let (_, name) = unresolved(&format!("v = \"${{{exact}}}\""), &[]);
        assert_eq!(name, exact);
        // A table target with a long name is capped too.
        let (_, name) = unresolved(&format!("[{long}]\nk = 1\n[x]\nv = \"${{{long}}}\""), &[]);
        assert_eq!(name, format!("{}…", "n".repeat(32)));
    }
}
