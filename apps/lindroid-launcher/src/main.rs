use gtk::prelude::*;
use gtk4 as gtk;
use std::cell::RefCell;
use std::collections::HashSet;
use std::process::{Command, Output};
use std::rc::Rc;

#[derive(Debug, Clone, PartialEq, Eq)]
struct AppEntry {
    name: String,
    package: String,
}

fn main() {
    let application = gtk::Application::builder()
        .application_id("io.github.alyawiguna310.LindroidLauncher")
        .build();

    application.connect_activate(build_ui);
    application.run();
}

fn build_ui(application: &gtk::Application) {
    let window = gtk::ApplicationWindow::builder()
        .application(application)
        .title("LINDROID")
        .default_width(560)
        .default_height(680)
        .build();

    let layout = gtk::Box::new(gtk::Orientation::Vertical, 12);
    layout.set_margin_top(16);
    layout.set_margin_bottom(16);
    layout.set_margin_start(16);
    layout.set_margin_end(16);

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let title = gtk::Label::new(Some("LINDROID"));
    title.set_xalign(0.0);
    title.add_css_class("title-1");
    title.set_hexpand(true);

    let refresh_button = gtk::Button::with_label("Refresh");
    refresh_button.set_tooltip_text(Some("Refresh the installed Android app list"));

    header.append(&title);
    header.append(&refresh_button);
    layout.append(&header);

    let status_label = gtk::Label::new(Some("Checking Waydroid…"));
    status_label.set_xalign(0.0);
    status_label.set_wrap(true);
    status_label.add_css_class("dim-label");
    layout.append(&status_label);

    let search_entry = gtk::SearchEntry::new();
    search_entry.set_placeholder_text(Some("Search installed Android apps"));
    layout.append(&search_entry);

    let list_box = gtk::ListBox::new();
    list_box.set_selection_mode(gtk::SelectionMode::None);
    list_box.add_css_class("boxed-list");

    let scrolled = gtk::ScrolledWindow::builder()
        .hexpand(true)
        .vexpand(true)
        .child(&list_box)
        .build();
    layout.append(&scrolled);

    let footer = gtk::Label::new(Some(
        "Apps open in separate desktop windows when Waydroid multi-window mode is enabled.",
    ));
    footer.set_xalign(0.0);
    footer.set_wrap(true);
    footer.add_css_class("dim-label");
    layout.append(&footer);

    window.set_child(Some(&layout));

    let apps: Rc<RefCell<Vec<AppEntry>>> = Rc::new(RefCell::new(Vec::new()));

    {
        let apps_for_search = Rc::clone(&apps);
        let list_for_search = list_box.clone();
        let status_for_search = status_label.clone();
        search_entry.connect_search_changed(move |entry| {
            let found = apps_for_search.borrow();
            render_apps(
                &list_for_search,
                found.as_slice(),
                entry.text().as_str(),
                &status_for_search,
            );
        });
    }

    {
        let apps_for_refresh = Rc::clone(&apps);
        let list_for_refresh = list_box.clone();
        let search_for_refresh = search_entry.clone();
        let status_for_refresh = status_label.clone();
        refresh_button.connect_clicked(move |_| {
            refresh_apps(
                &apps_for_refresh,
                &list_for_refresh,
                &search_for_refresh,
                &status_for_refresh,
            );
        });
    }

    refresh_apps(&apps, &list_box, &search_entry, &status_label);
    window.present();
}

fn refresh_apps(
    apps: &Rc<RefCell<Vec<AppEntry>>>,
    list_box: &gtk::ListBox,
    search: &gtk::SearchEntry,
    status: &gtk::Label,
) {
    match list_installed_apps() {
        Ok(found) => {
            let count = found.len();
            *apps.borrow_mut() = found;
            let session = waydroid_session_summary();
            status.set_text(&format!("{session} · {count} apps"));
        }
        Err(error) => status.set_text(&error),
    }

    let found = apps.borrow();
    render_apps(list_box, found.as_slice(), search.text().as_str(), status);
}

fn render_apps(
    list_box: &gtk::ListBox,
    apps: &[AppEntry],
    search_query: &str,
    status: &gtk::Label,
) {
    while let Some(child) = list_box.first_child() {
        list_box.remove(&child);
    }

    let query = search_query.trim().to_lowercase();
    let visible: Vec<&AppEntry> = apps
        .iter()
        .filter(|app| {
            query.is_empty()
                || app.name.to_lowercase().contains(&query)
                || app.package.to_lowercase().contains(&query)
        })
        .collect();

    if visible.is_empty() {
        let message = if apps.is_empty() {
            "No installed apps found. Start the Waydroid session, then press Refresh."
        } else {
            "No apps match this search."
        };
        let row = gtk::ListBoxRow::new();
        row.set_activatable(false);
        let label = gtk::Label::new(Some(message));
        label.set_xalign(0.0);
        label.set_wrap(true);
        label.set_margin_top(14);
        label.set_margin_bottom(14);
        label.set_margin_start(12);
        label.set_margin_end(12);
        row.set_child(Some(&label));
        list_box.append(&row);
        return;
    }

    for entry in visible {
        let row = gtk::ListBoxRow::new();
        row.set_activatable(false);

        let line = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        line.set_margin_top(8);
        line.set_margin_bottom(8);
        line.set_margin_start(10);
        line.set_margin_end(10);

        let icon = gtk::Image::from_icon_name("application-x-executable-symbolic");
        icon.set_pixel_size(32);
        line.append(&icon);

        let details = gtk::Box::new(gtk::Orientation::Vertical, 3);
        details.set_hexpand(true);

        let name = gtk::Label::new(Some(&entry.name));
        name.set_xalign(0.0);
        name.set_wrap(true);
        name.add_css_class("heading");

        let package = gtk::Label::new(Some(&entry.package));
        package.set_xalign(0.0);
        package.set_wrap(true);
        package.add_css_class("dim-label");

        details.append(&name);
        details.append(&package);
        line.append(&details);

        let launch_button = gtk::Button::with_label("Open");
        launch_button.add_css_class("suggested-action");
        let package_name = entry.package.clone();
        let status_for_launch = status.clone();
        launch_button.connect_clicked(move |_| match launch_app(&package_name) {
            Ok(()) => status_for_launch.set_text(&format!("Launch requested: {package_name}")),
            Err(error) => status_for_launch.set_text(&error),
        });
        line.append(&launch_button);

        row.set_child(Some(&line));
        list_box.append(&row);
    }
}

fn list_installed_apps() -> Result<Vec<AppEntry>, String> {
    let output = run_waydroid(&["app", "list"])?;
    Ok(parse_app_list(&output))
}

fn waydroid_session_summary() -> String {
    match run_waydroid(&["status"]) {
        Ok(output) => output
            .lines()
            .find_map(|line| field_value(line, "Session"))
            .map(|value| format!("Waydroid session: {}", value.trim()))
            .unwrap_or_else(|| "Waydroid detected".to_string()),
        Err(error) => error,
    }
}

fn launch_app(package: &str) -> Result<(), String> {
    if !is_valid_package_name(package) {
        return Err("Refusing to launch an invalid Android package name.".to_string());
    }

    let result = Command::new("waydroid")
        .args(["app", "launch", package])
        .output()
        .map_err(|error| format!("Could not start Waydroid: {error}"))?;

    if result.status.success() {
        Ok(())
    } else {
        Err(command_failure(
            "Waydroid could not launch the app",
            &result,
        ))
    }
}

fn run_waydroid(arguments: &[&str]) -> Result<String, String> {
    let result = Command::new("waydroid")
        .args(arguments)
        .output()
        .map_err(|error| {
            format!("Could not run Waydroid. Is it installed and on PATH? ({error})")
        })?;

    if result.status.success() {
        Ok(String::from_utf8_lossy(&result.stdout).into_owned())
    } else {
        Err(command_failure("Waydroid command failed", &result))
    }
}

fn command_failure(context: &str, output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!("{context} (exit status {}).", output.status)
    } else {
        format!("{context}: {stderr}")
    }
}

fn field_value<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (field, value) = line.split_once(':')?;
    field
        .trim()
        .eq_ignore_ascii_case(key)
        .then_some(value.trim())
}

fn parse_app_list(input: &str) -> Vec<AppEntry> {
    let mut current_name: Option<String> = None;
    let mut apps = Vec::new();

    for line in input.lines() {
        if let Some(name) = field_value(line, "Name") {
            current_name = (!name.is_empty()).then(|| name.to_string());
        } else if let Some(package) = field_value(line, "packageName") {
            if is_valid_package_name(package) {
                let name = current_name
                    .take()
                    .filter(|value| !value.is_empty())
                    .unwrap_or_else(|| package.to_string());
                apps.push(AppEntry {
                    name,
                    package: package.to_string(),
                });
            } else {
                current_name = None;
            }
        }
    }

    let mut seen_packages = HashSet::new();
    apps.retain(|app| seen_packages.insert(app.package.clone()));
    apps.sort_by(|left, right| {
        left.name
            .to_lowercase()
            .cmp(&right.name.to_lowercase())
            .then_with(|| left.package.cmp(&right.package))
    });
    apps
}

fn is_valid_package_name(package: &str) -> bool {
    let mut parts = package.split('.');
    let segments: Vec<&str> = parts.by_ref().collect();

    segments.len() >= 2
        && segments.iter().all(|segment| {
            !segment.is_empty()
                && segment
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_alphabetic())
                && segment
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || character == '_')
        })
        && package.len() <= 255
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_sorts_waydroid_app_entries() {
        let apps = parse_app_list(
            "Name: YouTube\npackageName: com.google.android.youtube\n\nName: Calculator\npackageName: com.android.calculator2\n",
        );

        assert_eq!(
            apps,
            vec![
                AppEntry {
                    name: "Calculator".to_string(),
                    package: "com.android.calculator2".to_string(),
                },
                AppEntry {
                    name: "YouTube".to_string(),
                    package: "com.google.android.youtube".to_string(),
                },
            ]
        );
    }

    #[test]
    fn ignores_invalid_packages_and_uses_package_as_fallback_name() {
        let apps = parse_app_list(
            "Name: Broken\npackageName: --delete\npackageName: org.example.valid_app\n",
        );

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "org.example.valid_app");
        assert_eq!(apps[0].package, "org.example.valid_app");
    }

    #[test]
    fn field_names_are_case_insensitive_and_duplicate_packages_are_removed() {
        let apps = parse_app_list(
            "name: First label\nPACKAGENAME: org.example.app\nName: Duplicate\npackageName: org.example.app\n",
        );

        assert_eq!(apps.len(), 1);
        assert_eq!(apps[0].name, "First label");
    }
}
