#!/usr/bin/python3
"""Cargo runner: prevent connector fixtures from consulting the owner's home.

Set CARGO_TARGET_<HOST>_RUNNER to this executable. Build tools keep their usual
configuration; only Rust test binaries receive the empty synthetic home.
"""
import os
import pathlib
import subprocess
import sys
import tempfile

with tempfile.TemporaryDirectory(prefix="hx-synthetic-tests-") as root:
    # The fixture anonymizer sources this file even in stdin-only mode.
    cargo = pathlib.Path(root) / ".cargo"
    cargo.mkdir()
    (cargo / "env").write_text("# synthetic test environment\n")
    key = pathlib.Path(root) / "synthetic-fixture-key"
    key.write_bytes(b"synthetic-only-fixture-key-32byte!")
    environment = os.environ.copy()
    environment["HX_FIXTURE_KEY_FILE"] = str(key)
    environment["HOME"] = root
    environment.pop("DATABASE_URL", None)
    environment.pop("HX_TEST_DATABASE_URL", None)
    environment.pop("HX_STATE_DIR", None)
    sys.exit(subprocess.run(sys.argv[1:], env=environment, check=False).returncode)
