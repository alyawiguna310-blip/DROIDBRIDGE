//! Portable, host-independent configuration.

use crate::errors::{ConfigError, ProfileIdError};
use crate::profile::validate_profile_id;
use serde::{Deserialize, Serialize};

/// User-facing performance policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PerformancePreset {
    Efficiency,
    #[default]
    Balanced,
    Performance,
}

/// Preference only; installing or redistributing Google components requires a
/// compatible runtime and appropriate licensing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GoogleServicesMode {
    /// Prefer Play Store and required Google services when supported.
    #[default]
    Play,
    /// Keep Google components optional and externally provisioned.
    Optional,
    /// Do not configure Google components.
    Disabled,
}

/// Settings safe to copy between computers. Host-specific paths and detected
/// hardware intentionally do not belong in this structure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BridgeConfig {
    pub schema_version: u32,
    #[serde(default)]
    pub performance: PerformancePreset,
    #[serde(default)]
    pub google_services: GoogleServicesMode,
    #[serde(default = "default_true")]
    pub portable_settings: bool,
    /// The stable `id` of the active device profile (not its display name).
    pub active_profile: String,
}

fn default_true() -> bool {
    true
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            schema_version: 1,
            performance: PerformancePreset::Balanced,
            google_services: GoogleServicesMode::Play,
            portable_settings: true,
            active_profile: "POCO_F6".to_owned(),
        }
    }
}

impl BridgeConfig {
    /// Validate the portable config before applying it.
    ///
    /// This checks that `active_profile` is a well-formed profile id; whether a
    /// profile with that id exists is decided by
    /// [`crate::ProfileRegistry::apply_config`].
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != 1 {
            return Err(ConfigError::UnsupportedSchema(self.schema_version));
        }
        match validate_profile_id(&self.active_profile) {
            Ok(()) => Ok(()),
            Err(ProfileIdError::Empty) => Err(ConfigError::EmptyProfileName),
            Err(problem) => Err(ConfigError::InvalidActiveProfile {
                id: self.active_profile.clone(),
                problem,
            }),
        }
    }

    pub fn from_json(input: &str) -> Result<Self, ConfigError> {
        let config: Self =
            serde_json::from_str(input).map_err(|e| ConfigError::InvalidJson(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn to_pretty_json(&self) -> Result<String, ConfigError> {
        self.validate()?;
        serde_json::to_string_pretty(self).map_err(|e| ConfigError::InvalidJson(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_initial_project_decisions() {
        let config = BridgeConfig::default();
        assert_eq!(config.performance, PerformancePreset::Balanced);
        assert_eq!(config.google_services, GoogleServicesMode::Play);
        assert!(config.portable_settings);
        assert_eq!(config.active_profile, "POCO_F6");
    }

    #[test]
    fn config_round_trips_json() {
        let config = BridgeConfig::default();
        let json = config.to_pretty_json().expect("serialize config");
        let decoded = BridgeConfig::from_json(&json).expect("deserialize config");
        assert_eq!(config, decoded);
    }

    #[test]
    fn rejects_empty_active_profile() {
        let config = BridgeConfig {
            active_profile: "  ".to_owned(),
            ..BridgeConfig::default()
        };
        assert_eq!(config.validate(), Err(ConfigError::EmptyProfileName));
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let config = BridgeConfig {
            schema_version: 999,
            ..BridgeConfig::default()
        };
        assert_eq!(config.validate(), Err(ConfigError::UnsupportedSchema(999)));
    }

    // ---- New tests ----

    #[test]
    fn default_active_profile_is_a_valid_profile_id() {
        assert_eq!(
            validate_profile_id(&BridgeConfig::default().active_profile),
            Ok(())
        );
    }

    #[test]
    fn rejects_malformed_active_profile_id() {
        let config = BridgeConfig {
            active_profile: "POCO F6".to_owned(),
            ..BridgeConfig::default()
        };
        assert_eq!(
            config.validate(),
            Err(ConfigError::InvalidActiveProfile {
                id: "POCO F6".to_owned(),
                problem: ProfileIdError::InvalidChar(' '),
            })
        );
    }

    #[test]
    fn from_json_rejects_unsupported_schema_version() {
        let json = r#"{"schema_version": 2, "active_profile": "POCO_F6"}"#;
        assert_eq!(
            BridgeConfig::from_json(json),
            Err(ConfigError::UnsupportedSchema(2))
        );
    }

    #[test]
    fn from_json_applies_defaults_for_optional_fields() {
        let json = r#"{"schema_version": 1, "active_profile": "POCO_F6"}"#;
        assert_eq!(
            BridgeConfig::from_json(json).expect("minimal config"),
            BridgeConfig::default()
        );
    }

    #[test]
    fn from_json_rejects_malformed_and_unknown_fields() {
        assert!(matches!(
            BridgeConfig::from_json("not json"),
            Err(ConfigError::InvalidJson(_))
        ));
        let json = r#"{"schema_version": 1, "active_profile": "POCO_F6", "extra": true}"#;
        assert!(matches!(
            BridgeConfig::from_json(json),
            Err(ConfigError::InvalidJson(_))
        ));
    }
}
