#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Check fixture directory safety without allocating APFS clones or large files."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().with_name("demo_fixture.sh")


class DemoFixtureTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="diskray-demo-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.bin = self.root / "bin"
        self.bin.mkdir()
        # Keep native path and file operations, substituting only the expensive
        # seed generation and macOS-specific cloning/date commands.
        stubs = {
            "dd": 'for arg do case "$arg" in of=*) printf fixture > "${arg#of=}" ;; esac; done',
            "cp": 'test "$1" = -c && /bin/cp "$2" "$3"',
            "date": "printf '202001010000\\n'",
        }
        for name, body in stubs.items():
            command = self.bin / name
            command.write_text("#!/bin/sh\nset -eu\n" + body + "\n")
            command.chmod(0o755)
        self.env = {
            **os.environ,
            "PATH": str(self.bin) + os.pathsep + os.environ["PATH"],
            "TMPDIR": str(self.root),
        }

    def run_fixture(self, *args):
        return subprocess.run(
            ["/bin/sh", str(SCRIPT), *map(str, args)], cwd=self.root,
            env=self.env, capture_output=True, text=True, timeout=20,
        )

    def assert_refused(self, *args):
        result = self.run_fixture(*args)
        self.assertNotEqual(result.returncode, 0, result.stdout)
        self.assertEqual(result.stdout, "")
        self.assertTrue(result.stderr)
        return result

    def assert_fixture(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)
        folder = Path(result.stdout.strip())
        self.assertTrue(folder.is_absolute())
        self.assertEqual((folder / "code/api/target/debug/blob-0").read_text(), "fixture")
        self.assertTrue((folder / "code/api/Cargo.toml").is_file())
        self.assertTrue((folder / "Downloads/installer.dmg.crdownload").is_file())
        self.assertFalse((folder / ".seed").exists())
        return folder

    def test_existing_user_files_are_never_deleted(self):
        folder = self.root / "user-data"
        folder.mkdir()
        sentinel = folder / "important.txt"
        sentinel.write_text("keep me")
        self.assert_refused(folder)
        self.assertEqual(sentinel.read_text(), "keep me")
        self.assertEqual(list(folder.iterdir()), [sentinel])

    def test_hidden_contents_are_preserved(self):
        folder = self.root / "hidden"
        folder.mkdir()
        for name in (".secret", "..also-hidden"):
            with self.subTest(name=name):
                sentinel = folder / name
                sentinel.write_text("keep me")
                self.assert_refused(folder)
                self.assertEqual(sentinel.read_text(), "keep me")
                sentinel.unlink()

    def test_dangling_child_symlink_makes_directory_nonempty(self):
        folder = self.root / "links"
        folder.mkdir()
        sentinel = folder / ".dangling"
        sentinel.symlink_to("missing")
        self.assert_refused(folder)
        self.assertTrue(sentinel.is_symlink())
        self.assertEqual(os.readlink(sentinel), "missing")

    def test_file_root_is_preserved(self):
        sentinel = self.root / "file"
        sentinel.write_text("keep me")
        self.assert_refused(sentinel)
        self.assertEqual(sentinel.read_text(), "keep me")

    def test_symlink_root_is_refused_even_with_trailing_slashes(self):
        target = self.root / "target"
        target.mkdir()
        link = self.root / "alias"
        link.symlink_to(target, target_is_directory=True)
        for suffix in ("", "/", "///"):
            with self.subTest(suffix=suffix):
                self.assert_refused(str(link) + suffix)
                self.assertTrue(link.is_symlink())
                self.assertEqual(list(target.iterdir()), [])

    def test_dangling_root_symlink_is_preserved(self):
        link = self.root / "dangling"
        link.symlink_to(self.root / "missing")
        self.assert_refused(link)
        self.assertTrue(link.is_symlink())
        self.assertFalse((self.root / "missing").exists())

    def test_filesystem_root_and_empty_argument_are_refused(self):
        for argument in ("/", "///", ""):
            with self.subTest(argument=argument):
                self.assert_refused(argument)

    def test_extra_arguments_fail_before_creating_directory(self):
        folder = self.root / "unused"
        self.assert_refused(folder, "unexpected")
        self.assertFalse(folder.exists())

    def test_default_uses_distinct_new_directories_and_preserves_old_default(self):
        previous = self.root / "diskray-demo"
        previous.mkdir()
        sentinel = previous / "old-recording.txt"
        sentinel.write_text("keep me")
        first = self.assert_fixture(self.run_fixture())
        second = self.assert_fixture(self.run_fixture())
        self.assertNotEqual(first, second)
        self.assertEqual(first.parent, self.root)
        self.assertEqual(second.parent, self.root)
        self.assertEqual(sentinel.read_text(), "keep me")

    def test_explicit_empty_directory_supports_recording_tape(self):
        folder = self.root / "existing empty folder"
        folder.mkdir()
        self.assertEqual(self.assert_fixture(self.run_fixture(folder)), folder)
        self.assert_refused(folder)
        self.assertEqual((folder / "code/api/target/debug/blob-0").read_text(), "fixture")

    def test_new_relative_option_like_directory_is_supported(self):
        self.assertEqual(
            self.assert_fixture(self.run_fixture("-new-demo")),
            self.root / "-new-demo",
        )


if __name__ == "__main__":
    unittest.main()
