# Encore app (desktop and Android)

Your Encore server in its own window: a small Rust app built with [Tauri 2](https://v2.tauri.app) that loads the
web app from the server you choose. No JavaScript framework, no bundled browser – the system webview is used, so
the download is a few MB instead of a hundred.

What it adds on top of the browser:

- **Media keys and system media controls**: play/pause/next/previous from the keyboard, the lock screen, the
  Windows media overlay, macOS Now Playing or any MPRIS client (`playerctl`), with the cover of the current song.
- **Tray**: what's playing, play/pause/next/previous, show the window, the mini player, change server, quit.
- **Windows taskbar**: previous / play-pause / next buttons under the taskbar preview, and the song's progress on
  the taskbar button. **macOS dock menu** (right-click the dock icon): what's playing, play/pause, next,
  previous, mini player. **Linux**: MPRIS (GNOME, KDE, `playerctl`) with position and seeking.
- **Mini player**: a small always-on-top window with cover, song, controls and seek bar (tray, the player's ⋯
  menu, Settings → This computer).
- **Global shortcuts** (Settings → This computer): play/pause, next, previous, volume up/down from any program.
  Defaults Ctrl+Alt+P / Ctrl+Alt+N / Ctrl+Alt+B / Ctrl+Alt+Page Up / Ctrl+Alt+Page Down (Cmd+Option on macOS);
  each one can be changed or cleared. The media keys work as before.
- **Start with the computer**: optional, starts in the tray without a window (`--minimized`).
- **Offline songs**: ⋯ → Save for offline on songs, albums and playlists; when the server can't be reached the
  offline page plays them through the embedded Cadenza engine.
- **Updates**: the app looks for a new release at start and every 6 hours (switchable) and installs it from
  Settings → This computer → Updates, when the build was signed for updates (see "Auto-update" below).
- **Saved servers**: the first start asks for your Encore address; the picker remembers every server that worked.
  If the saved server stops answering, the picker comes back with the reason.
- Outside links open in your browser; downloads from the web app go to your Downloads folder.
- One instance: starting it again brings the window to the front. Window size and position are remembered.
- **Cadenza inside** (desktop only): Encore's own playback engine runs in the app, so Settings → Playback →
  "Cadenza engine" works on this computer without a separate program. See below.

## Cadenza inside

The desktop build carries the [Cadenza](https://github.com/Krifmaz/cadenza) v2 engine (`src/engine.rs`). When
you are signed in, the web app asks the server for an app token for this computer and hands it to the engine
(`EncoreNative.engineLogin`); the engine then shows up under Your devices as "Cadenza on <hostname>" (the web
app calls it "This computer (Cadenza)") and links to it on its own. From then on the music plays through Cadenza
into your DAC – upsampling, dither and noise shaping, DSD, exclusive output – while the window shows the normal
Encore page. Signing out, changing the server or unlinking the device under Your devices stops it; quitting the
app releases the DAC. The token is an app session (Account → sessions: "Encore app · Cadenza on <hostname>")
and idles out when the app stops using it.

- **Config**: `<config dir>/cadenza/config.toml`, the same file a standalone `cadenza serve` uses, written
  whenever a setting changes. Filter coefficients are cached under `<config dir>/cadenza/cache`. The config
  dir is `~/.config/app.encore.client` on Linux, `~/Library/Application Support/app.encore.client` on macOS
  and `%APPDATA%\app.encore.client` on Windows.
- **Exclusive mode** (Settings → Playback → Cadenza engine → Exclusive) takes the DAC away from the OS mixer,
  so the device gets the exact rate and format: Linux uses ALSA `hw:` devices directly (the user must be
  allowed to open them, usually the `audio` group; PipeWire/PulseAudio release the card when nothing else is
  playing), Windows uses WASAPI exclusive mode (turn off "Allow applications to take exclusive control" only
  if you want to forbid it), macOS uses CoreAudio hog mode. Shared mode (the default) plays through the
  system mixer on every OS.
- **GPU**: filters can run on an NVIDIA GPU (driver 525 or newer; Linux and Windows – no CUDA toolkit to
  install, the driver is enough). "Auto" uses it when found; the settings show which GPU is in use or that
  none was found. macOS has no GPU path.
- **DSD**: output mode DSD with rates up to DSD2048 where the DAC takes native DSD (DoP where it does not),
  modulator, packing, gain and staged/full oversampling. The rate list only offers what the device reports.

Cadenza can also run on its own on a small computer next to the DAC (`cadenza serve`); link to it under Your
devices like any other device. Android does not carry the engine: the native Android app has its own Cadenza.

## Layout

- `src/lib.rs` – the Tauri app: window, plugins, commands.
- `src/server.rs` – which URLs stay in the window, the `/api/health` probe, the watchdog that notices a dead server.
- `src/bridge.rs` – `window.EncoreNative`, the script every page gets, and the commands behind it.
- `src/desktop.rs` – tray, OS media controls (souvlaki) with position/seek, taskbar progress.
- `src/extras.rs` – Settings → This computer: global shortcuts, start with the computer, updates, the mini player.
- `src/taskbar.rs` (Windows) – the thumbnail toolbar; `src/icons.rs` draws its glyphs.
- `src/dock.rs` (macOS) – the dock menu.
- `src/engine.rs` – the embedded Cadenza engine: device id/name, `engine_login` / `engine_logout` / `engine_status`.
- `src/offline.rs` – songs saved for offline listening ("Save for offline"; played by `ui/offline.html`, through
  Cadenza on the desktop: `offline_play_songs` / `offline_engine` / `offline_engine_status` in `src/engine.rs`).
- `src/mobile.rs` + `plugins/media/` – Android: the Kotlin plugin with the media notification / lock-screen
  controls (`PlaybackService`), "Share → Encore", downloads from the web app and the session cookie.
- `gen/android/` – the Android Studio project Tauri generated, with our changes: back button keeps the music
  playing, system-bar insets, share intent, cleartext http for LAN servers, release signing fallback.
- `ui/` – the app's own pages: server picker, offline player and mini player.
- `capabilities/` – `default.json`: the app's own pages may call every command; `server.json`: the server's pages (any http/https origin) get what `window.EncoreNative` needs, the engine commands included.

## Build

```sh
cargo install tauri-cli --version '^2' --locked
cd clients/app
cargo tauri dev                 # run against ui/ + your server
cargo tauri build --bundles deb,appimage   # or nsis (Windows), dmg (macOS)
```

Linux needs `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf libasound2-dev`
(the last one for Cadenza's ALSA output).

Android: Android SDK (platform 37) + NDK, `rustup target add aarch64-linux-android armv7-linux-androideabi`,
then `cargo tauri android build --apk --target aarch64 --target armv7`. The release workflow does this
(`.github/workflows/apps.yml`); the APK is signed with the runner's debug key unless the `ANDROID_KEYSTORE_*`
variables point at a real key, so updates need an uninstall first until a key is configured.
Dev builds run any JavaScript written to their stdin in the page (used by the automated checks).

## Auto-update

The desktop app updates itself from the GitHub release with
[tauri-plugin-updater](https://v2.tauri.app/plugin/updater/): it reads
`https://github.com/Krifmaz/Encore/releases/latest/download/latest.json` and installs the signed installer for its
platform (NSIS on Windows, AppImage on Linux, the `.app` on macOS; `.deb` / `.rpm` installs update through the
package manager instead). Updates are signed with a key pair of your own:

1. `cargo tauri signer generate -w ~/.tauri/encore.key` (keep the private key and its password safe).
2. Repository secrets: `TAURI_SIGNING_PRIVATE_KEY` (the content of `encore.key`) and, if it has one,
   `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.
3. Repository variable (or secret) `TAURI_UPDATER_PUBKEY`: the content of `encore.key.pub`.

With these, the desktop job (`.github/workflows/apps.yml`) puts the public key into the app and makes the `.sig`
files, and the release (`release.yml`) attaches `latest.json`. Without them the release still builds: the apps
just can't update themselves (Settings → This computer says so and links to the releases page). Changing the key
later means apps built with the old key can't verify new updates; they need one manual install.
