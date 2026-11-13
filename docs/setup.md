# Source build and foreground setup

For the managed binary install, use the [project README](../README.md#install).
Read its [safety notes](../README.md#before-you-collect-data) before collecting or connecting anything.

## Install from source

No Docker or database service is needed. The server uses SQLite and one explicit
state folder on a **local disk**. The Mac app requires Apple Silicon and macOS 14+;
the server also supports Linux x86_64/aarch64. Install Git, Python 3, curl and
[Rust through rustup](https://rustup.rs/). On a Mac, install the Xcode command line
tools first. On Linux, install a C compiler, linker and build tools
(Debian/Ubuntu: `build-essential` and `pkg-config`). Cargo uses the toolchain pinned
in `rust-toolchain.toml`.

```sh
set -e
git clone https://github.com/JohanSardar/hyperthymesia.git
cd hyperthymesia
cargo build --locked --release -p hx-server
server_dir="$(cd "${CARGO_TARGET_DIR:-target}/release" && pwd)"
export PATH="$server_dir:$PATH"
hx-server --version
```

Install the pinned model runner, fx 0.0.16, without replacing your existing fx:

```sh
case "$(uname -s):$(uname -m)" in
  Darwin:arm64) platform=macos-aarch64; checksum=2158a8d42e77457a774400d225d9802bb6c0e60a0bb0674a1116bc31252490d6 ;;
  Linux:x86_64) platform=linux-x86_64; checksum=f0fb822c7be546c80bcc89d50521bc1454435e0c6f586e8420c4e2b84fa80599 ;;
  Linux:aarch64) platform=linux-aarch64; checksum=ca9441b104357e2105039dbfd4c2eaf0d6ee65a84085f23f51e880c95cbf46af ;;
  *) echo 'Unsupported platform' >&2; exit 1 ;;
esac
fx_dir="$HOME/.local/share/hyperthymesia-source/tools/fx-0.0.16"
download_dir="$(mktemp -d)"
curl -fL "https://github.com/vercel-labs/fx/releases/download/v0.0.16/fx-$platform.tar.gz" \
  -o "$download_dir/fx.tar.gz"
if ! python3 -c 'import hashlib,pathlib,sys; actual=hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest(); sys.exit(0 if actual==sys.argv[2] else "fx checksum mismatch")' \
  "$download_dir/fx.tar.gz" "$checksum"; then
  rm -rf "$download_dir"
  echo 'Download verification failed. Nothing was installed; retry the download.' >&2
  exit 1
fi
if [ -e "$fx_dir" ] || [ -L "$fx_dir" ]; then
  rm -rf "$download_dir"
  echo 'Refusing an existing source-runner directory; use a new private destination.' >&2
  exit 3
fi
mkdir -p "$fx_dir"
tar -xzf "$download_dir/fx.tar.gz" -C "$fx_dir"
rm -rf "$download_dir"
export HX_FX_BINARY="$fx_dir/fx"
FX_AUTO_UPGRADE=0 FX_DISABLE_KEYCHAIN=1 "$HX_FX_BINARY" --version
```

If downloading or checksum verification fails, stop here. Do not run the archive.
For verified release artifacts and a managed user service, see
[the installer guide](install.md). This foreground build does not register a service
or replace the managed binary. Its runner uses a separate source-development folder.

## Log in and start the server

Choose an empty state folder for the first run. Keep using the same folder for
every server command. `migrate` initializes it; `serve` never silently creates
history. Existing installations should follow the [upgrade guide](install.md#upgrade-and-recovery).

```sh
export HX_STATE_DIR="$HOME/.local/state/hyperthymesia-source"
export HX_PROVIDER=fx
umask 077
hx-server migrate
touch "$HX_STATE_DIR/server.env" "$HX_STATE_DIR/server.log"
hx-server token create --name dashboard --prefix source: \
  --prefix macos-agent: --prefix macos-media: --prefix companion: \
  --scopes read_raw,read_l3
hx-server serve --listen 127.0.0.1:7388
```

No provider, model or effort is preselected. Choose them through the
[app API](reference/app-api.md), then save a Vercel AI Gateway API key or sign in
to the subscription provider you chose using `fx login PROVIDER`. Set
`HX_FX_PROVIDER` and `HX_FX_MODEL` explicitly for an environment-only CLI setup;
`vercel` selects AI Gateway (`gateway` is also accepted for older installations).
Set `HX_FX_EFFORT` only when the selected model supports it.

The dashboard token prints once. Keep it private: it grants raw evidence access
and administration for the listed namespaces. Open <http://127.0.0.1:7388/login>
and paste it there. Leave the server running; Ctrl-C stops it.
`hx-server doctor` checks local setup without calling a model. The app's explicit
model check makes one small provider request, which may consume paid usage.
See [provider limitations](provider-limitations.md).

The state folder contains `core.sqlite`, `agent.sqlite`, `blobs/`, `locks/` and
source keys in `secrets/`. Keep it private and back it up with
`hx-server backup --to /absolute/path/to/new-backup`. Backups include evidence and
agent tables but omit source keys and `server.env`; protect those separately.
Hyperthymesia does not encrypt the databases or source key files at rest.
Automatic garbage collection is off; see [GC safety](gc-safety.md).

## Connect a source

In **Connect sources**, select a template or **Add your own**. Enter a name and a
description with the API/MCP address or a CLI command in backticks, for example:

> Read my recent calendar events through `gws`. Use calendar events list for
> calendarId primary, with a bounded timeMin and timeMax.

Sign in to the source CLI first on the server, or provide a narrowly scoped,
read-only API key. CLI HOME can select that login's home directory. History days
limits initial lookback to 0–30 days. An optional connection check can catch
missing login before model setup. Read the notices, then **Connect and turn on**.

Connecting starts one paid setup job with at most two attempts. The agent can
save pull recipes, add-on instructions and tables in `agent.sqlite`. Subsequent
pulls run every four hours without AI. **What your agent set up** shows the
recipes and add-ons. Day-page runs use a cloud model and can cite pulled records
or saved live answers. Source switches do not start model runs. Use **Switch off**
to stop broker requests and pulls; it keeps credentials and saved data.

## Add the Mac app

On an Apple Silicon Mac, build an ad-hoc signed app without a signing account:

```sh
HX_SIGN_IDENTITY=- scripts/build-app.sh
hx-server pair --name laptop --url http://127.0.0.1:7388
```

Copy the single-use pairing link, open `dist/Hyperthymesia.app`, and choose
**Pair from clipboard**. Choose **Grant permissions** when ready to record.
The app uses a separate local outbox under
`~/Library/Application Support/Hyperthymesia`; this is not the server state folder.
See [Mac pairing](mac-pairing.md) for permissions, device IDs and re-pairing.
Ad-hoc rebuilds can reset macOS permission grants. A Linux server can receive
uploads from a paired Mac, or run with connected sources alone.

## Develop and license

Follow [CONTRIBUTING](../CONTRIBUTING.md) and run `./scripts/check.sh` for the complete
platform-aware checks. The privacy scanner reports paths and patterns without
printing matched content; published historical notes remain unchanged.

See [server details](../crates/hx-server/README.md), [the schema](../schema/), and
[the documentation](README.md). Original code, scripts, companions and documentation
in this repository are licensed **AGPL-3.0-only** under [LICENSE](../LICENSE).
Third-party dependencies retain their own licenses.
