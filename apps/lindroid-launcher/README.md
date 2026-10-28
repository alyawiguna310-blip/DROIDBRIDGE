# LINDROID Launcher

A tiny native GTK4 desktop frontend for launching apps that are already installed in Waydroid. It does not show Android's home screen or embed an emulator window: when Waydroid multi-window mode is enabled, each app is opened through Waydroid as its own desktop window.

## Scope

- Lists installed apps through `waydroid app list`.
- Filters by app label or Android package name.
- Launches an app through `waydroid app launch <package>`.
- Uses the existing Waydroid image and data; it does not download, initialize, remove, or modify Android images.
- Does not ask for root privileges.

The current LINDROID launcher is a desktop-side program, not a custom Android system image. It cannot make the existing 2.3 GB system/vendor image become a 1 GB image. Image slimming is a separate source-build task.

## Requirements

- Linux with Wayland or another supported GTK desktop.
- GTK4 development files and `pkg-config`.
- Rust stable toolchain.
- A working Waydroid installation.

For Ubuntu/Zorin, the GTK build dependency is provided by `libgtk-4-dev`. Do not install build dependencies until there is enough free disk space.

## Build and run

From the DROIDBRIDGE repository root:

```bash
cargo fmt --manifest-path apps/lindroid-launcher/Cargo.toml -- --check
cargo test --manifest-path apps/lindroid-launcher/Cargo.toml
cargo run --manifest-path apps/lindroid-launcher/Cargo.toml
```

The launcher does not start the privileged container. Start Waydroid using its normal system service/session flow, then open LINDROID and press **Refresh**.

## Google Play Store

Your existing Waydroid config currently points to a **VANILLA** system image, which does not include Google Play Store. The launcher will list and open apps that are installed in the current image. Play Store support requires a separate, compatible GAPPS image and enough free disk space; this project deliberately does not replace the existing image automatically.
