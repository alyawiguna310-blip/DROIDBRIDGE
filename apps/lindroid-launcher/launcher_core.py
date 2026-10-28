"""Small, dependency-free helpers for the LINDROID Waydroid launcher."""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path
import subprocess


@dataclass(frozen=True, slots=True)
class AppEntry:
    name: str
    package: str


def is_valid_package_name(package: str) -> bool:
    """Accept a conservative Android-style dotted package name."""
    if not package or len(package) > 255:
        return False
    segments = package.split(".")
    return len(segments) >= 2 and all(
        segment
        and segment[0].isascii()
        and segment[0].isalpha()
        and all(char.isascii() and (char.isalnum() or char == "_") for char in segment)
        for segment in segments
    )


def parse_app_list(output: str) -> list[AppEntry]:
    """Parse the Name/packageName pairs emitted by `waydroid app list`."""
    current_name: str | None = None
    entries: list[AppEntry] = []
    seen: set[str] = set()

    for raw_line in output.splitlines():
        field, separator, value = raw_line.partition(":")
        if not separator:
            continue
        field = field.strip().casefold()
        value = value.strip()

        if field == "name":
            current_name = value or None
        elif field == "packagename":
            if not is_valid_package_name(value):
                current_name = None
                continue
            if value not in seen:
                entries.append(AppEntry(current_name or value, value))
                seen.add(value)
            current_name = None

    return sorted(entries, key=lambda entry: (entry.name.casefold(), entry.package))


def load_hidden_packages(path: Path) -> set[str]:
    """Read hidden package names; a missing preferences file means none are hidden."""
    try:
        lines = path.read_text(encoding="utf-8").splitlines()
    except FileNotFoundError:
        return set()

    return {
        package
        for line in lines
        if is_valid_package_name(package := line.strip())
    }


def save_hidden_packages(path: Path, packages: set[str]) -> None:
    """Atomically save package names hidden only from this launcher."""
    normalized = sorted(
        package for package in packages if is_valid_package_name(package)
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary_path = path.with_name(path.name + ".tmp")
    content = "".join(f"{package}\n" for package in normalized)
    temporary_path.write_text(content, encoding="utf-8")
    temporary_path.replace(path)


def run_waydroid(arguments: list[str], timeout: float = 20.0) -> str:
    """Run a fixed-argument Waydroid CLI request without invoking a shell."""
    try:
        result = subprocess.run(
            ["waydroid", *arguments],
            check=False,
            capture_output=True,
            text=True,
            timeout=timeout,
        )
    except FileNotFoundError as error:
        raise RuntimeError("Waydroid CLI was not found on PATH.") from error
    except subprocess.TimeoutExpired as error:
        raise RuntimeError("Waydroid did not respond before the timeout.") from error

    if result.returncode:
        detail = result.stderr.strip() or result.stdout.strip()
        raise RuntimeError(detail or f"Waydroid exited with status {result.returncode}.")
    return result.stdout


def list_installed_apps() -> list[AppEntry]:
    return parse_app_list(run_waydroid(["app", "list"]))


def launch_app(package: str) -> None:
    """Launch only a validated package name; never construct a shell command."""
    if not is_valid_package_name(package):
        raise ValueError("Refusing to launch an invalid Android package name.")
    run_waydroid(["app", "launch", package], timeout=20.0)
