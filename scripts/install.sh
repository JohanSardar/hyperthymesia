#!/bin/sh
# Installer core only: never starts/stops services, migrates or reads state.
set -eu
umask 077

die() { printf 'install: %s\n' "$*" >&2; exit 1; }
owner() {
    printf 'install: existing unmanaged installation: %s\nSee docs/moving-to-sqlite.md before proceeding. There is no override.\n' "$1" >&2
    exit 3
}
exists() { [ -e "$1" ] || [ -L "$1" ]; }
managed_link() { [ -L "$1" ] && [ "$(readlink "$1")" = "$2" ] && [ -f "$prefix/.installed" ]; }

dry_run=false
uninstall=false
server_only=false
app_only=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --dry-run) dry_run=true ;;
        --uninstall) uninstall=true ;;
        --server-only) server_only=true ;;
        --app-only) app_only=true ;;
        --help)
            printf '%s\n' 'Usage: install.sh [--dry-run] [--uninstall] [--server-only|--app-only]' \
                'Verifies and installs files only; service activation and state initialization are not included.' \
                'HX_RELEASE_BASE: release download directory (default: GitHub latest/download).' \
                'HX_FX_RELEASE_BASE: fx download directory; pinned SHA-256 checks always apply.'
            exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
    shift
done
[ "$server_only$app_only" != truetrue ] || die '--server-only and --app-only conflict'
: "${HOME:?HOME must be set}"
case "$HOME" in /*) ;; *) die 'HOME must be absolute' ;; esac
# Generated shell/XML/unit files support spaces, quotes, &, and %; reject control
# characters that cannot be represented safely in these service formats.
case "$HOME" in *'
'*|*"$(printf '\r')"*) die 'HOME contains a control character' ;; esac

case "$(uname -s):$(uname -m)" in
    Darwin:arm64|Darwin:aarch64)
        platform=macos-aarch64
        fx_sha=2158a8d42e77457a774400d225d9802bb6c0e60a0bb0674a1116bc31252490d6 ;;
    Darwin:x86_64) die 'Intel Macs are unsupported; macOS requires Apple Silicon (arm64)' ;;
    Linux:x86_64)
        platform=linux-x86_64
        fx_sha=f0fb822c7be546c80bcc89d50521bc1454435e0c6f586e8420c4e2b84fa80599 ;;
    Linux:aarch64|Linux:arm64)
        platform=linux-aarch64
        fx_sha=ca9441b104357e2105039dbfd4c2eaf0d6ee65a84085f23f51e880c95cbf46af ;;
    *) die 'unsupported platform (requires Mac arm64 or Linux x86_64/aarch64)' ;;
esac
case "$platform" in linux-*) [ "$app_only" = false ] || die '--app-only requires macOS' ;; esac

prefix=$HOME/.local/share/hyperthymesia
bin=$HOME/.local/bin/hx-server
support=$HOME/Library/Application\ Support/Hyperthymesia
plist=$HOME/Library/LaunchAgents/com.hyperthymesia.server.plist
unit=$HOME/.config/systemd/user/hyperthymesia.service
app=$HOME/Applications/Hyperthymesia.app
for legacy in "$support/server-data" "$support/agent.json"; do
    if exists "$legacy"; then owner "$legacy"; fi
done
# Never read a plist to look for a comment: only our known symlink plus the
# prefix marker grants ownership. The rendered target also carries a comment.
if exists "$plist" && ! managed_link "$plist" "$prefix/current/com.hyperthymesia.server.plist"; then owner "$plist"; fi
if exists "$unit" && ! managed_link "$unit" "$prefix/current/hyperthymesia.service"; then owner "$unit"; fi
if exists "$bin" && ! managed_link "$bin" "$prefix/current/hx-server"; then owner "$bin"; fi
if exists "$app" && ! managed_link "$app" "$prefix/current/Hyperthymesia.app"; then owner "$app"; fi
if exists "$prefix" && [ ! -f "$prefix/.installed" ]; then owner "$prefix"; fi
if exists "$prefix/current"; then
    [ -L "$prefix/current" ] || owner "$prefix/current"
    current_target=$(readlink "$prefix/current")
    current_version=${current_target#"$prefix/"}
    case "$current_version" in ''|*/*|.|..) owner "$prefix/current" ;; esac
    [ "$current_target" = "$prefix/$current_version" ] && [ -f "$current_target/.installed" ] || owner "$prefix/current"
fi

if [ "$uninstall" = true ]; then
    # Only unlink known entry points. Retain version directories too: no
    # recursive deletion can accidentally remove a user's state directory.
    for path in "$plist" "$unit" "$bin" "$app" "$prefix/current"; do
        if exists "$path"; then
            if [ "$dry_run" = true ]; then printf 'REMOVE %s\n' "$path"; else rm -f "$path"; fi
        fi
    done
    if [ "$dry_run" = true ]; then
        printf '%s\n' 'Dry run: only entry points would be removed; state and retained versions are kept.'
    else
        printf '%s\n' 'Uninstalled entry points; state and retained versions were not deleted. Services were not stopped.'
    fi
    exit 0
fi

for command in curl tar sed mktemp readlink; do command -v "$command" >/dev/null 2>&1 || die "missing command: $command"; done
if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
    die 'requires sha256sum or shasum'
fi
work=$(mktemp -d "${TMPDIR:-/tmp}/hx-install.XXXXXXXX")
trap 'rm -rf "$work"' 0
trap 'exit 1' HUP INT TERM
fetch() { curl -fsSL "$1" -o "$2" || die "download failed: $1"; }
verify() { [ "$(sha256 "$1")" = "$2" ] || die "SHA-256 mismatch: $1"; }
# Reject traversal and links before extracting archives, even for checked assets.
safe_names() {
    while IFS= read -r member; do
        case "$member" in /*|../*|*/../*|*/..|..) return 1 ;; esac
    done
}
untar() {
    tar -tzf "$1" > "$work/members" || die 'invalid tar archive'
    safe_names < "$work/members" || die 'unsafe archive path'
    tar -tvzf "$1" > "$work/types" || die 'invalid tar archive'
    while IFS= read -r member; do
        case "$member" in -*|d*) ;; *) die 'archive contains a link or special file' ;; esac
    done < "$work/types"
    mkdir -p "$2"
    tar -xzf "$1" -C "$2" || die 'archive extraction failed'
}

release_base=${HX_RELEASE_BASE:-https://github.com/JohanSardar/hyperthymesia/releases/latest/download}
fx_base=${HX_FX_RELEASE_BASE:-https://github.com/vercel-labs/fx/releases/download/v0.0.16}
fetch "$release_base/SHA256SUMS" "$work/SHA256SUMS"
asset=
server_sha=
while read -r checksum name extra; do
    case "$name" in
        hx-server-*-$platform.tar.gz)
            [ -z "$asset" ] || die 'multiple matching server assets'
            [ -z "$extra" ] || die 'invalid checksum entry'
            asset=$name
            server_sha=$checksum ;;
    esac
done < "$work/SHA256SUMS"
[ -n "$asset" ] || die "no server asset for $platform"
version=${asset#hx-server-}
version=${version%-"$platform".tar.gz}
case "$version" in ''|*[!0-9A-Za-z.+-]*) die 'unsafe release version' ;; esac
case "$version" in [0-9]*|v[0-9]*) ;; *) die 'release version must begin with a number or vNUMBER' ;; esac
version_dir=$prefix/$version
# An upgrade needs A3's backup/lease/migration lifecycle, not a blind switch.
if exists "$prefix/current"; then
    [ -L "$prefix/current" ] && [ "$(readlink "$prefix/current")" = "$version_dir" ] || die 'upgrades require the later service lifecycle installer'
fi
if exists "$version_dir" && [ ! -f "$version_dir/.installed" ]; then owner "$version_dir"; fi

fetch "$fx_base/fx-$platform.tar.gz" "$work/fx.tar.gz"
verify "$work/fx.tar.gz" "$fx_sha"
untar "$work/fx.tar.gz" "$work/fx"
[ -f "$work/fx/fx" ] && [ -x "$work/fx/fx" ] || die 'fx executable missing'
# Even the version probe gets a disposable HOME, so fx cannot modify logins.
mkdir "$work/probe-home"
fx_version=$(HOME="$work/probe-home" FX_AUTO_UPGRADE=0 "$work/fx/fx" --version) || die 'fx --version failed'
[ "$fx_version" = 0.0.16 ] || die "expected fx 0.0.16, got: $fx_version"
fetch "$release_base/$asset" "$work/server.tar.gz"
verify "$work/server.tar.gz" "$server_sha"
untar "$work/server.tar.gz" "$work/payload"
[ -f "$work/payload/hx-server" ] && [ -x "$work/payload/hx-server" ] || die 'hx-server executable missing'

install_app=false
case "$platform:$server_only" in macos-aarch64:false) install_app=true ;; esac
if [ "$install_app" = true ]; then
    command -v unzip >/dev/null 2>&1 || die 'missing command: unzip'
    app_asset=Hyperthymesia-$version-macos-aarch64.zip
    app_sha=
    while read -r checksum name extra; do
        if [ "$name" = "$app_asset" ]; then
            [ -z "$app_sha" ] && [ -z "$extra" ] || die 'duplicate or invalid app checksum'
            app_sha=$checksum
        fi
    done < "$work/SHA256SUMS"
    [ -n "$app_sha" ] || die 'missing app checksum'
    fetch "$release_base/$app_asset" "$work/app.zip"
    verify "$work/app.zip" "$app_sha"
    unzip -Z1 "$work/app.zip" > "$work/members" || die 'invalid app archive'
    safe_names < "$work/members" || die 'unsafe app archive path'
    unzip -Z -l "$work/app.zip" > "$work/types" || die 'invalid app archive'
    while IFS= read -r member; do
        case "$member" in l*) die 'app archive contains a symlink' ;; esac
    done < "$work/types"
    while IFS= read -r member; do
        case "$member" in Hyperthymesia.app/*) ;; *) die 'unexpected app archive member' ;; esac
    done < "$work/members"
    unzip -q "$work/app.zip" -d "$work/payload" || die 'app extraction failed'
    [ -f "$work/payload/Hyperthymesia.app/Contents/Info.plist" ] || die 'app bundle missing'
fi

state=$HOME/.local/state/hyperthymesia
case "$platform" in macos-*) state=$support/server ;; esac
# Single-quote arbitrary paths for the POSIX wrapper, escape XML and systemd
# independently (systemd expands % specifiers even inside quoted arguments).
shell_quote() { printf "'"; printf '%s' "$1" | sed "s/'/'\\\\''/g"; printf "'"; }
xml_escape() { printf '%s' "$1" | sed 's/\&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g'; }
unit_escape() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g; s/%/%%/g; s/\$/$$/g'; }
cat > "$work/payload/hx-run" <<EOF
#!/bin/sh
# hyperthymesia installer N4
set -eu
set -a
. $(shell_quote "$state/server.env")
set +a
exec $(shell_quote "$prefix/current/hx-server") --state-dir $(shell_quote "$state") serve --listen 127.0.0.1:7377
EOF
chmod 755 "$work/payload/hx-run"
if [ "$platform" = macos-aarch64 ]; then
    service_name=com.hyperthymesia.server.plist
    service=$plist
    cat > "$work/payload/$service_name" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<!-- hyperthymesia installer N4; ownership uses the service symlink and prefix marker -->
<plist version="1.0"><dict>
<key>Label</key><string>com.hyperthymesia.server</string>
<key>ProgramArguments</key><array><string>$(xml_escape "$prefix/current/hx-run")</string></array>
<key>RunAtLoad</key><true/>
<key>KeepAlive</key><true/>
</dict></plist>
EOF
else
    service_name=hyperthymesia.service
    service=$unit
    cat > "$work/payload/$service_name" <<EOF
# hyperthymesia installer N4
[Unit]
Description=Hyperthymesia server
[Service]
Type=simple
ExecStart=/bin/sh "$(unit_escape "$prefix/current/hx-run")"
Restart=always
[Install]
WantedBy=default.target
EOF
fi
printf '%s\n' 'hyperthymesia installer N4' > "$work/payload/.installed"

if [ "$dry_run" = true ]; then
    # List every extracted file too, including licenses, source notices and app
    # bundle resources. Show generated service/wrapper contents for review.
    (cd "$work/payload" && find . -type f -print) | while IFS= read -r file; do
        printf 'WRITE %s/%s\n' "$version_dir" "${file#./}"
    done
    printf 'WRITE %s\n' "$prefix/.installed" "$prefix/tools/fx-0.0.16/fx"
    printf 'LINK %s -> %s\n' "$prefix/current" "$version_dir"
    if [ "$app_only" = false ]; then
        printf 'LINK %s -> %s\n' "$bin" "$prefix/current/hx-server" "$service" "$prefix/current/$service_name"
    fi
    if [ "$install_app" = true ]; then printf 'LINK %s -> %s\n' "$app" "$prefix/current/Hyperthymesia.app"; fi
    printf '\n--- hx-run ---\n'
    cat "$work/payload/hx-run"
    printf '\n--- %s ---\n' "$service_name"
    cat "$work/payload/$service_name"
    exit 0
fi

# No destination changes occur before every download, hash and version check.
mkdir -p "$prefix/tools/fx-0.0.16"
if ! exists "$version_dir"; then
    # Copy on the destination filesystem before publishing the version.
    stage=$(mktemp -d "$prefix/.staging.XXXXXXXX")
    cp -R "$work/payload/." "$stage/"
    mv "$stage" "$version_dir"
elif [ "$install_app" = true ] && ! exists "$version_dir/Hyperthymesia.app"; then
    cp -R "$work/payload/Hyperthymesia.app" "$version_dir/"
fi
cp "$work/fx/fx" "$prefix/tools/fx-0.0.16/fx"
chmod 755 "$prefix/tools/fx-0.0.16/fx"
cp "$work/payload/.installed" "$prefix/.installed"
if ! exists "$prefix/current"; then ln -s "$version_dir" "$prefix/current"; fi
if [ "$app_only" = false ]; then
    mkdir -p "$(dirname "$bin")" "$(dirname "$service")"
    if ! exists "$bin"; then ln -s "$prefix/current/hx-server" "$bin"; fi
    if ! exists "$service"; then ln -s "$prefix/current/$service_name" "$service"; fi
fi
if [ "$install_app" = true ]; then
    mkdir -p "$HOME/Applications"
    if ! exists "$app"; then ln -s "$prefix/current/Hyperthymesia.app" "$app"; fi
fi
printf '%s\n' 'Installed verified files only. Services were not started; server.env and first run require the later lifecycle installer.'
