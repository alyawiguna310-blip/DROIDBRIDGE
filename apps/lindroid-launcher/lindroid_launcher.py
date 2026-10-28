#!/usr/bin/env python3
"""Minimal GTK4 desktop launcher for applications installed in Waydroid."""
from __future__ import annotations

import gi

gi.require_version("Gtk", "4.0")
from gi.repository import Gtk  # noqa: E402

from launcher_core import AppEntry, launch_app, list_installed_apps


class LindroidLauncher(Gtk.Application):
    def __init__(self) -> None:
        super().__init__(application_id="io.github.alyawiguna310.LindroidLauncher")
        self.apps: list[AppEntry] = []
        self.window: Gtk.ApplicationWindow | None = None
        self.list_box: Gtk.ListBox | None = None
        self.search_entry: Gtk.SearchEntry | None = None
        self.status_label: Gtk.Label | None = None

    def do_activate(self) -> None:
        if self.window is not None:
            self.window.present()
            return

        self.window = Gtk.ApplicationWindow(application=self, title="LINDROID")
        self.window.set_default_size(560, 650)

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

        self.list_box = Gtk.ListBox()
        self.list_box.set_selection_mode(Gtk.SelectionMode.NONE)
        self.list_box.add_css_class("boxed-list")
        scroll = Gtk.ScrolledWindow()
        scroll.set_hexpand(True)
        scroll.set_vexpand(True)
        scroll.set_child(self.list_box)
        outer.append(scroll)

        footer = Gtk.Label(
            label="Apps open in separate desktop windows when Waydroid multi-window mode is enabled."
        )
        footer.set_xalign(0.0)
        footer.set_wrap(True)
        footer.add_css_class("dim-label")
        outer.append(footer)

        self.window.set_child(outer)
        self.window.present()
        self.refresh_apps()

    def on_refresh(self, _button: Gtk.Button) -> None:
        self.refresh_apps()

    def on_search_changed(self, _entry: Gtk.SearchEntry) -> None:
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

        self.status_label.set_text(
            f"{len(self.apps)} installed app(s). Select Open to launch one."
        )
        self.render_apps()

    def render_apps(self) -> None:
        assert self.list_box is not None
        query = self.search_entry.get_text().strip().casefold() if self.search_entry else ""
        while (child := self.list_box.get_first_child()) is not None:
            self.list_box.remove(child)

        visible = [
            app for app in self.apps
            if not query or query in app.name.casefold() or query in app.package.casefold()
        ]

        if not visible:
            label_text = (
                "No installed apps found. Start the Waydroid session, then press Refresh."
                if not self.apps else "No apps match this search."
            )
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
            for side in ("top", "bottom"):
                getattr(line, f"set_margin_{side}")(8)
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

            button = Gtk.Button(label="Open")
            button.add_css_class("suggested-action")
            button.connect("clicked", self.on_launch_clicked, app.package)
            line.append(button)

            row.set_child(line)
            self.list_box.append(row)

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
