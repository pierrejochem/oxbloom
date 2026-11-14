use std::sync::Arc;

use serde::Serialize;

/// Build facts of the application, each of which may be absent.
///
/// Absent fields are left out of the JSON. Create one in the application with
/// [`build_info!`](crate::build_info), which reads the variables that
/// [`build::emit`](crate::build::emit) sets.
///
/// # Examples
///
/// ```
/// use oxbloom_actuator::BuildInfo;
///
/// let info = BuildInfo::new(Some("app"), Some("1.2.3"), None, None, None);
/// assert_eq!(info.version.as_deref(), Some("1.2.3"));
/// assert_eq!(serde_json::to_value(&info).unwrap()["name"], "app");
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct BuildInfo {
    /// The crate name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The crate version.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The short git commit sha.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_sha: Option<String>,
    /// The git branch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git_branch: Option<String>,
    /// The build time, in seconds since the Unix epoch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_time: Option<String>,
}

impl BuildInfo {
    /// Creates a build info from the five values, in the order name, version, git sha, git
    /// branch and build time.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::BuildInfo;
    ///
    /// let info = BuildInfo::new(Some("app"), None, Some("abc123"), None, None);
    /// assert_eq!(info.git_sha.as_deref(), Some("abc123"));
    /// assert!(info.version.is_none());
    /// ```
    pub fn new(
        name: Option<&str>,
        version: Option<&str>,
        git_sha: Option<&str>,
        git_branch: Option<&str>,
        build_time: Option<&str>,
    ) -> Self {
        let own = |v: Option<&str>| v.map(str::to_string);
        Self {
            name: own(name),
            version: own(version),
            git_sha: own(git_sha),
            git_branch: own(git_branch),
            build_time: own(build_time),
        }
    }
}

/// Builds a [`BuildInfo`] from the variables that [`build::emit`](crate::build::emit) set
/// while compiling the calling crate.
///
/// The macro expands in the calling crate, so `option_env!` reads the application's
/// environment and not this library's.
///
/// # Examples
///
/// ```
/// let info = oxbloom_actuator::build_info!();
/// // Without a build script the values are absent.
/// assert!(info.git_sha.is_none());
/// ```
#[macro_export]
macro_rules! build_info {
    () => {
        $crate::BuildInfo::new(
            option_env!("OXBLOOM_BUILD_NAME"),
            option_env!("OXBLOOM_BUILD_VERSION"),
            option_env!("OXBLOOM_GIT_SHA"),
            option_env!("OXBLOOM_GIT_BRANCH"),
            option_env!("OXBLOOM_BUILD_TIME"),
        )
    };
}

/// The platform the application runs on.
///
/// # Examples
///
/// ```
/// use oxbloom_actuator::Platform;
///
/// let platform = Platform::detect_from(|k| (k == "KUBERNETES_SERVICE_HOST").then(|| "10.0.0.1".into()));
/// assert_eq!(platform, Platform::Kubernetes);
/// ```
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Platform {
    /// Running in a Kubernetes pod.
    Kubernetes,
    /// No known platform.
    None,
}

impl Platform {
    /// Detects the platform from the process environment.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::Platform;
    ///
    /// let _platform = Platform::detect();
    /// ```
    pub fn detect() -> Self {
        Self::detect_from(|key| std::env::var(key).ok())
    }

    /// Detects the platform using `get` to read environment variables.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::Platform;
    ///
    /// assert_eq!(Platform::detect_from(|_| None), Platform::None);
    /// ```
    pub fn detect_from(get: impl Fn(&str) -> Option<String>) -> Self {
        if get("KUBERNETES_SERVICE_HOST").is_some() {
            Platform::Kubernetes
        } else {
            Platform::None
        }
    }

    /// Returns the wire name: `kubernetes` or `none`.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::Platform;
    ///
    /// assert_eq!(Platform::Kubernetes.as_str(), "kubernetes");
    /// ```
    pub fn as_str(&self) -> &'static str {
        match self {
            Platform::Kubernetes => "kubernetes",
            Platform::None => "none",
        }
    }
}

/// Adds entries to the `/info` document.
///
/// # Examples
///
/// ```
/// use oxbloom_actuator::InfoContributor;
///
/// struct Owner;
///
/// impl InfoContributor for Owner {
///     fn contribute(&self, out: &mut serde_json::Map<String, serde_json::Value>) {
///         out.insert("owner".into(), "platform-team".into());
///     }
/// }
///
/// let mut out = serde_json::Map::new();
/// Owner.contribute(&mut out);
/// assert_eq!(out["owner"], "platform-team");
/// ```
pub trait InfoContributor: Send + Sync {
    /// Adds this contributor's entries to `out`.
    fn contribute(&self, out: &mut serde_json::Map<String, serde_json::Value>);
}

/// The data behind the `/info` endpoint.
///
/// # Examples
///
/// ```
/// use oxbloom_actuator::{BuildInfo, Info, Platform};
///
/// let info = Info {
///     build: BuildInfo::new(Some("app"), None, None, None, None),
///     platform: Platform::None,
///     contributors: vec![],
/// };
/// assert_eq!(info.render()["build"]["name"], "app");
/// ```
pub struct Info {
    /// Build and git facts.
    pub build: BuildInfo,
    /// The detected platform.
    pub platform: Platform,
    /// Extra contributors, applied in order after the build and platform entries.
    pub contributors: Vec<Arc<dyn InfoContributor>>,
}

impl Info {
    /// Renders the `/info` JSON: `build`, `platform`, then every contributor's entries.
    ///
    /// # Examples
    ///
    /// ```
    /// use oxbloom_actuator::{BuildInfo, Info, Platform};
    ///
    /// let info = Info { build: BuildInfo::default(), platform: Platform::Kubernetes, contributors: vec![] };
    /// assert_eq!(info.render()["platform"], "kubernetes");
    /// ```
    pub fn render(&self) -> serde_json::Value {
        let mut out = serde_json::Map::new();
        out.insert(
            "build".into(),
            serde_json::to_value(&self.build).unwrap_or_default(),
        );
        out.insert("platform".into(), self.platform.as_str().into());
        for contributor in &self.contributors {
            contributor.contribute(&mut out);
        }
        serde_json::Value::Object(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_is_kubernetes_when_host_var_set() {
        let platform = Platform::detect_from(|k| {
            (k == "KUBERNETES_SERVICE_HOST").then(|| "10.0.0.1".to_string())
        });
        assert_eq!(platform, Platform::Kubernetes);
    }

    #[test]
    fn platform_is_none_otherwise() {
        assert_eq!(Platform::detect_from(|_| None), Platform::None);
    }

    #[test]
    fn build_info_omits_missing_fields_in_json() {
        let info = BuildInfo::new(Some("app"), Some("1.0.0"), None, None, None);
        let json = serde_json::to_value(&info).unwrap();
        assert_eq!(json, serde_json::json!({"name": "app", "version": "1.0.0"}));
        assert!(!json.to_string().contains("null"));
    }

    #[test]
    fn render_includes_build_platform_and_contributors() {
        struct Extra;
        impl InfoContributor for Extra {
            fn contribute(&self, out: &mut serde_json::Map<String, serde_json::Value>) {
                out.insert("custom".into(), serde_json::json!({"a": 1}));
            }
        }
        let info = Info {
            build: BuildInfo::new(Some("app"), None, None, None, None),
            platform: Platform::Kubernetes,
            contributors: vec![std::sync::Arc::new(Extra)],
        };
        let json = info.render();
        assert_eq!(json["build"]["name"], "app");
        assert_eq!(json["platform"], "kubernetes");
        assert_eq!(json["custom"]["a"], 1);
    }
}
