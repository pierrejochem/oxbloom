use anyhow::{Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Available,
    Planned,
}

/// A selectable framework component and whether it is implemented.
pub struct Component {
    pub name: &'static str,
    pub status: Status,
}

pub const COMPONENTS: &[Component] = &[
    Component {
        name: "web",
        status: Status::Available,
    },
    Component {
        name: "security",
        status: Status::Planned,
    },
    Component {
        name: "database",
        status: Status::Planned,
    },
    Component {
        name: "jobs",
        status: Status::Planned,
    },
    Component {
        name: "ai",
        status: Status::Planned,
    },
    Component {
        name: "tests",
        status: Status::Planned,
    },
    Component {
        name: "frontend",
        status: Status::Planned,
    },
];

/// Errors with `<name> is not implemented yet` unless the component is available.
pub fn require_available(name: &str) -> Result<()> {
    match COMPONENTS.iter().find(|c| c.name == name) {
        Some(c) if c.status == Status::Available => Ok(()),
        Some(_) => bail!("{name} is not implemented yet"),
        None => bail!("unknown component {name:?}"),
    }
}

/// Human-readable component table for `oxbloom list`.
pub fn list_text() -> String {
    let planned: Vec<&str> = COMPONENTS
        .iter()
        .filter(|c| c.status == Status::Planned)
        .map(|c| c.name)
        .collect();
    format!("web   available (rest)\n{}   planned", planned.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn web_is_available() {
        assert!(require_available("web").is_ok());
    }

    #[test]
    fn planned_component_is_rejected() {
        assert_eq!(
            require_available("ai").unwrap_err().to_string(),
            "ai is not implemented yet"
        );
    }

    #[test]
    fn list_text_matches_spec() {
        assert_eq!(
            list_text(),
            "web   available (rest)\nsecurity, database, jobs, ai, tests, frontend   planned"
        );
    }
}
