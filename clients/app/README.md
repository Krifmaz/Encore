# Encore app (desktop and Android)

Your Encore server in its own window: a small Rust app built with [Tauri 2](https://v2.tauri.app) that loads the
web app from the server you choose. No JavaScript framework, no bundled browser – the system webview is used, so
the download is a few MB instead of a hundred.

What it adds on top of the browser:

- **Media keys and system media controls**: play/pause/next/previous from the keyboard, the lock screen, the
  Windows media overlay, macOS Now Playing or any MPRIS client (`playerctl`), with the cover of the current song.
- **Tray**: what's playing, play/pause/next/previous, show the window, change server, quit.
- **Saved servers**: the first start asks for your Encore address; the picker remembers every server that worked.
  If the saved server stops answering, the picker comes back with the reason.
- Outside links open in your browser; downloads from the web app go to your Downloads folder.
- One instance: starting it again brings the window to the front. Window size and position are remembered.

## Layout

- `src/lib.rs` – the Tauri app: window, plugins, commands.
- `src/server.rs` – which URLs stay in the window, the `/api/health` probe, the watchdog that notices a dead server.
- `src/bridge.rs` – `window.EncoreNative`, the script every page gets, and the commands behind it.
- `src/desktop.rs` – tray and OS media controls (souvlaki).
- `src/offline.rs` – songs saved for offline listening (Android: "Save for offline"; played by `ui/offline.html`).
- `src/mobile.rs` + `plugins/media/` – Android: the Kotlin plugin with the media notification / lock-screen
  controls (`PlaybackService`), "Share → Encore", downloads from the web app and the session cookie.
- `gen/android/` – the Android Studio project Tauri generated, with our changes: back button keeps the music
  playing, system-bar insets, share intent, cleartext http for LAN servers, release signing fallback.
- `ui/` – the app's own pages: server picker and offline player.
- `capabilities/default.json` – the server's pages may call the app's commands (any http/https origin).

## Build

```sh
cargo install tauri-cli --version '^2' --locked
cd clients/app
cargo tauri dev                 # run against ui/ + your server
cargo tauri build --bundles deb,appimage   # or nsis (Windows), dmg (macOS)
```

Linux needs `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev patchelf`.

Android: Android SDK (platform 37) + NDK, `rustup target add aarch64-linux-android armv7-linux-androideabi`,
then `cargo tauri android build --apk --target aarch64 --target armv7`. The release workflow does this
(`.github/workflows/apps.yml`); the APK is signed with the runner's debug key unless the `ANDROID_KEYSTORE_*`
variables point at a real key, so updates need an uninstall first until a key is configured.
Dev builds run any JavaScript written to their stdin in the page (used by the automated checks).
