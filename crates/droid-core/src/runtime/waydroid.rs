//! Linux adapter for a user-installed Waydroid runtime.
//!
//! The adapter uses the documented waydroid CLI and never invokes a host shell
//! for commands. It does not install Waydroid itself or initialize images.
//! The user must set up Waydroid and start its session first.

use super::{
    BackendStatus, CapabilitySupport, LaunchRequest, RuntimeBackend, RuntimeBackendInfo,
    RuntimeCapabilities, RuntimeError, RuntimeOperation, RuntimeSession, RuntimeSessionState,
};
use std::ffi::OsString;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

const DEFAULT_STARTUP_TIMEOUT: Duration = Duration::from_secs(30);

/// Selected properties reported by the Android guest running in Waydroid.
///
/// This describes the guest system, not a host-side ART library. Android API
/// level 21 and later use ART for app execution, so ART availability is inferred
/// from the reported API level rather than claimed from an independent binary probe.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AndroidRuntimeInfo {
    pub android_release: Option<String>,
    pub api_level: Option<u32>,
    pub build_fingerprint: Option<String>,
    pub supported_abis: Vec<String>,
    pub zygote_mode: Option<String>,
    pub jit_enabled: Option<bool>,
    pub boot_completed: Option<bool>,
}

impl AndroidRuntimeInfo {
    /// Whether the guest API level implies the ART runtime family.
    ///
    /// None means that the API level could not be read.
    pub fn art_expected(&self) -> Option<bool> {
        self.api_level.map(|api_level| api_level >= 21)
    }
}

/// Adapter for an existing Waydroid installation on Linux.
///
/// By default the executable is resolved as waydroid through the process PATH.
/// Tests and custom installations can provide an explicit executable path
/// with WaydroidBackend::new.
#[derive(Debug, Clone)]
pub struct WaydroidBackend {
    executable: PathBuf,
    info: RuntimeBackendInfo,
    startup_timeout: Duration,
}

impl Default for WaydroidBackend {
    fn default() -> Self {
        Self::new("waydroid")
    }
}

impl WaydroidBackend {
    /// Create an adapter using the supplied Waydroid executable path or name.
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            info: RuntimeBackendInfo {
                id: "waydroid".to_owned(),
                display_name: "Waydroid".to_owned(),
            },
            startup_timeout: DEFAULT_STARTUP_TIMEOUT,
        }
    }

    /// Set how long the adapter waits for a newly launched app process to appear.
    ///
    /// This only bounds the app startup observation window. CLI calls
    /// themselves are synchronous and rely on Waydroid to return.
    pub fn with_startup_timeout(mut self, timeout: Duration) -> Self {
        self.startup_timeout = timeout;
        self
    }

    fn command_error(&self, operation: RuntimeOperation, message: String) -> RuntimeError {
        RuntimeError::OperationFailed {
            backend_id: self.info.id.clone(),
            operation,
            message,
        }
    }

    /// Read a small, explicit set of properties from the running Android guest.
    ///
    /// Waydroid supplies the Android userspace (including ART and framework
    /// services). DROIDBRIDGE does not embed or replace ART itself.
    pub fn android_runtime_info(&self) -> Result<AndroidRuntimeInfo, RuntimeError> {
        match self.probe()? {
            BackendStatus::Ready => {}
            status => {
                return Err(RuntimeError::BackendUnavailable {
                    backend_id: self.info.id.clone(),
                    reason: format!(
                        "Android guest properties require a ready Waydroid session; current status: {status:?}"
                    ),
                });
            }
        }

        let release = self.get_android_property("ro.build.version.release")?;
        let api_level = self
            .get_android_property("ro.build.version.sdk")?
            .and_then(|value| value.parse::<u32>().ok());
        let fingerprint = self.get_android_property("ro.build.fingerprint")?;
        let abis = self.get_android_property("ro.product.cpu.abilist")?;
        let zygote_mode = self.get_android_property("ro.zygote")?;
        let jit_enabled =
            parse_android_bool(self.get_android_property("dalvik.vm.usejit")?.as_deref());
        let boot_completed =
            parse_android_bool(self.get_android_property("sys.boot_completed")?.as_deref());

        Ok(AndroidRuntimeInfo {
            android_release: release,
            api_level,
            build_fingerprint: fingerprint,
            supported_abis: abis
                .unwrap_or_default()
                .split(',')
                .map(str::trim)
                .filter(|abi| !abi.is_empty())
                .map(str::to_owned)
                .collect(),
            zygote_mode,
            jit_enabled,
            boot_completed,
        })
    }

    fn get_android_property(&self, property: &str) -> Result<Option<String>, RuntimeError> {
        let output = self.run(
            RuntimeOperation::Probe,
            [
                OsString::from("shell"),
                OsString::from("getprop"),
                OsString::from(property),
            ],
        )?;
        if !output.status.success() {
            return Err(self.command_error(
                RuntimeOperation::Probe,
                format!(
                    "Waydroid could not read Android property {property:?}: {}",
                    output_message(&output)
                ),
            ));
        }

        let value = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        Ok((!value.is_empty() && value != "(null)").then_some(value))
    }

    fn run(
        &self,
        operation: RuntimeOperation,
        args: impl IntoIterator<Item = OsString>,
    ) -> Result<Output, RuntimeError> {
        Command::new(&self.executable)
            .args(args)
            .output()
            .map_err(|error| self.command_error(operation, error.to_string()))
    }
}

impl RuntimeBackend for WaydroidBackend {
    fn info(&self) -> &RuntimeBackendInfo {
        &self.info
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities {
            // Waydroid documents the install and launch commands. This is a
            // statement about the adapter's operations, not app compatibility.
            apk_installation: CapabilitySupport::Supported,
            apk_launch: CapabilitySupport::Supported,
            // These depend on the host, Waydroid image, drivers, and app ABI.
            arm64_native_execution: CapabilitySupport::Unknown,
            hardware_accelerated_graphics: CapabilitySupport::Unknown,
        }
    }

    fn probe(&self) -> Result<BackendStatus, RuntimeError> {
        if !cfg!(target_os = "linux") {
            return Ok(BackendStatus::UnsupportedHost);
        }

        let output = match Command::new(&self.executable).arg("status").output() {
            Ok(output) => output,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(BackendStatus::NotInstalled);
            }
            Err(error) => {
                return Err(self.command_error(RuntimeOperation::Probe, error.to_string()));
            }
        };

        let details = output_message(&output);
        if !output.status.success() {
            return Ok(BackendStatus::Unavailable {
                reason: if details.is_empty() {
                    format!("Waydroid status exited with {}", output.status)
                } else {
                    details
                },
            });
        }

        Ok(
            parse_status_output(&details).unwrap_or_else(|| BackendStatus::Unavailable {
                reason:
                    "could not determine Waydroid session/container status from its status command"
                        .to_owned(),
            }),
        )
    }

    fn launch(&self, request: &LaunchRequest) -> Result<Box<dyn RuntimeSession>, RuntimeError> {
        if !valid_package_name(request.package_name()) {
            return Err(RuntimeError::InvalidLaunchRequest {
                field: "package_name",
                reason: "expected a dotted Android package name containing only ASCII letters, digits, or underscores".to_owned(),
            });
        }

        let metadata = fs::metadata(request.apk_path()).map_err(|error| {
            self.command_error(
                RuntimeOperation::Launch,
                format!(
                    "cannot access APK path {}: {error}",
                    request.apk_path().display()
                ),
            )
        })?;
        if !metadata.is_file() {
            return Err(self.command_error(
                RuntimeOperation::Launch,
                format!(
                    "APK path is not a regular file: {}",
                    request.apk_path().display()
                ),
            ));
        }

        match self.probe()? {
            BackendStatus::Ready => {}
            BackendStatus::NotInstalled => {
                return Err(RuntimeError::BackendUnavailable {
                    backend_id: self.info.id.clone(),
                    reason: "Waydroid was not found; install and initialize it separately"
                        .to_owned(),
                });
            }
            BackendStatus::NotRunning => {
                return Err(RuntimeError::BackendUnavailable {
                    backend_id: self.info.id.clone(),
                    reason: "Waydroid is installed but not running; start the user session with waydroid session start first".to_owned(),
                });
            }
            BackendStatus::UnsupportedHost => {
                return Err(RuntimeError::UnsupportedOperation {
                    backend_id: self.info.id.clone(),
                    operation: RuntimeOperation::Launch,
                });
            }
            BackendStatus::NeedsConfiguration => {
                return Err(RuntimeError::BackendUnavailable {
                    backend_id: self.info.id.clone(),
                    reason: "Waydroid needs initialization/configuration before launching apps"
                        .to_owned(),
                });
            }
            BackendStatus::Unavailable { reason } => {
                return Err(RuntimeError::BackendUnavailable {
                    backend_id: self.info.id.clone(),
                    reason,
                });
            }
        }

        let install_args = vec![
            OsString::from("app"),
            OsString::from("install"),
            request.apk_path().as_os_str().to_owned(),
        ];
        let install = self.run(RuntimeOperation::Launch, install_args)?;
        if !install.status.success() {
            return Err(self.command_error(
                RuntimeOperation::Launch,
                format!(
                    "Waydroid failed to install {}: {}",
                    request.apk_path().display(),
                    output_message(&install)
                ),
            ));
        }

        let launch_args = vec![
            OsString::from("app"),
            OsString::from("launch"),
            OsString::from(request.package_name()),
        ];
        let launch = self.run(RuntimeOperation::Launch, launch_args)?;
        if !launch.status.success() {
            return Err(self.command_error(
                RuntimeOperation::Launch,
                format!(
                    "Waydroid failed to launch package {:?}: {}",
                    request.package_name(),
                    output_message(&launch)
                ),
            ));
        }

        Ok(Box::new(WaydroidSession {
            executable: self.executable.clone(),
            package_name: request.package_name().to_owned(),
            startup_timeout: self.startup_timeout,
            started_at: Instant::now(),
            was_running: false,
            stopped: false,
        }))
    }
}

fn parse_android_bool(value: Option<&str>) -> Option<bool> {
    match value?.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "y" | "yes" => Some(true),
        "0" | "false" | "n" | "no" => Some(false),
        _ => None,
    }
}

fn parse_status_output(output: &str) -> Option<BackendStatus> {
    let lower = output.to_ascii_lowercase();

    if lower.contains("session: running") && lower.contains("container: running") {
        Some(BackendStatus::Ready)
    } else if lower.contains("session: stopped") || lower.contains("container: stopped") {
        Some(BackendStatus::NotRunning)
    } else if lower.contains("not initialized")
        || lower.contains("not initialised")
        || lower.contains("initialize waydroid")
        || lower.contains("initialise waydroid")
    {
        Some(BackendStatus::NeedsConfiguration)
    } else {
        None
    }
}

fn valid_package_name(package: &str) -> bool {
    let mut segments = package.split('.');
    let mut count = 0usize;

    for segment in &mut segments {
        count += 1;
        let mut chars = segment.chars();
        let Some(first) = chars.next() else {
            return false;
        };
        if !(first.is_ascii_alphabetic() || first == '_') {
            return false;
        }
        if !chars.all(|character| character.is_ascii_alphanumeric() || character == '_') {
            return false;
        }
    }

    count >= 2
}

fn output_message(output: &Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();

    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout,
        (true, false) => stderr,
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

struct WaydroidSession {
    executable: PathBuf,
    package_name: String,
    startup_timeout: Duration,
    started_at: Instant,
    was_running: bool,
    stopped: bool,
}

impl WaydroidSession {
    fn command_error(&self, operation: RuntimeOperation, message: String) -> RuntimeError {
        RuntimeError::OperationFailed {
            backend_id: "waydroid".to_owned(),
            operation,
            message,
        }
    }

    fn no_process_state(&self) -> RuntimeSessionState {
        if self.stopped || self.was_running {
            RuntimeSessionState::Exited { exit_code: None }
        } else if self.started_at.elapsed() < self.startup_timeout {
            RuntimeSessionState::Starting
        } else {
            RuntimeSessionState::Failed {
                reason: format!(
                    "Waydroid accepted the launch request, but process {:?} was not observed within {:?}",
                    self.package_name, self.startup_timeout
                ),
            }
        }
    }
}

impl RuntimeSession for WaydroidSession {
    fn package_name(&self) -> &str {
        &self.package_name
    }

    fn state(&mut self) -> Result<RuntimeSessionState, RuntimeError> {
        if self.stopped {
            return Ok(RuntimeSessionState::Exited { exit_code: None });
        }

        let output = Command::new(&self.executable)
            .args(["shell", "pidof", "-s"])
            .arg(&self.package_name)
            .output()
            .map_err(|error| {
                self.command_error(RuntimeOperation::PollSession, error.to_string())
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        if output.status.success() && !stdout.trim().is_empty() {
            self.was_running = true;
            return Ok(RuntimeSessionState::Running);
        }

        // Android pidof normally exits non-zero with no output when a process
        // doesn't exist. Some shell wrappers normalize that exit status to 0.
        if stdout.trim().is_empty()
            && stderr.trim().is_empty()
            && (output.status.success() || output.status.code() == Some(1))
        {
            return Ok(self.no_process_state());
        }

        Err(self.command_error(
            RuntimeOperation::PollSession,
            format!(
                "could not query process state for {:?}: {}",
                self.package_name,
                output_message(&output)
            ),
        ))
    }

    fn stop(&mut self) -> Result<(), RuntimeError> {
        if self.stopped {
            return Ok(());
        }

        let args = vec![
            OsString::from("shell"),
            OsString::from("am"),
            OsString::from("force-stop"),
            OsString::from(&self.package_name),
        ];
        let output = Command::new(&self.executable)
            .args(args)
            .output()
            .map_err(|error| {
                self.command_error(RuntimeOperation::StopSession, error.to_string())
            })?;

        if !output.status.success() {
            return Err(self.command_error(RuntimeOperation::StopSession, output_message(&output)));
        }

        self.stopped = true;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_runtime_info_infers_art_from_android_api_level() {
        let modern = AndroidRuntimeInfo {
            api_level: Some(34),
            ..Default::default()
        };
        let legacy = AndroidRuntimeInfo {
            api_level: Some(20),
            ..Default::default()
        };

        assert_eq!(modern.art_expected(), Some(true));
        assert_eq!(legacy.art_expected(), Some(false));
        assert_eq!(AndroidRuntimeInfo::default().art_expected(), None);
    }

    #[test]
    fn android_boolean_properties_are_parsed_without_guessing() {
        assert_eq!(parse_android_bool(Some("1")), Some(true));
        assert_eq!(parse_android_bool(Some("true")), Some(true));
        assert_eq!(parse_android_bool(Some("0")), Some(false));
        assert_eq!(parse_android_bool(Some("false")), Some(false));
        assert_eq!(parse_android_bool(Some("unknown")), None);
        assert_eq!(parse_android_bool(None), None);
    }

    #[test]
    fn status_parser_recognizes_running_and_stopped_states() {
        assert_eq!(
            parse_status_output("Session: RUNNING\nContainer: RUNNING\n"),
            Some(BackendStatus::Ready)
        );
        assert_eq!(
            parse_status_output("Session: STOPPED\nContainer: STOPPED\n"),
            Some(BackendStatus::NotRunning)
        );
    }

    #[test]
    fn status_parser_recognizes_uninitialized_runtime() {
        assert_eq!(
            parse_status_output("Waydroid is not initialized"),
            Some(BackendStatus::NeedsConfiguration)
        );
    }

    #[test]
    fn status_parser_does_not_guess_on_unknown_output() {
        assert_eq!(parse_status_output("some unfamiliar output"), None);
    }

    #[test]
    fn package_name_validation_rejects_shell_metacharacters_and_empty_segments() {
        assert!(!valid_package_name("com.example;touch /tmp/pwned"));
        assert!(!valid_package_name("com..game"));
        assert!(!valid_package_name(".com.game"));
        assert!(!valid_package_name("com.game."));
        assert!(!valid_package_name("single"));
        assert!(valid_package_name("com.example.game"));
        assert!(valid_package_name("org.example_app.game2"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn fake_waydroid_exercises_install_launch_poll_and_stop() {
        use std::os::unix::fs::PermissionsExt;
        use std::path::Path;
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        let unique = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "droidbridge-waydroid-test-{}-{unique}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create temp dir");

        struct TempDir(PathBuf);
        impl Drop for TempDir {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _temp = TempDir(root.clone());

        let executable = root.join("waydroid");
        let apk = root.join("game.apk");
        let log = root.join("calls.log");
        let stopped = root.join("stopped.flag");
        fs::write(&apk, b"fixture only; not a real APK").expect("write fixture");
        let quote = |path: &Path| format!("'{}'", path.to_string_lossy().replace('\'', "'\\''"));
        let script = format!(
            r#"#!/bin/sh
printf '%s\n' "$*" >> {log}
if [ "$1" = "status" ]; then
  printf 'Session: RUNNING\nContainer: RUNNING\n'
  exit 0
fi
if [ "$1" = "app" ] && [ "$2" = "install" ]; then exit 0; fi
if [ "$1" = "app" ] && [ "$2" = "launch" ]; then exit 0; fi
if [ "$1" = "shell" ] && [ "$2" = "getprop" ]; then
  case "$3" in
    ro.build.version.release) printf '13\n' ;;
    ro.build.version.sdk) printf '33\n' ;;
    ro.build.fingerprint) printf 'waydroid/lineage_x86_64/device:13/test-keys\n' ;;
    ro.product.cpu.abilist) printf 'x86_64,x86\n' ;;
    ro.zygote) printf 'zygote64_32\n' ;;
    dalvik.vm.usejit) printf 'true\n' ;;
    sys.boot_completed) printf '1\n' ;;
    *) exit 1 ;;
  esac
  exit 0
fi
if [ "$1" = "shell" ] && [ "$2" = "pidof" ]; then
  if [ -f {stopped} ]; then exit 1; fi
  printf '123\n'
  exit 0
fi
if [ "$1" = "shell" ] && [ "$2" = "am" ] && [ "$3" = "force-stop" ]; then
  : > {stopped}
  exit 0
fi
printf 'unexpected fake command: %s\n' "$*" >&2
exit 2
"#,
            log = quote(&log),
            stopped = quote(&stopped)
        );
        fs::write(&executable, script).expect("write fake executable");
        let mut permissions = fs::metadata(&executable)
            .expect("read fake executable metadata")
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&executable, permissions).expect("make fake executable runnable");

        let backend = WaydroidBackend::new(&executable);
        assert_eq!(
            backend.probe().expect("probe fake backend"),
            BackendStatus::Ready
        );
        let runtime_info = backend
            .android_runtime_info()
            .expect("query Android properties from fake guest");
        assert_eq!(runtime_info.android_release.as_deref(), Some("13"));
        assert_eq!(runtime_info.api_level, Some(33));
        assert_eq!(
            runtime_info.build_fingerprint.as_deref(),
            Some("waydroid/lineage_x86_64/device:13/test-keys")
        );
        assert_eq!(runtime_info.supported_abis, vec!["x86_64", "x86"]);
        assert_eq!(runtime_info.zygote_mode.as_deref(), Some("zygote64_32"));
        assert_eq!(runtime_info.jit_enabled, Some(true));
        assert_eq!(runtime_info.boot_completed, Some(true));
        assert_eq!(runtime_info.art_expected(), Some(true));

        let request =
            LaunchRequest::new(&apk, "com.example.game", "POCO_F6").expect("valid launch request");
        let mut session = backend
            .launch(&request)
            .expect("launch through fake backend");

        assert_eq!(session.package_name(), "com.example.game");
        assert_eq!(
            session.state().expect("poll process"),
            RuntimeSessionState::Running
        );
        session.stop().expect("stop fake session");
        assert_eq!(
            session.state().expect("poll stopped process"),
            RuntimeSessionState::Exited { exit_code: None }
        );

        let calls = fs::read_to_string(&log).expect("read fake command log");
        assert!(calls.contains("app install "));
        assert!(calls.contains("app launch com.example.game"));
        assert!(calls.contains("shell pidof -s com.example.game"));
        assert!(calls.contains("shell am force-stop com.example.game"));
    }
}
