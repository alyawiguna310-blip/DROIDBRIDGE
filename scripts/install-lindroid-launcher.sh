#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
APP_MANIFEST="${REPO_ROOT}/apps/lindroid-launcher/Cargo.toml"
APP_BINARY="${HOME}/.local/bin/lindroid-launcher"
DESKTOP_DIR="${HOME}/.local/share/applications"
DESKTOP_FILE="${DESKTOP_DIR}/lindroid-launcher.desktop"

command -v cargo >/dev/null 2>&1 || {
    printf 'Rust/Cargo is required.\n' >&2
    exit 1
}

cd "${REPO_ROOT}"
cargo build --release --manifest-path "${APP_MANIFEST}"
install -D -m 0755 "${REPO_ROOT}/apps/lindroid-launcher/target/release/lindroid-launcher" "${APP_BINARY}"
install -d "${DESKTOP_DIR}"

python3 - "${REPO_ROOT}/apps/lindroid-launcher/lindroid-launcher.desktop.in" "${DESKTOP_FILE}" "${APP_BINARY}" <<'PY'
from pathlib import Path
import sys

template = Path(sys.argv[1]).read_text(encoding="utf-8")
binary = sys.argv[3].replace("\\", "\\\\").replace(" ", "\\ ").replace("\t", "\\t")
Path(sys.argv[2]).write_text(
    template.replace("@LINDROID_BIN@", binary),
    encoding="utf-8",
)
PY

printf 'Installed LINDROID launcher: %s\n' "${APP_BINARY}"
printf 'Desktop entry: %s\n' "${DESKTOP_FILE}"
printf 'Launch it from your application menu or run: lindroid-launcher\n'
