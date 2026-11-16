# Installer design and acceptance

The public launcher is `docs/install`, served by GitLab Pages at
https://hyperthymesia.eilmer.si/install (see [releases](../releases.md)). Its first two settings pin a release
version and the SHA-256 of that release's SHA256SUMS file. This one manifest pin
binds the lifecycle installer and all platform archives. The launcher verifies
the manifest, then install.sh, before executing downloaded code. The lifecycle
installer verifies the same manifest again before extracting platform assets.
No checksum override is exposed by the public launcher.

Initial bootstrap pins are deliberately unusable (`0.0.0` and an all-zero hash).
After publishing the verified GitHub release, a separate PR changes **only**
VERSION and SHA256. Never present bootstrap pins as a working installation.

Program files stay under `~/.local/share/hyperthymesia`; entry points are managed
symlinks. State remains outside the program prefix so rollback cannot replace
history with a version directory. Existing unmanaged programs, folders, state or
service entries are refused. Previous versions and backups are retained.

PATH setup prefers an existing .zshrc or a zsh login shell, otherwise the first
existing .bash_profile, .bash_login, .profile or .bashrc; with none, it creates
.profile. It preserves existing content/mode, stages the marked idempotent block
beside the startup file, then atomically renames it. Partial-write failure leaves
the original untouched and removes the private stage during rollback.
Sourcing repeatedly does not duplicate the bin directory. A symlink or
non-regular startup path is refused before changing destinations rather than
writing into an unexpected external dotfiles target. Uninstall retains PATH
configuration and all state.

## Executed offline tests

`sh scripts/install.test.sh` uses scratch homes, real hashes/archives and stubbed
service managers. It covers fresh installation, update, tampered runner/server,
unrelated commands/folders, Bash-only startup, zsh startup, failed upgrades,
partial startup-file writes, and INT/TERM immediately after the atomic current-link swap. Both
signals restore the old version, evidence, environment and source keys.

`python3 scripts/installer-launcher.test.py` pipes the real launcher through sh.
It checks manifest tampering, installer tampering, duplicate checksum entries,
pinned URL/argument forwarding and scratch cleanup. These are fixture tests,
not proof of public hosting or a real service.

## Real-host evidence

The release workflow checks real user-service installation on disposable Linux
and macOS runners using produced artifacts. `Live release acceptance` separately
runs the exact public curl command against Pages and GitHub release assets on a
new GitHub-hosted Ubuntu VM, verifies CLI/health/rerun, then clones the public
repository and runs its default contributor checks without development caches.
It cannot run on a developer/self-hosted machine. It never logs in or calls a
model. Until that workflow completes against the final public pin, live curl
and clean-machine first run remain **unproven**.
