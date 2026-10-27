//! Runtime contracts and the first project-owned execution engine.
//!
//! The engine submodule interprets a limited integer-only DEX instruction set.
//! The Waydroid submodule is a separate adapter to an external Android runtime.
//! Neither component currently provides complete Android application compatibility.

use crate::profile::validate_profile_id;
use std::fmt;
use std::path::{Path, PathBuf};

pub mod engine;
pub mod waydroid;

/// Whether a backend capability has been verified for the current host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CapabilitySupport {
    /// The backend has verified that the capability is supported.
    Supported,
    /// The backend has verified that the capability is not supported.
    Unsupported,
    /// The backend has not checked the capability or cannot determine it.
    #[default]
    Unknown,
}

/// Runtime features reported by a concrete backend.
///
/// Unknown is the default on purpose: an adapter must not imply that graphics
/// acceleration or a CPU ABI is supported without evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RuntimeCapabilities {
    pub apk_installation: CapabilitySupport,
    pub apk_launch: CapabilitySupport,
    pub arm64_native_execution: CapabilitySupport,
    pub hardware_accelerated_graphics: CapabilitySupport,
}

/// Stable identity and human-readable name for a runtime adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeBackendInfo {
    /// Stable machine identifier, e.g. `waydroid` or `custom_vm`.
    pub id: String,
    /// Name suitable for display in a frontend.
    pub display_name: String,
}

/// Result of checking whether the backend can be used on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendStatus {
    /// The backend is installed and its required configuration is present.
    Ready,
    /// The runtime is not installed or could not be found.
    NotInstalled,
    /// The runtime is installed, but its session/container is not running.
    NotRunning,
    /// The current host cannot use this backend.
    UnsupportedHost,
    /// The runtime exists but needs user configuration before launch.
    NeedsConfiguration,
    /// The check failed for a backend-specific reason.
    Unavailable { reason: String },
}

/// A validated request to install (if needed) and launch an APK in a runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchRequest {
    apk_path: PathBuf,
    package_name: String,
    profile_id: String,
}

impl LaunchRequest {
    /// Build a launch request after basic input validation.
    ///
    /// This does not check that the file exists, inspect the APK, install it,
    /// or guarantee that the selected backend can run it.
    pub fn new(
        apk_path: impl Into<PathBuf>,
        package_name: impl Into<String>,
        profile_id: impl Into<String>,
    ) -> Result<Self, RuntimeError> {
        let apk_path = apk_path.into();
        let package_name = package_name.into();
        let profile_id = profile_id.into();

        if apk_path.as_os_str().is_empty() {
            return Err(RuntimeError::InvalidLaunchRequest {
                field: "apk_path",
                reason: "path cannot be empty".to_owned(),
            });
        }
        if package_name.trim().is_empty() {
            return Err(RuntimeError::InvalidLaunchRequest {
                field: "package_name",
                reason: "package name cannot be empty".to_owned(),
            });
        }
        validate_profile_id(&profile_id).map_err(|problem| RuntimeError::InvalidLaunchRequest {
            field: "profile_id",
            reason: problem.to_string(),
        })?;

        Ok(Self {
            apk_path,
            package_name,
            profile_id,
        })
    }

    pub fn apk_path(&self) -> &Path {
        &self.apk_path
    }

    pub fn package_name(&self) -> &str {
        &self.package_name
    }

    pub fn profile_id(&self) -> &str {
        &self.profile_id
    }
}

/// Operation named in a typed runtime error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeOperation {
    Probe,
    Launch,
    PollSession,
    StopSession,
}

/// Failures returned by runtime adapters and sessions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeError {
    InvalidLaunchRequest {
        field: &'static str,
        reason: String,
    },
    BackendUnavailable {
        backend_id: String,
        reason: String,
    },
    UnsupportedOperation {
        backend_id: String,
        operation: RuntimeOperation,
    },
    OperationFailed {
        backend_id: String,
        operation: RuntimeOperation,
        message: String,
    },
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLaunchRequest { field, reason } => {
                write!(f, "invalid launch request field {field}: {reason}")
            }
            Self::BackendUnavailable { backend_id, reason } => {
                write!(f, "runtime backend {backend_id:?} is unavailable: {reason}")
            }
            Self::UnsupportedOperation {
                backend_id,
                operation,
            } => write!(
                f,
                "runtime backend {backend_id:?} does not support {operation:?}"
            ),
            Self::OperationFailed {
                backend_id,
                operation,
                message,
            } => write!(
                f,
                "runtime backend {backend_id:?} failed during {operation:?}: {message}"
            ),
        }
    }
}

impl std::error::Error for RuntimeError {}

/// Observable state of a launched application session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeSessionState {
    /// The launch command was accepted, but the app process has not appeared yet.
    Starting,
    Running,
    Exited {
        exit_code: Option<i32>,
    },
    Failed {
        reason: String,
    },
}

/// Handle used to observe and stop one launched app session.
pub trait RuntimeSession: Send {
    fn package_name(&self) -> &str;

    /// Return the latest known state. Implementations may poll the underlying
    /// runtime as part of this call.
    fn state(&mut self) -> Result<RuntimeSessionState, RuntimeError>;

    /// Request a graceful stop. Backend-specific force-stop behavior is out of
    /// scope for this core contract.
    fn stop(&mut self) -> Result<(), RuntimeError>;
}

/// Adapter contract for a separately provisioned Android runtime.
///
/// Implementations are expected to report unknown capabilities honestly and
/// return typed errors instead of panicking when runtime operations fail.
pub trait RuntimeBackend: Send + Sync {
    fn info(&self) -> &RuntimeBackendInfo;

    fn capabilities(&self) -> RuntimeCapabilities;

    /// Check installation and host/configuration readiness. A problem that is
    /// merely an unavailable status should be represented in the returned
    /// value; unexpected adapter failures may be returned as an error.
    fn probe(&self) -> Result<BackendStatus, RuntimeError>;

    /// Install the APK if necessary and launch the requested package, or return
    /// a typed error. APK inspection and policy checks remain the caller's job.
    fn launch(&self, request: &LaunchRequest) -> Result<Box<dyn RuntimeSession>, RuntimeError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_default_to_unknown_instead_of_claiming_support() {
        assert_eq!(
            RuntimeCapabilities::default(),
            RuntimeCapabilities {
                apk_installation: CapabilitySupport::Unknown,
                apk_launch: CapabilitySupport::Unknown,
                arm64_native_execution: CapabilitySupport::Unknown,
                hardware_accelerated_graphics: CapabilitySupport::Unknown,
            }
        );
    }

    #[test]
    fn launch_request_accepts_nonempty_path_and_valid_profile_id() {
        let request = LaunchRequest::new("game.apk", "com.example.game", "POCO_F6")
            .expect("valid launch request");

        assert_eq!(request.apk_path(), Path::new("game.apk"));
        assert_eq!(request.package_name(), "com.example.game");
        assert_eq!(request.profile_id(), "POCO_F6");
    }

    #[test]
    fn launch_request_rejects_empty_path() {
        assert!(matches!(
            LaunchRequest::new("", "com.example.game", "POCO_F6"),
            Err(RuntimeError::InvalidLaunchRequest {
                field: "apk_path",
                ..
            })
        ));
    }

    #[test]
    fn launch_request_rejects_empty_package_name() {
        assert!(matches!(
            LaunchRequest::new("game.apk", "  ", "POCO_F6"),
            Err(RuntimeError::InvalidLaunchRequest {
                field: "package_name",
                ..
            })
        ));
    }

    #[test]
    fn launch_request_rejects_invalid_profile_id() {
        assert!(matches!(
            LaunchRequest::new("game.apk", "com.example.game", "POCO F6"),
            Err(RuntimeError::InvalidLaunchRequest {
                field: "profile_id",
                ..
            })
        ));
    }
}
