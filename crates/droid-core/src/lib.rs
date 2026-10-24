//! Shared, host-independent configuration types for DROIDBRIDGE.
//!
//! This crate intentionally does not execute APKs or spoof hardware directly.
//! It keeps configurable Android-facing properties separate from detected host
//! capabilities so a profile cannot silently imply unsupported host features.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

/// Android-facing identity properties that a compatible backend may expose.
/// Values are hints, not proof of physical hardware or certification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceProfile {
    pub profile_name: String,
    pub manufacturer: String,
    pub model: String,
    pub android_release: Option<String>,
    pub reported_abi: Vec<String>,
    pub display_width_px: Option<u32>,
    pub display_height_px: Option<u32>,
    pub display_density_dpi: Option<u32>,
    /// Extra backend-specific properties. Never use these to override actual
    /// host safety limits or advertise unsupported CPU/GPU features.
    #[serde(default)]
    pub properties: BTreeMap<String, String>,
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
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.schema_version != 1 {
            return Err(ConfigError::UnsupportedSchema(self.schema_version));
        }
        if self.active_profile.trim().is_empty() {
            return Err(ConfigError::EmptyProfileName);
        }
        Ok(())
    }

    pub fn from_json(input: &str) -> Result<Self, ConfigError> {
        let config: Self =
            serde_json::from_str(input).map_err(|e| ConfigError::InvalidJson(e.to_string()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn to_pretty_json(&self) -> Result<String, ConfigError> {
        self.validate()?;
        serde_json::to_string_pretty(self)
            .map_err(|e| ConfigError::InvalidJson(e.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    InvalidJson(String),
    UnsupportedSchema(u32),
    EmptyProfileName,
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson(message) => write!(f, "invalid configuration: {message}"),
            Self::UnsupportedSchema(version) => {
                write!(f, "unsupported configuration schema version: {version}")
            }
            Self::EmptyProfileName => write!(f, "active profile name cannot be empty"),
        }
    }
}

impl std::error::Error for ConfigError {}

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
        let mut config = BridgeConfig::default();
        config.active_profile = "  ".to_owned();
        assert_eq!(config.validate(), Err(ConfigError::EmptyProfileName));
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let mut config = BridgeConfig::default();
        config.schema_version = 999;
        assert_eq!(
            config.validate(),
            Err(ConfigError::UnsupportedSchema(999))
        );
    }
    #[test]
    fn loads_poco_f6_profile_json() {
        let json = include_str!("../../../profiles/POCO_F6.json");
        let profile: DeviceProfile =
            serde_json::from_str(json).expect("load POCO F6 profile");

        assert_eq!(profile.profile_name, "POCO F6");
        assert_eq!(profile.manufacturer, "Xiaomi");
        assert_eq!(profile.model, "POCO F6");
        assert_eq!(profile.reported_abi, vec!["arm64-v8a"]);
    }
}
