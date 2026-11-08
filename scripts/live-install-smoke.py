#!/usr/bin/env python3
"""Run the real public curl command on a fresh hosted VM, never a developer host."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import urllib.request

assert os.environ.get("GITHUB_ACTIONS") == "true", "requires disposable hosted CI"
assert os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted", "not a self-hosted runner"
assert os.uname().sysname == "Linux", "this acceptance run targets a fresh Linux VM"
root = Path(__file__).resolve().parent.parent
home = Path.home()
prefix = home / ".local/share/hyperthymesia"
command = "curl -fsSL https://johansardar.github.io/hyperthymesia/install | sh"
assert not prefix.exists(), "VM already has a Hyperthymesia installation"
assert not (home / ".local/bin/hx-server").exists(), "VM has an unrelated command"
env = {k: v for k, v in os.environ.items() if not k.startswith(("HX_", "FX_"))}
env["PATH"] = str(home / ".local/bin") + os.pathsep + env["PATH"]


def run(args, **kwargs):
    return subprocess.run(args, env=env, check=True, timeout=kwargs.pop("timeout", 120),
                          **kwargs)


def install():
    start = time.monotonic()
    run(["/bin/bash", "-o", "pipefail", "-c", command])
    print(f"real curl to healthy service: {time.monotonic() - start:.2f}s", flush=True)


try:
    install()
    server = home / ".local/bin/hx-server"
    actual = subprocess.check_output([server, "--version"], env=env, text=True).strip()
    assert actual == "hx-server " + os.environ["EXPECTED_VERSION"], actual
    print(actual, flush=True)
    state = Path((prefix / ".state-dir").read_text().strip())
    env["HX_STATE_DIR"] = str(state)
    run([server, "doctor", "--state-only"])
    status = json.loads(subprocess.check_output([server, "status"], env=env))
    assert status["running_interpret_leases"] == 0
    # These are the README's login-free first-run commands, not provider checks.
    with urllib.request.urlopen("http://127.0.0.1:7377/healthz", timeout=3) as response:
        assert response.read() == b"ok\n"
    token = subprocess.check_output(
        [server, "token", "create", "--name", "synthetic-live-acceptance",
         "--prefix", "example:", "--scopes", "read_raw,read_l3"], env=env)
    assert token.strip()
    del token  # Never print or upload this credential.
    install()
    assert "synthetic-live-acceptance" in subprocess.check_output(
        [server, "token", "list"], env=env, text=True)
    assert list((prefix / "backups").glob("pre-upgrade.*"))
    print("ok: live release install/update, private state, real user service, retained metadata", flush=True)
    with tempfile.TemporaryDirectory(prefix="hx-fresh-contributor-",
                                     dir=os.environ["RUNNER_TEMP"]) as tmp:
        clone = Path(tmp) / "source"
        run(["git", "clone", "--depth", "1",
             "https://github.com/JohanSardar/hyperthymesia.git", str(clone)])
        assert (clone / "CONTRIBUTING.md").is_file()
        # A fresh clone and its default verification entry point, no source
        # checkout or compiled artifacts from the development machine.
        start = time.monotonic()
        run(["./scripts/check.sh"], cwd=clone, timeout=1500)
        print(f"fresh-clone contributor checks: {time.monotonic() - start:.2f}s", flush=True)
finally:
    if (prefix / ".installed").is_file():
        run(["/bin/sh", str(root / "scripts/install.sh"), "--uninstall"])
