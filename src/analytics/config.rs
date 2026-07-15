//! Analytics opt-in config.
//!
//! Resolution order (each step overrides the previous):
//! 1. **Build-time defaults** baked in via [`option_env!`]. The CI release
//!    build (see `.github/workflows/release.yml`) exports the GitHub
//!    Variables `POSTHOG_*` into the `cargo build` step; those values get
//!    embedded as compile-time string literals so shipped binaries phone
//!    home without the player having to set anything.
//! 2. **`analytics.local.toml`** at `repo_root`. Gitignored. Used by local
//!    dev builds to point at the dev PostHog project, or by a packaged
//!    binary that ships a sibling TOML.
//! 3. **Runtime environment variables** listed in [`mod@env`]. Override
//!    anything above, useful for one-off runs, CI smoke tests, or
//!    flipping analytics off (`POSTHOG_ENABLED=false`) without touching
//!    the binary.
//!
//! Missing `api_key` or an explicit `enabled = false` after all overlays
//! produces a [`AnalyticsConfig::disabled`] value; the plugin then skips
//! the worker thread entirely.

use std::{fs, path::Path};

use serde::Deserialize;

/// Default host for the EU PostHog ingestion endpoint.
pub(crate) const DEFAULT_HOST: &str = "https://eu.i.posthog.com";

/// File at the repo root that opts a local checkout into analytics.
pub(crate) const FILE_NAME: &str = "analytics.local.toml";

/// Environment-variable names. Listed in one place so future drift between
/// the loader and the doc lives next to the constants instead of in two
/// places.
pub(crate) mod env {
    pub(crate) const API_KEY: &str = "POSTHOG_API_KEY";
    pub(crate) const HOST: &str = "POSTHOG_HOST";
    pub(crate) const ENVIRONMENT: &str = "POSTHOG_ENVIRONMENT";
    pub(crate) const ENABLED: &str = "POSTHOG_ENABLED";
    pub(crate) const DISABLE_GEOIP: &str = "POSTHOG_DISABLE_GEOIP";
}

/// Compile-time fallbacks. Resolved by [`option_env!`] when `cargo build`
/// runs, so whichever `POSTHOG_*` values are exported into the build's
/// environment get baked into the binary as `&'static str` literals.
/// Empty/unset at build time means "no default", runtime env vars and
/// the TOML can still enable analytics on top.
mod build {
    pub(super) const API_KEY: Option<&str> = option_env!("POSTHOG_API_KEY");
    pub(super) const HOST: Option<&str> = option_env!("POSTHOG_HOST");
    pub(super) const ENVIRONMENT: Option<&str> = option_env!("POSTHOG_ENVIRONMENT");
    pub(super) const ENABLED: Option<&str> = option_env!("POSTHOG_ENABLED");
    pub(super) const DISABLE_GEOIP: Option<&str> = option_env!("POSTHOG_DISABLE_GEOIP");
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Environment {
    Dev,
    Ci,
    /// Auto-deployed builds from `main`. Sits between `Dev` and `Prod` so
    /// dashboards can isolate bleeding-edge regressions from stable
    /// releases without polluting either pool.
    BleedingEdge,
    Prod,
}

impl Environment {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Dev => "dev",
            Self::Ci => "ci",
            Self::BleedingEdge => "bleeding-edge",
            Self::Prod => "prod",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "dev" | "development" => Some(Self::Dev),
            "ci" => Some(Self::Ci),
            "bleeding-edge" | "bleeding_edge" | "bleedingedge" | "edge" => Some(Self::BleedingEdge),
            "prod" | "production" => Some(Self::Prod),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct AnalyticsConfig {
    pub(crate) enabled: bool,
    pub(crate) api_key: Option<String>,
    pub(crate) host: String,
    pub(crate) environment: Environment,
    pub(crate) disable_geoip: bool,
}

impl AnalyticsConfig {
    pub(crate) fn disabled() -> Self {
        Self {
            enabled: false,
            api_key: None,
            host: DEFAULT_HOST.to_owned(),
            environment: Environment::Dev,
            disable_geoip: true,
        }
    }

    /// Resolve config by overlaying, in order: build-time defaults baked
    /// in via [`option_env!`], the local TOML at `repo_root`, and the
    /// runtime environment variables. Each step overrides the previous.
    pub(crate) fn load(repo_root: &Path) -> Self {
        let mut resolved = build_defaults();
        resolved = overlay_file(resolved, repo_root);
        resolved = overlay_env(resolved);

        // Honor opt-out even when the API key is set.
        let enabled = resolved.enabled.unwrap_or(false)
            && resolved
                .api_key
                .as_deref()
                .is_some_and(|key| !key.is_empty());
        if !enabled {
            return Self::disabled();
        }

        Self {
            enabled: true,
            api_key: resolved.api_key,
            host: resolved.host.unwrap_or_else(|| DEFAULT_HOST.to_owned()),
            environment: resolved.environment.unwrap_or(Environment::Dev),
            disable_geoip: resolved.disable_geoip.unwrap_or(true),
        }
    }
}

#[derive(Default, Debug, Deserialize)]
struct RawConfig {
    api_key: Option<String>,
    host: Option<String>,
    environment: Option<String>,
    enabled: Option<bool>,
    disable_geoip: Option<bool>,
}

#[derive(Default, Debug)]
struct Resolved {
    api_key: Option<String>,
    host: Option<String>,
    environment: Option<Environment>,
    enabled: Option<bool>,
    disable_geoip: Option<bool>,
}

impl From<RawConfig> for Resolved {
    fn from(raw: RawConfig) -> Self {
        Self {
            api_key: raw.api_key.filter(|key| !key.is_empty()),
            host: raw.host.filter(|host| !host.is_empty()),
            environment: raw.environment.and_then(|env| Environment::parse(&env)),
            enabled: raw.enabled,
            disable_geoip: raw.disable_geoip,
        }
    }
}

fn build_defaults() -> Resolved {
    Resolved {
        api_key: trim_static(build::API_KEY).map(str::to_owned),
        host: trim_static(build::HOST).map(str::to_owned),
        environment: trim_static(build::ENVIRONMENT).and_then(Environment::parse),
        enabled: trim_static(build::ENABLED).and_then(parse_bool),
        disable_geoip: trim_static(build::DISABLE_GEOIP).and_then(parse_bool),
    }
}

fn overlay_file(mut base: Resolved, repo_root: &Path) -> Resolved {
    let path = repo_root.join(FILE_NAME);
    let raw = match fs::read_to_string(&path) {
        Ok(contents) => match toml::from_str::<RawConfig>(&contents) {
            Ok(raw) => raw,
            Err(error) => {
                eprintln!("ignoring malformed {}: {error}", path.display());
                return base;
            }
        },
        Err(_) => return base,
    };
    let from_file: Resolved = raw.into();
    if from_file.api_key.is_some() {
        base.api_key = from_file.api_key;
    }
    if from_file.host.is_some() {
        base.host = from_file.host;
    }
    if from_file.environment.is_some() {
        base.environment = from_file.environment;
    }
    if from_file.enabled.is_some() {
        base.enabled = from_file.enabled;
    }
    if from_file.disable_geoip.is_some() {
        base.disable_geoip = from_file.disable_geoip;
    }
    base
}

fn overlay_env(mut base: Resolved) -> Resolved {
    if let Some(value) = env_string(env::API_KEY) {
        base.api_key = Some(value);
    }
    if let Some(value) = env_string(env::HOST) {
        base.host = Some(value);
    }
    if let Some(value) = std::env::var(env::ENVIRONMENT)
        .ok()
        .and_then(|value| Environment::parse(&value))
    {
        base.environment = Some(value);
    }
    if let Some(value) = env_bool(env::ENABLED) {
        base.enabled = Some(value);
    }
    if let Some(value) = env_bool(env::DISABLE_GEOIP) {
        base.disable_geoip = Some(value);
    }
    base
}

fn env_string(key: &str) -> Option<String> {
    let value = std::env::var(key).ok()?.trim().to_owned();
    if value.is_empty() { None } else { Some(value) }
}

fn env_bool(key: &str) -> Option<bool> {
    let raw = std::env::var(key).ok()?;
    parse_bool(raw.trim())
}

fn parse_bool(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Some(true),
        "0" | "false" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// Trim a build-time `option_env!` value and drop empties so an
/// unset-or-blank variable doesn't show up as a config override.
fn trim_static(value: Option<&'static str>) -> Option<&'static str> {
    let value = value?.trim();
    if value.is_empty() { None } else { Some(value) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // `std::env` is process-global. Serialize tests that mutate env vars so
    // they don't observe each other's writes.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        keys: Vec<&'static str>,
    }

    impl EnvGuard {
        fn capture() -> Self {
            Self {
                keys: vec![
                    env::API_KEY,
                    env::HOST,
                    env::ENVIRONMENT,
                    env::ENABLED,
                    env::DISABLE_GEOIP,
                ],
            }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for key in &self.keys {
                // SAFETY: tests are serialized by ENV_LOCK so concurrent
                // env mutation is excluded.
                unsafe { std::env::remove_var(key) };
            }
        }
    }

    fn set(key: &str, value: &str) {
        // SAFETY: tests are serialized by ENV_LOCK so concurrent env
        // mutation is excluded.
        unsafe { std::env::set_var(key, value) };
    }

    fn temp_root() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("game-analytics-config-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn missing_file_and_env_yields_disabled() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::capture();

        let root = temp_root();
        let config = AnalyticsConfig::load(&root);
        assert!(!config.enabled);
        assert!(config.api_key.is_none());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn file_alone_enables_with_defaults() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::capture();

        let root = temp_root();
        fs::write(
            root.join(FILE_NAME),
            "api_key = \"phc_abc\"\nenabled = true\n",
        )
        .unwrap();
        let config = AnalyticsConfig::load(&root);
        assert!(config.enabled);
        assert_eq!(config.api_key.as_deref(), Some("phc_abc"));
        assert_eq!(config.host, DEFAULT_HOST);
        assert_eq!(config.environment, Environment::Dev);
        assert!(config.disable_geoip);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn env_overrides_file_per_field() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::capture();

        let root = temp_root();
        fs::write(
            root.join(FILE_NAME),
            "api_key = \"phc_file\"\nhost = \"https://file.example\"\nenvironment = \"dev\"\nenabled = true\n",
        )
        .unwrap();
        set(env::API_KEY, "phc_env");
        set(env::ENVIRONMENT, "ci");

        let config = AnalyticsConfig::load(&root);
        assert!(config.enabled);
        assert_eq!(config.api_key.as_deref(), Some("phc_env"));
        // Host wasn't overridden, so file wins.
        assert_eq!(config.host, "https://file.example");
        assert_eq!(config.environment, Environment::Ci);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn env_can_disable_even_when_file_enables() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::capture();

        let root = temp_root();
        fs::write(
            root.join(FILE_NAME),
            "api_key = \"phc_abc\"\nenabled = true\n",
        )
        .unwrap();
        set(env::ENABLED, "false");
        let config = AnalyticsConfig::load(&root);
        assert!(!config.enabled);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn missing_api_key_disables_even_when_enabled() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _guard = EnvGuard::capture();

        let root = temp_root();
        fs::write(root.join(FILE_NAME), "enabled = true\n").unwrap();
        let config = AnalyticsConfig::load(&root);
        assert!(!config.enabled);

        let _ = fs::remove_dir_all(root);
    }
}
