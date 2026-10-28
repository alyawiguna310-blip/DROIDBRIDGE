#!/usr/bin/env python3
"""Minimal GTK4 desktop launcher for applications installed in Waydroid."""
from __future__ import annotations

from pathlib import Path

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk  # noqa: E402

from launcher_core import (
    AppEntry,
    launch_app,
    list_installed_apps,
    load_hidden_packages,
    save_hidden_packages,
)


class LindroidLauncher(Gtk.Application):
    def __init__(self) -> None:
        super().__init__(application_id="io.github.alyawiguna310.LindroidLauncher")
        self.apps: list[AppEntry] = []
        self.hidden_packages: set[str] = set()
        self.hidden_file = Path.home() / ".config" / "lindroid" / "hidden-apps.txt"
        self.preferences_error: str | None = None
        self.window: Gtk.ApplicationWindow | None = None
        self.list_box: Gtk.ListBox | None = None
        self.search_entry: Gtk.SearchEntry | None = None
        self.status_label: Gtk.Label | None = None
        self.show_hidden_check: Gtk.CheckButton | None = None

    def do_activate(self) -> None:
        if self.window is not None:
            self.window.present()
            return

        try:
            self.hidden_packages = load_hidden_packages(self.hidden_file)
        except OSError as error:
            self.preferences_error = str(error)

        self.window = Gtk.ApplicationWindow(application=self, title="LINDROID")
        self.window.set_default_size(560, 680)

        outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
        outer.set_margin_top(16)
        outer.set_margin_bottom(16)
        outer.set_margin_start(16)
        outer.set_margin_end(16)

        heading = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=12)
        title = Gtk.Label(label="LINDROID")
        title.set_xalign(0.0)
        title.set_hexpand(True)
        title.add_css_class("title-1")
        refresh = Gtk.Button(label="Refresh")
        refresh.set_tooltip_text("Refresh the installed Android app list")
        refresh.connect("clicked", self.on_refresh)
        heading.append(title)
        heading.append(refresh)
        outer.append(heading)

        self.status_label = Gtk.Label(label="Ready")
        self.status_label.set_xalign(0.0)
        self.status_label.set_wrap(True)
        self.status_label.add_css_class("dim-label")
        outer.append(self.status_label)

        self.search_entry = Gtk.SearchEntry()
        self.search_entry.set_placeholder_text("Search installed Android apps")
        self.search_entry.connect("search-changed", self.on_search_changed)
        outer.append(self.search_entry)

        self.show_hidden_check = Gtk.CheckButton(label="Show hidden apps")
        self.show_hidden_check.set_tooltip_text(
            "Show apps hidden from this launcher; hidden apps remain installed."
        )
        self.show_hidden_check.connect("toggled", self.on_show_hidden_changed)
        outer.append(self.show_hidden_check)

        self.list_box = Gtk.ListBox()
        self.list_box.set_selection_mode(Gtk.SelectionMode.NONE)
        self.list_box.add_css_class("boxed-list")
        scroll = Gtk.ScrolledWindow()
        scroll.set_hexpand(True)
        scroll.set_vexpand(True)
        scroll.set_child(self.list_box)
        outer.append(scroll)

        footer = Gtk.Label(
            label=(
                "Hide only removes an app from this list. It does not uninstall the app "
                "or reduce Android image size."
            )
        )
        footer.set_xalign(0.0)
        footer.set_wrap(True)
        footer.add_css_class("dim-label")
        outer.append(footer)

        self.window.set_child(outer)
        self.window.present()
        self.refresh_apps()

        if self.preferences_error and self.status_label is not None:
            self.status_label.set_text(
                f"Couldn't load hidden-app preferences: {self.preferences_error}"
            )

    def on_refresh(self, _button: Gtk.Button) -> None:
        self.refresh_apps()

    def on_search_changed(self, _entry: Gtk.SearchEntry) -> None:
        self.render_apps()

    def on_show_hidden_changed(self, _button: Gtk.CheckButton) -> None:
        self.render_apps()

    def refresh_apps(self) -> None:
        assert self.status_label is not None
        self.status_label.set_text("Refreshing installed apps…")
        try:
            self.apps = list_installed_apps()
        except RuntimeError as error:
            self.apps = []
            self.status_label.set_text(str(error))
            self.render_apps()
            return

        hidden_count = sum(app.package in self.hidden_packages for app in self.apps)
        self.status_label.set_text(
            f"{len(self.apps)} installed app(s) · {hidden_count} hidden from LINDROID"
        )
        self.render_apps()

    def render_apps(self) -> None:
        assert self.list_box is not None
        query = self.search_entry.get_text().strip().casefold() if self.search_entry else ""
        show_hidden = (
            self.show_hidden_check.get_active()
            if self.show_hidden_check is not None
            else False
        )
        while (child := self.list_box.get_first_child()) is not None:
            self.list_box.remove(child)

        visible = [
            app
            for app in self.apps
            if (show_hidden or app.package not in self.hidden_packages)
            and (
                not query
                or query in app.name.casefold()
                or query in app.package.casefold()
            )
        ]

        if not visible:
            if not self.apps:
                label_text = (
                    "No installed apps found. Start the Waydroid session, then press Refresh."
                )
            elif (
                not show_hidden
                and all(app.package in self.hidden_packages for app in self.apps)
                and not query
            ):
                label_text = (
                    "All apps are hidden from LINDROID. Enable Show hidden apps to restore them."
                )
            else:
                label_text = "No apps match this search."
            row = Gtk.ListBoxRow()
            row.set_activatable(False)
            label = Gtk.Label(label=label_text)
            label.set_xalign(0.0)
            label.set_wrap(True)
            label.set_margin_top(14)
            label.set_margin_bottom(14)
            label.set_margin_start(12)
            label.set_margin_end(12)
            row.set_child(label)
            self.list_box.append(row)
            return

        for app in visible:
            row = Gtk.ListBoxRow()
            row.set_activatable(False)
            line = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=12)
            line.set_margin_top(8)
            line.set_margin_bottom(8)
            line.set_margin_start(10)
            line.set_margin_end(10)

            icon = Gtk.Image.new_from_icon_name("application-x-executable-symbolic")
            icon.set_pixel_size(32)
            line.append(icon)

            details = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=3)
            details.set_hexpand(True)
            name = Gtk.Label(label=app.name)
            name.set_xalign(0.0)
            name.set_wrap(True)
            name.add_css_class("heading")
            package = Gtk.Label(label=app.package)
            package.set_xalign(0.0)
            package.set_wrap(True)
            package.add_css_class("dim-label")
            details.append(name)
            details.append(package)
            line.append(details)

            is_hidden = app.package in self.hidden_packages
            visibility_button = Gtk.Button(label="Show" if is_hidden else "Hide")
            visibility_button.set_tooltip_text(
                "Restore this app to the launcher"
                if is_hidden
                else "Hide this app from LINDROID without uninstalling it"
            )
            visibility_button.connect(
                "clicked", self.on_visibility_clicked, app.package
            )
            line.append(visibility_button)

            button = Gtk.Button(label="Open")
            button.add_css_class("suggested-action")
            button.connect("clicked", self.on_launch_clicked, app.package)
            line.append(button)

            row.set_child(line)
            self.list_box.append(row)

    def on_visibility_clicked(self, _button: Gtk.Button, package: str) -> None:
        assert self.status_label is not None
        updated = set(self.hidden_packages)
        will_hide = package not in updated
        if will_hide:
            updated.add(package)
        else:
            updated.remove(package)

        try:
            save_hidden_packages(self.hidden_file, updated)
        except OSError as error:
            self.status_label.set_text(f"Couldn't save launcher preferences: {error}")
            return

        self.hidden_packages = updated
        verb = "Hidden from LINDROID" if will_hide else "Restored to LINDROID"
        self.status_label.set_text(
            f"{verb}: {package}. The Android app is still installed."
        )
        self.render_apps()

    def on_launch_clicked(self, _button: Gtk.Button, package: str) -> None:
        assert self.status_label is not None
        try:
            launch_app(package)
        except (RuntimeError, ValueError) as error:
            self.status_label.set_text(f"Could not launch {package}: {error}")
            return
        self.status_label.set_text(f"Launch requested: {package}")


def main() -> None:
    LindroidLauncher().run()


if __name__ == "__main__":
    main()
