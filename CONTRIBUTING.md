# Contributing

Bug reports, ideas, translations and pull requests are welcome. For bigger
changes, please open an issue first so we can talk about the approach.

## Reporting a bug

Open an [issue](https://github.com/srcmkr/OxShot/issues). Please mention the
version (*Settings → Updates*), your distribution and desktop, your monitor
setup (number, resolution, scaling) and, for capture problems, which window or
program was on screen.

## Building from source

You need a current stable [Rust](https://rustup.rs/) and a few libraries. On
Debian, Ubuntu and Linux Mint:

```sh
sudo apt install build-essential libxkbcommon-x11-dev libxcb1-dev \
    libwayland-dev libfontconfig-dev libvulkan1
git clone https://github.com/srcmkr/OxShot
cd OxShot
cargo build --release
./target/release/oxshot
```

A copy run from `target/release` can add itself to your application menu and
autostart (*Settings → System*); those entries repair themselves when the
program moves. *Settings → Updates → Install for all users* installs it under
`/usr/local` instead, where it can update itself.

Only one OxShot runs per display: `oxshot capture` and friends hand their
command to the running instance through a Unix socket. To try a new build,
quit the old one first (`oxshot quit`).

Before a pull request, run both of these; they should pass without warnings:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

### Development aids

| Variable | Effect |
|---|---|
| `OXSHOT_UPDATE_FAKE=0.9.0` | pretends that version is out, to see the update offer without a release |
| `OXSHOT_UPDATE_DELAY=<seconds>` | shortens the wait before the first update check |
| `OXSHOT_UPDATE_API=<url>` | asks that address instead of GitHub for the latest release (files must still be signed with the release key) |
| `RUST_LOG=oxshot=debug` | more log output, e.g. capture and overlay timings |

To test without touching your desktop, run OxShot in a nested X server with
its own settings and pictures folder:

```sh
Xephyr :5 -screen 1600x1000 &
mkdir -p /tmp/oxtest/config /tmp/oxtest/pics
echo 'XDG_PICTURES_DIR="/tmp/oxtest/pics"' > /tmp/oxtest/config/user-dirs.dirs
DISPLAY=:5 XDG_CONFIG_HOME=/tmp/oxtest/config ./target/debug/oxshot open some.png
```

## Translations

All texts live in [`crates/oxshot/locales/`](crates/oxshot/locales): `app.yml`
and `workspace.yml` for OxShot, `components.yml` for the texts of the UI
library. Every key has one line per language. To fix a translation, edit the
line. To add a language, add a line to every key and the language to
`crates/oxshot/src/i18n.rs`. The test `complete_translations` fails if a key
is missing in any language. Native speakers checking the existing
translations are very welcome.

## Where things are

| Path | Contents |
|---|---|
| [`crates/oxshot-core`](crates/oxshot-core) | The document model, editing logic (handles, hit tests), the CPU renderer (tiny-skia, cosmic-text), image operations and project files. No GUI; the same renderer draws the editor and the exported image. |
| [`crates/oxshot-x11`](crates/oxshot-x11) | Screen capture through MIT-SHM, windows and monitors under the pointer, global hotkeys, the multi-format clipboard. |
| [`crates/oxshot-update`](crates/oxshot-update) | Finding, verifying and unpacking releases; how OxShot is installed; key and signing tools. |
| [`crates/oxshot`](crates/oxshot) | The app (GPUI): capture overlay and color picker, editor, history, settings, tray, countdown, updates, translations. |
| [`packaging`](packaging) | Package build, polkit rules, release key |
| [`vendor`](vendor) | A patched copy of a dependency, with the reason in its README |
| [`.github/workflows`](.github/workflows) | Tests for every push, releases for every tag |
| [`dev`](dev) | The release script |
| [`docs`](docs) | Screenshots for the README |

## Releases

Write what is new under `## Unreleased` in [CHANGELOG.md](CHANGELOG.md), then
run `dev/release.sh 0.2.0`. It sets the version, runs the tests, commits, tags
and pushes. The tag starts the release workflow: it tests, builds on Ubuntu
22.04 (so the program runs on older systems too), packs the archive, the
Debian package and the AppImage with `packaging/build-packages.sh`, signs all
three and publishes them as the latest release, which installed copies then
offer as an update.

The signing key is made once with
`cargo run -p oxshot-update --example release-key -- <secret key file>`. Its
public half goes into `packaging/minisign.pub` and is built into every copy of
OxShot; the secret half goes into the repository secret
`MINISIGN_SECRET_KEY`. Keep a backup of the secret key: without it, installed
copies cannot verify new releases.
