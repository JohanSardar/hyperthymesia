#!/usr/bin/env python3
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("release_notes", Path(__file__).with_name("release-notes.py"))
notes = importlib.util.module_from_spec(spec)
spec.loader.exec_module(notes)

CHANGELOG = """# Changelog
## [Unreleased]
not part of the release
## [0.1.3] - 2026-11-08
### Fixed
- Synthetic fixture.
## [0.1.2] - 2026-11-01
not part of the release either
"""


class ReleaseNotesTest(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory(prefix="hx-release-notes-")
        self.addCleanup(self.scratch.cleanup)
        self.assets = Path(self.scratch.name)
        names = [f"hx-server-0.1.3-{p}.tar.gz" for p in
                 ("linux-x86_64", "linux-aarch64", "macos-aarch64")]
        names += ["Hyperthymesia-0.1.3-macos-aarch64.zip", "install.sh"]
        for name in names:
            (self.assets / name).write_bytes(b"synthetic archive")
        self.manifest = self.assets / "SHA256SUMS"
        self.manifest.write_text("".join(f"{notes.sha256(self.assets / n)}  {n}\n" for n in names))

    def render(self, tag="v0.1.3", changelog=CHANGELOG):
        return notes.render(tag, self.assets, changelog, "0.1.3")

    def test_actual_hashes_and_only_selected_changelog_entry(self):
        body = self.render()
        self.assertIn(notes.sha256(self.manifest), body)
        self.assertIn(self.manifest.read_text().strip(), body)
        self.assertIn("Synthetic fixture.", body)
        self.assertNotIn("not part", body)

    def test_tampered_asset_is_refused(self):
        (self.assets / "install.sh").write_text("tampered")
        with self.assertRaisesRegex(ValueError, "checksum mismatch"):
            self.render()

    def test_duplicate_and_missing_assets_are_refused(self):
        original = self.manifest.read_text()
        self.manifest.write_text(original + original.splitlines()[0] + "\n")
        with self.assertRaisesRegex(ValueError, "duplicate"):
            self.render()
        self.manifest.write_text("\n".join(original.splitlines()[1:]) + "\n")
        with self.assertRaisesRegex(ValueError, "missing"):
            self.render()

    def test_unlisted_files_and_symlink_assets_are_refused(self):
        extra = self.assets / "unlisted.txt"
        extra.write_text("not checksummed")
        with self.assertRaisesRegex(ValueError, "unexpected"):
            self.render()
        extra.unlink()
        installer = self.assets / "install.sh"
        installer.unlink()
        installer.symlink_to(self.assets / "SHA256SUMS")
        with self.assertRaisesRegex(ValueError, "regular"):
            self.render()

    def test_wrong_tag_missing_changelog_and_unsafe_paths_are_refused(self):
        with self.assertRaisesRegex(ValueError, "tag"):
            self.render(tag="v0.1.2")
        with self.assertRaisesRegex(ValueError, "changelog"):
            self.render(changelog="# Empty\n")
        self.manifest.write_text("0" * 64 + "  ../install.sh\n")
        with self.assertRaisesRegex(ValueError, "checksum line"):
            self.render()


if __name__ == "__main__":
    unittest.main()
