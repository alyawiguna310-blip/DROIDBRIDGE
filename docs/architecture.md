# DROIDBRIDGE architecture

## Goals

DROIDBRIDGE is planned as a shared core with host-specific integrations:

- WINDROID: Windows frontend and host integration.
- LINDROID: Linux frontend and host integration.
- Shared Rust core: portable settings, profile schema, package metadata, and runtime-backend contracts.

## Non-goals for the first milestones

The scaffold does not emulate a CPU, execute DEX, implement ART, render Android graphics, provide Android system services, or install Google Play. These are separate milestones. Nothing in the repository can run an APK today.

## Layers

1. **Host frontend** — native windows, settings, input, accessibility, and host integration.
2. **DroidCore** — portable configuration, profile schema, runtime-backend contracts, and app metadata models.
3. **APK inspector** — safe ZIP/APK inspection, manifest parsing, ABI and SDK detection.
4. **Runtime adapter** — a defined core contract plus an initial Linux Waydroid CLI adapter for probing, installing/launching, polling, and stopping apps. Live Waydroid integration has not yet been validated.
5. **Android runtime** — ART, framework APIs, system services, graphics and app lifecycle.
6. **Native-code compatibility** — execute compatible host-native libraries directly where possible; use ARM translation only when necessary and supported.
7. **Google services** — optional, legally provisioned integration evaluated separately for each runtime.

## Core crate modules

`droid-core` contains `config`, `profile`, `registry`, `errors`, `dex`, `apk`, and `runtime`.

- `config` — `BridgeConfig`, versioned (`schema_version` 1), serializable, validated.
- `profile` — `DeviceProfile` and profile-id validation.
- `registry` — `ProfileRegistry`: in-memory store of validated profiles keyed by id, with an explicit active-profile selection.
- `errors` — typed errors so callers match on variants instead of parsing strings.
- `runtime` — adapter interfaces, capability declarations, validated launch requests, typed errors, and app-session lifecycle contracts. `runtime::waydroid::WaydroidBackend` is an initial Linux adapter around the documented Waydroid CLI; it has mocked-command tests but has not been tested against a live Waydroid installation.
- `launcher` — verified launch preflight: inspect the APK, match the manifest package, require backend readiness and explicitly supported install/launch capabilities, then invoke the adapter.
- `dex` — bounds-checked DEX parser (header, id tables, class data, code items). Parsing only; see "DEX parsing" below.
- `apk` — APK inspection (archive, binary manifest, DEX discovery, native library names). Inspection only; see "APK inspection" below.

## Profile identity and the registry

A profile has a stable machine `id` that is separate from its display `profile_name`. The registry looks profiles up by `id`, and `BridgeConfig.active_profile` stores an `id`. This keeps saved configuration valid when a display name changes.

- Ids are ASCII `[A-Za-z0-9_-]`, start with a letter or digit, max 64 characters, case-sensitive. Listing is sorted by byte order.
- Registering a duplicate id fails; overwriting requires the explicit `replace` API.
- Selecting an unknown id fails and leaves the previous selection unchanged.
- Profile JSON without an `id` is rejected (`ProfileError::MissingId`). Migration of such files is explicit, through `from_legacy_json(json, id)`, and never guesses an id.
- The registry holds no hardcoded profiles. `POCO_F6` is only a fixture and the default `active_profile` value; `apply_config` fails with `NotFound` if it has not been registered.

## Runtime backend contract

The `runtime` module specifies the boundary between the portable core and an independently provisioned Android runtime:

- `RuntimeBackend` exposes backend identity, capability declarations, a readiness probe, and an app launch operation.
- `RuntimeCapabilities` uses `CapabilitySupport::Unknown` by default so an adapter cannot imply verified ARM64 execution, APK launch/install, or hardware-accelerated graphics without reporting that evidence.
- `LaunchRequest` requires a nonempty APK path and package name plus a valid stable profile id. Construction does not inspect the APK, check file existence, install it, or guarantee compatibility.
- `RuntimeSession` provides state polling and a stop request. Typed `RuntimeError` variants allow frontends to handle failures without parsing human-readable messages.

The Waydroid adapter requires Waydroid to be installed and initialized and its user session/container to be running before launch. It does not automatically install or initialize Waydroid, start the session, inspect the APK manifest, verify that the supplied package name matches the APK, or apply the selected device-profile identity. It uses Waydroid's documented CLI for status, APK install, app launch, process polling, and force-stop. The adapter contract does not itself provide Android framework services or execute DEX, and command success does not guarantee an arbitrary app will run. A live test with a harmless test APK is still required.

## Verified APK launch preflight

The public `launch_verified_apk` orchestration function creates a narrow pre-install gate between the APK inspector and a backend adapter:

1. Inspect the APK using caller-supplied resource limits.
2. Require an exact match between the manifest package and the launch request.
3. Require the backend readiness probe to return `BackendStatus::Ready`.
4. Require the backend to explicitly report both APK installation and launch as `CapabilitySupport::Supported`. `Unknown` is not treated as permission to proceed.
5. Only then invoke the adapter's launch method, returning both the APK report and the runtime session handle.

The preflight prevents an accidental manifest/request package mismatch and stops before backend launch when inspection, readiness, or capability checks fail. It does not sandbox APK code, check every app-level compatibility condition, apply device-profile spoofing, or guarantee that the Android runtime can run an app. Live Waydroid testing remains necessary.

## DEX parsing

`dex::DexFile::parse` validates the header (magic, version 035-039, header size, endian tag, file size, Adler-32 checksum, section bounds) and the string, type, proto, field, method and class-definition tables, including every stored index. Strings, class data and code items are decoded lazily by bounds-checked accessors. Counts read from the file are checked against the bytes actually present before anything is allocated, and every failure is a `DexError`.

Not implemented: SHA-1 signature verification, the map list, annotations, debug info, try/catch handlers, DEX 040 and later, and any execution. Parsing a DEX file says nothing about whether it can run.

## APK inspection

`apk::inspect_file`, `inspect_bytes` and `inspect_reader` return an `ApkReport`. Inspection reads archive entries into bounded in-memory buffers; it never extracts files, never executes code, and never loads native libraries.

- **Archive:** read with the `zip` crate (default features off, `deflate` on). The entry count is checked against the end-of-central-directory record before the ZIP library parses the directory, and again afterwards. Entry names are validated (no empty, NUL, backslash, absolute, drive-letter or `..` names, at most 1024 bytes) and duplicates are rejected. Each entry read is capped at its limit (`max_manifest_size`, `max_dex_size`) by both the declared size and a hard cap on the bytes actually read, and a total read budget (`max_total_read`) applies per inspection.
- **Manifest:** `AndroidManifest.xml` must be Android binary XML, parsed by the in-tree reader `apk::axml` (UTF-8 and UTF-16 string pools, elements and typed attributes; namespaces, CDATA, styles and the resource id map are skipped). Plain-text XML and App Bundle protobuf manifests are rejected, never reinterpreted as text. Extracted: `package`, `versionCode`, `versionName`, `uses-sdk` min/target, `uses-permission`, `permission`, the `application` label, and `activity`/`service`/`receiver` names directly under `application` (with the fully qualified class name). Values that are resource references are reported as `ManifestValue::Unresolved`; `resources.arsc` is not parsed, so labels and version names that point into it stay unresolved.
- **DEX:** top-level `classes.dex`, `classes2.dex`, ... (numerically ordered; `classes1.dex`, `classes02.dex` and nested paths are not DEX entries) are parsed with `dex::DexFile`. A malformed DEX does not fail the inspection: the error is recorded against that entry in `ApkReport::dex_files`. A missing DEX is a warning (`ApkWarning::NoDexFiles`), not an error.
- **Native libraries:** `lib/<abi>/<name>.so` entries are listed by name and declared size only.

A missing or invalid manifest, an unsafe or duplicate entry name, too many entries, an oversized manifest, or an exhausted read budget fail the whole inspection.

Residual risk: the ZIP library parses the whole central directory in memory. The entry-count pre-check bounds this for ordinary archives; for ZIP64 archives the count is only known after parsing.

## Hardware identity and host capabilities

Keep Android-facing profile properties separate from actual host CPU, GPU, RAM, instruction-set support, and graphics capabilities. A profile can request supported identity properties, but it must not make the runtime claim that the host supports instructions or graphics features it cannot execute. Some apps may independently detect the host/runtime mismatch.

## Portability

Portable configuration contains profile identifiers and user preferences, not machine-specific absolute paths, secrets, runtime images, or detected host capabilities. Runtime installations and graphics drivers may need to be installed separately on each machine.

## Performance policy

The balanced preset should prefer on-demand runtime startup, avoid duplicate services where the chosen runtime permits sharing, and report measurements instead of promising a specific RAM saving. Do not disable services blindly when apps depend on them.

## Milestones

1. Portable config and profile validation, profile registry (done).
2. DEX parsing and APK archive inspection with binary manifest parsing (done, unexecuted).
3. DEX instruction decoding and a minimal interpreter.
4. Runtime adapter interface and backend capability report (contract plus initial Waydroid CLI adapter implemented; not live-validated).
5. Verify an APK and pass launch requests through a ready runtime backend (preflight implemented; live APK execution not yet verified).
6. Validate graphics acceleration, input, lifecycle and app isolation.
7. Evaluate ARM64 native library support.
8. Evaluate Google Play services using a compatible, properly licensed distribution path.
9. Benchmark resource use and test profile migration across Windows and Linux hosts.
