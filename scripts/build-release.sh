#!/usr/bin/env bash
# Native release build; no services, state access, fx bundling, or keychain writes.
set -euo pipefail
cd "$(dirname "$0")/.."

case "$(uname -s):$(uname -m)" in
    Linux:x86_64) platform=linux-x86_64; target=x86_64-unknown-linux-gnu ;;
    Linux:aarch64|Linux:arm64) platform=linux-aarch64; target=aarch64-unknown-linux-gnu ;;
    Darwin:arm64) platform=macos-aarch64; target=aarch64-apple-darwin ;;
    *) echo 'Unsupported release platform' >&2; exit 1 ;;
esac

work=$(mktemp -d "${TMPDIR:-/tmp}/hx-release.XXXXXXXX")
trap 'rm -rf "$work"' EXIT
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$work/target}"
mkdir -p "$CARGO_TARGET_DIR"
CARGO_TARGET_DIR=$(cd "$CARGO_TARGET_DIR" && pwd)
out="${HX_RELEASE_DIR:-$PWD/dist/release}"
mkdir -p "$out"
out=$(cd "$out" && pwd)
# Avoid stale assets entering SHA256SUMS or a later release.
if [[ -n "$(ls -A "$out")" ]]; then
    echo "Release output directory must be empty: $out" >&2
    exit 1
fi
version=$(cargo metadata --locked --no-deps --format-version 1 | python3 -c 'import json,sys; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == "hx-server"))')
[[ "$version" =~ ^[0-9][0-9A-Za-z.+-]*$ ]] || { echo 'Unsafe release version' >&2; exit 1; }
if [[ "${GITHUB_REF:-}" == refs/tags/* && "${GITHUB_REF#refs/tags/}" != "v$version" ]]; then
    echo "Tag ${GITHUB_REF#refs/tags/} does not match workspace version v$version" >&2
    exit 1
fi

cargo build --locked --release -p hx-server
mkdir "$work/server"
cp "$CARGO_TARGET_DIR/release/hx-server" "$work/server/"
cp LICENSE "$work/server/"
printf 'https://github.com/JohanSardar/hyperthymesia\ncommit %s\n' "$(git rev-parse HEAD)" > "$work/server/SOURCE"
cargo about generate --locked --workspace --target "$target" --fail \
    scripts/release-licenses.hbs -o "$work/server/THIRD-PARTY.txt"
[[ -s "$work/server/THIRD-PARTY.txt" ]]

if [[ "$platform" == macos-aarch64 ]]; then
    # Explicit - prevents build-app's development identity discovery on releases.
    export HX_SIGN_IDENTITY="${HX_SIGN_IDENTITY:--}"
    export HX_APP_OUTPUT="$work/Hyperthymesia.app"
    scripts/build-app.sh
    cp "$CARGO_TARGET_DIR/release/hx-vision-ocr" "$work/server/"
    flags=(--force --sign "$HX_SIGN_IDENTITY")
    if [[ "$HX_SIGN_IDENTITY" != - ]]; then
        flags+=(--options runtime --timestamp)
    fi
    codesign "${flags[@]}" "$work/server/hx-server"
    codesign "${flags[@]}" "$work/server/hx-vision-ocr"
    cp "$work/server/"{LICENSE,SOURCE,THIRD-PARTY.txt} "$HX_APP_OUTPUT/Contents/Resources/"
    # Resource notices must be present before the final bundle seal.
    codesign "${flags[@]}" "$HX_APP_OUTPUT"
    codesign --verify --deep --strict "$HX_APP_OUTPUT"
    if [[ "${HX_NOTARIZE:-0}" == 1 ]]; then
        [[ "${GITHUB_ACTIONS:-}" == true && "$HX_SIGN_IDENTITY" != - ]] || {
            echo 'Notarization is only supported on the signed CI runner' >&2; exit 1;
        }
        # Notarize the standalone executables as well as the app. A server tar
        # cannot carry a stapled ticket, but Gatekeeper can retrieve it online.
        ditto -c -k --keepParent "$HX_APP_OUTPUT" "$work/notarize.zip"
        (cd "$work/server" && zip -j "$work/notarize.zip" hx-server hx-vision-ocr)
        xcrun notarytool submit "$work/notarize.zip" --wait \
            --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_PASSWORD"
        xcrun stapler staple "$HX_APP_OUTPUT"
        xcrun stapler validate "$HX_APP_OUTPUT"
    fi
fi

# Write root-level regular files only. Python avoids AppleDouble/PAX additions
# and preserves executable modes without introducing links or parent directories.
python3 - "$work" "$out" "$version" "$platform" <<'PY'
from pathlib import Path
import stat
import sys
import tarfile
import zipfile

work, out = map(Path, sys.argv[1:3])
version, platform = sys.argv[3:]
with tarfile.open(out / f'hx-server-{version}-{platform}.tar.gz', 'w:gz', format=tarfile.USTAR_FORMAT) as archive:
    for path in sorted((work / 'server').iterdir()):
        if not stat.S_ISREG(path.lstat().st_mode):
            raise SystemExit(f'Non-regular server asset: {path}')
        archive.add(path, arcname=path.name, recursive=False)
if platform == 'macos-aarch64':
    app = work / 'Hyperthymesia.app'
    with zipfile.ZipFile(out / f'Hyperthymesia-{version}-{platform}.zip', 'w', zipfile.ZIP_DEFLATED) as archive:
        for path in sorted(app.rglob('*')):
            mode = path.lstat().st_mode
            if not (stat.S_ISREG(mode) or stat.S_ISDIR(mode)):
                raise SystemExit(f'Link or special file in app: {path}')
            archive.write(path, path.relative_to(work).as_posix())
PY
if [[ "$platform" == macos-aarch64 ]]; then
    # Check the actual zip's signatures/ticket after a round trip, not only the
    # staging bundle. This also catches packaging that loses notarization data.
    mkdir "$work/unpacked"
    unzip -q "$out/Hyperthymesia-$version-$platform.zip" -d "$work/unpacked"
    codesign --verify --deep --strict "$work/unpacked/Hyperthymesia.app"
    if [[ "${HX_NOTARIZE:-0}" == 1 ]]; then
        xcrun stapler validate "$work/unpacked/Hyperthymesia.app"
    fi
fi
cp scripts/install.sh "$out/install.sh"
chmod 755 "$out/install.sh"
python3 - "$out" <<'PY'
import hashlib
from pathlib import Path
import sys

out = Path(sys.argv[1])
with (out / 'SHA256SUMS').open('w') as sums:
    for path in sorted(out.iterdir()):
        if path.name != 'SHA256SUMS':
            sums.write(f'{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n')
PY
printf 'Built %s release assets in %s\n' "$platform" "$out"
