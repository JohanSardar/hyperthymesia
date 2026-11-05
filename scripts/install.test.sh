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
import shutil
import subprocess
import tarfile
import tempfile
import zipfile

SOURCE = Path('scripts/install.sh').resolve()
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
[ "$#" -eq 4 ] && [ "$1" = -fsSL ] && [ "$3" = -o ] || exit 97
case "$2" in file://*) cp "${2#file://}" "$4";; *) echo 'network forbidden' >&2; exit 97;; esac
''')
    for command in ('launchctl', 'systemctl', 'loginctl', 'sudo', 'security'):
        stub(command, 'echo "forbidden service command" >> "$TEST_COMMAND_LOG"; exit 97\n')
    fixture = root / 'release'
    fixture.mkdir()
    fx = root / 'fx.tar.gz'
    fx_script = b'#!/bin/sh\n[ "$FX_AUTO_UPGRADE" = 0 ] || exit 81\n[ "$1" = --version ] || exit 82\nprintf "0.0.16\\n"\n'
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
            'hx-server': b'#!/bin/sh\nprintf "%s\\n" "$@" > "$TEST_SERVER_ARGS"\nprintf "%s\\n" "$TEST_ENV" > "$TEST_SERVER_ENV"\n',
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
    def run(h, *args, os_name='Linux', arch='x86_64', script=installer):
        env = {k: v for k, v in os.environ.items() if not k.startswith(('HX_', 'FX_'))}
        env.update(HOME=str(h), PATH=str(stubs) + os.pathsep + os.environ['PATH'],
                   TMPDIR=str(root), HX_RELEASE_BASE=fixture.as_uri(),
                   HX_FX_RELEASE_BASE=fixture.as_uri(), TEST_OS=os_name, TEST_ARCH=arch,
                   TEST_COMMAND_LOG=str(root / 'commands'))
        result = subprocess.run(['/bin/sh', str(script), *args], env=env, text=True, capture_output=True, timeout=20)
        assert not (root / 'commands').exists(), 'called a real-service interface'
        return result
    def fails_unchanged(h, expected, *args, **kwargs):
        before = snapshot(h)
        result = run(h, *args, **kwargs)
        assert result.returncode == expected, (result.returncode, result.stdout, result.stderr)
        assert snapshot(h) == before, 'failure changed HOME'
        return result

    # A successful dry run must render the complete plan before rejection tests
    # can be credited: rejecting every invocation is not a valid installer.
    h = home()
    before = snapshot(h)
    result = run(h, '--dry-run')
    assert result.returncode == 0, result.stderr
    assert 'hyperthymesia.service' in result.stdout, 'missing Linux service rendering'
    assert '127.0.0.1:7377' in result.stdout, 'missing service wrapper'
    assert snapshot(h) == before, 'dry run changed HOME'
    print('ok: Linux dry run renders without writes')

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
        assert 'docs/moving-to-sqlite.md' in result.stderr
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
PY
