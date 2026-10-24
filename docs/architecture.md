# DROIDBRIDGE architecture

## Goals

DROIDBRIDGE is planned as a shared core with host-specific integrations:

- WINDROID: Windows frontend and host integration.
- LINDROID: Linux frontend and host integration.
- Shared Rust core: portable settings, profile schema, package metadata, and runtime-backend contracts.

## Non-goals for the first milestone

The initial scaffold does not emulate a CPU, execute DEX, implement ART, render Android graphics, provide Android system services, or install Google Play. These are separate milestones.

## Layers

1. **Host frontend** — native windows, settings, input, accessibility, and host integration.
2. **DroidCore** — portable configuration, app metadata models, lifecycle contracts.
3. **APK inspector** — safe ZIP/APK inspection, manifest parsing, ABI and SDK detection.
4. **Runtime adapter** — launches and monitors an independently provisioned Android runtime.
5. **Android runtime** — ART, framework APIs, system services, graphics and app lifecycle.
6. **Native-code compatibility** — execute compatible host-native libraries directly where possible; use ARM translation only when necessary and supported.
7. **Google services** — optional, legally provisioned integration evaluated separately for each runtime.

## Core crate modules (implemented)

`droid-core` currently contains `config`, `profile`, `registry`, and `errors`. Everything else in the layer list above is still planned.

- `config` — `BridgeConfig`, versioned (`schema_version` 1), serializable, validated.
- `profile` — `DeviceProfile` and profile-id validation.
- `registry` — `ProfileRegistry`: in-memory store of validated profiles keyed by id, with an explicit active-profile selection.
- `errors` — typed errors so callers match on variants instead of parsing strings.

## Profile identity and the registry

A profile has a stable machine `id` that is separate from its display `profile_name`. The registry looks profiles up by `id`, and `BridgeConfig.active_profile` stores an `id`. This keeps saved configuration valid when a display name changes.

- Ids are ASCII `[A-Za-z0-9_-]`, start with a letter or digit, max 64 characters, case-sensitive. Listing is sorted by byte order.
- Registering a duplicate id fails; overwriting requires the explicit `replace` API.
- Selecting an unknown id fails and leaves the previous selection unchanged.
- Profile JSON without an `id` is rejected (`ProfileError::MissingId`). Migration of such files is explicit, through `from_legacy_json(json, id)`, and never guesses an id.
- The registry holds no hardcoded profiles. `POCO_F6` is only a fixture and the default `active_profile` value; `apply_config` fails with `NotFound` if it has not been registered.

## Hardware identity and host capabilities

Keep Android-facing profile properties separate from actual host CPU, GPU, RAM, instruction-set support, and graphics capabilities. A profile can request supported identity properties, but it must not make the runtime claim that the host supports instructions or graphics features it cannot execute. Some apps may independently detect the host/runtime mismatch.

## Portability

Portable configuration contains profile identifiers and user preferences, not machine-specific absolute paths, secrets, runtime images, or detected host capabilities. Runtime installations and graphics drivers may need to be installed separately on each machine.

## Performance policy

The balanced preset should prefer on-demand runtime startup, avoid duplicate services where the chosen runtime permits sharing, and report measurements instead of promising a specific RAM saving. Do not disable services blindly when apps depend on them.

## Milestones

1. Portable config and profile validation, profile registry (current).
2. APK archive inspection and Android manifest parsing.
3. Runtime adapter interface and backend capability report.
4. Launch a real test APK through a selected runtime.
5. Validate graphics acceleration, input, lifecycle and app isolation.
6. Evaluate ARM64 native library support.
7. Evaluate Google Play services using a compatible, properly licensed distribution path.
8. Benchmark resource use and test profile migration across Windows and Linux hosts.
