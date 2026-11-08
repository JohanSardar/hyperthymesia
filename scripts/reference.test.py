#!/usr/bin/env python3
"""Offline checks for documentation drift/refusal and route expression parsing."""
import contextlib
import io
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

import reference


class ReferenceTest(unittest.TestCase):
    def test_balanced_nested_call_and_quoted_parentheses(self):
        source = 'prefix("/api", get(handler).post(other(")")))suffix'
        self.assertEqual(reference.balanced(source, 6),
                         '("/api", get(handler).post(other(")")))')

    def test_unbalanced_expression_is_not_silently_truncated(self):
        with self.assertRaises(ValueError):
            reference.balanced('(get(")"),', 0)

    def test_generation_check_missing_and_stale_refusal(self):
        with tempfile.TemporaryDirectory(prefix="hx-reference-test-") as home:
            root = Path(home)
            (root / "README.md").write_text("# Synthetic\n")
            out = root / "docs/reference"
            with patch.object(reference, "ROOT", root), patch.object(reference, "OUT", out), \
                 patch.object(reference, "generated", return_value={"fixture.md": "current\n"}), \
                 contextlib.redirect_stdout(io.StringIO()):
                with patch("sys.argv", ["reference"]):
                    reference.main()
                with patch("sys.argv", ["reference", "--check"]):
                    reference.main()
                    (out / "fixture.md").write_text("stale\n")
                    with self.assertRaisesRegex(SystemExit, "Reference drift"):
                        reference.main()
                    (out / "fixture.md").unlink()
                    with self.assertRaisesRegex(SystemExit, "Reference drift"):
                        reference.main()

    def test_local_link_refusal_and_external_link_exemption(self):
        with tempfile.TemporaryDirectory(prefix="hx-reference-test-") as home:
            root = Path(home)
            readme = root / "README.md"
            readme.write_text("[public](https://example.invalid/no-file) [here](#heading)\n")
            with patch.object(reference, "ROOT", root):
                reference.check_links()
                readme.write_text("[missing](docs/not-present.md)\n")
                with self.assertRaisesRegex(SystemExit, "Broken local link"):
                    reference.check_links()


if __name__ == "__main__":
    unittest.main()
