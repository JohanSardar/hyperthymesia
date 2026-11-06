#!/usr/bin/env python3
"""Real user-service smoke test. GitHub-hosted disposable runners only.

Unlike install.test.sh, this intentionally bootstraps a real LaunchAgent or
systemd user unit. Never invoke it on a developer/owner machine.
"""
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import sqlite3
import subprocess
import tempfile
import time
import urllib.request

assert os.environ.get('GITHUB_ACTIONS') == 'true', 'runner-only: use install.test.sh locally'
assert os.environ.get('RUNNER_ENVIRONMENT') == 'github-hosted', 'requires a disposable GitHub-hosted runner'
assets = Path(os.environ['HX_RELEASE_DIR']).resolve()
installer = assets / 'install.sh'
is_mac = platform.system() == 'Darwin'
label = 'com.hyperthymesia.server'
domain = f'gui/{os.getuid()}'
service = 'hyperthymesia.service'

with tempfile.TemporaryDirectory(prefix='hx-install-smoke-', dir=os.environ['RUNNER_TEMP']) as tmp:
    root = Path(tmp)
    home = root / 'home'
    home.mkdir(mode=0o700)
    env = {k: v for k, v in os.environ.items() if not k.startswith(('HX_', 'FX_'))}
    env.update(HOME=str(home), HX_RELEASE_BASE=assets.as_uri())
    state = home / ('Library/Application Support/Hyperthymesia/server' if is_mac else '.local/state/hyperthymesia')
    prefix = home / '.local/share/hyperthymesia'
    server = prefix / 'current/hx-server'

    # A pre-existing user manager keeps the runner's original HOME. Register
    # only a disposable runtime link so it can discover the generated unit in
    # our scratch HOME; never write the runner's persistent user configuration.
    runtime_link = None
    if not is_mac:
        runtime_link = Path(env['XDG_RUNTIME_DIR']) / 'systemd/user' / service
        assert not runtime_link.exists() and not runtime_link.is_symlink(), 'unmanaged runner unit exists'
        runtime_link.parent.mkdir(parents=True, exist_ok=True)
        runtime_link.symlink_to(home / '.config/systemd/user' / service)

    def run(args, *, check=True, timeout=120):
        result = subprocess.run([str(a) for a in args], env=env, capture_output=True, text=True, timeout=timeout)
        if check and result.returncode:
            raise AssertionError(f'{args[0]} failed ({result.returncode}):\n{result.stdout}\n{result.stderr}')
        return result

    def pid():
        if is_mac:
            result = run(['launchctl', 'print', f'{domain}/{label}'], check=False)
            for line in result.stdout.splitlines():
                if line.strip().startswith('pid = '):
                    return int(line.split('=')[1])
        else:
            result = run(['systemctl', '--user', 'show', service, '-p', 'MainPID', '--value'], check=False)
            if result.stdout.strip().isdigit():
                return int(result.stdout.strip()) or None
        return None

    def wait_for(predicate):
        deadline = time.monotonic() + 45
        while time.monotonic() < deadline:
            value = predicate()
            if value:
                return value
            time.sleep(0.25)
        raise AssertionError('timed out waiting for user service')

    def health():
        try:
            with urllib.request.urlopen('http://127.0.0.1:7377/healthz', timeout=2) as response:
                return response.read() == b'ok\n'
        except OSError:
            return False

    def api(path, token, body=None):
        request = urllib.request.Request('http://127.0.0.1:7377' + path,
            data=json.dumps(body).encode() if body is not None else None,
            headers={'Authorization': 'Bearer ' + token, 'Content-Type': 'application/json'})
        with urllib.request.urlopen(request, timeout=5) as response:
            return json.load(response)

    def db(sql, params=()):
        with sqlite3.connect(state / 'core.sqlite', timeout=10) as connection:
            return connection.execute(sql, params).fetchall()

    def load_env():
        # No shell output: secrets remain in a private file and inherited env.
        output = run(['/bin/sh', '-c', 'set -a; . "$1"; env -0', 'sh', state / 'server.env']).stdout
        return dict(item.split('=', 1) for item in output.split('\0') if '=' in item)

    try:
        run(['/bin/sh', installer, '--server-only'])
        wait_for(health)
        first = wait_for(pid)
        assert state.stat().st_mode & 0o777 == 0o700
        for name in ['core.sqlite', 'agent.sqlite', 'server.env', 'server.log']:
            assert (state / name).stat().st_mode & 0o777 == 0o600, name
        assert len(list(home.rglob('core.sqlite'))) == 1, 'first install created more than one core'
        print('ok: one command starts a real user service with private state', flush=True)

        # A fake model executable can only answer --version; no network/model
        # operation is allowed. The installer still verifies the real pinned fx.
        fake = root / 'fake-fx'
        fake.write_text('#!/bin/sh\n[ "$1" = --version ] && { echo 0.0.16; exit; }\necho "unexpected model call" >&2\nexit 99\n')
        fake.chmod(0o700)
        login = home / '.fx/chatgpt-auth.json'
        login.parent.mkdir(mode=0o700)
        login.write_text('{}')
        login.chmod(0o600)
        env_file = state / 'server.env'
        original = env_file.read_text()
        original = '\n'.join(line for line in original.splitlines() if not line.startswith('HX_FX_BINARY=')) + f"\nHX_FX_BINARY='{fake}'\n"
        env_file.write_text(original)
        before_env = env_file.read_bytes()
        os.kill(first, signal.SIGKILL)
        second = wait_for(lambda: (value if (value := pid()) and value != first else None))
        wait_for(health)
        print('ok: kill -9 restarts the user service', flush=True)
        doctor_env = load_env()
        result = subprocess.run([str(server), '--state-dir', str(state), 'doctor'], env=doctor_env,
                                capture_output=True, text=True, timeout=15)
        assert result.returncode == 0, result.stderr

        token = run([server, '--state-dir', state, 'token', 'create', '--name', 'synthetic smoke',
                     '--prefix', 'test:install', '--scopes', 'ingest,read_raw,read_l3']).stdout.strip()
        event = {
            'envelope_version': 1, 'observation_id': '01J00000000000000000000001',
            'connector_instance': 'test:install', 'source_key': 'synthetic', 'revision': '1',
            'operation': 'observe', 'kind': 'web.visit', 'schema_version': 1,
            # Current-window evidence cannot admit an interpretation yet.
            'time': {'observed_at': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())},
            'sensitivity': 'normal', 'payload': {'url': 'https://example.invalid', 'title': 'Synthetic installer evidence'},
        }
        ack = api('/v1/events', token, {'batch_id': '01J00000000000000000000002', 'envelopes': [event]})
        assert ack['accepted'] == 1, ack
        evidence_id = db("SELECT id FROM events WHERE connector_instance='test:install'")[0][0]
        evidence_before = api(f'/v1/evidence/{evidence_id}', token)
        rows_before = db('SELECT id,envelope FROM events ORDER BY id')

        # The public CLI has no way to manufacture a held lease. Inject just
        # that queue state, without invoking a provider or weakening admission.
        db("INSERT INTO jobs(output_key,kind,payload,state,available_at,lease_until,lease_owner) VALUES('smoke-held','interpret','{}','running',0,?,'synthetic')",
           (int((time.time() + 300) * 1_000_000),))
        status = json.loads(run([server, '--state-dir', state, 'status']).stdout)
        assert status['running_interpret_leases'] == 1
        before_current = os.readlink(prefix / 'current')
        refused = run(['/bin/sh', installer, '--server-only'], check=False)
        assert refused.returncode != 0, 'upgrade ignored a held lease'
        assert os.readlink(prefix / 'current') == before_current
        assert pid() == second, 'lease refusal stopped the service'
        assert env_file.read_bytes() == before_env
        db("DELETE FROM jobs WHERE output_key='smoke-held'")
        print('ok: held interpret lease refuses upgrade and leaves service running', flush=True)

        # Exercise a distinct installed version directory from the same build.
        # v0.1.0 and 0.1.0 identify the same binary; no second build or tag is
        # needed. The store's separate fail-first test proves missing-schema DDL.
        upgrade = root / 'upgrade'
        upgrade.mkdir()
        asset = next(assets.glob('hx-server-*.tar.gz'))
        alternate = asset.name.replace('hx-server-', 'hx-server-v', 1)
        target = upgrade / alternate
        target.write_bytes(asset.read_bytes())
        (upgrade / 'SHA256SUMS').write_text(hashlib.sha256(target.read_bytes()).hexdigest() + '  ' + target.name + '\n')
        env['HX_RELEASE_BASE'] = upgrade.as_uri()
        run(['/bin/sh', installer, '--server-only'])
        wait_for(health)
        assert os.readlink(prefix / 'current') != before_current
        assert Path(before_current).is_dir(), 'old binary was removed'
        assert env_file.read_bytes() == before_env
        assert db('SELECT id,envelope FROM events ORDER BY id') == rows_before
        assert api(f'/v1/evidence/{evidence_id}', token) == evidence_before
        assert list((prefix / 'backups').glob('*/manifest.json')), 'upgrade skipped P5 backup'
        print('ok: upgrade keeps seeded evidence, env, backup and previous version', flush=True)
    finally:
        # Always stop the real service before the temporary HOME is deleted.
        run(['/bin/sh', installer, '--uninstall'], check=False)
        if runtime_link is not None and runtime_link.is_symlink():
            runtime_link.unlink()
            run(['systemctl', '--user', 'daemon-reload'], check=False)
