//! Device-profile schema and validation.
//!
//! A profile describes Android-visible identity and display metadata only. It
//! makes no claims about host RAM, storage, CPU, or GPU capabilities.

use crate::errors::{ProfileError, ProfileIdError};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Maximum length of a profile identifier.
pub const MAX_PROFILE_ID_LEN: usize = 64;

/// Check that `id` is a valid stable profile identifier.
///
/// Rules: ASCII letters, digits, `_` and `-` only; must start with a letter or
/// digit; at most [`MAX_PROFILE_ID_LEN`] characters. Identifiers are
/// case-sensitive.
pub fn validate_profile_id(id: &str) -> Result<(), ProfileIdError> {
    if id.trim().is_empty() {
        return Err(ProfileIdError::Empty);
    }

    let mut chars = id.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return Err(ProfileIdError::InvalidStart),
    }
    for c in chars {
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '-') {
            return Err(ProfileIdError::InvalidChar(c));
        }
    }

    if id.len() > MAX_PROFILE_ID_LEN {
        return Err(ProfileIdError::TooLong {
            max: MAX_PROFILE_ID_LEN,
        });
    }
    Ok(())
}

/// Android-facing identity properties that a compatible backend may expose.
/// Values are hints, not proof of physical hardware or certification.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceProfile {
    /// Stable machine identifier used for registry lookup and
    /// `BridgeConfig.active_profile`. Never changes when the display name does.
    ///
    /// `#[serde(default)]` only lets direct deserialization reach validation;
    /// an empty id is always rejected by [`DeviceProfile::validate`], and
    /// [`DeviceProfile::from_json`] reports a missing id as
    /// [`ProfileError::MissingId`].
    #[serde(default)]
    pub id: String,
    /// Human-readable display name. May change without affecting `id`.
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

impl DeviceProfile {
    /// Validate identity and display metadata without making claims about
    /// host RAM, storage, CPU, or GPU capabilities.
    pub fn validate(&self) -> Result<(), ProfileError> {
        match validate_profile_id(&self.id) {
            Ok(()) => {}
            Err(ProfileIdError::Empty) => return Err(ProfileError::EmptyField("id")),
            Err(problem) => {
                return Err(ProfileError::InvalidId {
                    id: self.id.clone(),
                    problem,
                });
            }
        }

        for (field, value) in [
            ("profile_name", self.profile_name.as_str()),
            ("manufacturer", self.manufacturer.as_str()),
            ("model", self.model.as_str()),
        ] {
            if value.trim().is_empty() {
                return Err(ProfileError::EmptyField(field));
            }
        }

        if self
            .android_release
            .as_ref()
            .is_some_and(|v| v.trim().is_empty())
        {
            return Err(ProfileError::EmptyField("android_release"));
        }

        if self.reported_abi.is_empty() {
            return Err(ProfileError::EmptyAbiList);
        }
        if self.reported_abi.iter().any(|abi| abi.trim().is_empty()) {
            return Err(ProfileError::EmptyAbiEntry);
        }

        for (field, value) in [
            ("display_width_px", self.display_width_px),
            ("display_height_px", self.display_height_px),
            ("display_density_dpi", self.display_density_dpi),
        ] {
            if value == Some(0) {
                return Err(ProfileError::ZeroDisplayValue(field));
            }
        }

        Ok(())
    }

    /// Parse and validate a profile from JSON text.
    ///
    /// The JSON must be an object containing an `id` field. A profile without
    /// one is rejected with [`ProfileError::MissingId`]; it is never accepted
    /// silently. Use [`DeviceProfile::from_legacy_json`] to migrate such a
    /// profile explicitly.
    pub fn from_json(input: &str) -> Result<Self, ProfileError> {
        let value = parse_object(input)?;
        if value.get("id").is_none() {
            return Err(ProfileError::MissingId);
        }
        Self::from_value(value)
    }

    /// Parse a legacy profile (written before the `id` field existed) and
    /// assign it `id`.
    ///
    /// This is the only way to load an `id`-less profile. If the JSON already
    /// contains an `id` it must equal `id`, otherwise
    /// [`ProfileError::IdMismatch`] is returned. All other validation is
    /// identical to [`DeviceProfile::from_json`].
    pub fn from_legacy_json(input: &str, id: &str) -> Result<Self, ProfileError> {
        let mut value = parse_object(input)?;
        let Some(object) = value.as_object_mut() else {
            return Err(ProfileError::InvalidJson(
                "expected a JSON object".to_owned(),
            ));
        };

        if let Some(existing) = object.get("id") {
            if existing.as_str() != Some(id) {
                return Err(ProfileError::IdMismatch {
                    supplied: id.to_owned(),
                    found: existing
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| existing.to_string()),
                });
            }
        }
        object.insert("id".to_owned(), Value::String(id.to_owned()));

        Self::from_value(value)
    }

    fn from_value(value: Value) -> Result<Self, ProfileError> {
        let profile: Self = serde_json::from_value(value)
            .map_err(|e| ProfileError::InvalidJson(e.to_string()))?;
        profile.validate()?;
        Ok(profile)
    }
}

fn parse_object(input: &str) -> Result<Value, ProfileError> {
    let value: Value =
        serde_json::from_str(input).map_err(|e| ProfileError::InvalidJson(e.to_string()))?;
    if !value.is_object() {
        return Err(ProfileError::InvalidJson(
            "expected a JSON object".to_owned(),
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    const POCO_JSON: &str = include_str!("../../../profiles/POCO_F6.json");

    fn fixture_value() -> Value {
        serde_json::from_str(POCO_JSON).expect("fixture is valid JSON")
    }

    // ---- Original tests, preserved ----

    #[test]
    fn rejects_blank_profile_manufacturer() {
        let mut profile: DeviceProfile =
            serde_json::from_str(include_str!("../../../profiles/POCO_F6.json"))
                .expect("parse example profile");
        profile.manufacturer = "  ".to_owned();
        assert_eq!(
            profile.validate(),
            Err(ProfileError::EmptyField("manufacturer"))
        );
    }

    #[test]
    fn rejects_empty_abi_list() {
        let mut profile: DeviceProfile =
            serde_json::from_str(include_str!("../../../profiles/POCO_F6.json"))
                .expect("parse example profile");
        profile.reported_abi.clear();
        assert_eq!(profile.validate(), Err(ProfileError::EmptyAbiList));
    }

    #[test]
    fn rejects_zero_display_width() {
        let mut profile: DeviceProfile =
            serde_json::from_str(include_str!("../../../profiles/POCO_F6.json"))
                .expect("parse example profile");
        profile.display_width_px = Some(0);
        assert_eq!(
            profile.validate(),
            Err(ProfileError::ZeroDisplayValue("display_width_px"))
        );
    }

    #[test]
    fn rejects_blank_profile_name() {
        let mut profile = DeviceProfile::from_json(include_str!("../../../profiles/POCO_F6.json"))
            .expect("load example profile");
        profile.profile_name = " ".to_owned();
        assert_eq!(
            profile.validate(),
            Err(ProfileError::EmptyField("profile_name"))
        );
    }

    #[test]
    fn rejects_blank_abi_entry() {
        let mut profile = DeviceProfile::from_json(include_str!("../../../profiles/POCO_F6.json"))
            .expect("load example profile");
        profile.reported_abi = vec!["arm64-v8a".to_owned(), " ".to_owned()];
        assert_eq!(profile.validate(), Err(ProfileError::EmptyAbiEntry));
    }

    #[test]
    fn rejects_zero_display_height_and_density() {
        let mut profile = DeviceProfile::from_json(include_str!("../../../profiles/POCO_F6.json"))
            .expect("load example profile");
        profile.display_height_px = Some(0);
        assert_eq!(
            profile.validate(),
            Err(ProfileError::ZeroDisplayValue("display_height_px"))
        );

        profile.display_height_px = None;
        profile.display_density_dpi = Some(0);
        assert_eq!(
            profile.validate(),
            Err(ProfileError::ZeroDisplayValue("display_density_dpi"))
        );
    }

    #[test]
    fn loads_poco_f6_profile_json() {
        let json = include_str!("../../../profiles/POCO_F6.json");
        let profile = DeviceProfile::from_json(json).expect("load POCO F6 profile");

        assert_eq!(profile.profile_name, "POCO F6");
        assert_eq!(profile.manufacturer, "Xiaomi");
        assert_eq!(profile.model, "POCO F6");
        assert_eq!(profile.reported_abi, vec!["arm64-v8a"]);
    }

    // ---- New tests: stable id ----

    #[test]
    fn fixture_id_is_stable_and_matches_default_active_profile() {
        let profile = DeviceProfile::from_json(POCO_JSON).expect("load fixture");
        assert_eq!(profile.id, "POCO_F6");
        assert_ne!(profile.id, profile.profile_name);
        assert_eq!(
            profile.id,
            crate::config::BridgeConfig::default().active_profile
        );
    }

    #[test]
    fn from_json_rejects_missing_id() {
        let mut value = fixture_value();
        value.as_object_mut().unwrap().remove("id");
        assert_eq!(
            DeviceProfile::from_json(&value.to_string()),
            Err(ProfileError::MissingId)
        );
    }

    #[test]
    fn from_json_rejects_blank_or_empty_id() {
        for bad in ["", "   "] {
            let mut value = fixture_value();
            value["id"] = Value::String(bad.to_owned());
            assert_eq!(
                DeviceProfile::from_json(&value.to_string()),
                Err(ProfileError::EmptyField("id")),
                "id {bad:?}"
            );
        }
    }

    #[test]
    fn from_json_rejects_non_string_id() {
        for bad in [Value::from(5), Value::Null, Value::Bool(true)] {
            let mut value = fixture_value();
            value["id"] = bad;
            assert!(matches!(
                DeviceProfile::from_json(&value.to_string()),
                Err(ProfileError::InvalidJson(_))
            ));
        }
    }

    #[test]
    fn id_validation_rules() {
        assert_eq!(validate_profile_id("POCO_F6"), Ok(()));
        assert_eq!(validate_profile_id("a"), Ok(()));
        assert_eq!(validate_profile_id("pixel-8_pro"), Ok(()));
        assert_eq!(validate_profile_id("9lives"), Ok(()));
        assert_eq!(validate_profile_id(&"a".repeat(MAX_PROFILE_ID_LEN)), Ok(()));

        assert_eq!(validate_profile_id(""), Err(ProfileIdError::Empty));
        assert_eq!(validate_profile_id("  "), Err(ProfileIdError::Empty));
        assert_eq!(
            validate_profile_id("-poco"),
            Err(ProfileIdError::InvalidStart)
        );
        assert_eq!(
            validate_profile_id("_poco"),
            Err(ProfileIdError::InvalidStart)
        );
        assert_eq!(
            validate_profile_id(" poco"),
            Err(ProfileIdError::InvalidStart)
        );
        assert_eq!(
            validate_profile_id("poco f6"),
            Err(ProfileIdError::InvalidChar(' '))
        );
        assert_eq!(
            validate_profile_id("poco/f6"),
            Err(ProfileIdError::InvalidChar('/'))
        );
        assert_eq!(
            validate_profile_id(&"a".repeat(MAX_PROFILE_ID_LEN + 1)),
            Err(ProfileIdError::TooLong {
                max: MAX_PROFILE_ID_LEN
            })
        );
    }

    #[test]
    fn from_json_reports_invalid_id_with_reason() {
        let mut value = fixture_value();
        value["id"] = Value::String("poco f6".to_owned());
        assert_eq!(
            DeviceProfile::from_json(&value.to_string()),
            Err(ProfileError::InvalidId {
                id: "poco f6".to_owned(),
                problem: ProfileIdError::InvalidChar(' '),
            })
        );
    }

    #[test]
    fn from_json_rejects_malformed_input() {
        assert!(matches!(
            DeviceProfile::from_json("not json"),
            Err(ProfileError::InvalidJson(_))
        ));
        assert!(matches!(
            DeviceProfile::from_json("[]"),
            Err(ProfileError::InvalidJson(_))
        ));
        assert!(matches!(
            DeviceProfile::from_json("\"POCO_F6\""),
            Err(ProfileError::InvalidJson(_))
        ));
    }

    #[test]
    fn from_json_rejects_unknown_fields() {
        let mut value = fixture_value();
        value["unexpected"] = Value::from(1);
        assert!(matches!(
            DeviceProfile::from_json(&value.to_string()),
            Err(ProfileError::InvalidJson(_))
        ));
    }

    #[test]
    fn profile_round_trips_through_json() {
        let profile = DeviceProfile::from_json(POCO_JSON).expect("load fixture");
        let json = serde_json::to_string(&profile).expect("serialize");
        let decoded = DeviceProfile::from_json(&json).expect("deserialize");
        assert_eq!(profile, decoded);
    }

    // ---- New tests: legacy migration ----

    #[test]
    fn legacy_json_without_id_is_loaded_only_with_explicit_id() {
        let mut value = fixture_value();
        value.as_object_mut().unwrap().remove("id");
        let legacy = value.to_string();

        assert_eq!(
            DeviceProfile::from_json(&legacy),
            Err(ProfileError::MissingId)
        );

        let migrated =
            DeviceProfile::from_legacy_json(&legacy, "POCO_F6").expect("explicit migration");
        let current = DeviceProfile::from_json(POCO_JSON).expect("load fixture");
        assert_eq!(migrated, current);
    }

    #[test]
    fn legacy_json_accepts_matching_existing_id() {
        let profile =
            DeviceProfile::from_legacy_json(POCO_JSON, "POCO_F6").expect("matching id is fine");
        assert_eq!(profile.id, "POCO_F6");
    }

    #[test]
    fn legacy_json_rejects_conflicting_id() {
        assert_eq!(
            DeviceProfile::from_legacy_json(POCO_JSON, "OTHER"),
            Err(ProfileError::IdMismatch {
                supplied: "OTHER".to_owned(),
                found: "POCO_F6".to_owned(),
            })
        );
    }

    #[test]
    fn legacy_json_rejects_invalid_supplied_id() {
        let mut value = fixture_value();
        value.as_object_mut().unwrap().remove("id");
        assert_eq!(
            DeviceProfile::from_legacy_json(&value.to_string(), "bad id"),
            Err(ProfileError::InvalidId {
                id: "bad id".to_owned(),
                problem: ProfileIdError::InvalidChar(' '),
            })
        );
    }

    #[test]
    fn legacy_json_still_validates_everything_else() {
        let mut value = fixture_value();
        let object = value.as_object_mut().unwrap();
        object.remove("id");
        object.insert("reported_abi".to_owned(), Value::Array(Vec::new()));
        assert_eq!(
            DeviceProfile::from_legacy_json(&value.to_string(), "POCO_F6"),
            Err(ProfileError::EmptyAbiList)
        );

        assert!(matches!(
            DeviceProfile::from_legacy_json("[]", "POCO_F6"),
            Err(ProfileError::InvalidJson(_))
        ));
        assert!(matches!(
            DeviceProfile::from_legacy_json("not json", "POCO_F6"),
            Err(ProfileError::InvalidJson(_))
        ));
    }
}
