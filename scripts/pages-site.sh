#!/bin/sh
# Build the public site served at https://hyperthymesia.eilmer.si: the docs
# folder (including the pinned `install` launcher) plus the release assets that
# the launcher pins. Assets come from this project's generic package registry
# and are refused unless they match the SHA256 pin at the top of docs/install.
set -eu
out=${1:?usage: pages-site.sh OUTPUT_DIR}
api=${CI_API_V4_URL:?}/projects/${CI_PROJECT_ID:?}/packages/generic/hyperthymesia
version=$(sed -n 's/^VERSION=//p' docs/install)
pin=$(sed -n 's/^SHA256=//p' docs/install)
fetch() { curl -fsSL --retry 3 ${CI_JOB_TOKEN:+-H "JOB-TOKEN: $CI_JOB_TOKEN"} "$1" -o "$2"; }
rm -rf "$out"
mkdir -p "$out"
cp -R docs/. "$out/"
release="$out/releases/download/v$version"
mkdir -p "$release"
fetch "$api/$version/SHA256SUMS" "$release/SHA256SUMS"
[ "$(sha256sum "$release/SHA256SUMS" | cut -d ' ' -f 1)" = "$pin" ] ||
    { echo "pages: SHA256SUMS for $version does not match the docs/install pin" >&2; exit 1; }
for name in $(awk '{ print $2 }' "$release/SHA256SUMS"); do
    fetch "$api/$version/$name" "$release/$name"
done
(cd "$release" && sha256sum --strict -c SHA256SUMS)
mkdir -p "$out/releases/latest"
cp -R "$release" "$out/releases/latest/download"
cat > "$out/index.html" <<HTML
<!doctype html><meta charset="utf-8"><title>Hyperthymesia</title>
<meta http-equiv="refresh" content="0; url=https://gitlab.com/johansardar/hyperthymesia">
<p>Install: <code>curl -fsSL https://hyperthymesia.eilmer.si/install | sh</code></p>
<p>Source: <a href="https://gitlab.com/johansardar/hyperthymesia">gitlab.com/johansardar/hyperthymesia</a></p>
HTML
