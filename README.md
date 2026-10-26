# DROIDBRIDGE

**DROIDBRIDGE** is an experimental cross-platform Android compatibility project with two planned host frontends:

- **WINDROID** — Windows host integration.
- **LINDROID** — Linux host integration.

The shared core is intended to manage APK metadata, portable device profiles, runtime backends, and performance settings. The long-term goal is to run Android apps and games in host-integrated windows while using the host machine's actual CPU and GPU.

> **Status: early scaffold.** This repository still has not demonstrated launching a real Android APK. It can inspect APKs without executing them and now includes a first Linux Waydroid CLI adapter. The adapter is covered with a mocked-command test, but live Waydroid installation, graphics, and real-app execution have not yet been validated. Android framework compatibility, ART, ARM translation, and Google Play support remain separate milestones.

## Initial configuration

- Performance preset: **balanced**
- Google services preference: **Play Store + required services**, where a compatible, properly licensed runtime is available
- Portable settings: **enabled**
- Example device profile: **POCO F6** (id `POCO_F6`)
- Host hardware reporting: **actual host capabilities remain distinct from reported Android device properties**

Device identity profiles must not claim unsupported CPU instructions or GPU capabilities. Apps may detect differences between reported identity and real host capabilities.

## Repository layout

- `crates/droid-core` — shared core crate:
  - `config` — portable, versioned `BridgeConfig`.
  - `profile` — `DeviceProfile` schema and validation.
  - `registry` — `ProfileRegistry` for loading, lookup, and active-profile selection.
  - `errors` — typed errors (`ProfileError`, `ConfigError`, `RegistryError`, `ProfileIdError`).
  - `runtime` — runtime-backend trait and an initial Linux `WaydroidBackend` for readiness probing, APK installation/launch, process polling, and force-stop; integration with a live Waydroid installation is still unverified.
  - `dex` — bounds-checked DEX parser (no execution).
  - `apk` — APK inspection: ZIP reader with limits, Android binary manifest parser, DEX discovery.
- `profiles/POCO_F6.json` — example portable profile.
- `docs/architecture.md` — staged architecture and implementation plan.

## Device profiles and the registry

Every profile has an explicit, stable **`id`**, separate from its human-readable `profile_name`. The `id` is the registry lookup key and the value stored in `BridgeConfig.active_profile`. Renaming a profile's display name never breaks saved configuration.

`id` rules: ASCII letters, digits, `_` and `-`; must start with a letter or digit; at most 64 characters; case-sensitive. Registry listing order is by `id` in byte order, so it is deterministic.

```rust
use droid_core::{BridgeConfig, ProfileRegistry};

let mut registry = ProfileRegistry::new();
registry.register_file("profiles/POCO_F6.json")?;   // returns "POCO_F6"
registry.apply_config(&BridgeConfig::default())?;   // selects "POCO_F6"
let active = registry.active_profile();
# Ok::<(), droid_core::RegistryError>(())
```

- `register` / `register_json` / `register_file` reject duplicate ids with `RegistryError::DuplicateId`; `replace` overwrites explicitly.
- `set_active` and `apply_config` reject unknown ids with `RegistryError::NotFound` and keep the previous selection.
- Malformed JSON, invalid profiles, missing ids, and unreadable files each produce a distinct typed error.

### Older profile files (no `id`)

Profile JSON written before the `id` field existed is **rejected** by `DeviceProfile::from_json` and the registry's `register_*` methods with `ProfileError::MissingId`. It is never accepted silently, and no id is guessed from `profile_name`. To migrate an old file, either add an `"id"` field to it, or load it explicitly with `DeviceProfile::from_legacy_json(json, id)` / `ProfileRegistry::register_legacy_json(json, id)`, which assigns the id you supply. If the JSON already has a different `id`, that call fails with `ProfileError::IdMismatch`. Every other validation rule still applies, and unknown fields are still rejected.

The `BridgeConfig` schema version is unchanged (`1`); `active_profile` already held a string and now must be a valid profile id.

## Inspecting an APK

```rust
use droid_core::apk::{inspect_file, ApkLimits};

let report = inspect_file("app.apk", &ApkLimits::default())?;
println!("package: {}", report.manifest.package);
for dex in &report.dex_files {
    println!("{}: parsed ok = {}", dex.entry_name, dex.outcome.is_ok());
}
# Ok::<(), droid_core::apk::ApkError>(())
```

The report contains the manifest metadata (package, version, SDK levels, permissions, application label, activity/service/receiver names), a per-entry result for every `classes*.dex`, the native libraries found under `lib/<abi>/`, and warnings such as `NoDexFiles`.

- Inspection never executes, installs, extracts or loads anything from the APK.
- The manifest must be Android binary XML. Plain-text and App Bundle manifests are rejected.
- Resource references (for example `@string/app_name`) are reported as unresolved: `resources.arsc` is not read.
- Archives with unsafe or duplicate entry names, too many entries, or oversized entries are rejected; limits are configurable through `ApkLimits`.
- A malformed DEX file is reported against its entry and does not fail the whole inspection. A parsed DEX file does not mean the app can run.

See `docs/architecture.md` for the full list of supported fields and known limitations.

## Runtime backend contract

The `runtime` module is the seam between the portable core and a separately provisioned Android runtime. It defines backend identity, capability declarations, readiness probing, a validated launch request, and a session handle for observing/stopping a running app. Capabilities default to `Unknown` so a backend must not claim ARM64 execution or graphics support without verification.

This contract is not an emulator. The initial `WaydroidBackend` calls the documented Waydroid CLI for status, APK installation, package launch, process polling, and force-stop. Waydroid itself must already be installed, initialized, and running; the adapter does not initialize it, start its session, inspect the APK manifest, verify the requested package matches the APK, or apply the selected device-profile identity. A successful command does not prove that an arbitrary APK is compatible. The adapter has mocked-command coverage, but still needs a live Waydroid test with a harmless test APK.

## Development

Install a current stable Rust toolchain (edition 2024 requires Rust 1.85 or newer), then run:

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo test --workspace
cargo clippy --workspace --all-targets
```

The first build downloads the `zip` crate, which updates `Cargo.lock`.

## Roadmap

1. Validate portable configuration and host/device profile separation. (done)
2. Implement APK archive inspection and manifest metadata extraction. (done, no execution)
3. DEX instruction decoding and a minimal interpreter.
4. Define a replaceable runtime backend interface and backend capability report. (contract plus initial Waydroid CLI adapter implemented; live runtime not yet validated)
5. Evaluate and integrate existing Android runtimes separately for Windows and Linux. (Waydroid CLI path implemented for Linux; real APK launch not yet validated)
6. Validate execution of a real APK, graphics, input, and lifecycle.
7. Investigate ARM64 native-library translation where required.
8. Evaluate legitimate Google Play support and its certification constraints.
9. Benchmark RAM, CPU, GPU, startup time, and portability across machines.

## Licensing

The project license has not yet been selected. Runtime components and third-party dependencies must be evaluated under their own licenses before redistribution.
