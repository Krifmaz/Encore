# Encore desktop

Your Encore server in a native window on Windows, macOS and Linux, with every song playing through
[Cadenza](https://github.com/Krifmaz/cadenza), Encore's own playback engine, built into the app. Pure Rust:
[Iced](https://iced.rs) 0.14 for the window (wgpu, with a software fallback), no web view and no browser
engine. It replaces the classic Tauri app in `clients/app`, which ships one last time with 0.27 and is then
removed.

The app is the web app, native: the same pages in the same order (Home, Discover, For you, Library, Requests,
Playlists, the Downloader, Soulseek, the leaderboard, Storage, Bug reports, Settings), the same words and the
same server calls. On top of that:

- **Cadenza on this computer.** There is no other audio path: server songs stream from
  `/api/library/tracks/{id}/stream` into the engine, offline songs play from their files. Settings → Playback →
  *Play through Cadenza on this device* turns the engine's processing on (output and DAC, exclusive mode, PCM
  and DSD rates limited to what the device takes, link limit, filters, dither, noise shaping, EQ, convolution,
  *Test sound*, the full signal path); off, the engine plays bit-perfect.
- **A device like the others.** After you sign in the app asks the server for an app token and shows up under
  Your devices as "Encore on <computer name>": the phone or the browser can send music to it, and it can send
  music to them.
- **Tray, media keys and the OS media controls**: SMTC on Windows, Now Playing on macOS, MPRIS on Linux, with
  the cover, position and seeking. The tray (Show, Play / Pause, Next, Previous, Mini player, Quit) is on
  every system; on Linux it is a StatusNotifierItem over D-Bus (tray-icon's `ksni` backend), shown by
  desktops with a tray host (KDE, GNOME with the AppIndicator extension, most others).
- **Mini player**: a small window that stays on top (tray, the player's ⋯ menu, Settings → This computer).
- **Global shortcuts** (Settings → This computer): play / pause, next, previous, volume up and down from any
  program. Defaults Ctrl+Alt+P / N / B / Page Up / Page Down (Cmd+Option on macOS).
- **Start with the computer** (Settings → This computer), in the tray without a window (`--minimized`).
- **Offline songs**: Save for offline on songs, albums and playlists keeps the audio and the cover on this
  computer; the Offline music page plays them when the server cannot be reached.
- **Updates**: at start and every 6 hours (switchable) the app looks at the newest GitHub release, downloads
  the file for this computer, checks it against the release's `SHA256SUMS` and installs it (Windows: the
  installer, silently; macOS: the app from the disk image; Linux: the AppImage). A `.deb` install (or a build
  from source on Linux) opens the new `.deb` in the browser instead, to download and install with the package
  manager.
- **Saved servers**: the first page picks the server and signs in; every server that worked is remembered.
  When the server cannot be reached the page says so, retries, and offers the offline songs.
- One instance at a time: starting it again brings the window forward. The window's size and place are
  remembered.

How the crate is built (pages, widgets, the engine, the shell) is in [ARCHITECTURE.md](ARCHITECTURE.md).

## Build and run

Rust 1.90 or newer ([rustup.rs](https://rustup.rs)), then:

```sh
cd clients/desktop
cargo run -- --server http://192.168.1.10:8080     # a debug build against your server
cargo build --release                              # target/release/encore-desktop
cargo build --release --features cuda              # with Cadenza's CUDA offload (Windows, Linux)
```

What each OS needs besides Rust:

- **Linux** (Debian/Ubuntu names): `libasound2-dev libxkbcommon-dev libwayland-dev libudev-dev pkg-config`
  to build (no GTK: the tray talks D-Bus, file dialogs go through the desktop portal); at run time `libasound2`, `libxkbcommon0` and `libxkbcommon-x11-0` (without
  the last one the window does not open under X11), and Vulkan or OpenGL drivers for the GPU renderer.
- **macOS**: the Xcode command line tools (`xcode-select --install`).
- **Windows**: the MSVC toolchain (Visual Studio Build Tools with "Desktop development with C++").

The `cuda` feature needs no CUDA toolkit: the engine loads the NVIDIA driver at run time (driver 525 or newer)
and falls back to the CPU without one. macOS has no CUDA path.

### Flags

| Flag | What it does |
|---|---|
| `--server <url>` | open this Encore server (remembered once you sign in) |
| `--minimized` | start in the tray without a window (what "Start with the computer" uses) |
| `--screenshot-tour <dir>` | sign in, open every page in order, save one PNG per page in `<dir>`, then exit (checks under Xvfb) |
| `--login user:pass` | the account the tour signs in with |
| `--version`, `--help` | print and exit |

`RUST_LOG=debug` (or `info`, the default) sets how much the log on stderr says.

### Where things are kept

| What | Linux | macOS | Windows |
|---|---|---|---|
| Settings (`config.json`), the device link (`link.json`), where you left off (`resume.json`) | `~/.config/encore-desktop/` | `~/Library/Application Support/encore-desktop/` | `%APPDATA%\encore-desktop\` |
| Cadenza's settings (`cadenza/config.toml`) and filter cache (`cadenza/cache/`) | the same folder | the same folder | the same folder |
| Offline songs (`offline/`, listed in `offline/index.json`) | `~/.local/share/encore-desktop/` | `~/Library/Application Support/encore-desktop/` | `%LOCALAPPDATA%\encore-desktop\` |

`ENCORE_DESKTOP_CONFIG_DIR` and `ENCORE_DESKTOP_DATA_DIR` move them (the screenshot tour uses a temporary
folder of its own). Uninstalling leaves these folders; delete them by hand to start over. The classic app keeps
its own settings (`app.encore.client`); on Windows and Linux both can be installed side by side, on macOS the new
`Encore.app` takes the classic one's place in Applications.

### Exclusive mode

Exclusive mode (Settings → Playback, with *Play through Cadenza on this device* on) takes the DAC away from the
system mixer, so it gets the exact rate and format. Linux opens ALSA `hw:` devices directly (the user must be
allowed to open them, usually the `audio` group; PipeWire and PulseAudio let go of the card when nothing else
plays). Windows uses WASAPI exclusive mode ("Allow applications to take exclusive control of this device" must
stay on in the device's Sound settings); when exclusive mode fails, the card says "Exclusive mode failed" with
the reason instead of playing shared without a word. macOS uses CoreAudio hog mode. Shared mode plays through the system mixer on every OS.

## Checks

```sh
cargo check
cargo clippy --all-targets -- -D warnings
cargo test
# the tour (Linux, needs xvfb and libxkbcommon-x11-0): one PNG per page, exit 0
xvfb-run -a -s "-screen 0 1400x900x24" ./target/debug/encore-desktop \
  --server http://127.0.0.1:18085 --login admin:adminpass1234 --screenshot-tour /tmp/shots
```

Without a sound card the engine plays into its null output, so the tour also runs on a server or in CI.

## Packaging and releases

`scripts/desktop-package.sh` builds a release binary and packages it for the OS it runs on:

| OS | File | How |
|---|---|---|
| Windows | `Encore-<version>-windows-x86_64.exe` | NSIS (`packaging/windows/encore.nsi`): per user under `%LOCALAPPDATA%\Encore`, Start menu shortcut, Apps & features entry, `/S` for a silent install |
| macOS | `Encore-<version>-macos-universal.dmg` | `Encore.app` (`packaging/macos/Info.plist`, `packaging/icons/encore.icns`), Apple silicon and Intel joined with `lipo`, a disk image with an Applications link |
| Linux | `Encore-<version>-linux-x86_64.AppImage` | appimagetool (fetched once), desktop entry and icons from `packaging/linux` and `packaging/icons` |
| Linux | `encore_<version>_amd64.deb` | `dpkg-deb`; package `encore-desktop`, `/usr/bin/encore-desktop`, Depends from `dpkg-shlibdeps` |

```sh
scripts/desktop-package.sh                         # named after Cargo.toml's version, into out/
scripts/desktop-package.sh --version 0.27.1 --stamp --features cuda   # what a release does on Windows and Linux
scripts/desktop-package.sh --universal             # macOS: both architectures
scripts/desktop-package.sh --help
```

`--stamp` writes the version into `Cargo.toml` and `Cargo.lock` first, so the app reports it and the updater
compares against it; leave it out on your own machine unless you mean to change those files.

The `desktop` job of `.github/workflows/apps.yml` runs the tests (Linux, Windows) and this script on
ubuntu-22.04 (so the AppImage runs on glibc 2.35 and newer), windows-latest and macos-latest, and uploads the
files. A release (`release.yml` with "apps" ticked) attaches them, next to the classic app's files, and writes
`SHA256SUMS` over everything. The in-app updater relies on the four names above: a change there needs the same
change in `src/desktop/updater.rs`.

Signing is optional and comes from repository secrets; without them the job still builds:

- **Windows**: `WINDOWS_CERT_PFX_BASE64` (the .pfx, base64) and `WINDOWS_CERT_PASSWORD` – the app and the
  installer are signed with signtool. Unsigned, SmartScreen asks once ("More info → Run anyway").
- **macOS**: `MACOS_CERT_P12_BASE64`, `MACOS_CERT_PASSWORD` and `MACOS_SIGN_IDENTITY` (a Developer ID
  Application certificate) sign the app with the hardened runtime; with `APPLE_ID`, `APPLE_TEAM_ID` and
  `APPLE_APP_PASSWORD` (an app-specific password) the disk image is notarized and stapled. Without them the app
  is signed ad hoc: the first start needs System Settings → Privacy & Security → Open Anyway.
