# DROIDBRIDGE architecture

## Goals

DROIDBRIDGE is planned as a shared core with host-specific integrations:

- WINDROID: Windows frontend and host integration.
- LINDROID: Linux frontend and host integration.
- Shared Rust core: portable settings, profile schema, package metadata, and runtime-backend contracts.

## Non-goals for the first milestones

The scaffold does not emulate a CPU, implement ART, render Android graphics, provide Android system services, or install Google Play. A project-owned bounded interpreter now executes a small integer-only DEX subset supplied as a parsed code item, but no complete APK or Android application can run through that engine yet.

## Layers

1. **Host frontend** — native windows, settings, input, accessibility, and host integration.
2. **DroidCore** — portable configuration, profile schema, runtime-backend contracts, and app metadata models.
3. **APK inspector** — safe ZIP/APK inspection, manifest parsing, ABI and SDK detection.
4. **Project-owned execution engine** — the first internal DEX integer interpreter, operating on an already-parsed code item with strict instruction and step limits. It is not ART and cannot load a whole APK.
5. **External runtime adapter** — a defined core contract plus an optional Linux Waydroid CLI adapter for probing, installing/launching, polling, and stopping apps. Live Waydroid integration has not yet been validated.
6. **Android application environment** — ART-equivalent class loading, framework APIs, system services, graphics and app lifecycle are still missing.
6. **Native-code compatibility** — execute compatible host-native libraries directly where possible; use ARM translation only when necessary and supported.
7. **Google services** — optional, legally provisioned integration evaluated separately for each runtime.

## Core crate modules

`droid-core` contains `config`, `profile`, `registry`, `errors`, `dex`, `apk`, and `runtime`.

- `config` — `BridgeConfig`, versioned (`schema_version` 1), serializable, validated.
- `profile` — `DeviceProfile` and profile-id validation.
- `registry` — `ProfileRegistry`: in-memory store of validated profiles keyed by id, with an explicit active-profile selection.
- `errors` — typed errors so callers match on variants instead of parsing strings.
- `runtime` — adapter interfaces, capability declarations, validated launch requests, typed errors, and app-session lifecycle contracts.
- `runtime::engine` — the project-owned `IntegerDexEngine`: bounded DEX interpretation for scalar `int`, `boolean`, `byte`, `short`, and `char` values, with branch-boundary validation, register/code-size checks, literal arithmetic, unary operations, and static-call resolution across a validated APK DEX set. Values are carried in 32-bit registers and normalized at method boundaries. Nested calls share an instruction budget and have a depth limit; this is not a whole-APK runtime.
- `runtime::waydroid::WaydroidBackend` — an optional Linux adapter around the documented external Waydroid CLI; it has mocked-command tests but has not been tested against a live Waydroid installation.
- `launcher` — verified launch preflight: inspect the APK, match the manifest package, require backend readiness and explicitly supported install/launch capabilities, then invoke the adapter.
- `dex` — bounds-checked DEX parser (header, id tables, class data, code items); execution is a separate layer.
- `apk::read_dex_entry` — reads one recognized top-level DEX entry through the validated archive reader and configured size/read limits; it never extracts the entry to disk.
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

## Command-line entry point

The `droidbridge` binary exposes these paths into the shared core:

- `inspect <apk-path>` runs the bounded APK inspector and prints manifest, DEX, and native-library summaries without executing the APK.
- `status` probes the existing Waydroid installation/session and prints known adapter capabilities.
- `engine-demo` runs a synthetic integer method in the project-owned `IntegerDexEngine` without Waydroid, demonstrating the internal engine independently of an Android installation.
- `methods` lists static `int(int, ...)` candidates from one DEX entry.
- `probe-methods` tries at most 20 candidates with zero-valued integer-like arguments and reports methods that execute or are refused. This is a coverage probe, not a correctness test: many methods require meaningful nonzero inputs.
- `run-method` resolves and executes a selected static scalar method (`int`, `boolean`, `byte`, `short`, or `char`) from DEX tables and class data. Unsupported instructions or signatures fail explicitly.
- `launch <apk-path> <package-name> [profile-id]` uses `launch_verified_apk`, so inspection and exact manifest-package matching happen before the Waydroid adapter is allowed to install or launch. It polls for the app process and exits after observing it or reporting failure.

This CLI is an operational entry point, not an Android emulator. Waydroid must already be installed, initialized, and running. The optional profile id currently passes through `LaunchRequest` as metadata; the CLI does not yet load the profile registry or alter Android-visible device identity. A successful Waydroid command and an observed process are useful smoke-test signals, but do not prove app compatibility, correct rendering, input integration, or lifecycle behavior. Those still require a live test with a harmless APK.

## Project-owned DEX integer engine

`runtime::engine::IntegerDexEngine` is the first internal execution component, written in this repository and separate from the Waydroid adapter. It can run an already parsed `dex::CodeItem` or resolve a static scalar method from a parsed `DexFile`. The resolver walks class definitions and class data, resolves the signature and code offset, then passes the method's real code units to the interpreter. Scalar `int`, `boolean`, `byte`, `short`, and `char` inputs/returns are represented in 32-bit registers and normalized according to their declared DEX type at method boundaries. Execution occurs in a bounded register frame and reports the instruction-step count.

The CLI exposes `methods <apk> <dex-entry> [class-descriptor]` to list static scalar method candidates, `probe-methods` to attempt a bounded sample of candidates with zero-valued arguments, and `run-method <apk> <dex-entry> <class-descriptor> <method-name> [scalar-integer-argument ...]` to execute a selected method. The APK DEX reader uses the project's bounded archive validator, and the selected entry plus all other DEX entries that pass inspection are parsed with the normal bounds-checked parser before method lookup. Method IDs are decoded in the caller's DEX and then matched by full signature against definitions in the loaded set. Listing checks method metadata only; actual instruction support is established only when the interpreter decodes and executes the method. The probe is a coverage tool, not a correctness test, and does not load a whole application.

The supported subset includes no-op/move instructions; integer constants; returns; goto and integer-comparison branches; 32-bit integer arithmetic/bitwise instructions including two-address and literal forms; integer negation and bitwise not; byte/char/short narrowing; and `invoke-static`, `invoke-static/range`, and `move-result` for static calls whose definitions appear in any loaded APK DEX file. Static callees may use only scalar `int`, `boolean`, `byte`, `short`, and `char` parameters and scalar or `void` return types. Values are normalized to the declared scalar width at method boundaries. The interpreter rejects unknown opcodes, methods with try/catch tables, reference/wide argument types, external/native/abstract callees and incompatible return values. It pre-decodes instruction widths, validates branch destinations against instruction boundaries, checks register and outgoing-argument slots, bounds code units, shares a total instruction budget across caller and callee, and caps nested calls at 32 levels.

This implementation is deliberately narrow. It has no general class loader or virtual/dynamic dispatch, objects, fields, arrays, monitors, exception handling, native method calls, Android framework APIs, lifecycle, or graphics. Cross-DEX static resolution is still only a limited method-dispatch step; the engine is not ART and cannot run APKPure or an ordinary Android app as a whole. Framework methods absent from the APK's DEX files still cannot be executed. Continue expanding it through opcode and real-APK coverage probes before tackling a general object/value model and Android runtime services.

## DEX parsing

`dex::DexFile::parse` validates the header (magic, version 035-039, header size, endian tag, file size, Adler-32 checksum, section bounds) and the string, type, proto, field, method and class-definition tables, including every stored index. Strings, class data and code items are decoded lazily by bounds-checked accessors. Counts read from the file are checked against the bytes actually present before anything is allocated, and every failure is a `DexError`.

Not implemented in the parser: SHA-1 signature verification, the map list, annotations, debug info, try/catch handlers, and DEX 040 and later. Parsing a DEX file says nothing about whether it can run. Execution is implemented separately and only for the limited subset described above.

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
3. DEX instruction decoding and a minimal interpreter (bounded integer/scalar interpreter, real DEX method resolution, cross-DEX static calls, and a method-coverage probe implemented; full app execution remains future work).
4. Runtime adapter interface and backend capability report (contract plus initial Waydroid CLI adapter implemented; not live-validated).
5. Verify an APK and pass launch requests through a ready runtime backend (preflight implemented; live APK execution not yet verified).
6. Validate graphics acceleration, input, lifecycle and app isolation.
7. Evaluate ARM64 native library support.
8. Evaluate Google Play services using a compatible, properly licensed distribution path.
9. Benchmark resource use and test profile migration across Windows and Linux hosts.
