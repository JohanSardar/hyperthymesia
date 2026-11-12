#!/usr/bin/env python3
"""Test the actual piped Pages launcher with real hashes and offline curl fixtures."""
import hashlib
import os
from pathlib import Path
import re
import subprocess
import tempfile

root = Path(__file__).resolve().parent.parent
source = (root / "docs/install").read_text()
version = "9.8.7"

with tempfile.TemporaryDirectory(prefix="hx-launcher-test-") as tmp:
    work = Path(tmp)
    release = work / "release"
    release.mkdir()
    tools = work / "bin"
    tools.mkdir()
    curl = tools / "curl"
    curl.write_text("""#!/bin/sh
set -eu
[ "$#" = 4 ] && [ "$1" = -fsSL ] && [ "$3" = -o ] || exit 97
case "$2" in
  https://hyperthymesia.eilmer.si/releases/download/v9.8.7/SHA256SUMS) cp "$TEST_RELEASE/SHA256SUMS" "$4" ;;
  https://hyperthymesia.eilmer.si/releases/download/v9.8.7/install.sh) cp "$TEST_RELEASE/install.sh" "$4" ;;
  *) exit 97 ;;
esac
""")
    curl.chmod(0o700)
    payload = b"""#!/bin/sh
set -eu
case "$HX_RELEASE_BASE" in https://hyperthymesia.eilmer.si/releases/download/v9.8.7) ;; *) exit 98;; esac
[ "${#HX_RELEASE_SHA256}" = 64 ] || exit 98
printf '%s' "$*" > "$HOME/installed"
"""
    (release / "install.sh").write_bytes(payload)

    def manifest():
        sums = hashlib.sha256(payload).hexdigest() + "  install.sh\n"
        (release / "SHA256SUMS").write_text(sums)
        return sums.encode()

    def launcher(data):
        text = re.sub(r"(?m)^VERSION=.*$", "VERSION=" + version, source)
        return re.sub(r"(?m)^SHA256=.*$", "SHA256=" + hashlib.sha256(data).hexdigest(), text)

    def run(text, expected):
        home = work / f"home-{len(list(work.glob('home-*')))}"
        home.mkdir()
        scratch = home / "tmp"
        scratch.mkdir()
        env = {
            "HOME": str(home), "PATH": str(tools) + os.pathsep + os.environ["PATH"],
            "TEST_RELEASE": str(release), "TMPDIR": str(scratch),
            "HX_RELEASE_BASE": "file:///must-not-be-used",
        }
        result = subprocess.run(["/bin/sh", "-s", "--", "--server-only"],
                                input=text, env=env, cwd=home, text=True,
                                capture_output=True, timeout=10)
        assert (result.returncode == 0) == expected, result.stderr
        assert (home / "installed").exists() == expected
        assert not list(scratch.iterdir()), "launcher scratch leaked"
        if expected:
            assert (home / "installed").read_text() == "--server-only"
        else:
            assert "install:" in result.stderr

    good = manifest()
    pinned = launcher(good)
    run(pinned, True)
    (release / "SHA256SUMS").write_bytes(good + b"tampered\n")
    run(pinned, False)
    manifest()
    (release / "install.sh").write_bytes(payload + b"\n# tampered\n")
    run(pinned, False)
    (release / "install.sh").write_bytes(payload)
    duplicate = good + good
    (release / "SHA256SUMS").write_bytes(duplicate)
    run(launcher(duplicate), False)
    print("ok: piped pinned launcher, manifest/installer tampering, duplicate sums, argument forwarding and cleanup")
