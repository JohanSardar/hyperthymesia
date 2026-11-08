#!/usr/bin/env python3
"""Exercise real local CLI/HTTP paths with new state, no login or model calls."""
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
import urllib.error
import urllib.request

ROOT = Path(__file__).resolve().parent.parent
target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
if not target.is_absolute():
    target = ROOT / target
binary = target / "debug/hx-server"

with tempfile.TemporaryDirectory(prefix="hx-quickstart-") as tmp:
    home = Path(tmp)
    state = home / "state"
    env = {
        "HOME": str(home), "PATH": os.environ["PATH"],
        "HX_PROVIDER": "stub", "HX_HOST_BASH": "off",
    }

    def cli(*args, state_dir=state):
        result = subprocess.run(
            [str(binary), "--state-dir", str(state_dir), *args],
            env=env, cwd=home, text=True, capture_output=True, timeout=30,
        )
        if result.returncode:
            raise AssertionError(f"CLI {args[0]} exited {result.returncode}")
        return result.stdout

    cli("migrate")
    for name in ("server.env", "server.log"):
        (state / name).touch(mode=0o600)
    cli("doctor", "--state-only")
    assert json.loads(cli("status"))["running_interpret_leases"] == 0
    token = cli("token", "create", "--name", "synthetic-reader",
                "--prefix", "example:", "--scopes", "read_raw,read_l3").strip()
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    process = subprocess.Popen(
        [str(binary), "--state-dir", str(state), "serve",
         "--listen", f"127.0.0.1:{port}"],
        env=env, cwd=home, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    base = f"http://127.0.0.1:{port}"
    try:
        deadline = time.monotonic() + 20
        while True:
            if process.poll() is not None:
                raise AssertionError("server exited before health")
            try:
                with urllib.request.urlopen(base + "/healthz", timeout=1) as response:
                    assert response.read() == b"ok\n"
                break
            except OSError:
                if time.monotonic() >= deadline:
                    raise AssertionError("server health timeout") from None
                time.sleep(0.1)
        try:
            urllib.request.urlopen(base + "/v1/now", timeout=3)
            raise AssertionError("unauthenticated reader was accepted")
        except urllib.error.HTTPError as error:
            assert error.code == 401
        request = urllib.request.Request(base + "/v1/now",
                                         headers={"Authorization": "Bearer " + token})
        with urllib.request.urlopen(request, timeout=3) as response:
            json.load(response)
        backup = home / "backup"
        cli("backup", "--to", str(backup))
        restored = home / "restored"
        cli("restore", "--from", str(backup), state_dir=restored)
        for name in ("server.env", "server.log"):
            (restored / name).touch(mode=0o600)
        cli("doctor", "--state-only", state_dir=restored)
        assert "synthetic-reader" in cli("token", "list", state_dir=restored)
        print("ok: migrate, state doctor/status, loopback health/auth, online backup and restore")
    finally:
        process.send_signal(signal.SIGTERM)
        try:
            assert process.wait(timeout=10) == 0
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            raise
