# Android Runtime (ART) integration through Waydroid

## Decision: use an Android guest, not a host-side ART library

DROIDBRIDGE must not treat ART as a Rust crate or copy a few ART source files into the bounded DEX interpreter. ART is part of the Android platform. Android applications depend on more than DEX interpretation: class loading, managed core libraries, native runtime libraries, Zygote/process setup, framework APIs and system services must agree with the guest system image.

The Android Open Source Project documents ART as a system module. Since Android 11, ART and libcore are delivered as the `com.android.art` APEX; other native/platform components remain part of the platform. See the [official ART module documentation](https://source.android.com/docs/core/ota/modular-system/art) and [Android runtime overview](https://source.android.com/docs/core/runtime).

Waydroid runs a complete Android userspace in a Linux container and ships a customized system image based on LineageOS. This is the practical integration boundary for DROIDBRIDGE on Linux: Waydroid's Android guest provides ART and Android framework services, while DROIDBRIDGE remains the host-side coordinator. See the [Waydroid project overview](https://github.com/waydroid/waydroid) and [Waydroid documentation](https://docs.waydro.id/).

## Current implementation

- `runtime::waydroid::WaydroidBackend` uses the Waydroid CLI to check readiness, install an APK, launch its package, poll the process and request a force-stop.
- `WaydroidBackend::android_runtime_info()` reads a small allowlisted set of guest properties: Android release, API level, build fingerprint, supported ABI list, Zygote mode, JIT property and boot-complete state.
- `droidbridge runtime-info` prints those properties. The CLI labels ART as *expected* when the guest reports Android API 21 or newer. That is an inference from the Android API level, not an independent inspection of ART binaries.
- `droidbridge launch <apk-path> <package-name> [profile-id]` uses the real Android guest after APK inspection and exact manifest-package matching.
- `runtime::engine::IntegerDexEngine` is a separate, deliberately small interpreter for bounded DEX-analysis tasks. It is not the ART implementation and its probe count is not a measure of whole-APK compatibility.

The adapter requires Waydroid to already be installed, initialized and running. DROIDBRIDGE does not download system images, build LineageOS, install Waydroid, or initialize a guest implicitly.

## Verify the runtime

With a running Waydroid session:

```sh
cargo run -p droid-core --bin droidbridge -- status
cargo run -p droid-core --bin droidbridge -- runtime-info
```

The runtime information command intentionally reports missing properties as `unknown`; it does not guess ABI support, graphics support or a specific ART build. An API level and running process also do not prove that every APK can run correctly. Compatibility still depends on the guest's API level, supported ABIs, native libraries, required services, permissions, graphics and app-specific behavior.

## Why DROIDBRIDGE does not vendor the ART source tree

Building a usable ART runtime from source requires a compatible Android platform build environment and its matching platform dependencies. A standalone host library would not provide the complete Android process and framework environment required by ordinary APKs. Vendoring an Android system image or ART source tree would also introduce a large set of upstream components and their individual license notices, build requirements and update responsibilities.

The current implementation therefore integrates with an externally provisioned Android system instead of copying third-party runtime code into this repository. A future packaged runtime should use a reproducible Android/LineageOS build or a properly licensed prebuilt system image, document its provenance and licenses, and keep host integration behind the `RuntimeBackend` contract.
