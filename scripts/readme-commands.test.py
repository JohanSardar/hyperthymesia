#!/usr/bin/env python3
from pathlib import Path
import unittest

from readme_commands import command_block


class ReadmeCommandsTest(unittest.TestCase):
    def test_exact_section_not_a_prefix_or_next_section(self):
        text = "## Install from source\n```sh\nwrong\n```\n## Install\n```sh\nright\n```\n## Next\n```sh\nwrong\n```"
        self.assertEqual(command_block(text, "Install"), "right")

    def test_missing_ambiguous_and_empty_examples_are_refused(self):
        for text in ("## Other\n", "## Install\n```sh\n\n```\n",
                     "## Install\n```sh\none\n```\n```sh\ntwo\n```\n"):
            with self.assertRaises(ValueError):
                command_block(text, "Install")

    def test_actual_readme_has_public_install_and_login_free_check(self):
        root = Path(__file__).resolve().parent.parent
        readme = (root / "README.md").read_text()
        public_url = "https://hyperthymesia.eilmer.si/install"
        self.assertEqual(command_block(readme, "Install"), f"curl -fsSL {public_url} | sh")
        quickstart = command_block(readme, "30-second check")
        self.assertIn("doctor --state-only", quickstart)
        self.assertIn("/healthz", quickstart)
        self.assertNotIn(" login", quickstart)
        self.assertNotIn(" interpret", quickstart)


if __name__ == "__main__":
    unittest.main()
