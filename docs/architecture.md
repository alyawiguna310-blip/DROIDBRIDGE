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

## Hardware identity and host capabilities

Keep Android-facing profile properties separate from actual host CPU, GPU, RAM, instruction-set support, and graphics capabilities. A profile can request supported identity properties, but it must not make the runtime claim that the host supports instructions or graphics features it cannot execute. Some apps may independently detect the host/runtime mismatch.

## Portability

Portable configuration contains profile identifiers and user preferences, not machine-specific absolute paths, secrets, runtime images, or detected host capabilities. Runtime installations and graphics drivers may need to be installed separately on each machine.

## Performance policy

The balanced preset should prefer on-demand runtime startup, avoid duplicate services where the chosen runtime permits sharing, and report measurements instead of promising a specific RAM saving. Do not disable services blindly when apps depend on them.

## Milestones

1. Portable config and profile validation (current scaffold).
2. APK archive inspection and Android manifest parsing.
3. Runtime adapter interface and backend capability report.
4. Launch a real test APK through a selected runtime.
5. Validate graphics acceleration, input, lifecycle and app isolation.
6. Evaluate ARM64 native library support.
7. Evaluate Google Play services using a compatible, properly licensed distribution path.
8. Benchmark resource use and test profile migration across Windows and Linux hosts.
