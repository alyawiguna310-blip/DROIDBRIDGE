# LINDROID Launcher

LINDROID is a minimal native GTK4 frontend for applications already installed in Waydroid. It runs with the system Python GTK4 bindings, requires no Rust GTK build dependencies, and does not show Android's home screen. When Waydroid multi-window mode is enabled, each app launches through Waydroid in its own desktop window.

## Features

- Lists installed apps using `waydroid app list`.
- Filters by app label or package name.
- Opens an app using `waydroid app launch <package>`.
- Uses subprocess argument arrays; it never invokes a host shell to build commands.
- Does not require root and does not download, initialize, remove, or modify Android images.

## Requirements

- A working Waydroid installation and a started session.
- GTK4 runtime and Python GObject bindings for the system Python. On this Zorin setup these are already installed, so no GTK development package is needed.

## Run without installing

From the repository root:

```bash
/usr/bin/python3 apps/lindroid-launcher/lindroid_launcher.py
```

Check parser tests with:

```bash
PYTHONPATH=apps/lindroid-launcher /usr/bin/python3 -m unittest discover \
  -s apps/lindroid-launcher/tests -v
```

## Install into the current user's application menu

```bash
bash scripts/install-lindroid-launcher.sh
```

The installer copies the small Python files into `~/.local/share/lindroid`, creates a user-local launcher, and installs a desktop entry. It does not call apt or require sudo.

## Google Play Store and image size

The current Waydroid configuration uses a **VANILLA** image, which does not include Google Play Store. LINDROID lists only apps installed in the current image; it cannot add Play Store by itself. GAPPS support and minimizing the Android system image are separate tasks that require a compatible image/build and additional storage. This launcher does not turn the existing 2.3 GB system/vendor images into a 1 GB image.
