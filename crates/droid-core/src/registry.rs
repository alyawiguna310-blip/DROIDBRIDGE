//! In-memory registry of validated device profiles.
//!
//! Profiles are keyed by their stable `id`. Listing order is deterministic:
//! ids are sorted by byte order (so `"Alpha"` sorts before `"beta"`).

use crate::config::BridgeConfig;
use crate::errors::RegistryError;
use crate::profile::DeviceProfile;
use std::collections::BTreeMap;
use std::collections::btree_map::Entry;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct ProfileRegistry {
    profiles: BTreeMap<String, DeviceProfile>,
    active: Option<String>,
}

impl ProfileRegistry {
    /// Create an empty registry with no active profile.
    pub fn new() -> Self {
        Self::default()
    }

    /// Validate and register a profile. Fails with
    /// [`RegistryError::DuplicateId`] if its id is already taken; the existing
    /// profile is left untouched. Use [`ProfileRegistry::replace`] to overwrite.
    pub fn register(&mut self, profile: DeviceProfile) -> Result<(), RegistryError> {
        profile.validate()?;
        match self.profiles.entry(profile.id.clone()) {
            Entry::Occupied(_) => Err(RegistryError::DuplicateId(profile.id)),
            Entry::Vacant(slot) => {
                slot.insert(profile);
                Ok(())
            }
        }
    }

    /// Validate a profile and insert it, replacing any profile with the same
    /// id. Returns the previous profile, if there was one. The active profile
    /// selection is unaffected (it is tracked by id).
    pub fn replace(
        &mut self,
        profile: DeviceProfile,
    ) -> Result<Option<DeviceProfile>, RegistryError> {
        profile.validate()?;
        Ok(self.profiles.insert(profile.id.clone(), profile))
    }

    /// Parse profile JSON (see [`DeviceProfile::from_json`]) and register it.
    /// Returns the registered id.
    pub fn register_json(&mut self, json: &str) -> Result<String, RegistryError> {
        let profile = DeviceProfile::from_json(json)?;
        let id = profile.id.clone();
        self.register(profile)?;
        Ok(id)
    }

    /// Migrate a legacy `id`-less profile with an explicit id (see
    /// [`DeviceProfile::from_legacy_json`]) and register it.
    pub fn register_legacy_json(&mut self, json: &str, id: &str) -> Result<String, RegistryError> {
        let profile = DeviceProfile::from_legacy_json(json, id)?;
        let id = profile.id.clone();
        self.register(profile)?;
        Ok(id)
    }

    /// Read a profile JSON file and register it. Returns the registered id.
    pub fn register_file(&mut self, path: impl AsRef<Path>) -> Result<String, RegistryError> {
        let path = path.as_ref();
        let json = fs::read_to_string(path).map_err(|e| RegistryError::Io {
            path: path.to_path_buf(),
            kind: e.kind(),
        })?;
        self.register_json(&json)
    }

    pub fn get(&self, id: &str) -> Option<&DeviceProfile> {
        self.profiles.get(id)
    }

    /// Like [`ProfileRegistry::get`] but returns [`RegistryError::NotFound`].
    pub fn require(&self, id: &str) -> Result<&DeviceProfile, RegistryError> {
        self.get(id)
            .ok_or_else(|| RegistryError::NotFound(id.to_owned()))
    }

    pub fn contains(&self, id: &str) -> bool {
        self.profiles.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.profiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.profiles.is_empty()
    }

    /// Registered ids in deterministic (sorted) order.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.profiles.keys().map(String::as_str)
    }

    /// Registered profiles in deterministic (id-sorted) order.
    pub fn profiles(&self) -> impl Iterator<Item = &DeviceProfile> {
        self.profiles.values()
    }

    /// Select the active profile. Unknown ids are rejected and the previous
    /// selection is kept.
    pub fn set_active(&mut self, id: &str) -> Result<(), RegistryError> {
        if !self.profiles.contains_key(id) {
            return Err(RegistryError::NotFound(id.to_owned()));
        }
        self.active = Some(id.to_owned());
        Ok(())
    }

    pub fn active_id(&self) -> Option<&str> {
        self.active.as_deref()
    }

    pub fn active_profile(&self) -> Option<&DeviceProfile> {
        self.active.as_deref().and_then(|id| self.profiles.get(id))
    }

    /// Validate `config` and make its `active_profile` id the active profile.
    ///
    /// Fails with [`RegistryError::InvalidConfig`] for a bad config, or
    /// [`RegistryError::NotFound`] if no profile has that id. On failure the
    /// previous selection is kept.
    pub fn apply_config(&mut self, config: &BridgeConfig) -> Result<(), RegistryError> {
        config.validate()?;
        self.set_active(&config.active_profile)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::errors::{ConfigError, ProfileError};
    use std::path::PathBuf;

    const POCO_JSON: &str = include_str!("../../../profiles/POCO_F6.json");

    fn sample(id: &str) -> DeviceProfile {
        let mut profile = DeviceProfile::from_json(POCO_JSON).expect("load fixture");
        profile.id = id.to_owned();
        profile
    }

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("droid-core-{}-{tag}.json", std::process::id()))
    }

    #[test]
    fn register_and_lookup() {
        let mut registry = ProfileRegistry::new();
        assert!(registry.is_empty());

        registry.register(sample("a")).expect("register");

        assert_eq!(registry.len(), 1);
        assert!(registry.contains("a"));
        assert_eq!(registry.get("a").map(|p| p.id.as_str()), Some("a"));
        assert_eq!(registry.require("a").map(|p| p.id.as_str()), Ok("a"));
        assert_eq!(registry.get("b"), None);
        assert_eq!(
            registry.require("b"),
            Err(RegistryError::NotFound("b".to_owned()))
        );
    }

    #[test]
    fn rejects_duplicate_id_and_keeps_original() {
        let mut registry = ProfileRegistry::new();
        registry.register(sample("a")).expect("register");

        let mut other = sample("a");
        other.model = "Different".to_owned();
        assert_eq!(
            registry.register(other),
            Err(RegistryError::DuplicateId("a".to_owned()))
        );

        assert_eq!(registry.len(), 1);
        assert_eq!(registry.get("a").expect("still there").model, "POCO F6");
    }

    #[test]
    fn replace_overwrites_and_returns_previous() {
        let mut registry = ProfileRegistry::new();
        assert_eq!(registry.replace(sample("a")), Ok(None));

        let mut updated = sample("a");
        updated.model = "Updated".to_owned();
        let previous = registry.replace(updated).expect("replace");

        assert_eq!(previous.expect("had previous").model, "POCO F6");
        assert_eq!(registry.get("a").expect("present").model, "Updated");
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn replace_keeps_active_selection() {
        let mut registry = ProfileRegistry::new();
        registry.register(sample("a")).expect("register");
        registry.set_active("a").expect("activate");

        let mut updated = sample("a");
        updated.model = "Updated".to_owned();
        registry.replace(updated).expect("replace");

        assert_eq!(registry.active_id(), Some("a"));
        assert_eq!(registry.active_profile().expect("active").model, "Updated");
    }

    #[test]
    fn rejects_invalid_profiles_without_registering() {
        let mut registry = ProfileRegistry::new();

        let mut empty_abi = sample("a");
        empty_abi.reported_abi.clear();
        assert_eq!(
            registry.register(empty_abi.clone()),
            Err(RegistryError::InvalidProfile(ProfileError::EmptyAbiList))
        );
        assert_eq!(
            registry.replace(empty_abi),
            Err(RegistryError::InvalidProfile(ProfileError::EmptyAbiList))
        );

        assert!(matches!(
            registry.register(sample("bad id")),
            Err(RegistryError::InvalidProfile(
                ProfileError::InvalidId { .. }
            ))
        ));
        assert!(registry.is_empty());
    }

    #[test]
    fn listing_is_deterministic_regardless_of_insertion_order() {
        let mut registry = ProfileRegistry::new();
        for id in ["zeta", "Alpha", "beta"] {
            registry.register(sample(id)).expect("register");
        }

        assert_eq!(
            registry.ids().collect::<Vec<_>>(),
            vec!["Alpha", "beta", "zeta"]
        );
        assert_eq!(
            registry
                .profiles()
                .map(|p| p.id.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha", "beta", "zeta"]
        );
    }

    #[test]
    fn register_json_returns_id_and_reports_typed_errors() {
        let mut registry = ProfileRegistry::new();
        assert_eq!(registry.register_json(POCO_JSON), Ok("POCO_F6".to_owned()));
        assert_eq!(
            registry.register_json(POCO_JSON),
            Err(RegistryError::DuplicateId("POCO_F6".to_owned()))
        );
        assert!(matches!(
            registry.register_json("not json"),
            Err(RegistryError::InvalidProfile(ProfileError::InvalidJson(_)))
        ));
        assert_eq!(registry.len(), 1);
    }

    #[test]
    fn register_json_rejects_profile_without_id() {
        let mut value: serde_json::Value = serde_json::from_str(POCO_JSON).expect("fixture json");
        value.as_object_mut().unwrap().remove("id");

        let mut registry = ProfileRegistry::new();
        assert_eq!(
            registry.register_json(&value.to_string()),
            Err(RegistryError::InvalidProfile(ProfileError::MissingId))
        );
        assert!(registry.is_empty());
    }

    #[test]
    fn register_legacy_json_assigns_explicit_id() {
        let mut value: serde_json::Value = serde_json::from_str(POCO_JSON).expect("fixture json");
        value.as_object_mut().unwrap().remove("id");

        let mut registry = ProfileRegistry::new();
        assert_eq!(
            registry.register_legacy_json(&value.to_string(), "POCO_F6"),
            Ok("POCO_F6".to_owned())
        );
        assert!(registry.contains("POCO_F6"));
    }

    #[test]
    fn register_file_loads_the_repository_fixture() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../profiles/POCO_F6.json");
        let mut registry = ProfileRegistry::new();
        assert_eq!(registry.register_file(path), Ok("POCO_F6".to_owned()));
        assert_eq!(
            registry.get("POCO_F6").expect("registered").profile_name,
            "POCO F6"
        );
    }

    #[test]
    fn register_file_reports_missing_file() {
        let path = temp_path("missing");
        let _ = fs::remove_file(&path);

        let mut registry = ProfileRegistry::new();
        match registry.register_file(&path) {
            Err(RegistryError::Io { path: p, kind }) => {
                assert_eq!(p, path);
                assert_eq!(kind, std::io::ErrorKind::NotFound);
            }
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    #[test]
    fn register_file_reports_malformed_file() {
        let path = temp_path("malformed");
        fs::write(&path, "{ not json").expect("write temp file");

        let mut registry = ProfileRegistry::new();
        let result = registry.register_file(&path);
        let _ = fs::remove_file(&path);

        assert!(matches!(
            result,
            Err(RegistryError::InvalidProfile(ProfileError::InvalidJson(_)))
        ));
        assert!(registry.is_empty());
    }

    #[test]
    fn register_file_loads_a_temporary_copy() {
        let path = temp_path("copy");
        fs::write(&path, POCO_JSON).expect("write temp file");

        let mut registry = ProfileRegistry::new();
        let result = registry.register_file(&path);
        let _ = fs::remove_file(&path);

        assert_eq!(result, Ok("POCO_F6".to_owned()));
    }

    #[test]
    fn set_active_rejects_unknown_id_and_keeps_selection() {
        let mut registry = ProfileRegistry::new();
        assert_eq!(registry.active_id(), None);
        assert_eq!(registry.active_profile(), None);

        registry.register(sample("a")).expect("register");
        registry.register(sample("b")).expect("register");

        registry.set_active("a").expect("activate a");
        assert_eq!(registry.active_id(), Some("a"));

        assert_eq!(
            registry.set_active("missing"),
            Err(RegistryError::NotFound("missing".to_owned()))
        );
        assert_eq!(registry.active_id(), Some("a"));

        registry.set_active("b").expect("activate b");
        assert_eq!(registry.active_id(), Some("b"));
    }

    #[test]
    fn default_config_resolves_to_the_poco_f6_fixture() {
        let mut registry = ProfileRegistry::new();
        registry.register_json(POCO_JSON).expect("register fixture");

        registry
            .apply_config(&BridgeConfig::default())
            .expect("default config resolves");

        assert_eq!(registry.active_id(), Some("POCO_F6"));
        assert_eq!(
            registry.active_profile().expect("active").profile_name,
            "POCO F6"
        );
    }

    #[test]
    fn default_config_fails_cleanly_when_profile_is_not_registered() {
        let mut registry = ProfileRegistry::new();
        assert_eq!(
            registry.apply_config(&BridgeConfig::default()),
            Err(RegistryError::NotFound("POCO_F6".to_owned()))
        );
        assert_eq!(registry.active_id(), None);
    }

    #[test]
    fn apply_config_rejects_invalid_config_and_keeps_selection() {
        let mut registry = ProfileRegistry::new();
        registry.register(sample("a")).expect("register");
        registry.set_active("a").expect("activate");

        let mut config = BridgeConfig::default();
        config.schema_version = 5;
        assert_eq!(
            registry.apply_config(&config),
            Err(RegistryError::InvalidConfig(
                ConfigError::UnsupportedSchema(5)
            ))
        );

        let mut config = BridgeConfig::default();
        config.active_profile = "does-not-exist".to_owned();
        assert_eq!(
            registry.apply_config(&config),
            Err(RegistryError::NotFound("does-not-exist".to_owned()))
        );

        assert_eq!(registry.active_id(), Some("a"));
    }

    #[test]
    fn profile_name_is_not_a_lookup_key() {
        let mut registry = ProfileRegistry::new();
        registry.register_json(POCO_JSON).expect("register fixture");

        assert!(registry.get("POCO F6").is_none());
        assert!(registry.get("POCO_F6").is_some());
    }
}
