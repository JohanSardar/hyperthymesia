#!/bin/sh
# Verified user-service installer. Never operates on unmanaged owner state.
set -eu
umask 077

die() { printf 'install: %s\n' "$*" >&2; exit 1; }
owner() {
    printf 'install: existing unmanaged installation: %s\nSee crates/hx-move/README.md for the legacy mover and its limits. There is no override.\n' "$1" >&2
    exit 3
}
exists() { [ -e "$1" ] || [ -L "$1" ]; }
managed_link() { [ -L "$1" ] && [ "$(readlink "$1")" = "$2" ] && [ -f "$prefix/.installed" ]; }

dry_run=false
uninstall=false
start_fresh=false
server_only=false
app_only=false
requested_state=
provider=codex
model=gpt-6.1-sol
effort=xhigh
configured=false
while [ "$#" -gt 0 ]; do
    case "$1" in
        --dry-run) dry_run=true ;;
        --uninstall) uninstall=true ;;
        --start-fresh) start_fresh=true ;;
        --server-only) server_only=true ;;
        --app-only) app_only=true ;;
        --provider|--model|--effort|--state-dir)
            option=$1
            [ "$#" -ge 2 ] || die "missing value for $option"
            shift
            case "$option" in
                --provider) provider=$1; configured=true ;;
                --model) model=$1; configured=true ;;
                --effort) effort=$1; configured=true ;;
                --state-dir) requested_state=$1 ;;
            esac ;;
        --help)
            printf '%s\n' 'Usage: install.sh [--provider codex|grok|gateway --model M --effort E --state-dir D] [--dry-run] [--start-fresh] [--uninstall] [--server-only|--app-only]' \
                'Installs a loopback user service; upgrades are backed up and refused during interpret leases.' \
                'HX_RELEASE_BASE: release download directory (default: GitHub latest/download).' \
                'HX_FX_RELEASE_BASE: fx download directory; pinned SHA-256 checks always apply.'
            exit 0 ;;
        *) die "unknown argument: $1" ;;
    esac
    shift
done
case "$provider" in codex|grok|gateway) ;; *) die 'provider must be codex, grok or gateway' ;; esac
[ -n "$model" ] && [ -n "$effort" ] || die 'model and effort must not be empty'
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
if exists "$prefix"; then
    [ -d "$prefix" ] && [ ! -L "$prefix" ] && [ -f "$prefix/.installed" ] && [ ! -L "$prefix/.installed" ] || owner "$prefix"
fi
if exists "$prefix/.state-dir"; then
    [ -f "$prefix/.state-dir" ] && [ ! -L "$prefix/.state-dir" ] || owner "$prefix/.state-dir"
fi
if exists "$prefix/.last-version"; then
    [ -f "$prefix/.last-version" ] && [ ! -L "$prefix/.last-version" ] || owner "$prefix/.last-version"
    last_version=$(cat "$prefix/.last-version")
    case "$last_version" in "$prefix/"*) ;; *) owner "$prefix/.last-version" ;; esac
    last_name=${last_version#"$prefix/"}
    case "$last_name" in ''|*/*|.|..) owner "$prefix/.last-version" ;; esac
    [ -d "$last_version" ] && [ ! -L "$last_version" ] && [ -f "$last_version/.installed" ] && [ ! -L "$last_version/.installed" ] || owner "$prefix/.last-version"
fi
if exists "$prefix/current"; then
    [ -L "$prefix/current" ] || owner "$prefix/current"
    current_target=$(readlink "$prefix/current")
    current_version=${current_target#"$prefix/"}
    case "$current_version" in ''|*/*|.|..) owner "$prefix/current" ;; esac
    [ "$current_target" = "$prefix/$current_version" ] && [ -d "$current_target" ] && [ ! -L "$current_target" ] && [ -f "$current_target/.installed" ] && [ ! -L "$current_target/.installed" ] || owner "$prefix/current"
fi

state=$HOME/.local/state/hyperthymesia
case "$platform" in macos-*) state=$support/server ;; esac
if [ -n "$requested_state" ]; then state=$requested_state; fi
# Persist the chosen state path, without ever sourcing configuration for ownership.
if [ -f "$prefix/.state-dir" ]; then
    installed_state=$(cat "$prefix/.state-dir")
    [ -z "$requested_state" ] || [ "$state" = "$installed_state" ] || die 'installed state directory cannot be changed on re-run'
    state=$installed_state
fi
case "$state" in /*) ;; *) die '--state-dir must be absolute' ;; esac
case "$state" in *'
'*|*"$(printf '\r')"*) die 'state path contains a control character' ;; esac
# No state may be adopted from an unmarked installation (including a custom path).
if exists "$state" && [ ! -f "$prefix/.state-dir" ]; then owner "$state"; fi
if exists "$state"; then
    [ -d "$state" ] && [ ! -L "$state" ] || owner "$state"
    for file in core.sqlite agent.sqlite server.env server.log; do
        if exists "$state/$file"; then
            [ -f "$state/$file" ] && [ ! -L "$state/$file" ] || owner "$state/$file"
        fi
    done
fi

# Recover a killed lifecycle before checking for a missing registered core: a
# rollback may have been interrupted between moving state aside and publishing
# the verified restore. Only installer-owned, durable scratch is executable here.
if exists "$prefix/pending-upgrade"; then
    [ -L "$prefix/pending-upgrade" ] || owner "$prefix/pending-upgrade"
    recovery=$(readlink "$prefix/pending-upgrade")
    case "$recovery" in "$prefix"/.upgrade.*) ;; *) owner "$prefix/pending-upgrade" ;; esac
    recovery_name=${recovery#"$prefix/"}
    case "$recovery_name" in */*) owner "$prefix/pending-upgrade" ;; esac
    [ -d "$recovery" ] && [ ! -L "$recovery" ] && \
        [ -f "$recovery/lifecycle" ] && [ ! -L "$recovery/lifecycle" ] && \
        [ -d "$recovery/payload" ] && [ ! -L "$recovery/payload" ] && \
        [ -f "$recovery/payload/hx-server" ] && [ ! -L "$recovery/payload/hx-server" ] && \
        [ -x "$recovery/payload/hx-server" ] || die 'incomplete or redirected upgrade recovery record; retained state has not been changed'
    if [ "$dry_run" = false ]; then
        "$recovery/payload/hx-server" --state-dir "$state" upgrade-recover --lifecycle "$recovery/lifecycle" || die 'interrupted upgrade recovery failed; retained copies kept'
        ! exists "$prefix/pending-upgrade" || die 'another upgrade is still running; no changes made'
    fi
fi

# The path record is a promise of history, even after uninstall or an unmount.
# Refuse before downloads, manager commands or any destination mutation.
if [ "$uninstall" = false ] && [ "$app_only" = false ] && [ -f "$prefix/.state-dir" ] && [ ! -f "$state/core.sqlite" ]; then
    if [ "$start_fresh" = false ]; then
        die "registered state is missing core.sqlite at $state. Reconnect its volume or find old copies at $state.failed-* and $prefix/backups/. Stop the service and use a retained hx-server restore --from BACKUP into an absent/empty $state; keep server.env private. To intentionally create empty history, rerun with --start-fresh."
    fi
    if exists "$state" && [ -n "$(ls -A "$state")" ]; then
        die '--start-fresh requires an absent or empty state folder; move the remaining state aside for recovery first'
    fi
fi

# A stable state path must never move when current is switched, and backups
# must not be descendants of their source. Reject ambiguous path spellings.
case "$state/" in *'/../'*|*'/./'*|*'//'*) die 'state directory must use a normalized absolute path' ;; esac
case "$state/" in "$prefix/"*) die 'state directory must be outside the program prefix' ;; esac
case "$prefix/" in "$state/"*) die 'state directory must not contain the program prefix' ;; esac

service_stop() {
    case "$platform" in
        macos-*)
            if launchctl print "gui/$(id -u)/com.hyperthymesia.server" >/dev/null 2>&1; then
                pid=$(launchctl print "gui/$(id -u)/com.hyperthymesia.server" | sed -n 's/^[[:space:]]*pid = \([0-9][0-9]*\)$/\1/p')
                launchctl bootout "gui/$(id -u)/com.hyperthymesia.server" || return
                # bootout can return before graceful shutdown has finished.
                count=0
                while [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; do
                    [ "$count" -lt 60 ] || return 1
                    count=$((count + 1))
                    sleep 1
                done
            fi ;;
        linux-*)
            systemctl --user stop hyperthymesia.service || {
                # Exit 5 means not installed, as after a managed uninstall.
                # No other manager failure may be mistaken for a stopped unit.
                [ "$?" -eq 5 ]
            } ;;
    esac
}
service_start() {
    case "$platform" in
        macos-*) launchctl bootstrap "gui/$(id -u)" "$plist" ;;
        linux-*) systemctl --user daemon-reload && systemctl --user enable --now hyperthymesia.service ;;
    esac
}

if [ "$uninstall" = true ]; then
    if [ "$dry_run" = false ] && [ -f "$prefix/.state-dir" ]; then
        service_stop || die 'could not stop user service; entry points kept'
        case "$platform" in linux-*) systemctl --user disable hyperthymesia.service ;; esac
    fi
    if [ "$dry_run" = false ] && [ -n "${current_target:-}" ]; then
        printf '%s\n' "$current_target" > "$prefix/.last-version"
    fi
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
        printf '%s\n' 'Uninstalled entry points; state and retained versions were not deleted. The user service was stopped.'
    fi
    exit 0
fi

# These checks protect writes; they must not prevent uninstall or recovery
# from stopping an already-running service.
for path in "$prefix/tools" "$prefix/tools/fx-0.0.16"; do
    if exists "$path"; then [ -d "$path" ] && [ ! -L "$path" ] || owner "$path"; fi
done
if exists "$prefix/tools/fx-0.0.16/fx"; then
    [ -f "$prefix/tools/fx-0.0.16/fx" ] && [ ! -L "$prefix/tools/fx-0.0.16/fx" ] || owner "$prefix/tools/fx-0.0.16/fx"
fi
if exists "$state/.install-prefix"; then
    [ -f "$state/.install-prefix" ] && [ ! -L "$state/.install-prefix" ] || owner "$state/.install-prefix"
fi
# Never delete an unrecognized file after SIGKILL. Refuse another attempt
# before stopping a service rather than accumulating more staging files.
refuse_staging() {
    for staged in "$@"; do
        if exists "$staged"; then die "interrupted installer staging retained at $staged; move it aside and retry"; fi
    done
}
refuse_staging "$prefix"/.state-dir.???????? "$prefix"/.last-version.???????? \
    "$prefix"/.staging.???????? "$prefix".staging.???????? \
    "$prefix/tools/fx-0.0.16"/.fx.???????? "$state"/.install-prefix.????????

for command in curl tar sed mktemp readlink; do command -v "$command" >/dev/null 2>&1 || die "missing command: $command"; done
if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
    die 'requires sha256sum or shasum'
fi
work=$(mktemp -d "${TMPDIR:-/tmp}/hx-install.XXXXXXXX")
work=$(cd "$work" && pwd -P)
cleanup() {
    # Never erase the only recovery program after an incomplete rollback.
    if [ -L "$prefix/pending-upgrade" ] && [ "$(readlink "$prefix/pending-upgrade")" = "$work" ]; then
        printf 'Retained interrupted-upgrade recovery: %s\n' "$work" >&2
    else
        # The guard link resolves to a private, non-executable sibling of state
        # on the same filesystem. Remove only our exact anchor, never state's
        # own lock inode or an unexpected link outside that parent.
        if [ -L "$work/guard.lock" ]; then
            anchor=$(readlink "$work/guard.lock")
            if [ "$(dirname "$anchor")" = "$(dirname "$state")" ]; then
                case "${anchor##*/}" in .hx-upgrade-lock-*) [ -L "$anchor" ] || rm -f "$anchor" ;; esac
            fi
        fi
        rm -rf "$work"
    fi
}
trap cleanup 0
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
if exists "$version_dir"; then
    [ -d "$version_dir" ] && [ ! -L "$version_dir" ] && [ -f "$version_dir/.installed" ] && [ ! -L "$version_dir/.installed" ] || owner "$version_dir"
    for path in "$version_dir/hx-server" "$version_dir/hx-run" "$version_dir/com.hyperthymesia.server.plist" "$version_dir/hyperthymesia.service"; do
        if exists "$path"; then [ -f "$path" ] && [ ! -L "$path" ] || owner "$path"; fi
    done
fi
refuse_staging "$version_dir"/.hx-run.???????? "$version_dir"/.service.????????
if [ "$app_only" = true ] && [ -f "$prefix/.state-dir" ] && [ "${current_target:-}" != "$version_dir" ]; then
    die 'use the full installer to upgrade an installation with a managed server'
fi

fetch "$fx_base/fx-$platform.tar.gz" "$work/fx.tar.gz"
verify "$work/fx.tar.gz" "$fx_sha"
untar "$work/fx.tar.gz" "$work/fx"
[ -f "$work/fx/fx" ] && [ -x "$work/fx/fx" ] || die 'fx executable missing'
# A dry run does not execute downloaded binaries. A normal install probes in
# a disposable HOME/cwd with no inherited environment; this is not a sandbox.
mkdir "$work/probe-home"
if [ "$dry_run" = false ]; then
    fx_version=$(cd "$work/probe-home" && env -i HOME="$work/probe-home" PATH=/usr/bin:/bin FX_AUTO_UPGRADE=0 FX_DISABLE_KEYCHAIN=1 FX_NO_OPEN_BROWSER=1 "$work/fx/fx" --version) || die 'fx --version failed'
    [ "$fx_version" = 0.0.16 ] || die "expected fx 0.0.16, got: $fx_version"
fi
fetch "$release_base/$asset" "$work/server.tar.gz"
verify "$work/server.tar.gz" "$server_sha"
untar "$work/server.tar.gz" "$work/payload"
[ -f "$work/payload/hx-server" ] && [ -x "$work/payload/hx-server" ] || die 'hx-server executable missing'
# Checksums are fetched from the release host; they detect corruption but
# are not an independent signature. Normal probes run as the current user.
if [ "$dry_run" = false ]; then
    actual_version=$(cd "$work/probe-home" && env -i HOME="$work/probe-home" PATH=/usr/bin:/bin "$work/payload/hx-server" --version) || die 'hx-server --version failed'
    [ "$actual_version" = "hx-server ${version#v}" ] || die 'server version does not match release asset'
fi
if [ -f "$version_dir/hx-server" ]; then
    [ "$(sha256 "$version_dir/hx-server")" = "$(sha256 "$work/payload/hx-server")" ] || die 'retained version differs from the release; version directories are immutable'
fi

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

# Single-quote arbitrary paths for the POSIX wrapper, escape XML and systemd
# independently (systemd expands % specifiers even inside quoted arguments).
shell_quote() { printf "'"; printf '%s' "$1" | sed "s/'/'\\\\''/g"; printf "'"; }
xml_escape() { printf '%s' "$1" | sed 's/\&/\&amp;/g; s/</\&lt;/g; s/>/\&gt;/g; s/"/\&quot;/g'; }
unit_escape() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g; s/%/%%/g; s/\$/$$/g'; }
cat > "$work/payload/hx-run" <<EOF
#!/bin/sh
# hyperthymesia installer N4
set -eu
umask 077
export HOME=$(shell_quote "$HOME")
export PATH=/usr/bin:/bin:/usr/sbin:/sbin
if [ -L $(shell_quote "$prefix/pending-upgrade") ]; then
    HX_INSTALL_STARTUP_RECOVERY=1 $(shell_quote "$prefix/pending-upgrade/payload/hx-server") --state-dir $(shell_quote "$state") upgrade-recover --lifecycle $(shell_quote "$prefix/pending-upgrade/lifecycle")
fi
exec >> $(shell_quote "$state/server.log") 2>&1
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
<key>StandardOutPath</key><string>$(xml_escape "$state/server.log")</string>
<key>StandardErrorPath</key><string>$(xml_escape "$state/server.log")</string>
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
RestartSec=1
TimeoutStopSec=60
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
    printf 'WRITE %s\n' "$prefix/.installed" "$prefix/.last-version" "$prefix/tools/fx-0.0.16/fx"
    printf 'LINK %s -> %s\n' "$prefix/current" "$version_dir"
    if [ "$app_only" = false ]; then
        printf 'LINK %s -> %s\n' "$bin" "$prefix/current/hx-server" "$service" "$prefix/current/$service_name"
    fi
    if [ "$install_app" = true ]; then printf 'LINK %s -> %s\n' "$app" "$prefix/current/Hyperthymesia.app"; fi
    if [ "$app_only" = false ]; then
        printf 'WRITE %s\n' "$prefix/.state-dir" "$state/core.sqlite" "$state/agent.sqlite" "$state/server.env" "$state/server.log" "$state/.install-prefix"
        printf 'MIGRATE, START user service, WAIT /healthz, DOCTOR %s\n' "$state"
        printf 'UPGRADE: require idle, stop, backup, switch, migrate, health check; restore on failure\n'
    fi
    printf '\n--- hx-run ---\n'
    cat "$work/payload/hx-run"
    printf '\n--- %s ---\n' "$service_name"
    cat "$work/payload/$service_name"
    exit 0
fi

# Preflight the user bus before changing destinations. Never enable linger here.
if [ "$app_only" = false ]; then
    case "$platform" in
        linux-*)
            systemctl --user show-environment >/dev/null 2>&1 || die 'user systemd bus unavailable; ask the administrator to run: loginctl enable-linger USER (then log in again)'
            linger=$(loginctl show-user "$(id -un)" -p Linger --value) || die 'cannot check linger; ask the administrator to run: loginctl enable-linger USER'
            [ "$linger" = yes ] || die "persistent user service requires an administrator to run: loginctl enable-linger $(id -un)" ;;
        macos-*) launchctl print "gui/$(id -u)" >/dev/null 2>&1 || die 'no GUI login domain; log in to a macOS desktop session first' ;;
    esac
fi
old=
if [ -L "$prefix/current" ]; then old=$(readlink "$prefix/current"); fi
# Uninstall retains binaries but removes current. A retained marked version is
# still a rollback/backup tool; never depend on the current symlink existing.
if [ -z "$old" ] && [ -n "${last_version:-}" ]; then old=$last_version; fi
if [ -z "$old" ]; then
    for retained in "$prefix"/*; do
        if [ -d "$retained" ] && [ ! -L "$retained" ] && [ -f "$retained/.installed" ] && [ ! -L "$retained/.installed" ]; then
            if exists "$retained/hx-server"; then
                [ -f "$retained/hx-server" ] && [ ! -L "$retained/hx-server" ] || owner "$retained/hx-server"
                if [ -x "$retained/hx-server" ] && { [ ! -f "$state/core.sqlite" ] || "$retained/hx-server" --state-dir "$state" doctor --state-only >/dev/null 2>&1; }; then old=$retained; fi
            fi
        fi
    done
fi
# A retained version may be used for backup and rollback. Refuse redirected
# executables even when upgrading to a different version.
if [ -n "$old" ]; then
    [ -f "$old/hx-server" ] && [ ! -L "$old/hx-server" ] || owner "$old/hx-server"
fi
# status and the guard are read-only with respect to the databases. The guard
# prevents the service worker from claiming work until this lifecycle returns.
if [ "$app_only" = false ] && [ -f "$state/core.sqlite" ]; then
    /bin/sh -n "$state/server.env" >/dev/null 2>&1 || die 'invalid server.env syntax; old service left running'
    "$work/payload/hx-server" --state-dir "$state" doctor --state-only --allow-older-schema || die 'upgrade state preflight failed; old service left running'
    "$work/payload/hx-server" --state-dir "$state" status --require-idle || die 'upgrade refused; old service left running'
fi

# Keep upgrade recovery outside /tmp and outside mutable state. A reboot or a
# killed shell must not erase its progress, backup path or recovery executable.
if [ "$app_only" = false ] && [ -f "$state/core.sqlite" ]; then
    durable=$(mktemp -d "$prefix/.upgrade.XXXXXXXX")
    cp -R "$work/." "$durable/"
    rm -rf "$work"
    work=$durable
fi
# The guard holds its lock across stop, backup, migration and rollback.
setting() { printf '%s=%s\n' "$1" "$(shell_quote "$2")"; }
{
    printf '#!/bin/sh\nset -eu\numask 077\n'
    setting prefix "$prefix"
    setting bin "$bin"
    setting state "$state"
    setting platform "$platform"
    setting plist "$plist"
    setting unit "$unit"
    setting app "$app"
    setting version_dir "$version_dir"
    setting service "$service"
    setting service_name "$service_name"
    setting install_app "$install_app"
    setting app_only "$app_only"
    setting old "$old"
    setting work "$work"
    setting provider "$provider"
    setting model "$model"
    setting effort "$effort"
    setting configured "$configured"
} > "$work/lifecycle"
# $0 can be sh when piped. Render fixed functions without reading this script.
cat >> "$work/lifecycle" <<'LIFECYCLE'
service_stop() {
    case "$platform" in
        macos-*)
            if launchctl print "gui/$(id -u)/com.hyperthymesia.server" >/dev/null 2>&1; then
                pid=$(launchctl print "gui/$(id -u)/com.hyperthymesia.server" | sed -n 's/^[[:space:]]*pid = \([0-9][0-9]*\)$/\1/p')
                launchctl bootout "gui/$(id -u)/com.hyperthymesia.server" || return
                # bootout can return before graceful shutdown has finished.
                count=0
                while [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; do
                    [ "$count" -lt 60 ] || return 1
                    count=$((count + 1))
                    sleep 1
                done
            fi ;;
        linux-*)
            systemctl --user stop hyperthymesia.service || {
                # Exit 5 means not installed, as after a managed uninstall.
                # No other manager failure may be mistaken for a stopped unit.
                [ "$?" -eq 5 ]
            } ;;
    esac
}
service_start() {
    case "$platform" in
        macos-*) launchctl bootstrap "gui/$(id -u)" "$plist" ;;
        linux-*) systemctl --user daemon-reload && systemctl --user enable --now hyperthymesia.service ;;
    esac
}
switch_current() {
    # Rust rename replaces a symlink atomically on both supported platforms.
    "$work/payload/hx-server" --state-dir "$state" install-link --link "$prefix/current" --target "$1"
}
quote() { printf "'"; printf '%s' "$1" | sed "s/'/'\\\\''/g"; printf "'"; }
server() (
    set -a
    . "$state/server.env"
    set +a
    "$prefix/current/hx-server" --state-dir "$state" "$@"
)
healthy() {
    count=0
    while [ "$count" -lt 30 ]; do
        if curl -fsSL --max-time 2 http://127.0.0.1:7377/healthz -o "$work/health" 2>/dev/null; then
            # A response from another process is insufficient: confirm our user service.
            case "$platform" in
                macos-*) launchctl print "gui/$(id -u)/com.hyperthymesia.server" > "$work/service-status" && grep -q 'state = running' "$work/service-status" && return 0 ;;
                linux-*) systemctl --user is-active --quiet hyperthymesia.service && return 0 ;;
            esac
        fi
        count=$((count + 1))
        sleep 1
    done
    return 1
}
stopped=false
backup=
failed_state=$state.failed-$(date +%s)-$$
migration_started=false
committed=false
recovering=false
backup_server=$work/payload/hx-server
if [ -n "$old" ]; then backup_server=$old/hx-server; fi
save_progress() {
    {
        printf 'stopped=%s\nmigration_started=%s\ncommitted=%s\n' "$stopped" "$migration_started" "$committed"
        printf 'backup='; quote "$backup"; printf '\nfailed_state='; quote "$failed_state"; printf '\n'
    } > "$work/progress.next"
    mv "$work/progress.next" "$work/progress"
}
if [ "${1:-}" = --recover ]; then
    recovering=true
    . "$work/progress"
    if [ "$committed" = true ]; then
        rm -f "$prefix/pending-upgrade"
        exit 0
    fi
fi
rollback() {
    result=$?
    trap - 0 HUP INT TERM
    # Remove only staging paths created by this lifecycle. A SIGKILL can leave
    # them behind; the next install refuses them without deleting user files.
    for staged in "${next_wrapper:-}" "${next_service:-}" "${next_fx:-}" "${next_state:-}" "${next_marker:-}" "${next_version:-}"; do
        if [ -n "$staged" ] && [ -f "$staged" ] && [ ! -L "$staged" ]; then rm -f "$staged" || :; fi
    done
    # The prefix stage contains only our ownership marker. Never recursively
    # remove an unexpected directory or contents.
    if [ -n "${prefix_stage:-}" ] && [ -d "$prefix_stage" ] && [ ! -L "$prefix_stage" ]; then
        if [ -f "$prefix_stage/.installed" ] && [ ! -L "$prefix_stage/.installed" ]; then rm -f "$prefix_stage/.installed" || :; fi
        rmdir "$prefix_stage" 2>/dev/null || :
    fi
    # Recovery must attempt every step, even when storage itself is failing.
    set +e
    # The durable commit is the boundary: a cleanup error after it must not
    # roll back a version that the recovery program already considers committed.
    if [ "$committed" = true ]; then exit "$result"; fi
    if [ "$result" -ne 0 ] && [ "$stopped" = true ]; then
        if [ "${HX_INSTALL_STARTUP_RECOVERY:-0}" != 1 ]; then
            service_stop || printf 'Could not stop candidate service during recovery.\n' >&2
        fi
        restored=true
        if [ -n "$backup" ] && [ -f "$backup/manifest.json" ] && [ "$migration_started" = true ]; then
            # The original failed generation is never moved twice. Restore
            # publishes atomically, so a killed recovery can resume after rename.
            if [ -d "$failed_state" ] || mv "$state" "$failed_state"; then
                if [ -f "$state/core.sqlite" ] || "$backup_server" --state-dir "$state" restore --from "$backup"; then
                    cp "$failed_state/server.env" "$state/server.env" || restored=false
                    chmod 700 "$state" || restored=false
                    chmod 600 "$state/server.env" || restored=false
                    touch "$state/server.log" || restored=false
                    chmod 600 "$state/server.log" || restored=false
                    next_marker=$(mktemp "$state/.install-prefix.XXXXXXXX") || restored=false
                    if [ -n "${next_marker:-}" ]; then
                        if ! { printf '%s\n' "$prefix" > "$next_marker" && "$work/payload/hx-server" --state-dir "$state" install-marker --staged "$next_marker" --target "$state/.install-prefix"; }; then
                            restored=false
                            if [ -f "$next_marker" ] && [ ! -L "$next_marker" ]; then rm -f "$next_marker" || :; fi
                        fi
                    fi
                else
                    restored=false
                fi
            else
                restored=false
            fi
            if [ "$restored" = false ]; then
                printf 'Recovery incomplete. Backup: %s; retained failed state: %s. Restore into an absent/empty state folder before restarting.\n' "$backup" "$failed_state" >&2
            fi
        fi
        if [ -n "$old" ]; then
            if ! switch_current "$old"; then
                printf 'Could not restore current link atomically to %s; trying portable recovery.\n' "$old" >&2
                # Recovery already has the service stopped. If rename itself
                # failed, a non-atomic unlink/link is preferable to restarting
                # the candidate again. Never unlink an unexpected regular path.
                if [ -L "$prefix/current" ] && [ "$(readlink "$prefix/current")" = "$old" ]; then
                    : # Rename succeeded but its directory sync may have failed.
                else
                    if [ -L "$prefix/current" ]; then rm -f "$prefix/current" || restored=false; fi
                    if [ ! -e "$prefix/current" ] && [ ! -L "$prefix/current" ]; then
                        ln -s "$old" "$prefix/current" || restored=false
                    else
                        restored=false
                    fi
                fi
            fi
            if [ "$restored" = true ]; then
                # Startup recovery calls us from the service wrapper itself;
                # that wrapper will exec the restored binary when we return.
                rm -f "$prefix/pending-upgrade" || restored=false
            fi
            # Always try the old service after link restoration, including after
            # a failed copy/rename. Its exact-schema open refuses unsafe state.
            mkdir -p "$(dirname "$bin")" "$(dirname "$service")"
            [ -L "$bin" ] || ln -s "$prefix/current/hx-server" "$bin"
            [ -L "$service" ] || ln -s "$prefix/current/$service_name" "$service"
            if [ "${HX_INSTALL_STARTUP_RECOVERY:-0}" = 1 ] || { service_start && healthy; }; then
                if [ "$restored" = true ]; then
                    printf 'Previous service restarted.\n' >&2
                else
                    printf 'Service responded, but state recovery is incomplete; inspect the retained copies.\n' >&2
                fi
            else
                printf 'Previous service did not recover. Retained binary: %s; state: %s; backup: %s.\n' "$old" "$state" "$backup" >&2
            fi
        fi
    fi
    if [ "$stopped" = false ]; then
        restored=true
        if [ -L "$prefix/pending-upgrade" ] && [ "$(readlink "$prefix/pending-upgrade")" = "$work" ]; then rm -f "$prefix/pending-upgrade" || restored=false; fi
    fi
    if [ "$recovering" = true ] && [ "${restored:-false}" = true ]; then exit 0; fi
    exit "$result"
}
trap rollback 0
trap 'exit 1' HUP INT TERM
if [ "$recovering" = true ]; then exit 1; fi
if [ "$app_only" = false ] && [ -f "$state/core.sqlite" ]; then
    save_progress
    ln -s "$work" "$prefix/pending-upgrade"
    # Reject paid work before stopping. The enclosing guard closes the race.
    "$work/payload/hx-server" --state-dir "$state" status --require-idle
    stopped=true
    save_progress
    service_stop
    mkdir -p "$prefix/backups"
    backup=$(mktemp -d "$prefix/backups/pre-upgrade.XXXXXXXX")
    save_progress
    "$backup_server" --state-dir "$state" backup --to "$backup"
elif [ "$app_only" = false ] && [ -n "$old" ]; then
    stopped=true
    service_stop
fi
# Publish the ownership marker and directory together. An interrupted staging
# directory is outside the prefix; no unmarked prefix is ever exposed.
if [ ! -d "$prefix" ]; then
    mkdir -p "$(dirname "$prefix")"
    prefix_stage=$(mktemp -d "$prefix.staging.XXXXXXXX")
    cp "$work/payload/.installed" "$prefix_stage/.installed"
    mv "$prefix_stage" "$prefix"
fi
mkdir -p "$prefix/tools/fx-0.0.16"
if [ ! -e "$version_dir" ]; then
    stage=$(mktemp -d "$prefix/.staging.XXXXXXXX")
    cp -R "$work/payload/." "$stage/"
    mv "$stage" "$version_dir"
fi
# Refresh rendering atomically even on a same-version rerun. A killed copy must
# never truncate the wrapper or unit that the previous service needs to restart.
next_wrapper=$(mktemp "$version_dir/.hx-run.XXXXXXXX")
cp "$work/payload/hx-run" "$next_wrapper"
chmod 755 "$next_wrapper"
mv "$next_wrapper" "$version_dir/hx-run"
next_service=$(mktemp "$version_dir/.service.XXXXXXXX")
cp "$work/payload/$service_name" "$next_service"
mv "$next_service" "$version_dir/$service_name"
next_fx=$(mktemp "$prefix/tools/fx-0.0.16/.fx.XXXXXXXX")
cp "$work/fx/fx" "$next_fx"
chmod 755 "$next_fx"
mv "$next_fx" "$prefix/tools/fx-0.0.16/fx"
switch_current "$version_dir"
if [ "$app_only" = false ]; then
    next_state=$(mktemp "$prefix/.state-dir.XXXXXXXX")
    printf '%s\n' "$state" > "$next_state"
    mv "$next_state" "$prefix/.state-dir"
    mkdir -p "$state"
    chmod 700 "$state"
    new_env=false
    if [ ! -f "$state/server.env" ]; then
        new_env=true
        # Values are single-quoted without evaluation. Never echo the key.
        {
            printf 'HX_PROVIDER=fx\nFX_AUTO_UPGRADE=0\nFX_DISABLE_KEYCHAIN=1\nFX_NO_OPEN_BROWSER=1\n'
            printf 'HX_FX_BINARY='; quote "$prefix/tools/fx-0.0.16/fx"; printf '\n'
            printf 'HX_FX_PROVIDER='; quote "$provider"; printf '\n'
            printf 'HX_FX_MODEL='; quote "$model"; printf '\n'
            printf 'HX_FX_EFFORT='; quote "$effort"; printf '\n'
            if [ "$provider" = gateway ]; then
                printf 'AI_GATEWAY_API_KEY='; quote "${AI_GATEWAY_API_KEY:-}"; printf '\n'
            fi
        } > "$state/server.env"
    elif [ "$configured" = true ]; then
        printf 'Keeping existing server.env; edit it privately to change provider settings.\n'
    fi
    chmod 600 "$state/server.env"
    : >> "$state/server.log"
    chmod 600 "$state/server.log"
    migration_started=true
    save_progress
    server migrate
    next_marker=$(mktemp "$state/.install-prefix.XXXXXXXX")
    printf '%s\n' "$prefix" > "$next_marker"
    "$work/payload/hx-server" --state-dir "$state" install-marker --staged "$next_marker" --target "$state/.install-prefix"
    mkdir -p "$(dirname "$bin")" "$(dirname "$service")"
    [ -L "$bin" ] || ln -s "$prefix/current/hx-server" "$bin"
    [ -L "$service" ] || ln -s "$prefix/current/$service_name" "$service"
    service_start
    healthy || { printf 'User service did not become healthy.\n' >&2; exit 1; }
    server doctor --state-only
    if ! server doctor; then
        printf 'Service is running; provider setup is incomplete. Doctor above names what is missing. No login or model call was made.\n' >&2
    fi
fi
if [ "$install_app" = true ]; then
    mkdir -p "$HOME/Applications"
    [ -L "$app" ] || ln -s "$prefix/current/Hyperthymesia.app" "$app"
fi
next_version=$(mktemp "$prefix/.last-version.XXXXXXXX")
printf '%s\n' "$version_dir" > "$next_version"
mv "$next_version" "$prefix/.last-version"
committed=true
save_progress
if [ -L "$prefix/pending-upgrade" ] && [ "$(readlink "$prefix/pending-upgrade")" = "$work" ]; then rm -f "$prefix/pending-upgrade"; fi
stopped=false
trap - 0 HUP INT TERM
printf 'Installed. State: %s\n' "$state"
if [ "$app_only" = false ]; then
    if [ "$new_env" = false ]; then
        printf 'Next: run doctor with the preserved server.env; use its configured provider and private fx for login.\n'
    elif [ "$provider" = gateway ]; then
        printf 'Next: set AI_GATEWAY_API_KEY privately in server.env (mode 600).\n'
    else
        printf 'Next: FX_AUTO_UPGRADE=0 FX_DISABLE_KEYCHAIN=1 '
        quote "$prefix/tools/fx-0.0.16/fx"
        printf ' login %s\n' "$provider"
    fi
    printf 'Then: hx-server --state-dir "%s" doctor (load server.env first).\n' "$state"
    printf 'Pair a device: hx-server --state-dir "%s" pair --url http://127.0.0.1:7377 --name DEVICE.\n' "$state"
    printf 'Dashboard: '; quote "$bin"; printf ' --state-dir '; quote "$state"
    printf ' token create --name dashboard --prefix source: --prefix macos-agent: --prefix macos-media: --prefix companion: --scopes read_raw,read_l3\n'
    printf 'Keep the printed token private: it can read raw evidence and manage sources. Open http://127.0.0.1:7377/login. Add other connector prefixes when needed.\n'
    printf 'Remote access: tailscale serve --bg 7377, or ssh -N -L 7377:127.0.0.1:7377 user@host.\n'
    printf 'Automatic GC is off. A login delayed past 24 hours leaves old windows on the daily paid sweep cap.\n'
fi
LIFECYCLE
if [ "$app_only" = false ] && [ -f "$state/core.sqlite" ]; then
    HX_INSTALL_RECOVERY="$work/lifecycle" "$work/payload/hx-server" --state-dir "$state" upgrade-guard /bin/sh "$work/lifecycle"
else
    /bin/sh "$work/lifecycle"
fi
