//! Shared, host-independent configuration types for DROIDBRIDGE.
//!
//! This crate does not launch APKs end-to-end or spoof hardware directly.
//! It includes an experimental bounded DEX interpreter for scalar integer-like
//! instructions and constrained cross-DEX calls, but not the full Android runtime.
//! Configurable Android-facing properties stay separate from detected host
//! capabilities so profiles cannot silently imply unsupported host features.
//!
//! Modules:
//! - [`config`]: portable, versioned [`BridgeConfig`].
//! - [`profile`]: [`DeviceProfile`] schema (with a stable `id`) and validation.
//! - [`registry`]: [`ProfileRegistry`] for loading, looking up, and selecting profiles.
//! - [`errors`]: typed errors for all of the above.
//! - [`runtime`]: pluggable runtime contracts and the initial Linux Waydroid CLI adapter.
//! - [`runtime::engine`]: project-owned bounded interpreter for an integer-only DEX subset.
//! - [`launcher`]: APK package preflight before handing an app to a runtime backend.
//! - [`dex`]: bounds-checked parser for DEX files (untrusted input).
//! - [`apk`]: APK inspection (archive, binary manifest, DEX discovery); never executes code.

pub mod apk;
pub mod config;
pub mod dex;
pub mod errors;
pub mod launcher;
pub mod profile;
pub mod registry;
pub mod runtime;

pub use config::{BridgeConfig, GoogleServicesMode, PerformancePreset};
pub use errors::{ConfigError, ProfileError, ProfileIdError, RegistryError};
pub use launcher::{
    RequiredRuntimeCapability, VerifiedLaunch, VerifiedLaunchError, launch_verified_apk,
};
pub use profile::{DeviceProfile, MAX_PROFILE_ID_LEN, validate_profile_id};
pub use registry::ProfileRegistry;
pub use runtime::engine::{
    EngineError, EngineLimits, EngineReturn, ExecutionReport, IntegerDexEngine,
    MethodExecutionError, MethodExecutionReport, execute_static_integer_method,
    execute_static_scalar_method, execute_static_scalar_method_in_dex_set,
    execute_static_scalar_method_in_dex_set_at, list_static_integer_methods,
    list_static_scalar_methods,
};
pub use runtime::waydroid::WaydroidBackend;
pub use runtime::{
    BackendStatus, CapabilitySupport, LaunchRequest, RuntimeBackend, RuntimeBackendInfo,
    RuntimeCapabilities, RuntimeError, RuntimeOperation, RuntimeSession, RuntimeSessionState,
};
