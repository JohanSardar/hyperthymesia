#!/usr/bin/env python3
"""Check tracked Python and shell syntax without creating bytecode or running capture."""
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parent.parent
paths = subprocess.check_output(
    ["git", "-C", str(root), "ls-files", "-z", "*.py", "*.sh"]
).decode().split("\0")
for name in filter(None, paths):
    path = root / name
    if not path.is_file():
        continue
    if path.suffix == ".py":
        compile(path.read_bytes(), name, "exec")
    else:
        subprocess.run(["bash", "-n", str(path)], check=True)
print("ok: tracked Python and shell syntax")
