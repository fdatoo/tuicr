//! Source snapshots and the read-only provider runner for preparation and Ask.

mod runner;
mod snapshot;

pub(crate) use runner::run_with_stall;
pub use runner::{AgentEvent, AgentRequest, Cancel, CompanionRuntime, run};
pub(crate) use snapshot::{LocalTarget, is_object_id};
pub use snapshot::{Snapshot, SnapshotRequest};

use std::ffi::{OsStr, OsString};

use crate::config::{self, AppConfig};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Provider {
    #[default]
    Claude,
    Codex,
}

/// Only Claude offers native, scoped file tools without enabling an executor.
pub fn can_read_files(provider: Provider) -> bool {
    provider == Provider::Claude
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub provider: Provider,
    pub ask_model: Option<String>,
    pub reviewer_model: Option<String>,
    pub prepare_on_open: bool,
}

impl Settings {
    pub fn load() -> Result<Self, String> {
        let config = config::load_config()
            .map_err(|error| format!("Could not load review-agent settings: {error}"))?
            .config
            .unwrap_or_default();
        Self::resolve(config, |key| std::env::var_os(key))
    }

    fn resolve(config: AppConfig, env: impl Fn(&str) -> Option<OsString>) -> Result<Self, String> {
        let override_provider = env("TUICR_COMPANION");
        let provider = override_provider
            .as_deref()
            .or_else(|| config.companion.as_deref().map(OsStr::new));
        let provider = match provider.and_then(OsStr::to_str) {
            Some("claude") => Provider::Claude,
            Some("codex") => Provider::Codex,
            _ if provider.is_none() => Provider::default(),
            _ => {
                let key = if override_provider.is_some() {
                    "TUICR_COMPANION"
                } else {
                    "companion"
                };
                return Err(format!("{key} must be either `claude` or `codex`"));
            }
        };
        let ask_model = model(
            env("TUICR_COMPANION_MODEL"),
            config.companion_model,
            "TUICR_COMPANION_MODEL",
            "companion_model",
        )?;
        let reviewer_model = model(
            env("TUICR_REVIEWER_MODEL"),
            config.reviewer_model,
            "TUICR_REVIEWER_MODEL",
            "reviewer_model",
        )?;
        let prepare_on_open = match env("TUICR_PREPARE_ON_OPEN") {
            None => config.prepare_on_open.unwrap_or(true),
            Some(value) if value == "1" || value == "true" => true,
            Some(value) if value == "0" || value == "false" => false,
            Some(_) => {
                return Err("TUICR_PREPARE_ON_OPEN must be `0`, `1`, `false` or `true`".into());
            }
        };
        Ok(Self {
            provider,
            ask_model: ask_model.or_else(|| (provider == Provider::Claude).then(|| "haiku".into())),
            reviewer_model: reviewer_model
                .or_else(|| (provider == Provider::Claude).then(|| "sonnet".into())),
            prepare_on_open,
        })
    }
}

fn model(
    env: Option<OsString>,
    config: Option<String>,
    env_key: &str,
    config_key: &str,
) -> Result<Option<String>, String> {
    let (value, key) = match env {
        Some(value) => (Some(value), env_key),
        None => (config.map(OsString::from), config_key),
    };
    value
        .map(|model| {
            model
                .into_string()
                .ok()
                .filter(|model| !model.trim().is_empty() && !model.chars().any(char::is_control))
                .ok_or_else(|| format!("{key} must be a non-empty UTF-8 model name"))
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn review_agent_settings_defaults_and_overrides() {
        let default = Settings::resolve(AppConfig::default(), |_| None).unwrap();
        assert_eq!(default.provider, Provider::Claude);
        assert_eq!(default.ask_model.as_deref(), Some("haiku"));
        assert_eq!(default.reviewer_model.as_deref(), Some("sonnet"));
        assert!(default.prepare_on_open);

        let config = AppConfig {
            companion: Some("claude".into()),
            companion_model: Some("config-ask".into()),
            reviewer_model: Some("config-reviewer".into()),
            prepare_on_open: Some(false),
            ..Default::default()
        };
        let resolved = Settings::resolve(config.clone(), |_| None).unwrap();
        assert_eq!(resolved.ask_model.as_deref(), Some("config-ask"));
        assert_eq!(resolved.reviewer_model.as_deref(), Some("config-reviewer"));
        assert!(!resolved.prepare_on_open);
        let resolved = Settings::resolve(config, |key| match key {
            "TUICR_COMPANION" => Some("codex".into()),
            "TUICR_COMPANION_MODEL" => Some("env-ask".into()),
            "TUICR_REVIEWER_MODEL" => Some("env-reviewer".into()),
            "TUICR_PREPARE_ON_OPEN" => Some("1".into()),
            _ => None,
        })
        .unwrap();
        assert_eq!(resolved.provider, Provider::Codex);
        assert_eq!(resolved.ask_model.as_deref(), Some("env-ask"));
        assert_eq!(resolved.reviewer_model.as_deref(), Some("env-reviewer"));
        assert!(resolved.prepare_on_open);
    }

    #[test]
    fn review_agent_settings_refuse_invalid_overrides() {
        for (key, value) in [
            ("TUICR_COMPANION", "unknown"),
            ("TUICR_COMPANION_MODEL", " "),
            ("TUICR_REVIEWER_MODEL", ""),
            ("TUICR_PREPARE_ON_OPEN", "yes"),
        ] {
            assert!(
                Settings::resolve(AppConfig::default(), |k| (k == key).then(|| value.into()))
                    .unwrap_err()
                    .contains(key)
            );
        }
        #[cfg(unix)]
        for key in [
            "TUICR_COMPANION",
            "TUICR_COMPANION_MODEL",
            "TUICR_REVIEWER_MODEL",
            "TUICR_PREPARE_ON_OPEN",
        ] {
            use std::os::unix::ffi::OsStringExt;
            assert!(
                Settings::resolve(AppConfig::default(), |k| (k == key)
                    .then(|| OsString::from_vec(vec![0xff])))
                .unwrap_err()
                .contains(key)
            );
        }
    }
}
