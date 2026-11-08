#!/usr/bin/env python3
"""Render changelog and independently verified release checksums before publication."""
import hashlib
from pathlib import Path
import re
import sys

ROOT = Path(__file__).resolve().parent.parent


def sha256(path):
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def render(tag, assets, changelog, version):
    if tag != "v" + version or not re.fullmatch(r"v[0-9][0-9A-Za-z.+-]*", tag):
        raise ValueError("tag must match the server/app version")
    expected = {
        f"hx-server-{version}-{platform}.tar.gz"
        for platform in ("linux-x86_64", "linux-aarch64", "macos-aarch64")
    } | {f"Hyperthymesia-{version}-macos-aarch64.zip", "install.sh"}
    paths = list(assets.iterdir())
    if ({p.name for p in paths} != expected | {"SHA256SUMS"}
            or any(not p.is_file() or p.is_symlink() for p in paths)):
        raise ValueError("unexpected or missing regular release asset")
    manifest = assets / "SHA256SUMS"
    entries = []
    for line in manifest.read_text(encoding="ascii").splitlines():
        match = re.fullmatch(r"([a-f0-9]{64})  ([A-Za-z0-9.+_-]+)", line)
        if not match:
            raise ValueError("invalid checksum line")
        digest, name = match.groups()
        entries.append(name)
        if name not in expected or sha256(assets / name) != digest:
            raise ValueError("asset set or checksum mismatch")
    if set(entries) != expected or len(entries) != len(expected):
        raise ValueError("missing or duplicate asset")
    entry = re.search(rf"(?m)^## \[{re.escape(version)}\] - \d{{4}}-\d{{2}}-\d{{2}}\n", changelog)
    if not entry:
        raise ValueError("missing dated changelog entry")
    # Split after the selected heading, not on the selected heading itself.
    remainder = changelog[entry.end():]
    body = changelog[entry.start():entry.end()] + re.split(r"(?m)^## \[", remainder, maxsplit=1)[0]
    return body.rstrip() + f"""

## Verified SHA-256 checksums

SHA256SUMS itself: `{sha256(manifest)}`

The Pages launcher pins that manifest hash, which binds the installer and every
platform archive. The manifest does not authenticate itself.

```text
{manifest.read_text(encoding="ascii").rstrip()}
```

Download the assets and SHA256SUMS together, then run `sha256sum --check SHA256SUMS`
(macOS: `shasum -a 256 --check SHA256SUMS`). No registry packages are published.
"""


def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: release-notes.py TAG ASSET_DIRECTORY")
    versions = []
    for crate in ("hx-server", "hx-agent-app"):
        text = (ROOT / f"crates/{crate}/Cargo.toml").read_text()
        versions.append(re.search(r'(?m)^version = "([^"]+)"$', text)[1])
    if versions[0] != versions[1]:
        raise SystemExit("server and app versions must match")
    try:
        print(render(sys.argv[1], Path(sys.argv[2]), (ROOT / "CHANGELOG.md").read_text(),
                     versions[0]), end="")
    except (OSError, ValueError) as error:
        raise SystemExit(str(error)) from error


if __name__ == "__main__":
    main()
