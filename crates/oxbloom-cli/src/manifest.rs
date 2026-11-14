use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Contents of `oxbloom.toml`; the single source for generated files.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub project: Project,
    pub components: Components,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Project {
    pub name: String,
    pub cli_version: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Components {
    pub web: String,
}

impl Manifest {
    pub fn new(name: &str, web: &str) -> Self {
        Self {
            project: Project {
                name: name.to_owned(),
                cli_version: env!("CARGO_PKG_VERSION").to_owned(),
            },
            components: Components {
                web: web.to_owned(),
            },
        }
    }

    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("manifest is always serializable")
    }

    // Not used by `new` yet; the planned `add` command reads existing manifests.
    #[allow(dead_code)]
    pub fn from_toml(s: &str) -> Result<Self> {
        toml::from_str(s).context("invalid oxbloom.toml")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn to_toml_is_exact() {
        assert_eq!(
            Manifest::new("demo", "rest").to_toml(),
            "[project]\nname = \"demo\"\ncli_version = \"0.1.0\"\n\n[components]\nweb = \"rest\"\n"
        );
    }

    #[test]
    fn round_trips() {
        let m = Manifest::new("demo", "rest");
        assert_eq!(Manifest::from_toml(&m.to_toml()).unwrap(), m);
    }

    #[test]
    fn rejects_unknown_project_key() {
        let s = "[project]\nname = \"d\"\ncli_version = \"1\"\nextra = 1\n\n[components]\nweb = \"rest\"\n";
        assert!(Manifest::from_toml(s).is_err());
    }

    #[test]
    fn rejects_unknown_table() {
        let s = "[project]\nname = \"d\"\ncli_version = \"1\"\n\n[components]\nweb = \"rest\"\n\n[extra]\nx = 1\n";
        assert!(Manifest::from_toml(s).is_err());
    }
}
