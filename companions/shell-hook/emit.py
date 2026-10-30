#!/usr/bin/env python3
"""Interactive zsh observations only. Input stays on stdin, never command argv."""
import json
import os
import re
import socket
import sys

# Drop the entire command when secret-bearing syntax is present. Partial argv
# rewriting cannot safely account for shell quoting, expansion and pipelines.
SECRET = re.compile(
    r"(?i)(?:\b[\w-]*(?:token|password|passwd|secret|credential|api[_-]?key)[\w-]*\b|"
    r"\b[A-Z_][A-Z0-9_]*_KEY\s*=|\bbearer\s+|"
    r"\b[a-z][a-z0-9+.-]*://[^\s/]*@|"
    r"\b(?:sk|ghp|gho|github_pat|xox[baprs])[-_][A-Za-z0-9_-]+|\bAKIA[A-Z0-9]{16}\b)"
)


def redact(command):
    return "[REDACTED: secret-bearing command]" if SECRET.search(command) else command


def build_event(data):
    return {
        "companion": "shell", "kind": "shell.command",
        "source_key": data["source_key"], "occurred_at": data["occurred_at"],
        "ended_at": data["ended_at"], "sensitivity": "restricted",
        "payload": {"command": redact(data["command"]), "cwd": data["cwd"],
                    "exit_code": data["exit_code"], "duration_ms": data["duration_ms"], "shell": "zsh"},
    }


def main():
    try:
        data = json.load(sys.stdin)
        path = os.environ.get("HYPERTHYMESIA_SOCKET", os.path.expanduser("~/Library/Application Support/Hyperthymesia/agent.sock"))
        if not os.path.exists(path):
            return
        event = build_event(data)
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as client:
            client.settimeout(0.2)
            client.connect(path)
            client.sendall((json.dumps(event, ensure_ascii=True) + "\n").encode())
    except (OSError, ValueError, KeyError, TypeError):
        pass


if __name__ == "__main__":
    main()
