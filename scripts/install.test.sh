#!/bin/sh
# Offline contract tests. Every installer invocation has a scratch HOME and a
# PATH that cannot call a service manager or the network.
set -eu
cd "$(dirname "$0")/.."
python3 - <<'PY'
import hashlib
import io
import os
from pathlib import Path
import plistlib
import shlex
import shutil
import subprocess
import tarfile
import tempfile
import zipfile

SOURCE = Path(os.environ.get('HX_TEST_INSTALLER_SOURCE', 'scripts/install.sh')).resolve()
PINS = (
    '2158a8d42e77457a774400d225d9802bb6c0e60a0bb0674a1116bc31252490d6',
    'f0fb822c7be546c80bcc89d50521bc1454435e0c6f586e8420c4e2b84fa80599',
    'ca9441b104357e2105039dbfd4c2eaf0d6ee65a84085f23f51e880c95cbf46af',
)

def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()

def archive(path, files):
    with tarfile.open(path, 'w:gz') as out:
        for name, data in files.items():
            info = tarfile.TarInfo(name)
            info.size = len(data)
            info.mode = 0o755
            out.addfile(info, io.BytesIO(data))

def snapshot(home):
    return {str(p.relative_to(home)): (
        ('link', os.readlink(p)) if p.is_symlink() else
        ('dir', p.stat().st_mode) if p.is_dir() else
        ('file', p.stat().st_mode, p.read_bytes())
    ) for p in home.rglob('*')}

with tempfile.TemporaryDirectory(prefix='hx-installer-test-') as tmp:
    root = Path(tmp)
    stubs = root / 'bin'
    stubs.mkdir()
    def stub(name, script):
        path = stubs / name
        path.write_text('#!/bin/sh\nset -eu\n' + script)
        path.chmod(0o755)
    stub('uname', 'case "$1" in -s) echo "$TEST_OS";; -m) echo "$TEST_ARCH";; *) exit 97;; esac\n')
    stub('curl', '''# Only the installer's -fsSL URL -o FILE interface is allowed.
if [ "$#" -eq 6 ] && [ "$2" = --max-time ]; then shift 3; set -- -fsSL "$@"; fi
[ "$#" -eq 4 ] && [ "$1" = -fsSL ] && [ "$3" = -o ] || exit 97
case "$2" in
    file://*) cp "${2#file://}" "$4" ;;
    http://127.0.0.1:7377/healthz) [ "${TEST_FAIL_STEP:-}" != health ] || exit 1; printf '{"status":"ok"}' > "$4" ;;
    *) echo 'network forbidden' >&2; exit 97 ;;
esac
''')
    for command in ('sudo', 'security'):
        stub(command, 'echo "forbidden service command" >> "$TEST_COMMAND_LOG"; exit 97\n')
    stub('systemctl', 'printf "%s\\n" "$*" >> "$TEST_COMMAND_LOG"; [ "$1" = --user ] || exit 97\n')
    stub('launchctl', 'printf "%s\\n" "$*" >> "$TEST_COMMAND_LOG"; case "$*" in *gui/*) ;; *) exit 97;; esac; echo "state = running"\n')
    stub('loginctl', '[ "$1" = show-user ] || exit 97; echo yes\n')
    stub('sleep', 'exit 0\n')
    fixture = root / 'release'
    fixture.mkdir()
    fx = root / 'fx.tar.gz'
    probe_log = root / 'version-probes'
    fx_script = (b'#!/bin/sh\n[ "$FX_AUTO_UPGRADE" = 0 ] || exit 81\n'
                 b'[ "$1" = --version ] || exit 82\n[ "$PWD" = "$HOME" ] || exit 85\n' +
                 f'printf fx >> {shlex.quote(str(probe_log))}\n'.encode() + b'printf "0.0.16\\n"\n')
    archive(fx, {'fx': fx_script})
    good_fx = fx.read_bytes()
    source = SOURCE.read_text()
    installer = root / 'install.sh'
    # Fixture hashes replace pins only in this private copy. Production has no
    # checksum override, and real SHA-256 and tar still run in every test.
    for pin in PINS:
        assert pin in source, f'missing production pin {pin}'
        source = source.replace(pin, digest(fx))
    installer.write_text(source)
    for platform in ('linux-x86_64', 'linux-aarch64', 'macos-aarch64'):
        archive(fixture / f'hx-server-0.1.0-{platform}.tar.gz', {
            'hx-server': b'''#!/bin/sh
set -eu
if [ "$1" = --version ]; then
    case "$HOME" in */probe-home) ;; *) echo 'version probe used owner HOME' >&2; exit 83;; esac
    [ -z "${AI_GATEWAY_API_KEY+x}" ] || { echo 'version probe inherited credentials' >&2; exit 84; }
    [ "$PWD" = "$HOME" ] || { echo 'version probe used owner cwd' >&2; exit 85; }
    printf server >> __HX_PROBE_LOG__
    echo 'hx-server 0.1.0'; exit
fi
if { [ "$1" = recover-secrets ] || [ "$1" = retire-state ] || [ "$1" = probe-recovery ]; } && [ "${2:-}" = --help ]; then [ "$PWD" = "$HOME" ] || exit 85; exit 0; fi
if [ "$1" = probe-recovery ] && [ "${2:-}" = --state ]; then [ ! -e "$3/.probe-deny" ] || exit 1; exit 0; fi
[ "$1" = --state-dir ] || exit 97
state=$2
shift 2
command=$1
shift
[ "${TEST_FAIL_STEP:-}" != "$command" ] || exit 1
case "$command" in
migrate) mkdir -p "$state/blobs" "$state/locks"; : >> "$state/core.sqlite"; : >> "$state/agent.sqlite"; chmod 600 "$state/"*.sqlite ;;
status) [ "${TEST_HELD_LEASE:-}" != yes ] || exit 1; echo '{"running_interpret_leases":0}' ;;
upgrade-guard) exec "$@" ;;
backup) mkdir -p "$2"; cp "$state/core.sqlite" "$state/agent.sqlite" "$2/"; echo '{}' > "$2/manifest.json" ;;
restore) mkdir -p "$state/blobs" "$state/locks"; cp "$2/core.sqlite" "$2/agent.sqlite" "$state/" ;;
retire-state)
    [ "$1" = --from ] && [ "$3" = --to ] || exit 97
    [ ! -e "$4" ] && [ ! -L "$4" ] || exit 1
    mv "$2" "$4" ;;
recover-secrets)
    [ "$1" = --from ] && [ "$3" = --to ] || exit 97
    [ "${TEST_ROLLBACK_FAIL:-}" != secrets ] || exit 1
    if [ -d "$2/secrets" ]; then
        [ ! -L "$2/secrets" ] && [ ! -e "$4/secrets" ] && [ ! -L "$4/secrets" ] || exit 1
        mv "$2/secrets" "$4/secrets"
    fi ;;
doctor)
    case "$*" in
        *--allow-older-schema*) : ;;
        *--state-only*) [ "${TEST_FAIL_STEP:-}" != post-doctor ] || exit 1 ;;
        *) [ "${TEST_FAIL_STEP:-}" != provider ] || exit 1 ;;
    esac ;;
install-link)
    [ "$1" = --link ] && [ "$3" = --target ] || exit 97
    if [ "${TEST_ROLLBACK_FAIL:-}" = switch ] && [ "${4##*/}" = 0.1.0 ]; then exit 1; fi
    exec python3 -c 'import os,sys,tempfile; link,target=sys.argv[1:]; d=tempfile.mkdtemp(dir=os.path.dirname(link)); p=d+"/current"; os.symlink(target,p); os.rename(p,link); os.rmdir(d)' "$2" "$4" ;;

serve) printf "%s\\n" --state-dir "$state" serve "$@" > "$TEST_SERVER_ARGS"; printf "%s\\n" "$TEST_ENV" > "$TEST_SERVER_ENV" ;;
*) exit 97 ;;
esac
'''.replace(b'__HX_PROBE_LOG__', shlex.quote(str(probe_log)).encode()),
            'hx-vision-ocr': b'#!/bin/sh\nexit 0\n',
        })
    app_name = 'Hyperthymesia-0.1.0-macos-aarch64.zip'
    with zipfile.ZipFile(fixture / app_name, 'w') as app:
        app.writestr('Hyperthymesia.app/Contents/Info.plist', plistlib.dumps({'CFBundleIdentifier': 'com.hyperthymesia.agent'}))
    def sums():
        (fixture / 'SHA256SUMS').write_text(''.join(
            f'{digest(p)}  {p.name}\n' for p in sorted(fixture.iterdir()) if p.name != 'SHA256SUMS'))
    sums()
    for platform in ('macos-aarch64', 'linux-x86_64', 'linux-aarch64'):
        (fixture / f'fx-{platform}.tar.gz').write_bytes(good_fx)

    counter = 0
    def home():
        global counter
        counter += 1
        path = root / f'''home {counter} & quote' % " $ test'''
        path.mkdir()
        return path
    def run(h, *args, os_name='Linux', arch='x86_64', script=installer, tmpdir=None, cwd=None):
        env = {k: v for k, v in os.environ.items() if not k.startswith(('HX_', 'FX_'))}
        env.update(HOME=str(h), PATH=str(stubs) + os.pathsep + os.environ['PATH'],
                   TMPDIR=tmpdir or str(root), HX_RELEASE_BASE=fixture.as_uri(),
                   HX_FX_RELEASE_BASE=fixture.as_uri(), TEST_OS=os_name, TEST_ARCH=arch,
                   TEST_COMMAND_LOG=str(root / 'commands'),
                   AI_GATEWAY_API_KEY='synthetic-key-never-print',
                   TEST_FAIL_STEP=os.environ.get('TEST_FAIL_STEP', ''),
                   TEST_HELD_LEASE=os.environ.get('TEST_HELD_LEASE', ''))
        result = subprocess.run(['/bin/sh', str(script), *args], env=env, cwd=cwd, text=True, capture_output=True, timeout=20)
        return result
    def fails_unchanged(h, expected, *args, **kwargs):
        before = snapshot(h)
        result = run(h, *args, **kwargs)
        assert result.returncode == expected, (result.returncode, result.stdout, result.stderr)
        assert snapshot(h) == before, 'failure changed HOME'
        return result

    # Fail-first against the files-only installer: one command must initialize
    # private state and invoke a user service without exposing a supplied secret.
    h = home()
    result = run(h, '--provider', 'gateway')
    assert result.returncode == 0, result.stderr
    state = h / '.local/state/hyperthymesia'
    assert (state / 'core.sqlite').is_file(), 'one-command install did not migrate'
    assert state.stat().st_mode & 0o777 == 0o700
    assert (state / 'server.env').stat().st_mode & 0o777 == 0o600
    assert 'synthetic-key-never-print' not in result.stdout + result.stderr
    assert '--prefix source:' in result.stdout and '--scopes read_raw,read_l3' in result.stdout
    assert 'http://127.0.0.1:7377/login' in result.stdout
    original_env = (state / 'server.env').read_bytes()
    assert run(h).returncode == 0
    assert (state / 'server.env').read_bytes() == original_env
    print('ok: one command migrates, starts a user service, preserves private env')

    # A remembered history must never turn into an empty replacement core,
    # including a custom state path whose volume disappeared.
    for custom in (False, True):
        h = home()
        state = h / ('mounted/history' if custom else '.local/state/hyperthymesia')
        args = ('--state-dir', str(state)) if custom else ()
        assert run(h, *args).returncode == 0
        (state / 'core.sqlite').write_text('irreplaceable synthetic evidence')
        saved = state.with_name(state.name + '.failed-synthetic')
        state.rename(saved)
        result = fails_unchanged(h, 1, *args)
        assert 'backups/' in result.stderr and 'failed-' in result.stderr
        assert 'restore' in result.stderr and '--start-fresh' in result.stderr
        assert not state.exists()
        assert (saved / 'core.sqlite').read_text() == 'irreplaceable synthetic evidence'
        assert run(h, *args, '--start-fresh').returncode == 0
        assert (saved / 'core.sqlite').read_text() == 'irreplaceable synthetic evidence'
    print('ok: missing registered state refuses without writes; fresh history is explicit')

    # A successful dry run must render the complete plan before rejection tests
    # can be credited: rejecting every invocation is not a valid installer.
    h = home()
    before = snapshot(h)
    probe_log.unlink(missing_ok=True)
    result = run(h, '--dry-run')
    assert result.returncode == 0, result.stderr
    assert not probe_log.exists(), 'dry-run executed a downloaded binary'
    assert 'hyperthymesia.service' in result.stdout, 'missing Linux service rendering'
    assert '127.0.0.1:7377' in result.stdout, 'missing service wrapper'
    assert snapshot(h) == before, 'dry run changed HOME'
    print('ok: Linux dry run renders without writes')

    relative_work = root / 'relative-work'
    (relative_work / 'tmp').mkdir(parents=True)
    previous_cdpath = os.environ.get('CDPATH')
    os.environ['CDPATH'] = '.'
    result = run(home(), '--dry-run', tmpdir='tmp', cwd=relative_work)
    if previous_cdpath is None:
        os.environ.pop('CDPATH')
    else:
        os.environ['CDPATH'] = previous_cdpath
    assert result.returncode == 0, (result.stdout, result.stderr)
    assert not list((relative_work / 'tmp').iterdir()), 'scratch directory was not removed'
    print('ok: relative TMPDIR with CDPATH succeeds and cleans scratch')

    # Hash failures must not modify even an existing managed installation.
    installed_home = home()
    result = run(installed_home)
    assert result.returncode == 0, result.stderr
    for platform in ('linux-x86_64', 'linux-aarch64', 'macos-aarch64'):
        target = fixture / f'fx-{platform}.tar.gz'
        target.write_bytes(good_fx + b'tampered')
        os_name, arch = ('Darwin', 'arm64') if platform.startswith('macos') else ('Linux', platform[6:])
        fails_unchanged(home(), 1, os_name=os_name, arch=arch)
        if platform == 'linux-x86_64':
            fails_unchanged(installed_home, 1)
        target.write_bytes(good_fx)
    print('ok: all pinned fx archives reject tampering without writes')

    wrong = root / 'wrong.tar.gz'
    archive(wrong, {'fx': fx_script.replace(b'0.0.16', b'0.0.15')})
    wrong_installer = root / 'wrong-install.sh'
    wrong_installer.write_text(source.replace(digest(fx), digest(wrong)))
    (fixture / 'fx-linux-x86_64.tar.gz').write_bytes(wrong.read_bytes())
    fails_unchanged(home(), 1, script=wrong_installer)
    fails_unchanged(installed_home, 1, script=wrong_installer)
    (fixture / 'fx-linux-x86_64.tar.gz').write_bytes(good_fx)
    print('ok: authenticated wrong fx version rejects without writes')

    server_asset = fixture / 'hx-server-0.1.0-linux-x86_64.tar.gz'
    original = server_asset.read_bytes()
    server_asset.write_bytes(original + b'tampered')
    fails_unchanged(home(), 1)
    server_asset.write_bytes(original)
    sums_file = fixture / 'SHA256SUMS'
    original_sums = sums_file.read_bytes()
    sums_file.unlink()
    fails_unchanged(home(), 1)
    sums_file.write_bytes(original_sums)
    print('ok: tampered server and missing downloads reject without writes')

    # An earlier payload cannot perform the new recovery step. Reject it
    # before stopping a service or touching existing state.
    with tarfile.open(server_asset) as archive_in:
        files = {member.name: archive_in.extractfile(member).read() for member in archive_in}
    old_help = b'if { [ "$1" = recover-secrets ] || [ "$1" = retire-state ] || [ "$1" = probe-recovery ]; } && [ "${2:-}" = --help ]; then [ "$PWD" = "$HOME" ] || exit 85; exit 0; fi'
    assert old_help in files['hx-server']
    files['hx-server'] = files['hx-server'].replace(old_help, b'if [ "$1" = recover-secrets ]; then exit 97; fi')
    archive(server_asset, files)
    sums()
    log = root / 'commands'
    log.write_text('')
    fails_unchanged(installed_home, 1)
    assert '--user stop' not in log.read_text()
    server_asset.write_bytes(original)
    sums()
    print('ok: incompatible server payload is refused before stopping or migrating')

    probe_denied = installed_home / '.local/state/hyperthymesia/.probe-deny'
    probe_denied.write_text('synthetic')
    log.write_text('')
    fails_unchanged(installed_home, 1)
    assert '--user stop' not in log.read_text()
    probe_denied.unlink()
    print('ok: unsupported recovery rename refuses before stopping the service')

    for legacy in ('Library/LaunchAgents/com.hyperthymesia.server.plist',
                   'Library/Application Support/Hyperthymesia/server-data',
                   'Library/Application Support/Hyperthymesia/agent.json'):
        h = home()
        p = h / legacy
        p.parent.mkdir(parents=True, exist_ok=True)
        # A FIFO makes an accidental attempt to read contents hang/fail.
        os.mkfifo(p)
        before_paths = sorted(str(path.relative_to(h)) for path in h.rglob('*'))
        result = run(h, '--dry-run', os_name='Darwin', arch='arm64')
        assert result.returncode == 3, result.stderr
        assert 'crates/hx-move/README.md' in result.stderr
        assert p.is_fifo()
        result = run(h, '--uninstall', os_name='Darwin', arch='arm64')
        assert result.returncode == 3, result.stderr
        assert before_paths == sorted(str(path.relative_to(h)) for path in h.rglob('*'))
    print('ok: owner files are checked by existence only, also on uninstall')

    fails_unchanged(home(), 1, '--dry-run', os_name='Darwin', arch='x86_64')
    fails_unchanged(home(), 1, '--dry-run', arch='riscv64')
    print('ok: unsupported platforms reject without writes')

    for os_name, arch in (('Linux', 'x86_64'), ('Linux', 'aarch64'), ('Darwin', 'arm64')):
        h = home()
        dry = run(h, '--dry-run', os_name=os_name, arch=arch)
        assert dry.returncode == 0, dry.stderr
        result = run(h, os_name=os_name, arch=arch)
        assert result.returncode == 0, result.stderr
        prefix = h / '.local/share/hyperthymesia'
        assert (prefix / 'tools/fx-0.0.16/fx').is_file()
        assert (h / '.local/bin/hx-server').is_symlink()
        for p in h.rglob('*'):
            if p.is_file() or p.is_symlink():
                assert str(p) in dry.stdout, f'dry run omitted {p}'
        wrapper = prefix / 'current/hx-run'
        assert subprocess.run(['/bin/sh', '-n', str(wrapper)]).returncode == 0
        if os_name == 'Darwin':
            service = h / 'Library/LaunchAgents/com.hyperthymesia.server.plist'
            contents = plistlib.loads(service.read_bytes())
            assert contents['RunAtLoad'] and contents['KeepAlive']
            assert contents['ProgramArguments'] == [str(wrapper)]
            if shutil.which('plutil') and os.uname().sysname == 'Darwin':
                subprocess.run(['plutil', '-lint', str(service)], check=True)
        else:
            service = h / '.config/systemd/user/hyperthymesia.service'
            assert 'Restart=always' in service.read_text()
            assert 'WantedBy=default.target' in service.read_text()
            assert '%%' in service.read_text(), 'systemd specifiers must be escaped'
            if shutil.which('systemd-analyze') and os.uname().sysname == 'Linux':
                subprocess.run(['systemd-analyze', 'verify', str(service)], check=True)
        # Repeat installation is safe; state and a personal fx survive removal.
        assert run(h, os_name=os_name, arch=arch).returncode == 0
        state = h / ('Library/Application Support/Hyperthymesia/server' if os_name == 'Darwin' else '.local/state/hyperthymesia')
        state.mkdir(parents=True, exist_ok=True)
        (state / 'sentinel').write_text('keep state')
        env_file = state / 'server.env'
        env_file.write_text("TEST_ENV='fixture env value'\n")
        env_file.chmod(0o600)
        args_file, env_output = root / 'server-args', root / 'server-env'
        env = dict(os.environ, HOME=str(h), PATH=str(stubs) + os.pathsep + os.environ['PATH'],
                   TEST_SERVER_ARGS=str(args_file), TEST_SERVER_ENV=str(env_output))
        subprocess.run(['/bin/sh', str(wrapper)], env=env, check=True)
        assert args_file.read_text().splitlines() == ['--state-dir', str(state), 'serve', '--listen', '127.0.0.1:7377']
        assert env_output.read_text() == 'fixture env value\n'
        personal_fx = h / '.local/bin/fx'
        personal_fx.write_text('keep personal fx')
        result = run(h, '--uninstall', os_name=os_name, arch=arch)
        assert result.returncode == 0, result.stderr
        assert (state / 'sentinel').read_text() == 'keep state'
        assert personal_fx.read_text() == 'keep personal fx'
        assert not service.exists()
        assert not (h / '.local/bin/hx-server').is_symlink()
    print('ok: installs, service syntax, repeat install and state-preserving uninstall')

    # Publishing a prefix without its marker must be impossible even if the
    # marker copy fails. The staged directory can be ignored safely on retry.
    h = home()
    stub('cp', 'case "$*" in *hyperthymesia.staging.*/.installed*) exit 1;; esac\nexec /bin/cp "$@"\n')
    failed = run(h)
    (stubs / 'cp').unlink()
    assert failed.returncode != 0
    assert not (h / '.local/share/hyperthymesia').exists()
    assert run(h).returncode == 0
    print('ok: ownership marker is published atomically with prefix')

    # A disk-full or interrupted first install must stay ours to retry; the owner
    # refusal has no override, so a marker-less prefix would be a dead end.
    h = home()
    stub('cp', 'case "$*" in */tools/fx-0.0.16/*) exit 1;; esac\nexec /bin/cp "$@"\n')
    failed = run(h)
    (stubs / 'cp').unlink()
    assert failed.returncode != 0, 'stubbed cp failure was not reached'
    retry = run(h)
    assert retry.returncode == 0, (retry.returncode, retry.stderr)
    assert (h / '.local/share/hyperthymesia/tools/fx-0.0.16/fx').is_file()
    print('ok: an interrupted first install can be retried')

    h = home()
    assert run(h).returncode == 0
    before = snapshot(h)
    os.environ['TEST_HELD_LEASE'] = 'yes'
    result = run(h)
    os.environ.pop('TEST_HELD_LEASE')
    assert result.returncode != 0
    assert snapshot(h) == before, 'held lease changed installation'
    print('ok: held lease refuses upgrade without writes')

    # Refusable state checks happen before any stop or destination write.
    log = root / 'commands'
    log.write_text('')
    os.environ['TEST_FAIL_STEP'] = 'doctor'
    fails_unchanged(h, 1)
    os.environ.pop('TEST_FAIL_STEP')
    assert '--user stop' not in log.read_text()
    print('ok: state preflight refusal leaves old service and files untouched')

    # An upgraded payload must survive rollback with its old evidence/env intact.
    state = h / '.local/state/hyperthymesia'
    (state / 'core.sqlite').write_text('synthetic seeded evidence')
    (state / 'secrets').mkdir(mode=0o700)
    (state / 'secrets/source.key').write_text('synthetic-source-key')
    (state / 'secrets/source.key').chmod(0o600)
    env_before = (state / 'server.env').read_bytes()
    old = os.readlink(h / '.local/share/hyperthymesia/current')
    for platform in ('linux-x86_64', 'linux-aarch64', 'macos-aarch64'):
        asset = fixture / f'hx-server-0.1.0-{platform}.tar.gz'
        files = {}
        with tarfile.open(asset) as archive_in:
            for member in archive_in:
                files[member.name] = archive_in.extractfile(member).read().replace(b'hx-server 0.1.0', b'hx-server 0.1.1')
        archive(fixture / f'hx-server-0.1.1-{platform}.tar.gz', files)
        asset.unlink()
    sums()
    os.environ['TEST_FAIL_STEP'] = 'post-doctor'
    result = run(h, '--server-only')
    os.environ.pop('TEST_FAIL_STEP')
    assert result.returncode != 0
    assert os.readlink(h / '.local/share/hyperthymesia/current') == old
    assert (state / 'core.sqlite').read_text() == 'synthetic seeded evidence'
    assert (state / 'server.env').read_bytes() == env_before
    assert (state / 'secrets/source.key').read_text() == 'synthetic-source-key'
    assert (state / 'secrets/source.key').stat().st_mode & 0o777 == 0o600
    assert (state / 'server.log').stat().st_mode & 0o777 == 0o600
    os.environ['TEST_FAIL_STEP'] = 'provider'
    provider_warning = run(h, '--server-only')
    os.environ.pop('TEST_FAIL_STEP')
    assert provider_warning.returncode == 0, provider_warning.stderr
    assert 'provider setup is incomplete' in provider_warning.stderr
    assert (state / 'core.sqlite').read_text() == 'synthetic seeded evidence'
    assert Path(old).is_dir(), 'previous version was removed'
    print('ok: failed upgrade restores old version/evidence; missing provider only warns')

    # Uninstall does not erase history or the obligation to back up on reinstall.
    assert run(h, '--uninstall').returncode == 0
    backups = set((h / '.local/share/hyperthymesia/backups').iterdir())
    assert run(h, '--server-only').returncode == 0
    assert set((h / '.local/share/hyperthymesia/backups').iterdir()) > backups
    assert (state / 'core.sqlite').read_text() == 'synthetic seeded evidence'
    print('ok: reinstall without current backs up existing evidence')

    # Failed recovery IO must never short-circuit the attempt to restart old.
    for fault in ('move', 'copy', 'switch', 'secrets'):
        h = home()
        # Reuse the earlier retained version as a fixture of an old install.
        prefix = h / '.local/share/hyperthymesia'
        shutil.copytree(Path(old).parent, prefix, symlinks=True)
        current = prefix / 'current'
        current.unlink()
        current.symlink_to(prefix / '0.1.0')
        (prefix / '.last-version').write_text(str(prefix / '0.1.0') + '\n')
        state = h / '.local/state/hyperthymesia'
        state.mkdir(parents=True, mode=0o700)
        (prefix / '.state-dir').write_text(str(state) + '\n')
        for name in ('core.sqlite', 'agent.sqlite', 'server.env', 'server.log'):
            (state / name).write_text('' if name != 'core.sqlite' else 'saved')
            (state / name).chmod(0o600)
        if fault == 'secrets':
            (state / 'secrets').mkdir(mode=0o700)
            (state / 'secrets/source.key').write_text('retained-synthetic-key')
            (state / 'secrets/source.key').chmod(0o600)
        if fault == 'move':
            stub('mv', 'case "$*" in *failed-*) exit 1;; esac\nexec /bin/mv "$@"\n')
        if fault == 'copy':
            stub('cp', 'case "$*" in *failed-*/server.env*) exit 1;; esac\nexec /bin/cp "$@"\n')
        log = root / 'commands'
        log.write_text('')
        os.environ['TEST_FAIL_STEP'] = 'post-doctor'
        os.environ['TEST_ROLLBACK_FAIL'] = fault
        result = run(h, '--server-only')
        os.environ.pop('TEST_FAIL_STEP')
        os.environ.pop('TEST_ROLLBACK_FAIL')
        for command in ('mv', 'cp'):
            if (stubs / command).exists(): (stubs / command).unlink()
        assert result.returncode != 0
        if fault != 'switch':
            assert log.read_text().count('enable --now hyperthymesia.service') == 1, result.stderr
            assert (prefix / 'pending-upgrade').is_symlink()
            if fault == 'secrets':
                retained = list(state.parent.glob(state.name + '.failed-*'))
                assert len(retained) == 1 and (retained[0] / 'secrets/source.key').read_text() == 'retained-synthetic-key'
                assert 'Source-key recovery incomplete' in result.stderr
            else:
                assert 'Recovery incomplete' in result.stderr
            continue
        assert log.read_text().count('enable --now hyperthymesia.service') == 2, result.stderr
        if fault == 'switch':
            assert os.readlink(prefix / 'current') == str(prefix / '0.1.0'), 'restart must target the old version even if atomic switch fails'
        assert 'Recovery incomplete' in result.stderr or 'Could not restore current link' in result.stderr
    print('ok: incomplete recovery retains its record and does not restart the old service')

    # Run the README fx snippet against a bad download and an existing
    # synthetic binary; checksum failure must stop before that binary runs.
    text = Path('README.md').read_text()
    block = text.split('Install the pinned model runner, fx 0.0.16', 1)[1].split('```sh\n', 1)[1].split('\n```', 1)[0]
    h = home()
    old_fx = h / '.local/share/hyperthymesia/tools/fx-0.0.16/fx'
    old_fx.parent.mkdir(parents=True)
    old_fx.write_text('#!/bin/sh\ntouch "$HOME/existing-fx-ran"\n')
    old_fx.chmod(0o700)
    old_curl = (stubs / 'curl').read_text()
    stub('curl', '[ "$1" = -fL ] && [ "$3" = -o ] || exit 97\nprintf bad-archive > "$4"\n')
    env = dict(os.environ, HOME=str(h), PATH=str(stubs) + os.pathsep + os.environ['PATH'],
               TMPDIR=str(root), TEST_OS='Darwin', TEST_ARCH='arm64')
    result = subprocess.run(['/bin/sh', '-c', 'set -e\n' + block], env=env,
                            capture_output=True, text=True, timeout=20)
    (stubs / 'curl').write_text(old_curl)
    assert result.returncode != 0 and 'verification failed' in result.stderr.lower(), result.stderr
    assert not (h / 'existing-fx-ran').exists()
    print('ok: README checksum failure stops before any existing fx can run')
PY
