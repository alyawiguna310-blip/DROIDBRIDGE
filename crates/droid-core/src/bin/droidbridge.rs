//! Small command-line entry point for inspecting APKs and using the Waydroid adapter.
//!
//! This CLI is an integration surface for the shared core, not an emulator. A
//! real launch requires a separately installed, initialized, running Waydroid
//! environment. The launch command reports process observation, not app
//! compatibility or graphics correctness.

use droid_core::apk::{ApkLimits, ApkReport, inspect_file, read_dex_entry};
use droid_core::dex::{CodeItem, DexFile, DexLimits};
use droid_core::launcher::launch_verified_apk;
use droid_core::runtime::engine::{
    DexSetExecutor, EngineReturn, IntegerDexEngine, list_static_scalar_methods,
};
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
        CliCommand::EngineDemo => run_engine_demo()?,
        CliCommand::Methods {
            apk,
            dex_entry,
            class_descriptor,
        } => list_methods(&apk, &dex_entry, class_descriptor.as_deref())?,
        CliCommand::ProbeMethods {
            apk,
            dex_entry,
            class_descriptor,
        } => probe_methods(&apk, &dex_entry, class_descriptor.as_deref())?,
        CliCommand::RunMethod {
            apk,
            dex_entry,
            class_descriptor,
            method_name,
            parameters,
        } => run_method(
            &apk,
            &dex_entry,
            &class_descriptor,
            &method_name,
            &parameters,
        )?,
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
    EngineDemo,
    Methods {
        apk: PathBuf,
        dex_entry: String,
        class_descriptor: Option<String>,
    },
    ProbeMethods {
        apk: PathBuf,
        dex_entry: String,
        class_descriptor: Option<String>,
    },
    RunMethod {
        apk: PathBuf,
        dex_entry: String,
        class_descriptor: String,
        method_name: String,
        parameters: Vec<i32>,
    },
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
        "engine-demo" if arguments.len() == 1 => Ok(CliCommand::EngineDemo),
        "probe-methods" if (3..=4).contains(&arguments.len()) => Ok(CliCommand::ProbeMethods {
            apk: PathBuf::from(&arguments[1]),
            dex_entry: utf8_arg(&arguments, 2, "DEX entry")?,
            class_descriptor: arguments
                .get(3)
                .map(|_| utf8_arg(&arguments, 3, "class descriptor"))
                .transpose()?,
        }),
        "methods" if (3..=4).contains(&arguments.len()) => Ok(CliCommand::Methods {
            apk: PathBuf::from(&arguments[1]),
            dex_entry: utf8_arg(&arguments, 2, "DEX entry")?,
            class_descriptor: arguments
                .get(3)
                .map(|_| utf8_arg(&arguments, 3, "class descriptor"))
                .transpose()?,
        }),
        "run-method" if arguments.len() >= 5 => {
            let class_descriptor = utf8_arg(&arguments, 3, "class descriptor")?;
            let method_name = utf8_arg(&arguments, 4, "method name")?;
            let mut parameters = Vec::with_capacity(arguments.len().saturating_sub(5));
            for value in arguments.iter().skip(5) {
                let text = value
                    .to_str()
                    .ok_or_else(|| "integer arguments must be valid UTF-8".to_owned())?;
                parameters.push(text.parse::<i32>().map_err(|_| {
                    format!("invalid 32-bit integer argument {text:?}")
                })?);
            }
            Ok(CliCommand::RunMethod {
                apk: PathBuf::from(&arguments[1]),
                dex_entry: utf8_arg(&arguments, 2, "DEX entry")?,
                class_descriptor,
                method_name,
                parameters,
            })
        }
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
        "engine-demo" => Err("usage: droidbridge engine-demo".to_owned()),
        "probe-methods" => Err(
            "usage: droidbridge probe-methods <apk-path> <dex-entry> [class-descriptor]".to_owned()
        ),
        "methods" => Err(
            "usage: droidbridge methods <apk-path> <dex-entry> [class-descriptor]".to_owned()
        ),
        "run-method" => Err(
            "usage: droidbridge run-method <apk-path> <dex-entry> <class-descriptor> <method-name> [scalar-integer-argument ...]".to_owned()
        ),
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
           droidbridge engine-demo\n\
           droidbridge methods <apk-path> <dex-entry> [class-descriptor]\n\
           droidbridge probe-methods <apk-path> <dex-entry> [class-descriptor]\n\
           droidbridge run-method <apk-path> <dex-entry> <class-descriptor> <method-name> [integer-argument ...]\n\
           droidbridge launch <apk-path> <package-name> [profile-id]\n\
           droidbridge help\n\n\
         Commands:\n\
           inspect      Parse APK metadata without installing or executing it.\n\
           status       Probe the existing Waydroid installation/session.\n\
           engine-demo  Run a synthetic integer method in our own DEX engine.\n\
           methods      List static scalar method candidates (int/boolean/byte/short/char).\n\
           probe-methods Execute up to 20 candidates with zero-valued scalar arguments to measure current engine coverage.\n\
           run-method   Execute a supported static scalar method from actual DEX metadata.\n\
           launch       Verify the APK package and backend before delegating launch.\n\n\
         Method execution supports only a small DEX instruction subset; it does not run a whole APK."
    );
}

fn utf8_arg(arguments: &[OsString], index: usize, label: &str) -> Result<String, String> {
    arguments
        .get(index)
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or_else(|| format!("{label} must be valid UTF-8"))
}

fn load_dex_from_apk<'a>(
    apk: &std::path::Path,
    dex_entry: &str,
    limits: &ApkLimits,
    bytes: &'a mut Vec<u8>,
) -> Result<DexFile<'a>, Box<dyn Error>> {
    let report = inspect_file(apk, limits)?;
    if !report
        .dex_files
        .iter()
        .any(|entry| entry.entry_name == dex_entry)
    {
        return Err(
            io::Error::other(format!("DEX entry {dex_entry:?} was not found in the APK")).into(),
        );
    }
    *bytes = read_dex_entry(apk, dex_entry, limits)?;
    let dex_limits = DexLimits {
        max_file_size: usize::try_from(limits.max_dex_size).unwrap_or(usize::MAX),
    };
    Ok(DexFile::parse(bytes.as_slice(), &dex_limits)?)
}

/// Load every DEX entry that passed APK inspection so method-call resolution can
/// look up definitions declared outside the selected entry. The byte buffers
/// are owned by the caller and outlive the parsed DexFile views.
fn load_dex_bundle_from_apk<'a>(
    apk: &std::path::Path,
    requested_entry: &str,
    limits: &ApkLimits,
    buffers: &'a mut Vec<Vec<u8>>,
    entry_names: &mut Vec<String>,
) -> Result<Vec<DexFile<'a>>, Box<dyn Error>> {
    let report = inspect_file(apk, limits)?;
    if !report
        .dex_files
        .iter()
        .any(|entry| entry.entry_name == requested_entry)
    {
        return Err(io::Error::other(format!(
            "DEX entry {requested_entry:?} was not found in the APK"
        ))
        .into());
    }

    buffers.clear();
    entry_names.clear();
    for entry in report
        .dex_files
        .iter()
        .filter(|entry| entry.outcome.is_ok())
    {
        buffers.push(read_dex_entry(apk, &entry.entry_name, limits)?);
        entry_names.push(entry.entry_name.clone());
    }

    if !entry_names.iter().any(|name| name == requested_entry) {
        return Err(io::Error::other(format!(
            "requested DEX entry {requested_entry:?} did not pass DEX validation"
        ))
        .into());
    }

    let dex_limits = DexLimits {
        max_file_size: usize::try_from(limits.max_dex_size).unwrap_or(usize::MAX),
    };
    let mut dexes = Vec::with_capacity(buffers.len());
    for bytes in buffers.iter() {
        dexes.push(DexFile::parse(bytes.as_slice(), &dex_limits)?);
    }
    Ok(dexes)
}

fn list_methods(
    apk: &std::path::Path,
    dex_entry: &str,
    class_filter: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    const MAX_METHODS_TO_PRINT: usize = 100;
    let limits = ApkLimits::default();
    let mut bytes = Vec::new();
    let dex = load_dex_from_apk(apk, dex_entry, &limits, &mut bytes)?;
    let methods = list_static_scalar_methods(&dex, class_filter)?;
    println!(
        "static scalar method candidates: {} (metadata signature only; instruction support is checked during execution)",
        methods.len()
    );
    for signature in methods.iter().take(MAX_METHODS_TO_PRINT) {
        println!(
            "{}->{}({}){}",
            signature.class_descriptor,
            signature.name,
            signature.parameter_types.join(","),
            signature.return_type
        );
    }
    if methods.len() > MAX_METHODS_TO_PRINT {
        println!(
            "output truncated to the first {MAX_METHODS_TO_PRINT} candidates; provide a class descriptor to narrow the list"
        );
    }
    Ok(())
}

fn probe_methods(
    apk: &std::path::Path,
    dex_entry: &str,
    class_filter: Option<&str>,
) -> Result<(), Box<dyn Error>> {
    const MAX_PROBE_METHODS: usize = 20;
    let limits = ApkLimits::default();
    let mut buffers = Vec::new();
    let mut entry_names = Vec::new();
    let dexes = load_dex_bundle_from_apk(apk, dex_entry, &limits, &mut buffers, &mut entry_names)?;
    let root_dex_index = entry_names
        .iter()
        .position(|name| name == dex_entry)
        .expect("requested DEX was validated and loaded");
    println!("loaded DEX bundle: {}", entry_names.join(", "));
    let executor = DexSetExecutor::new(&dexes)?;
    let candidates = list_static_scalar_methods(&dexes[root_dex_index], class_filter)?;
    let attempts = candidates.len().min(MAX_PROBE_METHODS);

    println!(
        "probing {attempts} of {} static scalar method candidates with zero-valued arguments",
        candidates.len()
    );
    println!(
        "This is a coverage probe, not a correctness test; methods may require nonzero inputs."
    );

    let mut executed = 0usize;
    let mut refused = 0usize;
    for signature in candidates.iter().take(attempts) {
        let parameters = vec![0; signature.parameter_types.len()];
        match executor.execute_static_scalar_method_at(
            root_dex_index,
            &signature.class_descriptor,
            &signature.name,
            &parameters,
        ) {
            Ok(report) => {
                executed += 1;
                println!(
                    "  EXECUTED {}->{}({}){} => {:?} ({} total steps including callees)",
                    signature.class_descriptor,
                    signature.name,
                    signature.parameter_types.join(","),
                    signature.return_type,
                    report.execution.return_value,
                    report.execution.instruction_steps
                );
            }
            Err(error) => {
                refused += 1;
                println!(
                    "  REFUSED  {}->{}({}){}: {error}",
                    signature.class_descriptor,
                    signature.name,
                    signature.parameter_types.join(","),
                    signature.return_type
                );
            }
        }
    }
    println!("probe summary: {executed} executed, {refused} refused");
    if candidates.len() > attempts {
        println!(
            "remaining candidates not attempted: {}; use a class descriptor to narrow the probe",
            candidates.len() - attempts
        );
    }
    Ok(())
}

fn run_method(
    apk: &std::path::Path,
    dex_entry: &str,
    class_descriptor: &str,
    method_name: &str,
    parameters: &[i32],
) -> Result<(), Box<dyn Error>> {
    let limits = ApkLimits::default();
    let mut buffers = Vec::new();
    let mut entry_names = Vec::new();
    let dexes = load_dex_bundle_from_apk(apk, dex_entry, &limits, &mut buffers, &mut entry_names)?;
    let root_dex_index = entry_names
        .iter()
        .position(|name| name == dex_entry)
        .expect("requested DEX was validated and loaded");
    println!("loaded DEX bundle: {}", entry_names.join(", "));
    let executor = DexSetExecutor::new(&dexes)?;
    let report = executor.execute_static_scalar_method_at(
        root_dex_index,
        class_descriptor,
        method_name,
        parameters,
    )?;
    println!(
        "method: {}->{}({}){}",
        report.signature.class_descriptor,
        report.signature.name,
        report.signature.parameter_types.join(","),
        report.signature.return_type
    );
    println!("engine: DROIDBRIDGE IntegerDexEngine");
    println!(
        "instructions executed: {}",
        report.execution.instruction_steps
    );
    match report.execution.return_value {
        EngineReturn::Int(value) => println!("return: {value}"),
        EngineReturn::Void => {
            return Err(io::Error::other("scalar method unexpectedly returned void").into());
        }
        EngineReturn::NullReference
        | EngineReturn::ObjectReference { .. }
        | EngineReturn::StringReference { .. } => {
            return Err(io::Error::other(
                "scalar method unexpectedly returned an object reference",
            )
            .into());
        }
    }
    Ok(())
}

fn run_engine_demo() -> Result<(), Box<dyn Error>> {
    let code = CodeItem {
        registers_size: 3,
        ins_size: 0,
        outs_size: 0,
        tries_size: 0,
        debug_info_off: 0,
        insns: vec![0x0013, 7, 0x0113, 35, 0x0290, 0x0100, 0x020f],
    };
    let report = IntegerDexEngine::default().execute(&code, &[])?;
    println!("engine: DROIDBRIDGE IntegerDexEngine");
    println!("program: const/16 v0, 7; const/16 v1, 35; add-int v2, v0, v1; return v2");
    println!("instructions executed: {}", report.instruction_steps);
    match report.return_value {
        EngineReturn::Int(value) => println!("return: {value}"),
        EngineReturn::Void => {
            return Err(io::Error::other("integer engine demo returned void").into());
        }
        EngineReturn::NullReference
        | EngineReturn::ObjectReference { .. }
        | EngineReturn::StringReference { .. } => {
            return Err(
                io::Error::other("integer engine demo returned an object reference").into(),
            );
        }
    }
    Ok(())
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
    fn parses_probe_methods_command_with_optional_class_filter() {
        assert_eq!(
            parse_command(args(&["probe-methods", "app.apk", "classes.dex", "LTest;"])),
            Ok(CliCommand::ProbeMethods {
                apk: PathBuf::from("app.apk"),
                dex_entry: "classes.dex".to_owned(),
                class_descriptor: Some("LTest;".to_owned()),
            })
        );
        assert_eq!(
            parse_command(args(&["probe-methods", "app.apk", "classes.dex"])),
            Ok(CliCommand::ProbeMethods {
                apk: PathBuf::from("app.apk"),
                dex_entry: "classes.dex".to_owned(),
                class_descriptor: None,
            })
        );
        assert!(parse_command(args(&["probe-methods", "app.apk"])).is_err());
    }

    #[test]
    fn parses_methods_command_with_optional_class_filter() {
        assert_eq!(
            parse_command(args(&["methods", "app.apk", "classes.dex", "LTest;"])),
            Ok(CliCommand::Methods {
                apk: PathBuf::from("app.apk"),
                dex_entry: "classes.dex".to_owned(),
                class_descriptor: Some("LTest;".to_owned()),
            })
        );
        assert_eq!(
            parse_command(args(&["methods", "app.apk", "classes.dex"])),
            Ok(CliCommand::Methods {
                apk: PathBuf::from("app.apk"),
                dex_entry: "classes.dex".to_owned(),
                class_descriptor: None,
            })
        );
    }

    #[test]
    fn parses_run_method_integer_arguments() {
        assert_eq!(
            parse_command(args(&[
                "run-method",
                "app.apk",
                "classes.dex",
                "LTest;",
                "sum",
                "19",
                "23"
            ])),
            Ok(CliCommand::RunMethod {
                apk: PathBuf::from("app.apk"),
                dex_entry: "classes.dex".to_owned(),
                class_descriptor: "LTest;".to_owned(),
                method_name: "sum".to_owned(),
                parameters: vec![19, 23],
            })
        );
        assert!(
            parse_command(args(&[
                "run-method",
                "app.apk",
                "classes.dex",
                "LTest;",
                "sum",
                "not-an-int"
            ]))
            .is_err()
        );
    }

    #[test]
    fn engine_demo_command_is_recognized() {
        assert_eq!(
            parse_command(args(&["engine-demo"])),
            Ok(CliCommand::EngineDemo)
        );
        assert!(parse_command(args(&["engine-demo", "extra"])).is_err());
    }

    #[test]
    fn malformed_arguments_are_rejected() {
        assert!(parse_command(args(&["inspect"])).is_err());
        assert!(parse_command(args(&["status", "extra"])).is_err());
        assert!(parse_command(args(&["launch", "game.apk"])).is_err());
        assert!(parse_command(args(&["unknown"])).is_err());
    }
}
