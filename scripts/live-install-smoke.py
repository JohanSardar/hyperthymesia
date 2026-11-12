#!/usr/bin/env python3
"""Run the real public curl command on a fresh hosted VM, never a developer host."""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import time
import urllib.request
from readme_commands import command_block

assert os.environ.get("GITHUB_ACTIONS") == "true", "requires disposable hosted CI"
assert os.environ.get("RUNNER_ENVIRONMENT") == "github-hosted", "not a self-hosted runner"
assert os.uname().sysname == "Linux", "this acceptance run targets a fresh Linux VM"
root = Path(__file__).resolve().parent.parent
home = Path.home()
prefix = home / ".local/share/hyperthymesia"
command = "curl -fsSL https://hyperthymesia.eilmer.si/install | sh"
readme = (root / "README.md").read_text()
assert command_block(readme, "Install") == command, "README public command drift"
quickstart = command_block(readme, "30-second check")
assert not (prefix.exists() or prefix.is_symlink()), "VM already has a Hyperthymesia installation"
entrypoint = home / ".local/bin/hx-server"
assert not (entrypoint.exists() or entrypoint.is_symlink()), "VM has an unrelated command"
env = {k: v for k, v in os.environ.items()
       if not k.startswith(("HX_", "FX_", "AI_GATEWAY_"))
       and not re.search(r"TOKEN|SECRET|PASSWORD|CREDENTIAL|API_KEY", k)}
bin_dir = str(home / ".local/bin")
env["PATH"] = os.pathsep.join([bin_dir, *[p for p in env["PATH"].split(os.pathsep) if p != bin_dir]])


def run(args, **kwargs):
    return subprocess.run(args, env=kwargs.pop("env", env), check=True,
                          timeout=kwargs.pop("timeout", 120),
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
    start = time.monotonic()
    run(["/bin/bash", "-o", "pipefail", "-c", quickstart], timeout=30)
    elapsed = time.monotonic() - start
    assert elapsed < 30, "README check exceeded thirty seconds"
    print(f"exact README login-free check: {elapsed:.2f}s", flush=True)
    state = Path((prefix / ".state-dir").read_text().strip())
    env["HX_STATE_DIR"] = str(state)
    run([server, "doctor", "--state-only"])
    status = json.loads(subprocess.check_output([server, "status"], env=env))
    assert status["running_interpret_leases"] == 0
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
    print("ok: live release install/rerun, private state, real user service, retained metadata", flush=True)
    with tempfile.TemporaryDirectory(prefix="hx-fresh-contributor-",
                                     dir=os.environ["RUNNER_TEMP"]) as tmp:
        clone = Path(tmp) / "source"
        clone_env = {k: v for k, v in env.items() if not k.startswith(("HX_", "FX_", "GIT_"))}
        clone_env.update(GIT_CONFIG_GLOBAL="/dev/null", GIT_CONFIG_NOSYSTEM="1",
                         GIT_TERMINAL_PROMPT="0", CARGO_TARGET_DIR=str(Path(tmp) / "build"))
        assert not Path(clone_env["CARGO_TARGET_DIR"]).exists()
        run(["git", "clone", "https://gitlab.com/johansardar/hyperthymesia.git", str(clone)],
            cwd=tmp, env=clone_env)
        assert (clone / "CONTRIBUTING.md").is_file()
        # A fresh clone and its default verification entry point, no source
        # checkout or compiled artifacts from the development machine.
        start = time.monotonic()
        run(["./scripts/check.sh"], cwd=clone, env=clone_env, timeout=1500)
        print(f"fresh-clone contributor checks: {time.monotonic() - start:.2f}s", flush=True)
finally:
    if (prefix / ".installed").is_file():
        run(["/bin/sh", str(root / "scripts/install.sh"), "--uninstall"])
