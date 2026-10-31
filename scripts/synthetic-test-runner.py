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
    database = environment.get("HX_TEST_DATABASE_URL", "")
    if not database or any(part in database for part in ("hx_e2e", "hx-e2e")):
        sys.exit("Set HX_TEST_DATABASE_URL to your own synthetic scratch database")
    sys.exit(subprocess.run(sys.argv[1:], env=environment, check=False).returncode)
