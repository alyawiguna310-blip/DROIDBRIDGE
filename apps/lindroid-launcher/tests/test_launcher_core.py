import unittest

from launcher_core import AppEntry, is_valid_package_name, parse_app_list


class PackageNameTests(unittest.TestCase):
    def test_accepts_dotted_package_names(self):
        self.assertTrue(is_valid_package_name("com.android.settings"))
        self.assertTrue(is_valid_package_name("org.example.app_2"))

    def test_rejects_shell_metacharacters_and_invalid_segments(self):
        for package in ("", "org", "--remove", "org.example;id", "org..app", ".org.app", "org.2app"):
            with self.subTest(package=package):
                self.assertFalse(is_valid_package_name(package))


class AppListTests(unittest.TestCase):
    def test_parses_and_sorts_waydroid_app_entries(self):
        apps = parse_app_list(
            "Name: YouTube\npackageName: com.google.android.youtube\n"
            "categories:\n android.intent.category.LAUNCHER\n"
            "Name: Calculator\npackageName: com.android.calculator2\n"
        )
        self.assertEqual(
            apps,
            [
                AppEntry("Calculator", "com.android.calculator2"),
                AppEntry("YouTube", "com.google.android.youtube"),
            ],
        )

    def test_invalid_package_is_ignored_and_missing_name_falls_back(self):
        apps = parse_app_list(
            "Name: Broken\npackageName: --bad\n"
            "packageName: org.example.valid_app\n"
        )
        self.assertEqual(apps, [AppEntry("org.example.valid_app", "org.example.valid_app")])

    def test_field_names_are_case_insensitive_and_duplicates_are_removed(self):
        apps = parse_app_list(
            "name: First label\nPACKAGENAME: org.example.app\n"
            "Name: Duplicate\npackageName: org.example.app\n"
        )
        self.assertEqual(apps, [AppEntry("First label", "org.example.app")])


if __name__ == "__main__":
    unittest.main()
