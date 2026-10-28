#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${SCRIPT_DIR}/.." && pwd)"
APP_DIR="${HOME}/.local/share/lindroid"
BIN_DIR="${HOME}/.local/bin"
DESKTOP_DIR="${HOME}/.local/share/applications"
APP_BIN="${BIN_DIR}/lindroid-launcher"
DESKTOP_FILE="${DESKTOP_DIR}/lindroid-launcher.desktop"

command -v /usr/bin/python3 >/dev/null 2>&1 || {
    printf 'The system Python 3 interpreter is required.\n' >&2
    exit 1
}
/usr/bin/python3 -c 'import gi; gi.require_version("Gtk", "4.0"); from gi.repository import Gtk' || {
    printf 'GTK4 Python bindings are missing for /usr/bin/python3.\n' >&2
    exit 1
}

install -d "${APP_DIR}" "${BIN_DIR}" "${DESKTOP_DIR}"
install -m 0644 "${REPO_ROOT}/apps/lindroid-launcher/launcher_core.py" "${APP_DIR}/launcher_core.py"
install -m 0644 "${REPO_ROOT}/apps/lindroid-launcher/lindroid_launcher.py" "${APP_DIR}/lindroid_launcher.py"

cat > "${APP_BIN}" <<'LAUNCHER'
#!/usr/bin/env bash
set -euo pipefail
exec /usr/bin/python3 "${HOME}/.local/share/lindroid/lindroid_launcher.py" "$@"
LAUNCHER
chmod 0755 "${APP_BIN}"

python3 - "${REPO_ROOT}/apps/lindroid-launcher/lindroid-launcher.desktop.in" "${DESKTOP_FILE}" "${APP_BIN}" <<'PY'
from pathlib import Path
import sys

def desktop_escape(value: str) -> str:
    return value.replace("\\", "\\\\").replace(" ", "\\ ").replace("\t", "\\t")

template = Path(sys.argv[1]).read_text(encoding="utf-8")
Path(sys.argv[2]).write_text(
    template.replace("@LINDROID_BIN@", desktop_escape(sys.argv[3])),
    encoding="utf-8",
)
PY

printf 'Installed LINDROID in your user application menu.\n'
printf 'Run directly with: %s\n' "${APP_BIN}"
printf 'No GTK development packages or root privileges were needed.\n'
