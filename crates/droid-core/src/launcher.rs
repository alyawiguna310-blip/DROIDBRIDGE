//! Safe preflight orchestration before handing an APK to a runtime.
//!
//! This module inspects the APK and verifies that its manifest package matches
//! the requested package before allowing a backend to install or launch it. It
//! is not a sandbox and does not guarantee that the app is compatible or safe
//! to execute; concrete runtime behavior remains the backend's responsibility.

use crate::apk::{ApkError, ApkLimits, ApkReport, inspect_file};
use crate::runtime::{
    BackendStatus, CapabilitySupport, LaunchRequest, RuntimeBackend, RuntimeError, RuntimeSession,
};
use std::error::Error;
use std::fmt;

/// Runtime capability required by the verified launch path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequiredRuntimeCapability {
    /// The backend must be able to install the APK.
    ApkInstallation,
    /// The backend must be able to launch a package.
    ApkLaunch,
}

impl fmt::Display for RequiredRuntimeCapability {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApkInstallation => f.write_str("APK installation"),
            Self::ApkLaunch => f.write_str("APK launch"),
        }
    }
}

/// Failure while preparing or handing a validated APK to a runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifiedLaunchError {
    /// APK archive or manifest inspection failed.
    ApkInspection(ApkError),
    /// The requested package did not match the APK manifest.
    PackageMismatch {
        requested_package: String,
        manifest_package: String,
    },
    /// The backend did not report a ready state.
    BackendNotReady { status: BackendStatus },
    /// A required operation was not explicitly reported as supported.
    CapabilityNotConfirmed {
        capability: RequiredRuntimeCapability,
        support: CapabilitySupport,
    },
    /// The runtime backend itself failed.
    Backend(RuntimeError),
}

impl fmt::Display for VerifiedLaunchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ApkInspection(error) => write!(f, "APK inspection failed: {error}"),
            Self::PackageMismatch {
                requested_package,
                manifest_package,
            } => write!(
                f,
                "requested package {requested_package:?} does not match APK manifest package {manifest_package:?}"
            ),
            Self::BackendNotReady { status } => {
                write!(f, "runtime backend is not ready: {status:?}")
            }
            Self::CapabilityNotConfirmed {
                capability,
                support,
            } => write!(
                f,
                "runtime backend has not confirmed {capability}: {support:?}"
            ),
            Self::Backend(error) => write!(f, "runtime backend failed: {error}"),
        }
    }
}

impl Error for VerifiedLaunchError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ApkInspection(error) => Some(error),
            Self::Backend(error) => Some(error),
            _ => None,
        }
    }
}

impl From<ApkError> for VerifiedLaunchError {
    fn from(error: ApkError) -> Self {
        Self::ApkInspection(error)
    }
}

impl From<RuntimeError> for VerifiedLaunchError {
    fn from(error: RuntimeError) -> Self {
        Self::Backend(error)
    }
}

/// An app session plus the APK report produced before launching it.
///
/// The report is retained so a frontend can show package metadata and
/// inspection findings without parsing the APK a second time.
pub struct VerifiedLaunch {
    pub report: ApkReport,
    pub session: Box<dyn RuntimeSession>,
}

/// Inspect an APK and hand it to a ready runtime only after preflight succeeds.
///
/// This function:
/// 1. Inspects the file with the caller's resource limits.
/// 2. Requires the manifest package to match the requested package exactly.
/// 3. Requires the backend probe to return BackendStatus::Ready.
/// 4. Requires installation and launch capabilities to be explicitly reported
///    as CapabilitySupport::Supported; unknown is not treated as support.
/// 5. Calls the backend's launch method only after all checks pass.
///
/// Inspection alone does not guarantee app compatibility, prevent runtime
/// vulnerabilities, or verify graphics/ABI support. The caller remains
/// responsible for policy decisions that depend on the inspection report.
pub fn launch_verified_apk(
    backend: &dyn RuntimeBackend,
    request: &LaunchRequest,
    limits: &ApkLimits,
) -> Result<VerifiedLaunch, VerifiedLaunchError> {
    let report = inspect_file(request.apk_path(), limits)?;
    let manifest_package = report.manifest.package.clone();

    if manifest_package != request.package_name() {
        return Err(VerifiedLaunchError::PackageMismatch {
            requested_package: request.package_name().to_owned(),
            manifest_package,
        });
    }

    let status = backend.probe()?;
    if status != BackendStatus::Ready {
        return Err(VerifiedLaunchError::BackendNotReady { status });
    }

    let capabilities = backend.capabilities();
    require_capability(
        RequiredRuntimeCapability::ApkInstallation,
        capabilities.apk_installation,
    )?;
    require_capability(
        RequiredRuntimeCapability::ApkLaunch,
        capabilities.apk_launch,
    )?;

    let session = backend.launch(request)?;
    Ok(VerifiedLaunch { report, session })
}

fn require_capability(
    capability: RequiredRuntimeCapability,
    support: CapabilitySupport,
) -> Result<(), VerifiedLaunchError> {
    if support == CapabilitySupport::Supported {
        Ok(())
    } else {
        Err(VerifiedLaunchError::CapabilityNotConfirmed {
            capability,
            support,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apk::testutil::{build_zip, minimal_manifest_axml};
    use crate::runtime::{RuntimeBackendInfo, RuntimeCapabilities, RuntimeSessionState};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use zip::CompressionMethod;

    static NEXT_TEMP_ID: AtomicUsize = AtomicUsize::new(0);

    struct TempApk {
        path: PathBuf,
    }

    impl TempApk {
        fn valid(package: &str) -> Self {
            let manifest = minimal_manifest_axml(package);
            let bytes = build_zip(
                &[("AndroidManifest.xml", manifest.as_slice())],
                CompressionMethod::Stored,
            );
            Self::from_bytes(&bytes)
        }

        fn from_bytes(bytes: &[u8]) -> Self {
            let id = NEXT_TEMP_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "droid-core-verified-launch-{}-{id}.apk",
                std::process::id()
            ));
            fs::write(&path, bytes).expect("write temporary APK");
            Self { path }
        }
    }

    impl Drop for TempApk {
        fn drop(&mut self) {
            let _ = fs::remove_file(&self.path);
        }
    }

    struct MockBackend {
        info: RuntimeBackendInfo,
        status: BackendStatus,
        capabilities: RuntimeCapabilities,
        launch_calls: Arc<AtomicUsize>,
    }

    impl MockBackend {
        fn new(status: BackendStatus, capabilities: RuntimeCapabilities) -> Self {
            Self {
                info: RuntimeBackendInfo {
                    id: "mock".to_owned(),
                    display_name: "Mock runtime".to_owned(),
                },
                status,
                capabilities,
                launch_calls: Arc::new(AtomicUsize::new(0)),
            }
        }
    }

    struct MockSession {
        package_name: String,
        stopped: bool,
    }

    impl RuntimeSession for MockSession {
        fn package_name(&self) -> &str {
            &self.package_name
        }

        fn state(&mut self) -> Result<RuntimeSessionState, RuntimeError> {
            Ok(if self.stopped {
                RuntimeSessionState::Exited { exit_code: Some(0) }
            } else {
                RuntimeSessionState::Running
            })
        }

        fn stop(&mut self) -> Result<(), RuntimeError> {
            self.stopped = true;
            Ok(())
        }
    }

    impl RuntimeBackend for MockBackend {
        fn info(&self) -> &RuntimeBackendInfo {
            &self.info
        }

        fn capabilities(&self) -> RuntimeCapabilities {
            self.capabilities
        }

        fn probe(&self) -> Result<BackendStatus, RuntimeError> {
            Ok(self.status.clone())
        }

        fn launch(&self, request: &LaunchRequest) -> Result<Box<dyn RuntimeSession>, RuntimeError> {
            self.launch_calls.fetch_add(1, Ordering::Relaxed);
            Ok(Box::new(MockSession {
                package_name: request.package_name().to_owned(),
                stopped: false,
            }))
        }
    }

    fn supported_capabilities() -> RuntimeCapabilities {
        RuntimeCapabilities {
            apk_installation: CapabilitySupport::Supported,
            apk_launch: CapabilitySupport::Supported,
            arm64_native_execution: CapabilitySupport::Unknown,
            hardware_accelerated_graphics: CapabilitySupport::Unknown,
        }
    }

    fn request(path: PathBuf, package: &str) -> LaunchRequest {
        LaunchRequest::new(path, package, "POCO_F6").expect("valid request")
    }

    fn expect_error(result: Result<VerifiedLaunch, VerifiedLaunchError>) -> VerifiedLaunchError {
        match result {
            Err(error) => error,
            Ok(_) => panic!("expected verified launch to fail"),
        }
    }

    #[test]
    fn launches_only_after_manifest_package_matches_and_backend_is_ready() {
        let apk = TempApk::valid("com.example.app");
        let backend = MockBackend::new(BackendStatus::Ready, supported_capabilities());
        let result = launch_verified_apk(
            &backend,
            &request(apk.path.clone(), "com.example.app"),
            &ApkLimits::default(),
        )
        .expect("verified launch");

        assert_eq!(result.report.manifest.package, "com.example.app");
        assert_eq!(result.session.package_name(), "com.example.app");
        assert_eq!(backend.launch_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn package_mismatch_is_rejected_before_backend_launch() {
        let apk = TempApk::valid("com.example.actual");
        let backend = MockBackend::new(BackendStatus::Ready, supported_capabilities());

        assert_eq!(
            expect_error(launch_verified_apk(
                &backend,
                &request(apk.path.clone(), "com.example.requested"),
                &ApkLimits::default(),
            )),
            VerifiedLaunchError::PackageMismatch {
                requested_package: "com.example.requested".to_owned(),
                manifest_package: "com.example.actual".to_owned(),
            }
        );
        assert_eq!(backend.launch_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_non_ready_backend_is_never_asked_to_launch() {
        let apk = TempApk::valid("com.example.app");
        let backend = MockBackend::new(BackendStatus::NotRunning, supported_capabilities());

        assert_eq!(
            expect_error(launch_verified_apk(
                &backend,
                &request(apk.path.clone(), "com.example.app"),
                &ApkLimits::default(),
            )),
            VerifiedLaunchError::BackendNotReady {
                status: BackendStatus::NotRunning,
            }
        );
        assert_eq!(backend.launch_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn unknown_install_capability_is_not_treated_as_supported() {
        let apk = TempApk::valid("com.example.app");
        let backend = MockBackend::new(
            BackendStatus::Ready,
            RuntimeCapabilities {
                apk_installation: CapabilitySupport::Unknown,
                apk_launch: CapabilitySupport::Supported,
                arm64_native_execution: CapabilitySupport::Unknown,
                hardware_accelerated_graphics: CapabilitySupport::Unknown,
            },
        );

        assert_eq!(
            expect_error(launch_verified_apk(
                &backend,
                &request(apk.path.clone(), "com.example.app"),
                &ApkLimits::default(),
            )),
            VerifiedLaunchError::CapabilityNotConfirmed {
                capability: RequiredRuntimeCapability::ApkInstallation,
                support: CapabilitySupport::Unknown,
            }
        );
        assert_eq!(backend.launch_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn invalid_archive_is_rejected_before_backend_launch() {
        let apk = TempApk::from_bytes(b"not a ZIP archive");
        let backend = MockBackend::new(BackendStatus::Ready, supported_capabilities());

        assert!(matches!(
            launch_verified_apk(
                &backend,
                &request(apk.path.clone(), "com.example.app"),
                &ApkLimits::default(),
            ),
            Err(VerifiedLaunchError::ApkInspection(
                ApkError::InvalidArchive { .. }
            ))
        ));
        assert_eq!(backend.launch_calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn stopping_the_returned_session_is_supported() {
        let apk = TempApk::valid("com.example.app");
        let backend = MockBackend::new(BackendStatus::Ready, supported_capabilities());
        let mut result = launch_verified_apk(
            &backend,
            &request(apk.path.clone(), "com.example.app"),
            &ApkLimits::default(),
        )
        .expect("verified launch");

        result.session.stop().expect("stop session");
        assert_eq!(
            result.session.state(),
            Ok(RuntimeSessionState::Exited { exit_code: Some(0) })
        );
    }
}
