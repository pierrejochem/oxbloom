use anyhow::{Result, bail};

/// Crate names (after `-` -> `_`) that clash with generated dependencies or built-in crates.
const RESERVED_CRATES: &[&str] = &[
    "oxbloom_web",
    "oxbloom_core",
    "oxbloom_macros",
    "tokio",
    "test",
    "core",
    "std",
    "alloc",
    "proc_macro",
];

/// Target names Cargo refuses for a binary (it uses them for build directories).
const CARGO_FORBIDDEN: &[&str] = &["build", "deps", "examples", "incremental"];

/// Rust keywords (edition 2024, including reserved ones). The crate name is used as a path
/// in generated code, so none of these can be a project name.
const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "gen", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut",
    "pub", "ref", "return", "self", "static", "struct", "super", "trait", "true", "try", "type",
    "unsafe", "use", "where", "while", "abstract", "become", "box", "do", "final", "macro",
    "override", "priv", "typeof", "unsized", "virtual", "yield",
];

/// Checks `[a-z][a-z0-9_-]*` and that the name is usable as a crate and Cargo target.
pub fn validate_project_name(name: &str) -> Result<()> {
    let mut chars = name.chars();
    let valid = matches!(chars.next(), Some('a'..='z'))
        && chars.all(|c| matches!(c, 'a'..='z' | '0'..='9' | '-' | '_'));
    if !valid {
        bail!(
            "invalid project name {name:?}: use lowercase letters, digits, '-' or '_', starting with a letter"
        );
    }
    let crate_name = name.replace('-', "_");
    if RESERVED_CRATES.contains(&crate_name.as_str())
        || KEYWORDS.contains(&crate_name.as_str())
        || CARGO_FORBIDDEN.contains(&name)
    {
        bail!(
            "project name {name:?} is reserved (Rust keyword, built-in crate, or clashes with a dependency)"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_valid_names() {
        for n in ["demo", "my-app", "a1", "a_b"] {
            assert!(validate_project_name(n).is_ok(), "{n}");
        }
    }

    #[test]
    fn rejects_invalid_names() {
        for n in ["", "1app", "Demo", "a b", "../x", "ünï"] {
            assert!(validate_project_name(n).is_err(), "{n:?}");
        }
    }

    #[test]
    fn rejects_reserved_names() {
        for n in [
            "oxbloom-web",
            "oxbloom-core",
            "tokio",
            "test",
            "core",
            "std",
            "alloc",
            "proc_macro",
        ] {
            let e = validate_project_name(n).unwrap_err().to_string();
            assert!(e.contains("reserved"), "{n}: {e}");
        }
    }

    #[test]
    fn rejects_rust_keywords_and_cargo_forbidden_names() {
        for n in [
            "match",
            "type",
            "async",
            "try",
            "dyn",
            "gen",
            "self",
            "crate",
            "super",
            "fn",
            "build",
            "deps",
            "examples",
            "incremental",
        ] {
            let e = validate_project_name(n).unwrap_err().to_string();
            assert!(e.contains("reserved"), "{n}: {e}");
        }
    }

    #[test]
    fn reserved_check_uses_normalized_crate_name() {
        for n in [
            "oxbloom_web",
            "oxbloom-core",
            "proc-macro",
            "oxbloom_macros",
        ] {
            let e = validate_project_name(n).unwrap_err().to_string();
            assert!(e.contains("reserved"), "{n}: {e}");
        }
    }
}
