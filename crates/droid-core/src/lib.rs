//! Shared, host-independent configuration types for DROIDBRIDGE.
//!
//! This crate intentionally does not execute APKs or spoof hardware directly.
//! It keeps configurable Android-facing properties separate from detected host
//! capabilities so a profile cannot silently imply unsupported host features.
//!
//! Modules:
//! - [`config`]: portable, versioned [`BridgeConfig`].
//! - [`profile`]: [`DeviceProfile`] schema (with a stable `id`) and validation.
//! - [`registry`]: [`ProfileRegistry`] for loading, looking up, and selecting profiles.
//! - [`errors`]: typed errors for all of the above.
//! - [`runtime`]: pluggable runtime contracts and the initial Linux Waydroid CLI adapter.
//! - [`dex`]: bounds-checked parser for DEX files (untrusted input).
//! - [`apk`]: APK inspection (archive, binary manifest, DEX discovery); never executes code.

pub mod apk;
pub mod config;
pub mod dex;
pub mod errors;
pub mod profile;
pub mod registry;
pub mod runtime;

pub use config::{BridgeConfig, GoogleServicesMode, PerformancePreset};
pub use errors::{ConfigError, ProfileError, ProfileIdError, RegistryError};
pub use profile::{DeviceProfile, MAX_PROFILE_ID_LEN, validate_profile_id};
pub use registry::ProfileRegistry;
pub use runtime::{
    BackendStatus, CapabilitySupport, LaunchRequest, RuntimeBackend, RuntimeBackendInfo,
    RuntimeCapabilities, RuntimeError, RuntimeOperation, RuntimeSession, RuntimeSessionState,
};
pub use runtime::waydroid::WaydroidBackend;
