//! Small command-line entry point for inspecting APKs and using the Waydroid adapter.
//!
//! This CLI is an integration surface for the shared core, not an emulator. A
//! real launch requires a separately installed, initialized, running Waydroid
//! environment. The launch command reports process observation, not app
//! compatibility or graphics correctness.

use droid_core::apk::{ApkLimits, ApkReport, inspect_file};
use droid_core::launcher::launch_verified_apk;
use droid_core::runtime::{
    LaunchRequest, RuntimeBackend, RuntimeSession, RuntimeSessionState, waydroid::WaydroidBackend,
};
use std::error::Error;
use std::ffi::OsString;
use std::io;
use std::path::PathBuf;
use std::process;
use std::thread;
use std::time::{Duration, Instant};

const DEFAULT_PROFILE: &str = "POCO_F6";
const STARTUP_OBSERVATION_TIMEOUT: Duration = Duration::from_secs(35);

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    let command = parse_command(std::env::args_os().skip(1)).map_err(io::Error::other)?;
    match command {
        CliCommand::Help => print_usage(),
        CliCommand::Inspect { apk } => inspect_apk(&apk)?,
        CliCommand::Status => print_status(&WaydroidBackend::default())?,
        CliCommand::Launch {
            apk,
            package,
            profile,
        } => launch_apk(apk, package, profile)?,
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
enum CliCommand {
    Help,
    Inspect {
        apk: PathBuf,
    },
    Status,
    Launch {
        apk: PathBuf,
        package: String,
        profile: String,
    },
}

fn parse_command<I>(arguments: I) -> Result<CliCommand, String>
where
    I: IntoIterator<Item = OsString>,
{
    let arguments: Vec<OsString> = arguments.into_iter().collect();
    let Some(command) = arguments.first().and_then(|arg| arg.to_str()) else {
        return if arguments.is_empty() {
            Ok(CliCommand::Help)
        } else {
            Err("command name must be valid UTF-8".to_owned())
        };
    };

    match command {
        "help" | "--help" | "-h" if arguments.len() == 1 => Ok(CliCommand::Help),
        "inspect" if arguments.len() == 2 => Ok(CliCommand::Inspect {
            apk: PathBuf::from(&arguments[1]),
        }),
        "status" if arguments.len() == 1 => Ok(CliCommand::Status),
        "launch" if (3..=4).contains(&arguments.len()) => {
            let package = arguments[2]
                .to_str()
                .ok_or_else(|| "package name must be valid UTF-8".to_owned())?
                .to_owned();
            let profile = arguments
                .get(3)
                .map(|value| {
                    value
                        .to_str()
                        .map(str::to_owned)
                        .ok_or_else(|| "profile id must be valid UTF-8".to_owned())
                })
                .transpose()?
                .unwrap_or_else(|| DEFAULT_PROFILE.to_owned());
            Ok(CliCommand::Launch {
                apk: PathBuf::from(&arguments[1]),
                package,
                profile,
            })
        }
        "help" | "--help" | "-h" => Err("help accepts no additional arguments".to_owned()),
        "inspect" => Err("usage: droidbridge inspect <apk-path>".to_owned()),
        "status" => Err("usage: droidbridge status".to_owned()),
        "launch" => {
            Err("usage: droidbridge launch <apk-path> <package-name> [profile-id]".to_owned())
        }
        _ => Err(format!(
            "unknown command {command:?}; run 'droidbridge help'"
        )),
    }
}

fn print_usage() {
    println!(
        "DROIDBRIDGE CLI\n\n\
         Usage:\n\
           droidbridge inspect <apk-path>\n\
           droidbridge status\n\
           droidbridge launch <apk-path> <package-name> [profile-id]\n\
           droidbridge help\n\n\
         Commands:\n\
           inspect  Parse APK metadata without installing or executing it.\n\
           status   Probe the existing Waydroid installation/session.\n\
           launch   Verify the APK package and backend before delegating launch.\n\n\
         The default launch profile id is {DEFAULT_PROFILE}. This does not apply\n\
         device identity spoofing. Waydroid must already be installed, initialized,\n\
         and running."
    );
}

fn inspect_apk(path: &std::path::Path) -> Result<(), Box<dyn Error>> {
    let report = inspect_file(path, &ApkLimits::default())?;
    print_apk_report(&report);
    Ok(())
}

fn print_apk_report(report: &ApkReport) {
    println!("package: {}", report.manifest.package);
    println!("archive entries: {}", report.entry_count);
    println!("version code: {:?}", report.manifest.version_code);
    println!("version name: {:?}", report.manifest.version_name);
    println!("minimum SDK: {:?}", report.manifest.min_sdk);
    println!("target SDK: {:?}", report.manifest.target_sdk);
    println!("application label: {:?}", report.manifest.label);
    println!(
        "requested permissions: {}",
        report.manifest.uses_permissions.len()
    );
    for permission in &report.manifest.uses_permissions {
        println!("  permission: {permission}");
    }

    println!("DEX entries: {}", report.dex_files.len());
    for dex in &report.dex_files {
        match &dex.outcome {
            Ok(summary) => println!(
                "  {}: valid DEX {}, {} classes, {} methods",
                dex.entry_name, summary.version, summary.class_count, summary.method_count
            ),
            Err(error) => println!("  {}: parse failed: {error:?}", dex.entry_name),
        }
    }

    println!("native libraries: {}", report.native_libraries.len());
    for library in &report.native_libraries {
        println!(
            "  {}/{} ({} bytes)",
            library.abi, library.file_name, library.size
        );
    }
    for warning in &report.warnings {
        println!("warning: {warning:?}");
    }
}

fn print_status(backend: &dyn RuntimeBackend) -> Result<(), Box<dyn Error>> {
    println!(
        "backend: {} ({})",
        backend.info().display_name,
        backend.info().id
    );
    println!("status: {:?}", backend.probe()?);
    let capabilities = backend.capabilities();
    println!("APK installation: {:?}", capabilities.apk_installation);
    println!("APK launch: {:?}", capabilities.apk_launch);
    println!(
        "ARM64 native execution: {:?}",
        capabilities.arm64_native_execution
    );
    println!(
        "hardware-accelerated graphics: {:?}",
        capabilities.hardware_accelerated_graphics
    );
    Ok(())
}

fn launch_apk(apk: PathBuf, package: String, profile: String) -> Result<(), Box<dyn Error>> {
    let request = LaunchRequest::new(apk, package, profile)?;
    let backend = WaydroidBackend::default();
    println!("Verifying APK and checking Waydroid before installation...");
    let launch = launch_verified_apk(&backend, &request, &ApkLimits::default())?;
    println!("verified package: {}", launch.report.manifest.package);
    println!("APK preflight passed; Waydroid accepted the install/launch request.");
    wait_for_running(launch.session)
}

fn wait_for_running(mut session: Box<dyn RuntimeSession>) -> Result<(), Box<dyn Error>> {
    let deadline = Instant::now() + STARTUP_OBSERVATION_TIMEOUT;
    loop {
        match session.state()? {
            RuntimeSessionState::Running => {
                println!("app process observed: {}", session.package_name());
                println!("The app remains under Waydroid control after this command exits.");
                return Ok(());
            }
            RuntimeSessionState::Starting if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(500));
            }
            RuntimeSessionState::Starting => {
                return Err(io::Error::other("timed out waiting for the app process").into());
            }
            RuntimeSessionState::Exited { exit_code } => {
                return Err(io::Error::other(format!(
                    "app process exited before it could be observed (exit code: {exit_code:?})"
                ))
                .into());
            }
            RuntimeSessionState::Failed { reason } => {
                return Err(io::Error::other(reason).into());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(|value| OsString::from(*value)).collect()
    }

    #[test]
    fn no_arguments_show_help() {
        assert_eq!(parse_command(Vec::<OsString>::new()), Ok(CliCommand::Help));
    }

    #[test]
    fn inspect_preserves_apk_paths_with_spaces() {
        assert_eq!(
            parse_command(args(&["inspect", "/tmp/my apps/game.apk"])),
            Ok(CliCommand::Inspect {
                apk: PathBuf::from("/tmp/my apps/game.apk")
            })
        );
    }

    #[test]
    fn launch_uses_the_default_profile_when_omitted() {
        assert_eq!(
            parse_command(args(&["launch", "game.apk", "com.example.game"])),
            Ok(CliCommand::Launch {
                apk: PathBuf::from("game.apk"),
                package: "com.example.game".to_owned(),
                profile: DEFAULT_PROFILE.to_owned(),
            })
        );
    }

    #[test]
    fn launch_accepts_an_explicit_profile() {
        assert_eq!(
            parse_command(args(&[
                "launch",
                "game.apk",
                "com.example.game",
                "MY_DEVICE"
            ])),
            Ok(CliCommand::Launch {
                apk: PathBuf::from("game.apk"),
                package: "com.example.game".to_owned(),
                profile: "MY_DEVICE".to_owned(),
            })
        );
    }

    #[test]
    fn malformed_arguments_are_rejected() {
        assert!(parse_command(args(&["inspect"])).is_err());
        assert!(parse_command(args(&["status", "extra"])).is_err());
        assert!(parse_command(args(&["launch", "game.apk"])).is_err());
        assert!(parse_command(args(&["unknown"])).is_err());
    }
}
